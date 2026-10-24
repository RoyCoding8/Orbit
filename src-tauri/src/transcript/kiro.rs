//! Read-only ingestion of Kiro CLI v2 ACP session files.
//!
//! Kiro CLI v3 owns its session storage behind ACP and is intentionally not
//! read from this adapter.

use super::archive::{ArchiveAction, ArchiveReport};
use super::ingest::{
    open_database, prune_vanished_sessions, replace_indexed_session, IndexedMessage,
    IndexedSession, IngestReport,
};
use super::jsonl::{read_bounded_line, LineStatus};
use super::TranscriptError;
use crate::harness::atomic;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

const SESSION_PREFIX: &str = "kiro-v2:";
const MAX_SESSION_LOG_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct SessionHeader {
    session_id: String,
    cwd: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

/// Read the documented CLI v2 ACP session files beneath `KIRO_HOME` and
/// replace their normalized records in Orbit's transcript cache.
pub fn ingest(config_root: &Path, app_data_dir: &Path) -> Result<IngestReport, TranscriptError> {
    let session_root = config_root.join("sessions").join("cli");
    crate::harness::claude::skills::ensure_plain_components(&session_root)
        .map_err(|_| TranscriptError::InvalidRoot)?;
    let mut connection = open_database(app_data_dir)?;
    let mut report = IngestReport {
        sessions: 0,
        lines_read: 0,
        messages_indexed: 0,
        malformed_lines: 0,
        skipped_files: 0,
        skipped_state_lines: 0,
    };
    let root_metadata = match fs::symlink_metadata(&session_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            prune_vanished_sessions(&mut connection, &[])?;
            return Ok(report);
        }
        Err(error) => return Err(error.into()),
    };
    if atomic::is_link_or_reparse(&root_metadata) || !root_metadata.is_dir() {
        return Err(TranscriptError::InvalidRoot);
    }

    let mut headers = Vec::new();
    for entry in fs::read_dir(&session_root)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".json") || name.ends_with(".history.json") {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
            continue;
        }
        headers.push(path);
    }
    headers.sort();

    let mut seen_paths = Vec::with_capacity(headers.len());

    for header_path in headers {
        let Some(session_id) = header_path.file_stem().and_then(|stem| stem.to_str()) else {
            report.skipped_files += 1;
            continue;
        };
        let log_path = header_path.with_extension("jsonl");
        if !plain_file(&log_path)? {
            report.skipped_files += 1;
            continue;
        }
        seen_paths.push(log_path.to_string_lossy().into_owned());

        let header = match read_header(&header_path) {
            Ok(Some(header))
                if header.session_id == session_id && !header.cwd.trim().is_empty() =>
            {
                header
            }
            _ => {
                report.skipped_files += 1;
                continue;
            }
        };

        match ingest_session(&mut connection, &header, &log_path, &mut report) {
            Ok(indexed) => {
                report.sessions += 1;
                report.messages_indexed += indexed;
            }
            Err(TranscriptError::Io(_)) => report.skipped_files += 1,
            Err(error) => return Err(error),
        }
    }

    prune_vanished_sessions(&mut connection, &seen_paths)?;
    Ok(report)
}

