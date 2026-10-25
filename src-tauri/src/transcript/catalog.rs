use super::{codex, ingest, kiro, opencode, pi, SessionSummary, TranscriptError};
use crate::harness::{atomic, selected::SelectedHarness, Harness};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(super) struct Metadata {
    pub id: String,
    pub project: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub path: PathBuf,
    pub fingerprint: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexProgress {
    pub completed: usize,
    pub total: usize,
    pub failed: usize,
    pub title: Option<String>,
}

pub fn refresh(
    harness: &SelectedHarness,
    app: &Path,
    project: &Path,
) -> Result<Vec<String>, String> {
    let cache = harness.transcript_cache(app);
    let mut connection = ingest::open_database(&cache).map_err(|e| e.to_string())?;
    let cached = cached_metadata(&connection).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    let mut seen = Vec::new();
    let mut entries = Vec::new();
    if matches!(harness, SelectedHarness::Opencode(_)) {
        entries = opencode::metadata()?;
        seen.extend(
            entries
                .iter()
                .map(|entry| entry.path.to_string_lossy().into_owned()),
        );
    } else {
        let files = source_files(harness, project, &cached)?;
        let titles = if matches!(harness, SelectedHarness::Codex(_)) {
            codex::read_titles(&harness.config_root().join("session_index.jsonl"))
        } else {
            HashMap::new()
        };
        let mut ids = HashSet::new();
        for path in files.into_iter().rev() {
            let fingerprint = source_fingerprint(harness, &path);
            let previous = cached.get(&path);
            let metadata = if let Some(previous) =
                previous.filter(|entry| fingerprint.is_some() && entry.fingerprint == fingerprint)
            {
                Ok(previous.clone())
            } else {
                read_metadata(harness, &path, fingerprint)
            };
            match metadata {
                Ok(mut entry) if ids.insert(entry.id.clone()) => {
                    if let Some(title) = titles.get(&entry.id) {
                        entry.title = Some(title.clone());
                    }
                    seen.push(path.to_string_lossy().into_owned());
                    entries.push(entry);
                }
                Ok(_) => {}
                Err(error) => {
                    seen.push(path.to_string_lossy().into_owned());
                    warnings.push(format!("{}: {error}", path.display()));
                }
            }
        }
    }
    let transaction = connection.transaction().map_err(|e| e.to_string())?;
    for entry in entries {
        let source = entry.path.to_string_lossy();
        let previous = {
            let mut query = transaction
                .prepare_cached(
                    "SELECT session_id FROM sessions WHERE source_path = ?1 AND session_id <> ?2",
                )
                .map_err(|e| e.to_string())?;
            let rows = query
                .query_map(params![source, entry.id], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        for id in previous {
            transaction
                .execute("DELETE FROM messages_fts WHERE session_id = ?1", [&id])
                .map_err(|e| e.to_string())?;
            transaction
                .execute("DELETE FROM sessions WHERE session_id = ?1", [&id])
                .map_err(|e| e.to_string())?;
        }
        transaction.execute(
            "INSERT INTO sessions(session_id, project, source_path, title, updated_at, catalog_fingerprint, indexed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
             ON CONFLICT(session_id) DO UPDATE SET project = excluded.project, source_path = excluded.source_path,
             title = COALESCE(excluded.title, sessions.title), updated_at = COALESCE(excluded.updated_at, sessions.updated_at),
             catalog_fingerprint = excluded.catalog_fingerprint,
             indexed = COALESCE(sessions.indexed AND sessions.source_path = excluded.source_path AND sessions.fingerprint = excluded.catalog_fingerprint, 0)",
            params![entry.id, entry.project, source, entry.title, entry.updated_at, entry.fingerprint],
        ).map_err(|e| e.to_string())?;
    }
    transaction.commit().map_err(|e| e.to_string())?;
    ingest::prune_vanished_sessions(&mut connection, &seen).map_err(|e| e.to_string())?;
    Ok(warnings)
}

fn cached_metadata(connection: &Connection) -> Result<HashMap<PathBuf, Metadata>, TranscriptError> {
    let mut statement = connection.prepare("SELECT session_id, project, title, updated_at, source_path, COALESCE(catalog_fingerprint, fingerprint) FROM sessions")?;
    let rows = statement.query_map([], |row| {
        Ok(Metadata {
            id: row.get(0)?,
            project: row.get(1)?,
            title: row.get(2)?,
            updated_at: row.get(3)?,
            path: PathBuf::from(row.get::<_, String>(4)?),
            fingerprint: row.get(5)?,
        })
    })?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect())
}

fn source_files(
    harness: &SelectedHarness,
    project: &Path,
    cached: &HashMap<PathBuf, Metadata>,
) -> Result<Vec<PathBuf>, String> {
    let config = harness.config_root();
    let mut files = Vec::new();
    match harness {
        SelectedHarness::Claude(_) => {
            let root = config.join("projects");
            if root.is_dir() {
                for directory in fs::read_dir(root).map_err(|e| e.to_string())? {
                    let directory = directory.map_err(|e| e.to_string())?;
                    if !directory.file_type().map_err(|e| e.to_string())?.is_dir() {
                        continue;
                    }
                    for file in fs::read_dir(directory.path()).map_err(|e| e.to_string())? {
                        let file = file.map_err(|e| e.to_string())?;
                        if file.file_type().map_err(|e| e.to_string())?.is_file()
                            && file
                                .path()
                                .extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
                        {
                            files.push(file.path());
                        }
                    }
                }
            }
        }
        SelectedHarness::Codex(_) => {
            for root in [config.join("sessions"), config.join("archived_sessions")] {
                if root.is_dir() {
                    codex::collect_rollouts(&root, &mut files).map_err(|e| e.to_string())?;
                }
            }
        }
        SelectedHarness::Pi(_) => {
            let mut roots = pi::session_roots(config, project)?;
            roots.push(config.join("sessions"));
            for root in roots {
                files.extend(pi::session_files(&root)?);
            }
            files.extend(cached.keys().filter(|path| path.is_file()).cloned());
        }
        SelectedHarness::Kiro(_) => {
            let root = config.join("sessions/cli");
            crate::harness::claude::skills::ensure_plain_components(&root)?;
            if root.is_dir() {
                for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("");
                    if name.ends_with(".json")
                        && !name.ends_with(".history.json")
                        && path.with_extension("jsonl").is_file()
                    {
                        files.push(path.with_extension("jsonl"));
                    }
                }
            }
        }
        SelectedHarness::Opencode(_) => unreachable!(),
    }
    files.sort();
    files.dedup();
    Ok(files)
}

pub(super) fn source_fingerprint(harness: &SelectedHarness, path: &Path) -> Option<String> {
    match harness {
        SelectedHarness::Claude(_) => {
            ingest::file_fingerprint(path, Some(&ingest::spill_directory(path)))
        }
        SelectedHarness::Codex(_) => {
            ingest::file_fingerprint(path, None).map(|stamp| format!("thread-id-v2|{stamp}"))
        }
        SelectedHarness::Kiro(_) => Some(format!(
            "{}|{}",
            ingest::file_fingerprint(path, None)?,
            ingest::file_fingerprint(&path.with_extension("json"), None)?
        )),
        SelectedHarness::Pi(_) => ingest::file_fingerprint(path, None),
        SelectedHarness::Opencode(_) => None,
    }
}

fn read_metadata(
    harness: &SelectedHarness,
    path: &Path,
    fingerprint: Option<String>,
) -> Result<Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err("Chat source is not a plain file".into());
    }
    if matches!(harness, SelectedHarness::Kiro(_)) {
        let header = kiro::read_header(&path.with_extension("json"))
            .map_err(|e| e.to_string())?
            .ok_or("Invalid Kiro session header")?;
        if path.file_stem().and_then(|name| name.to_str()) != Some(&header.session_id)
            || header.cwd.trim().is_empty()
        {
            return Err("Invalid Kiro session identity".into());
        }
        return Ok(Metadata {
            id: format!("kiro-v2:{}", header.session_id),
            project: header.cwd,
            title: header.title,
            updated_at: header.updated_at,
            path: path.to_owned(),
            fingerprint,
        });
    }
    let reader: Box<dyn Read> = if matches!(harness, SelectedHarness::Codex(_)) {
        codex::rollout_reader(path).map_err(|e| e.to_string())?
    } else {
        Box::new(fs::File::open(path).map_err(|e| e.to_string())?)
    };
    let mut reader = BufReader::new(reader.take(8 * 1024 * 1024));
    let mut bytes = Vec::new();
    reader
        .read_until(b'\n', &mut bytes)
        .map_err(|e| e.to_string())?;
    reader
        .take(64 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let records = String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let mut title = super::SessionTitle::default();
    for record in &records {
        title.observe(record);
    }
    let fallback = records.iter().find_map(|record| {
        let text = if record["type"] == "event_msg" && record["payload"]["type"] == "user_message" {
            record["payload"]["message"].as_str().map(str::to_owned)
        } else if record["message"]["role"] == "user" || record["type"] == "user" {
            Some(text_content(&record["message"]["content"]))
        } else {
            None
        };
        text.filter(|text| !text.trim().is_empty())
            .map(|text| text.chars().take(80).collect())
    });
    let (id, project, timestamp) = match harness {
        SelectedHarness::Claude(_) => {
            let key = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .ok_or("Invalid Claude project directory")?;
            let cwd = records
                .iter()
                .find_map(|record| record["cwd"].as_str())
                .unwrap_or(key);
            (
                ingest::qualified_session_id(key, path),
                cwd.to_owned(),
                None,
            )
        }
        SelectedHarness::Codex(_) => {
            let payload = records
                .iter()
                .find(|record| record["type"] == "session_meta")
                .map(|record| &record["payload"])
                .ok_or("No session metadata in the catalog header")?;
            (
                codex::metadata_id(payload)
                    .ok_or("Missing native chat ID")?
                    .to_owned(),
                payload["cwd"]
                    .as_str()
                    .unwrap_or("Unknown project")
                    .to_owned(),
                payload["timestamp"].as_str().map(str::to_owned),
            )
        }
        SelectedHarness::Pi(_) => {
            let header = records
                .first()
                .filter(|record| record["type"] == "session" && record["version"] == 3)
                .ok_or("Invalid Pi v3 header")?;
            (
                format!("pi:{}", header["id"].as_str().ok_or("Missing Pi chat ID")?),
                header["cwd"]
                    .as_str()
                    .ok_or("Missing Pi working directory")?
                    .to_owned(),
                header["timestamp"].as_str().map(str::to_owned),
            )
        }
        SelectedHarness::Kiro(_) | SelectedHarness::Opencode(_) => unreachable!(),
    };
    let title = title
        .finish()
        .or_else(|| {
            records.iter().rev().find_map(|record| {
                (record["type"] == "session_info")
                    .then(|| record["name"].as_str().map(str::to_owned))
                    .flatten()
            })
        })
        .or(fallback);
    let updated_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_millis().to_string())
        .or(timestamp);
    Ok(Metadata {
        id,
        project,
        title,
        updated_at,
        path: path.to_owned(),
        fingerprint,
    })
}

