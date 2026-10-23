use super::{database_path, SessionSummary, SessionTitle, TranscriptError};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{collections::HashMap, fs, path::Path};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestReport {
    pub sessions: usize,
    pub lines_read: usize,
    pub messages_indexed: usize,
    pub malformed_lines: usize,
    pub skipped_files: usize,
    pub skipped_state_lines: usize,
}

pub(super) struct IndexedMessage {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub message_type: String,
    pub text: String,
    pub raw_json: String,
    pub position: usize,
    pub attachment: Option<String>,
    pub tool_result: Option<String>,
    pub tool_result_note: Option<String>,
}

pub(super) struct IndexedSession<'a> {
    pub id: &'a str,
    pub project: &'a str,
    pub source_path: &'a Path,
    pub updated_at: Option<&'a str>,
    pub title: Option<&'a str>,
}

/// Version 3 fixed case-sensitive filename collisions. Version 4 adds native
/// chat titles without discarding an existing version 3 message cache.
const SCHEMA_VERSION: i64 = 4;

pub fn open_database(app_data_dir: &Path) -> Result<Connection, TranscriptError> {
    fs::create_dir_all(app_data_dir)?;
    let mut connection = Connection::open(database_path(app_data_dir))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == SCHEMA_VERSION {
        return Ok(connection);
    }
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(TranscriptError::UnsupportedSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == SCHEMA_VERSION {
        transaction.commit()?;
        return Ok(connection);
    }
    if version < 3 {
        transaction.execute_batch(
            "DROP TABLE IF EXISTS messages;\n\
             DROP TABLE IF EXISTS messages_fts;\n\
             DROP TABLE IF EXISTS transcript_ingest_errors;\n\
             DROP TABLE IF EXISTS sessions;",
        )?;
    }
    transaction.execute_batch(include_str!("../../migrations/001_transcripts.sql"))?;
    if version == 3 {
        transaction.execute("ALTER TABLE sessions ADD COLUMN title TEXT", [])?;
    }
    if version != SCHEMA_VERSION {
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    transaction.commit()?;
    Ok(connection)
}

pub fn ingest_projects(
    config_root: &Path,
    app_data_dir: &Path,
) -> Result<IngestReport, TranscriptError> {
    let projects = config_root.join("projects");
    if !projects.is_dir() {
        return Err(TranscriptError::InvalidRoot);
    }
    let mut connection = open_database(app_data_dir)?;
    let mut report = IngestReport {
        sessions: 0,
        lines_read: 0,
        messages_indexed: 0,
        malformed_lines: 0,
        skipped_files: 0,
        skipped_state_lines: 0,
    };
    let mut seen_paths = Vec::new();
    for project_entry in fs::read_dir(&projects)? {
        let project_entry = project_entry?;
        if !project_entry.file_type()?.is_dir() {
            continue;
        }
        let project_path = project_entry.path();
        let project = project_entry.file_name().to_string_lossy().into_owned();
        for session_entry in fs::read_dir(&project_path)? {
            let session_entry = session_entry?;
            if !session_entry.file_type()?.is_file()
                || session_entry
                    .path()
                    .extension()
                    .is_none_or(|extension| !extension.eq_ignore_ascii_case("jsonl"))
            {
                continue;
            }
            let path = session_entry.path();
            seen_paths.push(path.to_string_lossy().into_owned());
            let session_id = qualified_session_id(&project, &path);
            let session_result = match ingest_session(&mut connection, &path, &project, &session_id)
            {
                Ok(result) => result,
                Err(TranscriptError::Io(_)) => {
                    report.skipped_files += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            report.sessions += 1;
            report.lines_read += session_result.0;
            report.messages_indexed += session_result.1;
            report.malformed_lines += session_result.2;
            report.skipped_state_lines += session_result.3;
        }
    }
    prune_vanished_sessions(&mut connection, &seen_paths)?;
    Ok(report)
}

/// The database identity includes the complete source name so files differing
/// only by extension case remain distinct on case-sensitive filesystems.
fn qualified_session_id(project: &str, path: &Path) -> String {
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    format!("{project}/{file_name}")
}

/// Sessions whose source file is gone are stale cache entries. Runs only
/// after the full traversal succeeded, so a partial walk can never delete a
/// session whose file was simply not visited yet.
pub(super) fn prune_vanished_sessions(
    connection: &mut Connection,
    ingested_paths: &[String],
) -> Result<(), TranscriptError> {
    let mut statement = connection.prepare("SELECT session_id, source_path FROM sessions")?;
    let vanished = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let seen = ingested_paths
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let transaction = connection.transaction()?;
    for (session_id, source_path) in vanished {
        if !seen.contains(&source_path) {
            transaction.execute(
                "DELETE FROM messages_fts WHERE session_id = ?1",
                [&session_id],
            )?;
            transaction.execute("DELETE FROM sessions WHERE session_id = ?1", [&session_id])?;
        }
    }
    transaction.commit()?;
    Ok(())
}

fn ingest_session(
    connection: &mut Connection,
    path: &Path,
    project: &str,
    session_id: &str,
) -> Result<(usize, usize, usize, usize), TranscriptError> {
    let bytes = fs::read(path)?;
    let input = String::from_utf8_lossy(&bytes);
    let modified = fs::metadata(path)?
        .modified()
        .ok()
        .map(|time| format!("{time:?}"));
    let parsed = input
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line, serde_json::from_str::<Value>(line)))
        .collect::<Vec<_>>();
    let valid_values = parsed
        .iter()
        .filter_map(|(_, _, result)| result.as_ref().ok())
        .collect::<Vec<_>>();
    let tool_ids = tool_call_owners(&valid_values);
    let mut title = SessionTitle::default();
    for value in &valid_values {
        title.observe(value);
    }
    let fallback_message = valid_values
        .iter()
        .find_map(|value| value.get("uuid").and_then(Value::as_str))
        .map(str::to_owned);
    let spills = read_spills(path, &tool_ids, fallback_message.as_deref());
    let mut malformed = 0usize;
    let mut skipped_state = 0usize;
    let mut messages = Vec::new();
    let mut errors = Vec::new();
    for (position, line, value) in parsed {
        if line.trim().is_empty() {
            skipped_state += 1;
            continue;
        }
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                malformed += 1;
                errors.push((position, error.to_string()));
                continue;
            }
        };
        if is_state_line(&value) {
            skipped_state += 1;
            continue;
        }
        let Some(uuid) = value.get("uuid").and_then(Value::as_str) else {
            malformed += 1;
            errors.push((position, "JSON object is missing a string uuid".to_owned()));
            continue;
        };
        let parent_uuid = value.get("parentUuid").and_then(Value::as_str);
        let message_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let text = message_text(&value);
        let attachment = attachment_text(&value);
        let (tool_result, tool_result_note) = spills.get(uuid).cloned().unwrap_or((None, None));
        messages.push(IndexedMessage {
            uuid: uuid.to_owned(),
            parent_uuid: parent_uuid.map(str::to_owned),
            message_type: message_type.to_owned(),
            text,
            raw_json: serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned()),
            position,
            attachment,
            tool_result,
            tool_result_note,
        });
    }
    let indexed = replace_indexed_session(
        connection,
        IndexedSession {
            id: session_id,
            project,
            source_path: path,
            updated_at: modified.as_deref(),
            title: title.finish().as_deref(),
        },
        &messages,
        &errors,
    )?;
    Ok((input.lines().count(), indexed, malformed, skipped_state))
}

