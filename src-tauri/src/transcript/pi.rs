//! Pi v3 JSONL sessions. Native records and inline images stay in the archive.
use super::{
    archive::{ArchiveAction, ArchiveReport},
    ingest::{
        open_database, prune_vanished_sessions, replace_indexed_session, IndexedMessage,
        IndexedSession, IngestReport,
    },
    jsonl::{read_bounded_line, LineStatus},
    TranscriptError,
};
use crate::harness::{
    atomic,
    claude::skills::{
        ensure_plain_components, ensure_plain_directory, ensure_plain_file_if_present,
    },
    json_store,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_LINE: usize = 16 * 1024 * 1024;
const FORMAT: &str = "orbit.pi.jsonl";

#[derive(Debug)]
struct Document {
    header: Value,
    entries: Vec<(usize, Value)>,
    errors: Vec<(usize, String)>,
    lines: usize,
}

pub(super) fn stage_conversion(root: &Path, id: &str, bytes: &[u8]) -> Result<(), String> {
    let manifest = json!({"format": FORMAT, "version": 1, "sessionId": id,
        "sha256": atomic::hex(&Sha256::digest(bytes))});
    super::conversion::stage_archive(root, "session.jsonl", bytes, &manifest)
}

pub fn ingest(
    config: &Path,
    cache: &Path,
    project: &Path,
) -> Result<IngestReport, TranscriptError> {
    let project = checked_directory(project).map_err(invalid)?;
    let roots = session_roots(config, &project).map_err(invalid)?;
    let mut connection = open_database(cache)?;
    let mut seen = Vec::new();
    // Retain other custom session directories that were indexed previously.
    let mut statement = connection.prepare("SELECT source_path FROM sessions")?;
    for path in statement.query_map([], |row| row.get::<_, String>(0))? {
        let path = path?;
        if !roots.iter().any(|root| Path::new(&path).starts_with(root))
            && Path::new(&path).is_file()
        {
            seen.push(path);
        }
    }
    drop(statement);
    let mut report = IngestReport {
        sessions: 0,
        lines_read: 0,
        messages_indexed: 0,
        malformed_lines: 0,
        skipped_files: 0,
        skipped_state_lines: 0,
    };
    let mut files = HashSet::new();
    for root in roots {
        files.extend(session_files(&root).map_err(invalid)?);
    }
    for path in files {
        seen.push(path.to_string_lossy().into_owned());
        let doc = match read_document(&path, false) {
            Ok(doc) => doc,
            Err(_) => {
                report.skipped_files += 1;
                continue;
            }
        };
        if doc.header["cwd"]
            .as_str()
            .and_then(|cwd| Path::new(cwd).canonicalize().ok())
            .as_ref()
            != Some(&project)
        {
            continue;
        }
        let mut parents: HashMap<String, Option<String>> = HashMap::new();
        let mut messages = Vec::new();
        for (position, entry) in &doc.entries {
            let Some(id) = entry["id"].as_str() else {
                report.skipped_state_lines += 1;
                continue;
            };
            let parent = entry["parentId"]
                .as_str()
                .and_then(|id| parents.get(id))
                .cloned()
                .flatten();
            let kind = entry["type"].as_str().unwrap_or("");
            let (role, text) = match kind {
                "message" => (
                    entry["message"]["role"].as_str().unwrap_or(""),
                    text_content(&entry["message"]["content"]),
                ),
                "custom_message" => ("custom", text_content(&entry["content"])),
                "compaction" | "branch_summary" => {
                    (kind, entry["summary"].as_str().unwrap_or("").to_owned())
                }
                _ => ("", String::new()),
            };
            if role.is_empty() {
                parents.insert(id.to_owned(), parent);
                report.skipped_state_lines += 1;
                continue;
            }
            let role = if role == "toolResult" {
                "tool_result"
            } else {
                role
            };
            let images = entry["message"]["content"].as_array().map_or(0, |blocks| {
                blocks
                    .iter()
                    .filter(|block| block["type"] == "image")
                    .count()
            });
            messages.push(IndexedMessage {
                uuid: id.to_owned(),
                parent_uuid: parent,
                message_type: role.to_owned(),
                text,
                raw_json: entry.to_string(),
                position: *position,
                attachment: (images > 0)
                    .then(|| format!("{images} inline image(s), preserved in the native archive")),
                tool_result: (role == "tool_result")
                    .then(|| text_content(&entry["message"]["content"])),
                tool_result_note: None,
            });
            parents.insert(id.to_owned(), Some(id.to_owned()));
        }
        let id = format!("pi:{}", doc.header["id"].as_str().unwrap());
        let title = session_title(&doc);
        report.messages_indexed += replace_indexed_session(
            &mut connection,
            IndexedSession {
                id: &id,
                project: doc.header["cwd"].as_str().unwrap(),
                source_path: &path,
                updated_at: doc
                    .entries
                    .last()
                    .and_then(|(_, entry)| entry["timestamp"].as_str())
                    .or(doc.header["timestamp"].as_str()),
                title: title.as_deref(),
            },
            &messages,
            &doc.errors,
        )?;
        report.sessions += 1;
        report.lines_read += doc.lines;
        report.malformed_lines += doc.errors.len();
    }
    prune_vanished_sessions(&mut connection, &seen)?;
    Ok(report)
}

pub fn export_archive(
    config: &Path,
    cache: &Path,
    session: &str,
    destination: &Path,
) -> Result<ArchiveReport, String> {
    let connection = open_database(cache).map_err(|e| e.to_string())?;
    let (source, project): (String, String) = connection
        .query_row(
            "SELECT source_path, project FROM sessions WHERE session_id = ?1",
            [session],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let source = PathBuf::from(source);
    let roots = session_roots(config, Path::new(&project))?;
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    if !roots.iter().any(|root| source.starts_with(root)) {
        return Err("Indexed Pi session is outside its configured session directories.".into());
    }
    let bytes = read_snapshot(&source)?;
    let doc = parse_document(BufReader::new(bytes.as_slice()), true)?;
    if session != format!("pi:{}", doc.header["id"].as_str().unwrap()) {
        return Err("Pi session identity changed. Index again.".into());
    }
    let destination = checked_directory(destination)?;
    let archive = destination.join(format!("pi-{}", Uuid::new_v4()));
    fs::create_dir(&archive).map_err(|e| e.to_string())?;
    create_file(&archive.join("session.jsonl"), &bytes)?;
    let manifest = json!({"format": FORMAT, "version": 1, "sessionId": doc.header["id"], "sha256": atomic::hex(&Sha256::digest(&bytes))});
    create_file(
        &archive.join("manifest.json"),
        manifest.to_string().as_bytes(),
    )?;
    Ok(ArchiveReport {
        action: ArchiveAction::Exported,
        source_session_id: session.into(),
        session_id: session.into(),
        files: 2,
        path: archive.to_string_lossy().into_owned(),
    })
}

pub fn import_archive(
    config: &Path,
    source: &Path,
    project: &Path,
) -> Result<ArchiveReport, String> {
    let source = checked_directory(source)?;
    let manifest_path = source.join("manifest.json");
    ensure_plain_components(&manifest_path)?;
    ensure_plain_file_if_present(&manifest_path)?;
    if fs::metadata(&manifest_path)
        .map_err(|e| e.to_string())?
        .len()
        > 64 * 1024
    {
        return Err("Pi archive manifest is too large.".into());
    }
    let manifest = json_store::read_value(&manifest_path)?;
    if manifest["format"] != FORMAT || manifest["version"] != 1 {
        return Err("Not a supported Orbit Pi archive.".into());
    }
    let file = source.join("session.jsonl");
    let bytes = read_snapshot(&file)?;
    if manifest["sha256"].as_str() != Some(atomic::hex(&Sha256::digest(&bytes)).as_str()) {
        return Err("Pi archive checksum does not match.".into());
    }
    let mut doc = parse_document(BufReader::new(bytes.as_slice()), true)?;
    if manifest["sessionId"] != doc.header["id"] {
        return Err("Pi archive session identity does not match.".into());
    }
    let project = checked_directory(project)?;
    let target = write_session_root(config, &project)?;
    ensure_plain_components(&target)?;
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let target = checked_directory(&target)?;
    let titles: HashSet<String> = session_files(&target)?
        .iter()
        .filter_map(|p| read_document(p, false).ok())
        .filter(|d| {
            d.header["cwd"]
                .as_str()
                .and_then(|cwd| Path::new(cwd).canonicalize().ok())
                .as_ref()
                == Some(&project)
        })
        .filter_map(|d| session_title(&d))
        .collect();
    if let Some(title) = session_title(&doc) {
        let mut name = title.clone();
        let mut suffix = 1;
        while titles.contains(&name) {
            name = format!("{title}-{suffix}");
            suffix += 1;
        }
        if name != title {
            if let Some((_, entry)) = doc
                .entries
                .iter_mut()
                .rev()
                .find(|(_, entry)| entry["type"] == "session_info")
            {
                entry["name"] = json!(name);
            } else {
                let parent = doc
                    .entries
                    .last()
                    .and_then(|(_, entry)| entry["id"].as_str());
                doc.entries.push((0, json!({"type":"session_info", "id": Uuid::new_v4().simple().to_string()[..8], "parentId":parent, "timestamp":doc.header["timestamp"], "name":name})));
            }
        }
    }
    let old_id = format!("pi:{}", doc.header["id"].as_str().unwrap());
    let new_id = Uuid::new_v4().to_string();
    doc.header["id"] = json!(new_id);
    doc.header["cwd"] = json!(native_path(&project));
    doc.header.as_object_mut().unwrap().remove("parentSession");
    let stamp = doc.header["timestamp"]
        .as_str()
        .unwrap()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let destination = target.join(format!("{stamp}_{new_id}.jsonl"));
    let mut data = doc.header.to_string();
    for (_, entry) in doc.entries {
        data.push('\n');
        data.push_str(&entry.to_string());
    }
    data.push('\n');
    create_file(&destination, data.as_bytes())?;
    Ok(ArchiveReport {
        action: ArchiveAction::Imported,
        source_session_id: old_id,
        session_id: format!("pi:{new_id}"),
        files: 1,
        path: destination.to_string_lossy().into_owned(),
    })
}

fn read_document(path: &Path, strict: bool) -> Result<Document, String> {
    ensure_plain_components(path)?;
    ensure_plain_file_if_present(path)?;
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("Pi session exceeds the 256 MiB limit.".into());
    }
    let reader = BufReader::new(
        File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_BYTES + 1),
    );
    parse_document(reader, strict)
}