fn text_content(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

pub fn ensure_loaded(
    harness: &SelectedHarness,
    app: &Path,
    id: &str,
) -> Result<ingest::IngestReport, String> {
    let cache = harness.transcript_cache(app);
    let mut connection = ingest::open_database(&cache).map_err(|e| e.to_string())?;
    let (path, project, indexed, catalog_stamp): (String, String, bool, Option<String>) = connection
        .query_row(
            "SELECT source_path, project, indexed, catalog_fingerprint FROM sessions WHERE session_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|e| format!("Chat is no longer in the catalog. Refresh chats. {e}"))?;
    let path = Path::new(&path);
    let fingerprint = if matches!(harness, SelectedHarness::Opencode(_)) {
        catalog_stamp
    } else {
        source_fingerprint(harness, path)
    };
    let unchanged = match &fingerprint {
        Some(stamp) if indexed => {
            ingest::unchanged_source(&connection, &path.to_string_lossy(), stamp)
                .map_err(|e| e.to_string())?
        }
        _ => false,
    };
    if unchanged {
        return Ok(ingest::IngestReport {
            sessions: 1,
            ..Default::default()
        });
    }
    connection
        .execute(
            "UPDATE sessions SET indexed = 0 WHERE session_id = ?1",
            [id],
        )
        .map_err(|e| e.to_string())?;
    let mut report = ingest::IngestReport::default();
    match harness {
        SelectedHarness::Claude(_) => {
            let (lines, messages, malformed, skipped) =
                ingest::ingest_session(&mut connection, path, &project, id, fingerprint)
                    .map_err(|e| e.to_string())?;
            report = ingest::IngestReport {
                sessions: 1,
                lines_read: lines,
                messages_indexed: messages,
                malformed_lines: malformed,
                skipped_state_lines: skipped,
                skipped_files: 0,
            };
        }
        SelectedHarness::Codex(_) => {
            let titles = codex::read_titles(&harness.config_root().join("session_index.jsonl"));
            let (actual, lines, messages, malformed, skipped) =
                codex::ingest_rollout(&mut connection, path, &titles, fingerprint.as_deref())
                    .map_err(|e| e.to_string())?;
            if actual != id {
                return Err("Chat identity changed. Refresh chats.".into());
            }
            report = ingest::IngestReport {
                sessions: 1,
                lines_read: lines,
                messages_indexed: messages,
                malformed_lines: malformed,
                skipped_state_lines: skipped,
                skipped_files: 0,
            };
        }
        SelectedHarness::Pi(_) => {
            if read_metadata(harness, path, fingerprint.clone())?.id != id {
                return Err("Chat identity changed. Refresh chats.".into());
            }
            report = pi::index_file(&mut connection, path, fingerprint.as_deref())
                .map_err(|e| e.to_string())?;
        }
        SelectedHarness::Kiro(_) => {
            let header = kiro::read_header(&path.with_extension("json"))
                .map_err(|e| e.to_string())?
                .ok_or("Kiro header is unavailable")?;
            if format!("kiro-v2:{}", header.session_id) != id {
                return Err("Chat identity changed. Refresh chats.".into());
            }
            report.messages_indexed = kiro::ingest_session(
                &mut connection,
                &header,
                path,
                &mut report,
                fingerprint.as_deref(),
            )
            .map_err(|e| e.to_string())?;
            report.sessions = 1;
        }
        SelectedHarness::Opencode(_) => {
            report = opencode::index_session(&mut connection, id, &project)?;
        }
    }
    Ok(report)
}

pub fn index_all(
    harness: &SelectedHarness,
    app: &Path,
    project: &Path,
    progress: impl Fn(IndexProgress),
) -> Result<ingest::IngestReport, String> {
    let warnings = refresh(harness, app, project)?;
    let sessions =
        ingest::list_sessions(&harness.transcript_cache(app)).map_err(|e| e.to_string())?;
    let mut state = IndexProgress {
        completed: 0,
        total: sessions.len(),
        failed: 0,
        title: None,
    };
    let mut report = ingest::IngestReport {
        skipped_files: warnings.len(),
        ..Default::default()
    };
    progress(state.clone());
    for SessionSummary {
        session_id, title, ..
    } in sessions
    {
        state.title = Some(title.unwrap_or_else(|| session_id.clone()));
        progress(state.clone());
        match ensure_loaded(harness, app, &session_id) {
            Ok(indexed) => {
                report.sessions += indexed.sessions;
                report.lines_read += indexed.lines_read;
                report.messages_indexed += indexed.messages_indexed;
                report.malformed_lines += indexed.malformed_lines;
                report.skipped_state_lines += indexed.skipped_state_lines;
            }
            Err(_) => {
                state.failed += 1;
                report.skipped_files += 1;
            }
        }
        state.completed += 1;
        progress(state.clone());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{
        claude::ClaudeCodeHarness, codex::CodexHarness, kiro::KiroHarness, pi::PiHarness,
        resolve_paths, Client,
    };
    use crate::transcript::{conversion, spine};
    use serde_json::json;
    use std::{cell::RefCell, io::Write};

    fn harness(client: Client, root: &Path) -> SelectedHarness {
        fs::create_dir_all(root).unwrap();
        match client {
            Client::ClaudeCode => SelectedHarness::Claude(ClaudeCodeHarness::new(resolve_paths(
                Some(root.to_str().unwrap()),
                root,
            ))),
            Client::Codex => SelectedHarness::Codex(CodexHarness::new(root.into(), root.into())),
            Client::Pi => SelectedHarness::Pi(PiHarness::new(root.into(), root.into())),
            Client::Kiro => SelectedHarness::Kiro(KiroHarness::new(root.into())),
            Client::Opencode => unreachable!(),
        }
    }

    fn write_session(harness: &SelectedHarness, cwd: &Path, id: &str, text: &str) -> PathBuf {
        let config = harness.config_root();
        let (path, records) = match harness {
            SelectedHarness::Claude(_) => (
                config.join(format!("projects/project/{id}.jsonl")),
                vec![json!({"uuid":"m1","type":"user","cwd":cwd,"message":{"content":text}})],
            ),
            SelectedHarness::Codex(_) => (
                config.join(format!(
                    "sessions/rollout-2026-10-25T10-00-00-{id}.jsonl.zst"
                )),
                vec![
                    json!({"type":"session_meta","payload":{"id":id,"cwd":cwd,"timestamp":"2026-10-25T10:00:00Z","base_instructions":{"text":"x".repeat(96 * 1024)}}}),
                    json!({"type":"event_msg","payload":{"type":"user_message","message":text}}),
                ],
            ),
            SelectedHarness::Pi(_) => (
                pi::session_roots(config, cwd).unwrap()[0].join(format!("{id}.jsonl")),
                vec![
                    json!({"type":"session","version":3,"id":id,"cwd":cwd,"timestamp":"2026-10-25T10:00:00Z"}),
                    json!({"type":"message","id":"m1","parentId":null,"timestamp":"2026-10-25T10:00:01Z","message":{"role":"user","content":[{"type":"text","text":text}]}}),
                ],
            ),
            SelectedHarness::Kiro(_) => {
                let path = config.join(format!("sessions/cli/{id}.jsonl"));
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(
                    path.with_extension("json"),
                    json!({"session_id":id,"cwd":cwd,"title":text}).to_string(),
                )
                .unwrap();
                (
                    path,
                    vec![
                        json!({"kind":"Prompt","data":{"message_id":"m1","content":[{"kind":"Text","data":text}]}}),
                    ],
                )
            }
            SelectedHarness::Opencode(_) => unreachable!(),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let data = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let bytes = if path.extension().is_some_and(|ext| ext == "zst") {
            zstd::encode_all(data.as_bytes(), 0).unwrap()
        } else {
            data.into_bytes()
        };
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn catalog_export_and_selected_loading_leave_other_chats_unindexed() {
        for client in [Client::ClaudeCode, Client::Codex, Client::Pi, Client::Kiro] {
            let temp = crate::test_support::tempdir().unwrap();
            let native = harness(client, &temp.path().join("native"));
            let app = temp.path().join("app");
            let cwd = temp.path().join("cwd");
            let exports = temp.path().join("exports");
            fs::create_dir_all(&cwd).unwrap();
            fs::create_dir_all(&exports).unwrap();
            write_session(
                &native,
                &cwd,
                "00000000-0000-4000-8000-000000000001",
                "first needle",
            );
            write_session(
                &native,
                &cwd,
                "00000000-0000-4000-8000-000000000002",
                "second needle",
            );
            assert_eq!(refresh(&native, &app, &cwd).unwrap(), Vec::<String>::new());
            let cache = native.transcript_cache(&app);
            let sessions = ingest::list_sessions(&cache).unwrap();
            assert_eq!(sessions.len(), 2);
            assert_eq!(sessions.iter().filter(|session| session.indexed).count(), 0);
            let selected = sessions
                .iter()
                .find(|session| session.session_id.contains("000000000001"))
                .unwrap();
            assert_eq!(selected.project, cwd.to_string_lossy());
            assert_eq!(selected.title.as_deref(), Some("first needle"));
            let exported = native
                .transcript_export(&app, &selected.session_id, &exports)
                .unwrap();
            assert!(Path::new(&exported.path).is_dir());
            let connection = ingest::open_database(&cache).unwrap();
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM messages", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM messages_fts", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            drop(connection);
            assert_eq!(
                ensure_loaded(&native, &app, &selected.session_id)
                    .unwrap()
                    .messages_indexed,
                1
            );
            assert_eq!(
                spine::thread_page(&cache, &selected.session_id, 0, 100).unwrap()[0].text,
                "first needle"
            );
            let sessions = ingest::list_sessions(&cache).unwrap();
            assert_eq!(sessions.iter().filter(|session| session.indexed).count(), 1);
            assert_eq!(
                spine::search(&cache, "needle", 10)
                    .unwrap()
                    .iter()
                    .map(|hit| hit.text.as_str())
                    .collect::<Vec<_>>(),
                ["first needle"]
            );
            assert_eq!(
                ensure_loaded(&native, &app, &selected.session_id)
                    .unwrap()
                    .lines_read,
                0
            );
            refresh(&native, &app, &cwd).unwrap();
            assert_eq!(
                ingest::list_sessions(&cache)
                    .unwrap()
                    .iter()
                    .filter(|session| session.indexed)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn dirty_sources_leave_search_until_reloaded_and_progress_counts_failed_chats() {
        let temp = crate::test_support::tempdir().unwrap();
        let native = harness(Client::ClaudeCode, &temp.path().join("native"));
        let app = temp.path().join("app");
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&cwd).unwrap();
        let first = write_session(&native, &cwd, "first", "first needle");
        let second = write_session(&native, &cwd, "second", "second needle");
        refresh(&native, &app, &cwd).unwrap();
        ensure_loaded(&native, &app, "project/first.jsonl").unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&first).unwrap();
        writeln!(
            file,
            "{}",
            json!({"uuid":"m2","type":"user","cwd":cwd,"message":{"content":"updated needle"}})
        )
        .unwrap();
        drop(file);
        refresh(&native, &app, &cwd).unwrap();
        let cache = native.transcript_cache(&app);
        assert_eq!(
            ingest::list_sessions(&cache)
                .unwrap()
                .iter()
                .filter(|session| session.indexed)
                .count(),
            0
        );
        assert_eq!(spine::search(&cache, "needle", 10).unwrap().len(), 0);
        let events = RefCell::new(Vec::new());
        let report = index_all(&native, &app, &cwd, |progress| {
            if progress.title.as_deref() == Some("second needle") && second.exists() {
                fs::remove_file(&second).unwrap();
            }
            events.borrow_mut().push(progress);
        })
        .unwrap();
        let events = events.into_inner();
        assert_eq!((events[0].completed, events[0].total), (0, 2));
        let last = events.last().unwrap();
        assert_eq!((last.completed, last.total, last.failed), (2, 2, 1));
        assert_eq!((report.sessions, report.skipped_files), (1, 1));
        assert_eq!(
            spine::search(&cache, "updated", 10).unwrap()[0].text,
            "updated needle"
        );
    }

    #[test]
    fn conversion_from_metadata_only_catalog_does_not_index_the_source_history() {
        let temp = crate::test_support::tempdir().unwrap();
        let native = harness(Client::ClaudeCode, &temp.path().join("native"));
        let target = harness(Client::Codex, &temp.path().join("target"));
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&cwd).unwrap();
        let app = temp.path().join("app");
        write_session(
            &native,
            &cwd,
            "00000000-0000-4000-8000-000000000001",
            "convert without indexing",
        );
        refresh(&native, &app, &cwd).unwrap();
        let source = ingest::list_sessions(&native.transcript_cache(&app))
            .unwrap()
            .remove(0);
        let converted =
            conversion::convert(&native, &target, &app, &source.session_id, None).unwrap();
        assert!(!ingest::list_sessions(&native.transcript_cache(&app)).unwrap()[0].indexed);
        let destination = ingest::list_sessions(&target.transcript_cache(&app)).unwrap();
        assert_eq!(destination[0].session_id, converted.session_id);
        assert!(!destination[0].indexed);
        ensure_loaded(&target, &app, &converted.session_id).unwrap();
        assert_eq!(
            spine::thread_page(
                &target.transcript_cache(&app),
                &converted.session_id,
                0,
                100
            )
            .unwrap()[0]
                .text,
            "convert without indexing"
        );
    }
}