pub(super) fn replace_indexed_session(
    connection: &mut Connection,
    session: IndexedSession<'_>,
    messages: &[IndexedMessage],
    errors: &[(usize, String)],
) -> Result<usize, TranscriptError> {
    let IndexedSession {
        id: session_id,
        project,
        source_path,
        updated_at,
        title,
    } = session;
    let transaction = connection.transaction()?;
    transaction.execute(
        "DELETE FROM messages_fts WHERE session_id = ?1",
        [session_id],
    )?;
    transaction.execute("DELETE FROM sessions WHERE session_id = ?1", [session_id])?;
    transaction.execute(
        "INSERT INTO sessions(session_id, project, source_path, updated_at, message_count, title) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
        params![session_id, project, source_path.to_string_lossy(), updated_at, title],
    )?;
    for (position, error) in errors {
        transaction.execute(
            "INSERT INTO transcript_ingest_errors(session_id, position, error) VALUES (?1, ?2, ?3)",
            params![session_id, position, error],
        )?;
    }
    for message in messages {
        transaction.execute(
            "INSERT OR REPLACE INTO messages(session_id, uuid, parent_uuid, message_type, text, raw_json, position, attachment, tool_result, tool_result_note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![session_id, message.uuid, message.parent_uuid, message.message_type, message.text, message.raw_json, message.position, message.attachment, message.tool_result, message.tool_result_note],
        )?;
        transaction.execute(
            "DELETE FROM messages_fts WHERE session_id = ?1 AND uuid = ?2",
            params![session_id, message.uuid],
        )?;
        let search_text = match &message.tool_result {
            Some(result) => format!("{}\n{result}", message.text),
            None => message.text.clone(),
        };
        transaction.execute(
            "INSERT INTO messages_fts(session_id, uuid, text) VALUES (?1, ?2, ?3)",
            params![session_id, message.uuid, search_text],
        )?;
    }
    let indexed = transaction.query_row(
        "SELECT count(*) FROM messages WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, i64>(0),
    )? as usize;
    transaction.execute(
        "UPDATE sessions SET message_count = ?2 WHERE session_id = ?1",
        params![session_id, indexed],
    )?;
    transaction.commit()?;
    Ok(indexed)
}