fn parse_document(mut reader: impl BufRead, strict: bool) -> Result<Document, String> {
    let mut line = Vec::new();
    let mut doc = Document {
        header: Value::Null,
        entries: Vec::new(),
        errors: Vec::new(),
        lines: 0,
    };
    let mut ids = HashSet::new();
    loop {
        line.clear();
        let status =
            read_bounded_line(&mut reader, &mut line, MAX_LINE).map_err(|e| e.to_string())?;
        if status == LineStatus::Eof {
            break;
        }
        doc.lines += 1;
        if matches!(status, LineStatus::Unterminated { .. }) && !strict {
            break;
        }
        let value = if matches!(
            status,
            LineStatus::Complete { oversized: false }
                | LineStatus::Unterminated { oversized: false }
        ) {
            serde_json::from_slice::<Value>(&line).ok()
        } else {
            None
        };
        let Some(value) = value else {
            if strict || doc.lines == 1 {
                return Err("Pi session has invalid or oversized JSONL records. Close the chat and export again.".into());
            }
            doc.errors
                .push((doc.lines, "Invalid or oversized Pi JSONL record".into()));
            continue;
        };
        if doc.lines == 1 {
            if value["type"] != "session"
                || value["version"] != 3
                || value["id"]
                    .as_str()
                    .is_none_or(|id| Uuid::parse_str(id).is_err())
                || value["cwd"].as_str().is_none_or(str::is_empty)
                || value["timestamp"].as_str().is_none_or(str::is_empty)
            {
                return Err(
                    "Only Pi version 3 JSONL sessions with a valid header are supported.".into(),
                );
            }
            doc.header = value;
        } else {
            let id = value["id"].as_str().filter(|id| !id.is_empty());
            let valid = id.is_some_and(|id| !ids.contains(id))
                && value["type"].as_str().is_some()
                && (value["parentId"].is_null()
                    || value["parentId"]
                        .as_str()
                        .is_some_and(|parent| ids.contains(parent) && Some(parent) != id));
            if !valid {
                if strict {
                    return Err("Pi session has duplicate IDs or invalid branch links.".into());
                }
                doc.errors
                    .push((doc.lines, "Invalid Pi entry identity or branch link".into()));
                continue;
            }
            ids.insert(id.unwrap().to_owned());
            doc.entries.push((doc.lines, value));
        }
    }
    if doc.header.is_null() {
        return Err("Pi session has no complete header.".into());
    }
    Ok(doc)
}