/// Copy a Kiro v2 session's native header and event log into a new directory.
/// Include a manifest identifying this as a source backup without import support.
pub fn export_archive(
    config_root: &Path,
    app_data_dir: &Path,
    session_id: &str,
    destination: &Path,
) -> Result<ArchiveReport, String> {
    let native_id = session_id
        .strip_prefix(SESSION_PREFIX)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "The selected Kiro session is not a v2 session.".to_owned())?;
    let connection = open_database(app_data_dir).map_err(|error| error.to_string())?;
    let source_path: String = connection
        .query_row(
            "SELECT source_path FROM sessions WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("Could not find the indexed Kiro session: {error}"))?;
    let session_root = checked_session_root(config_root)?;
    let log_path = PathBuf::from(source_path);
    if !plain_file(&log_path).map_err(|error| error.to_string())? {
        return Err("The indexed Kiro session log is missing or is not a regular file.".to_owned());
    }
    let log_path = log_path
        .canonicalize()
        .map_err(|error| format!("Could not resolve the Kiro session log: {error}"))?;
    if !log_path.starts_with(&session_root)
        || log_path.file_name().and_then(|name| name.to_str())
            != Some(format!("{native_id}.jsonl").as_str())
        || !plain_file(&log_path).map_err(|error| error.to_string())?
    {
        return Err(
            "The indexed Kiro session is outside the configured v2 session directory.".to_owned(),
        );
    }
    let header_path = log_path.with_extension("json");
    if !plain_file(&header_path).map_err(|error| error.to_string())? {
        return Err("The Kiro session header is missing or is not a regular file.".to_owned());
    }
    let header = read_header(&header_path)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "The Kiro session header is invalid.".to_owned())?;
    if header.session_id != native_id {
        return Err("The Kiro session header does not match the selected session.".to_owned());
    }

    let original_destination_metadata = fs::symlink_metadata(destination)
        .map_err(|error| format!("Could not inspect the archive destination: {error}"))?;
    if atomic::is_link_or_reparse(&original_destination_metadata)
        || !original_destination_metadata.is_dir()
    {
        return Err("The archive destination must be an existing regular directory.".to_owned());
    }
    let destination = destination
        .canonicalize()
        .map_err(|error| format!("Could not resolve the archive destination: {error}"))?;
    if destination.starts_with(&session_root) {
        return Err("Choose an archive destination outside Kiro's session storage.".to_owned());
    }

    let archive_path = (0..8)
        .find_map(|_| {
            let candidate =
                destination.join(format!("kiro-v2-{}-{}", native_id, uuid::Uuid::new_v4()));
            match fs::create_dir(&candidate) {
                Ok(()) => Some(Ok(candidate)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .ok_or_else(|| "Could not allocate a unique Kiro archive directory.".to_owned())?
        .map_err(|error| format!("Could not create the Kiro archive directory: {error}"))?;

    let copy_result = (|| {
        copy_new_file(
            &header_path,
            &archive_path.join(format!("{native_id}.json")),
        )?;
        copy_new_file(&log_path, &archive_path.join(format!("{native_id}.jsonl")))?;
        let manifest = serde_json::to_vec_pretty(&json!({
            "format": "orbit.kiro.v2-source-backup",
            "version": 1,
            "sourceSessionId": session_id,
            "nativeFiles": [format!("{native_id}.json"), format!("{native_id}.jsonl")],
            "importSupported": false,
            "importLimitation": "Kiro session loading is interactive; Orbit has not verified a safe native import API."
        }))
        .map_err(|error| format!("Could not encode the Kiro backup manifest: {error}"))?;
        write_new_file(&archive_path.join("orbit-backup.json"), &manifest)?;
        Ok::<(), String>(())
    })();
    if let Err(error) = copy_result {
        let _ = fs::remove_dir_all(&archive_path);
        return Err(error);
    }

    Ok(ArchiveReport {
        action: ArchiveAction::Exported,
        source_session_id: session_id.to_owned(),
        session_id: session_id.to_owned(),
        files: 3,
        path: archive_path.to_string_lossy().into_owned(),
    })
}

/// Kiro's documented archive loading flow is an interactive chat command. The
/// v2 native pair is not a documented automation import API, so keep this
/// operation disabled instead of copying an unverified session into Kiro.
pub fn import_archive(
    _config_root: &Path,
    _source: &Path,
    _project_root: &Path,
) -> Result<ArchiveReport, String> {
    Err("Kiro session import is unavailable: Kiro only documents loading session archives from an interactive chat, and Orbit has not verified a safe native import API.".to_owned())
}

fn checked_session_root(config_root: &Path) -> Result<PathBuf, String> {
    let session_root = config_root.join("sessions").join("cli");
    crate::harness::claude::skills::ensure_plain_components(&session_root)?;
    let metadata = fs::symlink_metadata(&session_root)
        .map_err(|error| format!("Could not inspect Kiro's v2 session directory: {error}"))?;
    if atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err("Kiro's v2 session directory is not a regular directory.".to_owned());
    }
    session_root
        .canonicalize()
        .map_err(|error| format!("Could not resolve Kiro's v2 session directory: {error}"))
}

fn copy_new_file(source: &Path, destination: &Path) -> Result<(), String> {
    let mut input =
        File::open(source).map_err(|error| format!("Could not read Kiro session file: {error}"))?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("Could not create Kiro archive file: {error}"))?;
    std::io::copy(&mut input, &mut output)
        .map_err(|error| format!("Could not copy Kiro session file: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("Could not finish Kiro archive file: {error}"))?;
    output
        .sync_all()
        .map_err(|error| format!("Could not sync Kiro archive file: {error}"))
}

fn write_new_file(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("Could not create the Kiro backup manifest: {error}"))?;
    output
        .write_all(bytes)
        .map_err(|error| format!("Could not write the Kiro backup manifest: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("Could not finish the Kiro backup manifest: {error}"))?;
    output
        .sync_all()
        .map_err(|error| format!("Could not sync the Kiro backup manifest: {error}"))
}

fn read_header(path: &Path) -> Result<Option<SessionHeader>, TranscriptError> {
    if fs::metadata(path)?.len() > MAX_RECORD_BYTES as u64 {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes).ok())
}

fn plain_file(path: &Path) -> Result<bool, TranscriptError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(!atomic::is_link_or_reparse(&metadata) && metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn ingest_session(
    connection: &mut Connection,
    header: &SessionHeader,
    source_path: &Path,
    report: &mut IngestReport,
) -> Result<usize, TranscriptError> {
    if !plain_file(source_path)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Kiro transcript file disappeared during indexing",
        )
        .into());
    }
    if fs::symlink_metadata(source_path)?.len() > MAX_SESSION_LOG_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Kiro session log exceeds the 256 MiB indexing limit",
        )
        .into());
    }
    let file = File::open(source_path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut position = 0usize;
    let mut errors = Vec::new();
    let mut messages = Vec::<IndexedMessage>::new();
    let mut message_index = HashMap::<(String, String), usize>::new();
    let mut parent_uuid: Option<String> = None;

    loop {
        line.clear();
        let line_status = read_bounded_line(&mut reader, &mut line, MAX_RECORD_BYTES)?;
        if line_status == LineStatus::Eof {
            break;
        }
        position += 1;
        report.lines_read += 1;

        // Kiro appends to this file while a session is active. Ignore an
        // unfinished final record and let the next ingest read it in full.
        if matches!(line_status, LineStatus::Unterminated { .. }) {
            report.skipped_state_lines += 1;
            continue;
        }
        if matches!(line_status, LineStatus::Complete { oversized: true }) {
            report.malformed_lines += 1;
            errors.push((
                position,
                "record exceeds the 8 MiB indexing limit".to_owned(),
            ));
            continue;
        }
        while line
            .last()
            .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
        {
            line.pop();
        }
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            report.skipped_state_lines += 1;
            continue;
        }

        let record = match serde_json::from_slice::<Value>(&line) {
            Ok(record) => record,
            Err(error) => {
                report.malformed_lines += 1;
                errors.push((position, error.to_string()));
                continue;
            }
        };
        let kind = record.get("kind").and_then(Value::as_str);
        let Some(message_type) = (match kind {
            Some("Prompt") => Some("user"),
            Some("AssistantMessage") => Some("assistant"),
            _ => None,
        }) else {
            report.skipped_state_lines += 1;
            continue;
        };
        let Some(data) = record.get("data") else {
            report.skipped_state_lines += 1;
            continue;
        };
        let Some(text) = visible_text(data.get("content")) else {
            report.skipped_state_lines += 1;
            continue;
        };
        let native_message_id = data
            .get("message_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("line-{position}"));
        let key = (message_type.to_owned(), native_message_id.clone());

        if let Some(index) = message_index.get(&key).copied() {
            messages[index].text.push_str(&text);
            messages[index].raw_json = json!({
                "kind": message_type,
                "text": messages[index].text,
            })
            .to_string();
            continue;
        }

        let uuid = format!("{message_type}:{native_message_id}");
        let raw_json = json!({ "kind": message_type, "text": text }).to_string();
        message_index.insert(key, messages.len());
        messages.push(IndexedMessage {
            uuid: uuid.clone(),
            parent_uuid: parent_uuid.clone(),
            message_type: message_type.to_owned(),
            text,
            raw_json,
            position,
            attachment: None,
            tool_result: None,
            tool_result_note: None,
        });
        parent_uuid = Some(uuid);
    }

    let session_id = format!("{SESSION_PREFIX}{}", header.session_id);
    replace_indexed_session(
        connection,
        IndexedSession {
            id: &session_id,
            project: &header.cwd,
            source_path,
            updated_at: header.updated_at.as_deref(),
            title: header.title.as_deref(),
        },
        &messages,
        &errors,
    )
}