/// Valid state records that carry no `uuid` and no conversation content. They
/// are healthy lines, not malformed ones, so they are counted separately and
/// never reach the error table.
fn is_state_line(value: &Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
        Some(kind) => matches!(
            kind,
            "last-prompt"
                | "custom-title"
                | "atis-latch"
                | "mode"
                | "permission-mode"
                | "ai-title"
                | "cost-state"
                | "file-history-snapshot"
                | "queue-operation"
                | "summary"
        ),
        None => false,
    }
}

fn message_text(value: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(message) = value.get("message") {
        collect_text(message, &mut parts);
    }
    if let Some(content) = value.get("content") {
        collect_text(content, &mut parts);
    }
    for key in ["rendered", "lastPrompt", "toolUseResult"] {
        if let Some(candidate) = value.get(key) {
            collect_text(candidate, &mut parts);
        }
    }
    parts.join("\n")
}

fn attachment_text(value: &Value) -> Option<String> {
    value.get("attachment").map(|attachment| match attachment {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

fn collect_text(value: &Value, parts: &mut Vec<String>) {
    match value {
        Value::String(text) => parts.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| collect_text(item, parts)),
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                parts.push(text.to_owned());
            } else if let Some(content) = map.get("content") {
                collect_text(content, parts);
            }
        }
        _ => {}
    }
}

fn tool_call_owners(values: &[&Value]) -> HashMap<String, String> {
    let mut owners = HashMap::new();
    for value in values {
        if let (Some(message_uuid), Some(message)) = (
            value.get("uuid").and_then(Value::as_str),
            value.get("message"),
        ) {
            collect_tool_ids(message, message_uuid, &mut owners);
        }
    }
    owners
}

fn collect_tool_ids(value: &Value, message_uuid: &str, owners: &mut HashMap<String, String>) {
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_tool_ids(item, message_uuid, owners)),
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("tool_use") {
                if let Some(id) = map.get("id").and_then(Value::as_str) {
                    owners.insert(id.to_owned(), message_uuid.to_owned());
                }
            }
            for child in map.values() {
                collect_tool_ids(child, message_uuid, owners);
            }
        }
        _ => {}
    }
}