fn read_snapshot(path: &Path) -> Result<Vec<u8>, String> {
    ensure_plain_components(path)?;
    ensure_plain_file_if_present(path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Pi session exceeds the 256 MiB limit.".into());
    }
    Ok(bytes)
}

fn session_title(doc: &Document) -> Option<String> {
    doc.entries
        .iter()
        .rev()
        .find_map(|(_, entry)| {
            (entry["type"] == "session_info")
                .then(|| entry["name"].as_str())
                .flatten()
                .filter(|name| !name.trim().is_empty())
                .map(str::to_owned)
        })
        .or_else(|| {
            doc.entries.iter().find_map(|(_, entry)| {
                (entry["message"]["role"] == "user")
                    .then(|| text_content(&entry["message"]["content"]))
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| text.chars().take(80).collect())
            })
        })
}

fn text_content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.into();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn session_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    ensure_plain_components(root)?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    ensure_plain_directory(root)?;
    let mut files = Vec::new();
    let mut directories = vec![(root.to_owned(), 0)];
    while let Some((directory, depth)) = directories.pop() {
        for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if atomic::is_link_or_reparse(&metadata) {
                continue;
            }
            if metadata.is_dir() && depth == 0 {
                directories.push((path, 1));
            } else if metadata.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(path.canonicalize().map_err(|e| e.to_string())?);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn session_roots(config: &Path, project: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots =
        vec![
            crate::history::canonical_location(&default_session_root(config, project)?)
                .ok_or("Invalid Pi session root")?,
        ];
    let root = write_session_root(config, project)?;
    if !roots.iter().any(|parent| root.starts_with(parent)) {
        roots.push(root);
    }
    Ok(roots)
}

fn write_session_root(config: &Path, project: &Path) -> Result<PathBuf, String> {
    let user = json_store::read_value(&config.join("settings.json"))?;
    let local = json_store::read_value(&project.join(".pi/settings.json"))?;
    let configured = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            local
                .get("sessionDir")
                .or_else(|| user.get("sessionDir"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        });
    let root = if let Some(path) = configured {
        if path.starts_with("~") {
            let home = std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .ok_or("Cannot resolve Pi session home directory")?;
            PathBuf::from(home).join(path.strip_prefix("~").map_err(|e| e.to_string())?)
        } else if path.is_absolute() {
            path
        } else {
            project.join(path)
        }
    } else {
        default_session_root(config, project)?
    };
    ensure_plain_components(&root)?;
    crate::history::canonical_location(&root).ok_or_else(|| "Invalid Pi session directory".into())
}

fn default_session_root(config: &Path, project: &Path) -> Result<PathBuf, String> {
    let cwd = native_path(&checked_directory(project)?);
    let encoded = cwd
        .strip_prefix(['/', '\\'])
        .unwrap_or(&cwd)
        .replace(['/', '\\', ':'], "-");
    Ok(config.join("sessions").join(format!("--{encoded}--")))
}

fn checked_directory(path: &Path) -> Result<PathBuf, String> {
    ensure_plain_components(path)?;
    ensure_plain_directory(path)?;
    path.canonicalize().map_err(|e| e.to_string())
}

fn native_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
}