fn visible_text(content: Option<&Value>) -> Option<String> {
    let blocks = content?.as_array()?;
    let mut text = String::new();
    for block in blocks {
        let kind = block.get("kind").and_then(Value::as_str);
        if kind.is_some_and(|kind| !kind.eq_ignore_ascii_case("text")) {
            continue;
        }
        let Some(data) = block.get("data") else {
            continue;
        };
        let part = data
            .as_str()
            .or_else(|| data.get("text").and_then(Value::as_str));
        if let Some(part) = part {
            text.push_str(part);
        }
    }
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::spine;
    use serde_json::json;

    fn write_jsonl(path: &Path, records: &[Value]) {
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend(serde_json::to_vec(record).unwrap());
            bytes.push(b'\n');
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn v2_ingest_lists_and_reads_visible_text_without_indexing_tool_records() {
        let root = crate::test_support::tempdir().unwrap();
        let config_root = root.path().join("kiro");
        let session_dir = config_root.join("sessions").join("cli");
        fs::create_dir_all(&session_dir).unwrap();
        let session_id = "7a2ce8a4-91e9-4cf3-9da0-001122334455";
        let header = json!({
            "session_id": session_id,
            "cwd": "C:/workspace/demo",
            "created_at": "2026-10-01T00:00:00Z",
            "updated_at": "2026-10-02T00:00:00Z",
            "title": "Synthetic fixture",
            "session_state": {}
        });
        fs::write(
            session_dir.join(format!("{session_id}.json")),
            serde_json::to_vec(&header).unwrap(),
        )
        .unwrap();
        write_jsonl(
            &session_dir.join(format!("{session_id}.jsonl")),
            &[
                json!({"version":"1","kind":"Prompt","data":{"message_id":"user-1","content":[{"kind":"Text","data":"Question"}]}}),
                json!({"version":"1","kind":"AssistantMessage","data":{"message_id":"assistant-1","content":[{"kind":"Text","data":{"text":"First "}}]}}),
                json!({"version":"1","kind":"ToolResults","data":{"message_id":"tool-1","content":[{"kind":"Text","data":"secret tool output"}]}}),
                json!({"version":"1","kind":"AssistantMessage","data":{"message_id":"assistant-1","content":[{"kind":"Text","data":{"text":"answer"}}]}}),
            ],
        );

        let cache = root.path().join("cache");
        let report = ingest(&config_root, &cache).unwrap();
        let sessions = crate::transcript::ingest::list_sessions(&cache).unwrap();
        let messages =
            spine::thread_page(&cache, &format!("{SESSION_PREFIX}{session_id}"), 0, 20).unwrap();

        assert_eq!(report.sessions, 1);
        assert_eq!(report.lines_read, 4);
        assert_eq!(report.messages_indexed, 2);
        assert_eq!(report.malformed_lines, 0);
        assert_eq!(report.skipped_files, 0);
        assert_eq!(report.skipped_state_lines, 1);
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].session_id,
            format!("{SESSION_PREFIX}{session_id}")
        );
        assert_eq!(sessions[0].title.as_deref(), Some("Synthetic fixture"));
        assert_eq!(sessions[0].project, "C:/workspace/demo");
        assert_eq!(
            sessions[0].updated_at.as_deref(),
            Some("2026-10-02T00:00:00Z")
        );
        assert_eq!(sessions[0].message_count, 2);
        assert_eq!(
            messages
                .iter()
                .map(|message| (message.message_type.as_str(), message.text.as_str()))
                .collect::<Vec<_>>(),
            vec![("user", "Question"), ("assistant", "First answer")]
        );

        let backup_root = root.path().join("backups");
        fs::create_dir(&backup_root).unwrap();
        let backup = export_archive(
            &config_root,
            &cache,
            &format!("{SESSION_PREFIX}{session_id}"),
            &backup_root,
        )
        .unwrap();
        assert_eq!(backup.files, 3);
        let backup_path = PathBuf::from(backup.path);
        assert_eq!(
            fs::read(backup_path.join(format!("{session_id}.jsonl"))).unwrap(),
            fs::read(session_dir.join(format!("{session_id}.jsonl"))).unwrap()
        );
        let manifest: Value =
            serde_json::from_slice(&fs::read(backup_path.join("orbit-backup.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["format"], "orbit.kiro.v2-source-backup");
        assert_eq!(manifest["importSupported"], false);
        assert!(
            import_archive(&config_root, &backup_path, Path::new("C:/workspace/demo"))
                .unwrap_err()
                .contains("interactive chat")
        );
    }

    #[test]
    fn v2_ingest_ignores_an_unterminated_final_record_until_the_next_write() {
        let root = crate::test_support::tempdir().unwrap();
        let config_root = root.path().join("kiro");
        let session_dir = config_root.join("sessions").join("cli");
        fs::create_dir_all(&session_dir).unwrap();
        let session_id = "7a2ce8a4-91e9-4cf3-9da0-001122334455";
        fs::write(
            session_dir.join(format!("{session_id}.json")),
            serde_json::to_vec(&json!({
                "session_id": session_id,
                "cwd": "C:/workspace/demo",
                "updated_at": "2026-10-02T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();
        let log_path = session_dir.join(format!("{session_id}.jsonl"));
        fs::write(
            &log_path,
            b"{\"version\":\"1\",\"kind\":\"Prompt\",\"data\":{\"message_id\":\"user-1\",\"content\":[{\"kind\":\"Text\",\"data\":\"Question\"}]}}\n{\"unfinished\":",
        )
        .unwrap();

        let cache = root.path().join("cache");
        let report = ingest(&config_root, &cache).unwrap();
        let messages =
            spine::thread_page(&cache, &format!("{SESSION_PREFIX}{session_id}"), 0, 20).unwrap();

        assert_eq!(report.malformed_lines, 0);
        assert_eq!(report.skipped_state_lines, 1);
        assert_eq!(
            messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Question"]
        );
    }

    #[test]
    fn bounded_line_reader_discards_oversized_records_and_resumes_at_next_line() {
        let mut reader = BufReader::new(&b"012345\nnext\n"[..]);
        let mut line = Vec::new();
        assert_eq!(
            read_bounded_line(&mut reader, &mut line, 5).unwrap(),
            LineStatus::Complete { oversized: true }
        );
        assert!(line.is_empty());
        assert_eq!(
            read_bounded_line(&mut reader, &mut line, 5).unwrap(),
            LineStatus::Complete { oversized: false }
        );
        assert_eq!(line, b"next\n");
    }
}