fn read_spills(
    session_path: &Path,
    tool_ids: &HashMap<String, String>,
    fallback_message: Option<&str>,
) -> HashMap<String, (Option<String>, Option<String>)> {
    let session_id = session_path.file_stem().unwrap_or_default();
    let spill_dir = session_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(session_id)
        .join("tool-results");
    let mut attached = HashMap::new();
    let Ok(entries) = fs::read_dir(spill_dir) else {
        return attached;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|file_type| file_type.is_file())
            || entry.path().extension().is_none_or(|ext| ext != "json")
        {
            continue;
        }
        let path = entry.path();
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let raw = fs::read_to_string(&path);
        let target = tool_ids.get(&stem).cloned().or_else(|| {
            raw.as_ref().ok().and_then(|raw| {
                serde_json::from_str::<Value>(raw).ok().and_then(|value| {
                    find_tool_link(&value).and_then(|id| tool_ids.get(&id).cloned())
                })
            })
        });
        match raw {
            Ok(raw) => {
                let result = serde_json::from_str::<Value>(&raw).map(|value| {
                    let mut parts = Vec::new();
                    collect_text(&value, &mut parts);
                    if parts.is_empty() {
                        value.to_string()
                    } else {
                        parts.join("\n")
                    }
                });
                match (target, result) {
                    (Some(uuid), Ok(text)) => {
                        attached.insert(uuid, (Some(text), None));
                    }
                    (Some(uuid), Err(_)) => {
                        attached.insert(
                            uuid,
                            (
                                None,
                                Some("Tool result spill could not be parsed.".to_owned()),
                            ),
                        );
                    }
                    (None, Err(_)) => {
                        let key = fallback_message.map(str::to_owned).unwrap_or(stem.clone());
                        attach_spill_note(
                            &mut attached,
                            key,
                            format!("Unlinked tool result spill {stem} could not be parsed."),
                        );
                    }
                    (None, Ok(_)) => {
                        let key = fallback_message.map(str::to_owned).unwrap_or(stem.clone());
                        attach_spill_note(
                            &mut attached,
                            key,
                            format!("Tool result spill {stem} has no matching tool call."),
                        );
                    }
                }
            }
            Err(_) => {
                let key = target
                    .or_else(|| fallback_message.map(str::to_owned))
                    .unwrap_or(stem.clone());
                attach_spill_note(
                    &mut attached,
                    key,
                    format!("Tool result spill {stem} is unavailable or unlinked."),
                );
            }
        }
    }
    attached
}

fn attach_spill_note(
    attached: &mut HashMap<String, (Option<String>, Option<String>)>,
    uuid: String,
    note: String,
) {
    let entry = attached.entry(uuid).or_insert((None, None));
    entry.1 = Some(match entry.1.take() {
        Some(previous) => format!("{previous}\n{note}"),
        None => note,
    });
}

fn find_tool_link(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => map.iter().find_map(|(key, child)| {
            if matches!(key.as_str(), "tool_use_id" | "toolUseId" | "tool_call_id") {
                child.as_str().map(str::to_owned)
            } else {
                find_tool_link(child)
            }
        }),
        Value::Array(items) => items.iter().find_map(find_tool_link),
        _ => None,
    }
}