fn create_file(path: &Path, data: &[u8]) -> Result<(), String> {
    ensure_plain_components(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(data)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Pi archive write failed; inspect {}: {e}", path.display()))
}

fn invalid(message: String) -> TranscriptError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{ingest::list_sessions, spine};

    fn fixture(project: &Path) -> Vec<Value> {
        vec![
            json!({"type":"session","version":3,"id":"10000000-0000-4000-8000-000000000001","cwd":native_path(project),"timestamp":"2026-10-23T12:00:00.000Z","parentSession":"/previous/chat.jsonl"}),
            json!({"type":"model_change","id":"s0","parentId":null,"timestamp":"2026-10-23T12:00:00Z","modelId":"fixture"}),
            json!({"type":"message","id":"u1","parentId":"s0","timestamp":"2026-10-23T12:00:01Z","message":{"role":"user","content":"Question"}}),
            json!({"type":"message","id":"a1","parentId":"u1","timestamp":"2026-10-23T12:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"First answer"}]}}),
            json!({"type":"compaction","id":"c1","parentId":"a1","timestamp":"2026-10-23T12:00:03Z","summary":"Summary","firstKeptEntryId":"u1","tokensBefore":10}),
            json!({"type":"message","id":"u2","parentId":"c1","timestamp":"2026-10-23T12:00:04Z","message":{"role":"user","content":[{"type":"text","text":"Follow up"},{"type":"image","data":"AAAA","mimeType":"image/png"}]}}),
            json!({"type":"message","id":"b1","parentId":"u1","timestamp":"2026-10-23T12:00:05Z","message":{"role":"assistant","content":[{"type":"text","text":"Alternative answer"}]}}),
            json!({"type":"session_info","id":"n1","parentId":"b1","timestamp":"2026-10-23T12:00:06Z","name":"Agent"}),
            json!({"type":"custom","id":"x1","parentId":"n1","timestamp":"2026-10-23T12:00:07Z","customType":"future","data":{"preserve":true}}),
        ]
    }

    fn write_fixture(path: &Path, records: &[Value]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let data = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(path, data).unwrap();
    }

    #[test]
    fn indexing_scopes_default_and_shared_directories_to_the_project_and_retains_prior_projects() {
        for shared in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let config = temp.path().join("agent");
            let cache = temp.path().join("cache");
            let a = temp.path().join("a");
            let b = temp.path().join("b");
            for path in [&config, &a, &b] {
                fs::create_dir_all(path).unwrap();
            }
            if shared {
                fs::write(
                    config.join("settings.json"),
                    json!({"sessionDir":temp.path().join("shared")}).to_string(),
                )
                .unwrap();
            }
            for (project, id) in [
                (&a, "10000000-0000-4000-8000-000000000001"),
                (&b, "10000000-0000-4000-8000-000000000002"),
            ] {
                let mut records = fixture(project);
                records[0]["id"] = json!(id);
                let root = write_session_root(&config, project).unwrap();
                write_fixture(&root.join(format!("{id}.jsonl")), &records);
            }
            assert_eq!(ingest(&config, &cache, &a).unwrap().sessions, 1);
            assert_eq!(list_sessions(&cache).unwrap().len(), 1);
            assert_eq!(ingest(&config, &cache, &b).unwrap().sessions, 1);
            assert_eq!(list_sessions(&cache).unwrap().len(), 2);
        }
    }

    #[test]
    fn native_archive_preserves_branches_images_unknown_records_and_imports_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("agent");
        let cache = temp.path().join("cache");
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        let exports = temp.path().join("exports");
        for project in [&a, &b] {
            fs::create_dir_all(project.join(".pi")).unwrap();
            fs::write(
                project.join(".pi/settings.json"),
                r#"{"sessionDir":".pi/local-sessions"}"#,
            )
            .unwrap();
        }
        fs::create_dir(&config).unwrap();
        fs::create_dir(&exports).unwrap();
        let a = a.canonicalize().unwrap();
        let b = b.canonicalize().unwrap();
        let file = write_session_root(&config, &a)
            .unwrap()
            .join("fixture.jsonl");
        let records = fixture(&a);
        write_fixture(&file, &records);
        let original = fs::read(&file).unwrap();
        let id = "pi:10000000-0000-4000-8000-000000000001";
        let report = ingest(&config, &cache, &a).unwrap();
        assert_eq!(report.messages_indexed, 5);
        let messages = spine::thread_page(&cache, id, 0, 20).unwrap();
        let follow_up = messages.iter().find(|m| m.uuid == "u2").unwrap();
        assert_eq!(follow_up.parent_uuid.as_deref(), Some("c1"));
        assert!(follow_up
            .attachment
            .as_deref()
            .unwrap()
            .contains("1 inline image"));
        for id in ["a1", "b1"] {
            assert_eq!(
                messages
                    .iter()
                    .find(|m| m.uuid == id)
                    .unwrap()
                    .parent_uuid
                    .as_deref(),
                Some("u1")
            );
        }
        let exported = export_archive(&config, &cache, id, &exports).unwrap();
        let archive = PathBuf::from(exported.path);
        assert_eq!(fs::read(archive.join("session.jsonl")).unwrap(), original);
        let first = import_archive(&config, &archive, &b).unwrap();
        let second = import_archive(&config, &archive, &b).unwrap();
        assert_ne!(first.session_id, id);
        assert_ne!(first.session_id, second.session_id);
        let imported = read_document(Path::new(&second.path), true).unwrap();
        assert_eq!(imported.header["cwd"], native_path(&b));
        assert!(imported.header.get("parentSession").is_none());
        assert_eq!(session_title(&imported).as_deref(), Some("Agent-1"));
        for index in [0, 1, 2, 3, 4, 5, 7] {
            assert_eq!(imported.entries[index].1, records[index + 1]);
        }
        ingest(&config, &cache, &b).unwrap();
        assert_eq!(list_sessions(&cache).unwrap().len(), 3);
        assert_eq!(fs::read(&file).unwrap(), original);
        assert_eq!(fs::read(archive.join("session.jsonl")).unwrap(), original);
        let mut tampered = original;
        tampered.push(b'\n');
        fs::write(archive.join("session.jsonl"), tampered).unwrap();
        assert!(import_archive(&config, &archive, &b)
            .unwrap_err()
            .contains("checksum"));
        assert_eq!(
            session_files(&write_session_root(&config, &b).unwrap())
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn indexing_tolerates_unfinished_tail_but_import_rejects_bad_branch_records() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("fixture.jsonl");
        let records = fixture(temp.path());
        write_fixture(&file, &records);
        OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"{\"unfinished\":")
            .unwrap();
        assert_eq!(read_document(&file, false).unwrap().entries.len(), 8);
        assert!(read_document(&file, true).is_err());
        let mut broken = records;
        broken[3]["parentId"] = json!("missing");
        write_fixture(&file, &broken);
        assert!(read_document(&file, true).unwrap_err().contains("branch"));
    }
}