pub fn list_sessions(app_data_dir: &Path) -> Result<Vec<SessionSummary>, TranscriptError> {
    let connection = open_database(app_data_dir)?;
    let mut statement = connection.prepare("SELECT session_id, project, updated_at, message_count, title FROM sessions ORDER BY updated_at DESC, session_id")?;
    let rows = statement.query_map([], |row| {
        Ok(SessionSummary {
            session_id: row.get(0)?,
            title: row.get(4)?,
            project: row.get(1)?,
            updated_at: row.get(2)?,
            message_count: row.get::<_, i64>(3)? as usize,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
fn read_spills_for_tests(session_path: &Path) -> HashMap<String, (Option<String>, Option<String>)> {
    read_spills(session_path, &HashMap::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn adding_titles_preserves_the_existing_version_three_cache() {
        let directory = tempfile::tempdir().unwrap();
        let connection = Connection::open(database_path(directory.path())).unwrap();
        connection.execute_batch("CREATE TABLE sessions (session_id TEXT PRIMARY KEY, project TEXT NOT NULL, source_path TEXT NOT NULL, updated_at TEXT, message_count INTEGER NOT NULL DEFAULT 0); INSERT INTO sessions VALUES ('existing.jsonl', 'project', 'source.jsonl', NULL, 7); PRAGMA user_version = 3;").unwrap();
        drop(connection);
        let sessions = list_sessions(directory.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "existing.jsonl");
        assert_eq!(sessions[0].message_count, 7);
        assert_eq!(sessions[0].title, None);
    }

    #[test]
    fn ingestion_counts_source_lines_and_indexes_valid_messages() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        fs::create_dir_all(&project).unwrap();
        let source = project.join("session-1.jsonl");
        let mut file = fs::File::create(&source).unwrap();
        writeln!(
            file,
            "{{\"uuid\":\"a\",\"type\":\"user\",\"message\":{{\"content\":\"known phrase\"}}}}"
        )
        .unwrap();
        writeln!(file, "{{bad json").unwrap();
        writeln!(file, "{{\"uuid\":\"b\",\"parentUuid\":\"a\",\"type\":\"future-kind\",\"newField\":{{\"x\":1}},\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"answer\"}}]}}}}").unwrap();
        let report = ingest_projects(temp.path(), &temp.path().join("app-data")).unwrap();
        assert_eq!(report.lines_read, 3);
        assert_eq!(report.messages_indexed, 2);
        assert_eq!(report.malformed_lines, 1);
        let connection = open_database(&temp.path().join("app-data")).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let raw: String = connection
            .query_row(
                "SELECT raw_json FROM messages WHERE uuid = 'b'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(raw.contains("future-kind") && raw.contains("newField"));
        let indexed: i64 = connection
            .query_row(
                "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'phrase'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
        ingest_projects(temp.path(), &temp.path().join("app-data")).unwrap();
        let indexed_after_reingest: i64 = connection
            .query_row(
                "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'phrase'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed_after_reingest, 1);
    }

    #[test]
    fn spill_reattaches_by_tool_call_link_in_raw_json() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        let spills = project.join("session-1").join("tool-results");
        fs::create_dir_all(&spills).unwrap();
        let source = project.join("session-1.jsonl");
        fs::write(&source, "{\"uuid\":\"m1\",\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"call-7\",\"name\":\"Read\"}]}}\n").unwrap();
        fs::write(
            spills.join("spill-blob.json"),
            "{\"tool_use_id\":\"call-7\",\"content\":[{\"type\":\"text\",\"text\":\"spill output\"}]}",
        )
        .unwrap();
        ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        let connection = open_database(&temp.path().join("db")).unwrap();
        let result: String = connection
            .query_row(
                "SELECT tool_result FROM messages WHERE uuid = 'm1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(result.contains("spill output"));
    }

    #[test]
    fn unlinked_spill_is_reported_on_the_session_thread() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        let spills = project.join("session-1").join("tool-results");
        fs::create_dir_all(&spills).unwrap();
        let source = project.join("session-1.jsonl");
        fs::write(
            &source,
            "{\"uuid\":\"first\",\"type\":\"user\",\"message\":{\"content\":\"start\"}}\n",
        )
        .unwrap();
        fs::write(spills.join("unlinked.json"), "{\"text\":\"orphan result\"}").unwrap();
        ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        let connection = open_database(&temp.path().join("db")).unwrap();
        let note: String = connection
            .query_row(
                "SELECT tool_result_note FROM messages WHERE uuid = 'first'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(note.contains("no matching tool call"));
    }

    #[test]
    fn missing_spill_directory_is_non_fatal() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_spills_for_tests(&temp.path().join("missing.jsonl")).is_empty());
    }

    #[test]
    fn state_lines_and_blank_lines_are_skipped_not_malformed() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        fs::create_dir_all(&project).unwrap();
        let source = project.join("session-1.jsonl");
        let mut file = fs::File::create(&source).unwrap();
        writeln!(
            file,
            "{{\"type\":\"summary\",\"summary\":\"chat about c++\"}}"
        )
        .unwrap();
        writeln!(
            file,
            "{{\"uuid\":\"m1\",\"type\":\"user\",\"message\":{{\"content\":\"hello\"}}}}"
        )
        .unwrap();
        writeln!(file, "{{\"type\":\"last-prompt\",\"prompt\":\"do thing\"}}").unwrap();
        writeln!(file, "{{\"type\":\"mode\",\"mode\":\"bypassPermissions\"}}").unwrap();
        writeln!(
            file,
            "{{\"type\":\"custom-title\",\"customTitle\":\"Agent\"}}"
        )
        .unwrap();
        writeln!(
            file,
            "{{\"type\":\"ai-title\",\"aiTitle\":\"Generated name\"}}"
        )
        .unwrap();
        writeln!(file).unwrap();
        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.lines_read, 7);
        assert_eq!(report.messages_indexed, 1);
        assert_eq!(report.malformed_lines, 0);
        assert_eq!(report.skipped_state_lines, 6);
        assert_eq!(
            list_sessions(&temp.path().join("db")).unwrap()[0]
                .title
                .as_deref(),
            Some("Agent")
        );
        let connection = open_database(&temp.path().join("db")).unwrap();
        let errors: i64 = connection
            .query_row("SELECT count(*) FROM transcript_ingest_errors", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(errors, 0);
    }

    #[test]
    fn non_utf8_session_file_does_not_abort_the_walk() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("good.jsonl"),
            "{\"uuid\":\"m1\",\"type\":\"user\",\"message\":{\"content\":\"good text\"}}\n",
        )
        .unwrap();
        fs::write(project.join("bad.jsonl"), [0xFF, 0xFE, 0x00, 0x62]).unwrap();
        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.sessions, 2);
        assert_eq!(report.skipped_files, 0);
        assert_eq!(report.messages_indexed, 1);
        let connection = open_database(&temp.path().join("db")).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn same_filename_in_two_projects_keeps_two_sessions() {
        let temp = tempfile::tempdir().unwrap();
        for (project, text) in [("alpha", "alpha message"), ("beta", "beta message")] {
            let project_dir = temp.path().join("projects").join(project);
            fs::create_dir_all(&project_dir).unwrap();
            fs::write(
                project_dir.join("dup.jsonl"),
                format!("{{\"uuid\":\"m1\",\"type\":\"user\",\"message\":{{\"content\":\"{text}\"}}}}\n"),
            )
            .unwrap();
        }
        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.sessions, 2);
        let connection = open_database(&temp.path().join("db")).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let alpha: String = connection
            .query_row(
                "SELECT text FROM messages WHERE session_id = 'alpha/dup.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alpha, "alpha message");
        let beta: String = connection
            .query_row(
                "SELECT text FROM messages WHERE session_id = 'beta/dup.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(beta, "beta message");
    }

    #[test]
    fn same_stem_with_case_distinct_extensions_keeps_two_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("session.jsonl"),
            "{\"uuid\":\"lower\",\"type\":\"user\",\"message\":{\"content\":\"lower extension\"}}\n",
        )
        .unwrap();
        fs::write(
            project.join("session.JSONL"),
            "{\"uuid\":\"upper\",\"type\":\"user\",\"message\":{\"content\":\"upper extension\"}}\n",
        )
        .unwrap();
        if fs::read_dir(&project).unwrap().count() < 2 {
            return; // The filesystem treats extension case as the same name.
        }

        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.sessions, 2);
        let connection = open_database(&temp.path().join("db")).unwrap();
        let mut statement = connection
            .prepare("SELECT session_id FROM sessions ORDER BY session_id")
            .unwrap();
        let session_ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            session_ids,
            ["munged/session.JSONL", "munged/session.jsonl"]
        );
        let lower: String = connection
            .query_row(
                "SELECT text FROM messages WHERE uuid = 'lower'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let upper: String = connection
            .query_row(
                "SELECT text FROM messages WHERE uuid = 'upper'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lower, "lower extension");
        assert_eq!(upper, "upper extension");
    }

    #[test]
    fn deleted_source_file_is_pruned_after_a_full_reingest() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("keep.jsonl"),
            "{\"uuid\":\"m1\",\"type\":\"user\",\"message\":{\"content\":\"kept\"}}\n",
        )
        .unwrap();
        fs::write(
            project.join("gone.jsonl"),
            "{\"uuid\":\"m2\",\"type\":\"user\",\"message\":{\"content\":\"doomed\"}}\n",
        )
        .unwrap();
        ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        fs::remove_file(project.join("gone.jsonl")).unwrap();
        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.sessions, 1);
        let connection = open_database(&temp.path().join("db")).unwrap();
        let sessions: i64 = connection
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sessions, 1);
        let messages: i64 = connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(messages, 1);
        let fts: i64 = connection
            .query_row(
                "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'doomed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts, 0);
    }

    #[test]
    fn uppercase_jsonl_extension_is_ingested() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("caps.JSONL"),
            "{\"uuid\":\"m1\",\"type\":\"user\",\"message\":{\"content\":\"caps text\"}}\n",
        )
        .unwrap();
        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.sessions, 1);
        assert_eq!(report.messages_indexed, 1);
    }

    #[test]
    fn duplicate_uuid_indexes_only_the_last_record_and_fts_text() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        fs::create_dir_all(&project).unwrap();
        let mut file = fs::File::create(project.join("session-1.jsonl")).unwrap();
        writeln!(
            file,
            "{{\"uuid\":\"dup\",\"type\":\"user\",\"message\":{{\"content\":\"first wording\"}}}}"
        )
        .unwrap();
        writeln!(
            file,
            "{{\"uuid\":\"dup\",\"type\":\"user\",\"message\":{{\"content\":\"later wording\"}}}}"
        )
        .unwrap();
        writeln!(
            file,
            "{{\"uuid\":\"solo\",\"type\":\"user\",\"message\":{{\"content\":\"independent wording\"}}}}"
        )
        .unwrap();
        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.messages_indexed, 2);
        let connection = open_database(&temp.path().join("db")).unwrap();
        let message_count: i64 = connection
            .query_row(
                "SELECT message_count FROM sessions WHERE session_id = 'munged/session-1.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(message_count, 2);
        let fts_rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM messages_fts WHERE uuid = 'dup'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_rows, 1);
        let hits = super::super::spine::search_connection(&connection, "later", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("later wording"));
        assert!(
            super::super::spine::search_connection(&connection, "first wording", 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn spilled_tool_result_text_is_searchable() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        let spills = project.join("session-1").join("tool-results");
        fs::create_dir_all(&spills).unwrap();
        fs::write(
            project.join("session-1.jsonl"),
            "{\"uuid\":\"m1\",\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"call-9\",\"name\":\"Read\"}]}}\n",
        )
        .unwrap();
        fs::write(
            spills.join("spill-blob.json"),
            "{\"tool_use_id\":\"call-9\",\"content\":[{\"type\":\"text\",\"text\":\"unfindable spill phrase\"}]}",
        )
        .unwrap();
        ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        let connection = open_database(&temp.path().join("db")).unwrap();
        let hits = super::super::spine::search_connection(&connection, "spill phrase", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].uuid, "m1");
    }

    #[test]
    fn previous_cache_versions_rebuild_to_the_full_filename_schema() {
        for previous_version in [1, 2] {
            let temp = tempfile::tempdir().unwrap();
            let data_dir = temp.path().join("db");
            fs::create_dir_all(&data_dir).unwrap();
            let legacy = format!(
                "PRAGMA foreign_keys = ON;\n\
                 CREATE TABLE sessions (session_id TEXT PRIMARY KEY, project TEXT NOT NULL, source_path TEXT NOT NULL, updated_at TEXT, message_count INTEGER NOT NULL DEFAULT 0);\n\
                 INSERT INTO sessions VALUES ('old', 'p', 'old.jsonl', NULL, 1);\n\
                 PRAGMA user_version = {previous_version};\n"
            );
            let connection =
                rusqlite::Connection::open(data_dir.join("transcripts.sqlite3")).unwrap();
            connection.execute_batch(&legacy).unwrap();
            drop(connection);

            let connection = open_database(&data_dir).unwrap();
            let version: i64 = connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version, super::SCHEMA_VERSION);
            let rows: i64 = connection
                .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
                .unwrap();
            assert_eq!(rows, 0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn locked_source_file_is_counted_and_prune_keeps_it() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("good.jsonl"),
            "{\"uuid\":\"m1\",\"type\":\"user\",\"message\":{\"content\":\"good\"}}\n",
        )
        .unwrap();
        let locked_path = project.join("locked.jsonl");
        fs::write(
            &locked_path,
            "{\"uuid\":\"m2\",\"type\":\"user\",\"message\":{\"content\":\"held\"}}\n",
        )
        .unwrap();
        let initial = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(initial.sessions, 2);
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked_path)
            .unwrap();
        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.sessions, 1);
        assert_eq!(report.skipped_files, 1);
        let cached = list_sessions(&temp.path().join("db")).unwrap();
        assert_eq!(cached.len(), 2);
        assert!(cached.iter().any(
            |session| session.session_id == "munged/locked.jsonl" && session.message_count == 1
        ));
        drop(guard);
        let report = ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        assert_eq!(report.sessions, 2);
        assert_eq!(report.skipped_files, 0);
    }

    #[test]
    fn current_cache_opens_while_another_connection_holds_the_writer_slot() {
        let temp = tempfile::tempdir().unwrap();
        let mut writer = open_database(temp.path()).unwrap();
        let transaction = writer
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let reader = open_database(temp.path()).unwrap();
        let version: i64 = reader
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        transaction.rollback().unwrap();
    }

    #[test]
    fn newer_schema_database_is_refused_without_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("db");
        fs::create_dir_all(&data_dir).unwrap();
        {
            let connection = rusqlite::Connection::open(super::database_path(&data_dir)).unwrap();
            connection
                .execute_batch(&format!(
                    "PRAGMA user_version = {};\n\
                     CREATE TABLE sessions (session_id TEXT PRIMARY KEY);\n\
                     INSERT INTO sessions VALUES ('newer-cache');\n",
                    super::SCHEMA_VERSION + 1
                ))
                .unwrap();
        }
        let error = open_database(&data_dir).unwrap_err();
        assert!(error.to_string().contains("newer than this build supports"));
        let connection = rusqlite::Connection::open(super::database_path(&data_dir)).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION + 1);
        let rows: i64 = connection
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn duplicate_uuid_keeps_final_record_spill_attachment() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("projects").join("munged");
        let spills = project.join("session-1").join("tool-results");
        fs::create_dir_all(&spills).unwrap();
        let mut file = fs::File::create(project.join("session-1.jsonl")).unwrap();
        writeln!(file, "{{\"uuid\":\"dup\",\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"call-1\",\"name\":\"Read\"}}]}}}}").unwrap();
        writeln!(file, "{{\"uuid\":\"dup\",\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"call-1\",\"name\":\"Read\"}}]}}}}").unwrap();
        drop(file);
        fs::write(
            spills.join("call-1.json"),
            "{\"tool_use_id\":\"call-1\",\"content\":[{\"type\":\"text\",\"text\":\"surviving spill\"}]}",
        )
        .unwrap();
        ingest_projects(temp.path(), &temp.path().join("db")).unwrap();
        let connection = open_database(&temp.path().join("db")).unwrap();
        let tool_result: Option<String> = connection
            .query_row(
                "SELECT tool_result FROM messages WHERE uuid = 'dup'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let result = tool_result.expect("final record keeps its spill attachment");
        assert!(result.contains("surviving spill"));
    }
}
