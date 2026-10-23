use super::archive::{export_codex_archive, import_codex_archive, ArchiveReport};
use super::ingest::{
    open_database, prune_vanished_sessions, replace_indexed_session, IndexedMessage,
    IndexedSession, IngestReport,
};
use super::TranscriptError;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub fn ingest(config_root: &Path, app_data_dir: &Path) -> Result<IngestReport, TranscriptError> {
    let roots = [
        config_root.join("sessions"),
        config_root.join("archived_sessions"),
    ];
    if !roots.iter().any(|root| root.is_dir()) {
        return Err(TranscriptError::InvalidRoot);
    }

    let titles = read_titles(&config_root.join("session_index.jsonl"));
    let mut files = Vec::new();
    for root in roots.iter().filter(|root| root.is_dir()) {
        collect_rollouts(root, &mut files)?;
    }
    files.sort();

    let mut connection = open_database(app_data_dir)?;
    let mut seen_paths = Vec::with_capacity(files.len());
    let mut report = IngestReport {
        sessions: 0,
        lines_read: 0,
        messages_indexed: 0,
        malformed_lines: 0,
        skipped_files: 0,
        skipped_state_lines: 0,
    };
    for path in files {
        seen_paths.push(path.to_string_lossy().into_owned());
        match ingest_rollout(&mut connection, &path, &titles) {
            Ok((session_id, lines, messages, malformed, skipped)) => {
                if session_id.is_empty() {
                    report.skipped_files += 1;
                    continue;
                }
                report.sessions += 1;
                report.lines_read += lines;
                report.messages_indexed += messages;
                report.malformed_lines += malformed;
                report.skipped_state_lines += skipped;
            }
            Err(TranscriptError::Io(_)) => report.skipped_files += 1,
            Err(error) => return Err(error),
        }
    }
    prune_vanished_sessions(&mut connection, &seen_paths)?;
    Ok(report)
}

pub fn export_archive(
    config_root: &Path,
    app_data_dir: &Path,
    session_id: &str,
    destination: &Path,
) -> Result<ArchiveReport, String> {
    export_codex_archive(config_root, app_data_dir, session_id, destination)
}

pub fn import_archive(
    config_root: &Path,
    source: &Path,
    project: &Path,
) -> Result<ArchiveReport, String> {
    import_codex_archive(config_root, source, project)
}

fn collect_rollouts(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), TranscriptError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if crate::harness::atomic::is_link_or_reparse(&metadata) {
            continue;
        }
        let path = entry.path();
        if metadata.is_dir() {
            collect_rollouts(&path, files)?;
        } else if metadata.is_file() {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with("rollout-") {
                continue;
            }
            if let Some(plain_name) = name.strip_suffix(".zst") {
                if !plain_name.ends_with(".jsonl") {
                    continue;
                }
                if path.with_file_name(plain_name).is_file() {
                    continue;
                }
                files.push(path);
            } else if name.ends_with(".jsonl") {
                files.push(path);
            }
        }
    }
    Ok(())
}

fn read_titles(path: &Path) -> HashMap<String, String> {
    fs::read_to_string(path)
        .ok()
        .into_iter()
        .flat_map(|contents| contents.lines().map(str::to_owned).collect::<Vec<_>>())
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter_map(|entry| {
            Some((
                entry.get("id")?.as_str()?.to_owned(),
                entry.get("thread_name")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn ingest_rollout(
    connection: &mut Connection,
    path: &Path,
    titles: &HashMap<String, String>,
) -> Result<(String, usize, usize, usize, usize), TranscriptError> {
    let contents = read_rollout(path)?;
    let lines = contents.lines().collect::<Vec<_>>();
    let parsed = lines
        .iter()
        .enumerate()
        .map(|(position, line)| (position + 1, *line, serde_json::from_str::<Value>(line)))
        .collect::<Vec<_>>();
    let Some((session_id, project, updated_at)) = parsed
        .iter()
        .find_map(|(_, _, result)| result.as_ref().ok().and_then(session_metadata))
    else {
        return Ok((String::new(), lines.len(), 0, 0, lines.len()));
    };

    let mut messages = Vec::<IndexedMessage>::new();
    let mut errors = Vec::new();
    let mut malformed = 0;
    let mut skipped = 0;
    let mut parent_uuid: Option<String> = None;
    let mut tool_calls = HashMap::<String, usize>::new();
    let mut duplicate_responses = response_duplicate_budget(&parsed);
    for (position, line, result) in parsed {
        if line.trim().is_empty() {
            skipped += 1;
            continue;
        }
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                malformed += 1;
                errors.push((position, error.to_string()));
                continue;
            }
        };
        match normalized_record(&value) {
            NormalizedRecord::Visible {
                kind,
                text,
                attachment,
                call_id,
            } => {
                if is_response_record(&value) {
                    let key = duplicate_key(&kind, &text, call_id.as_deref());
                    if let Some(remaining) = duplicate_responses.get_mut(&key) {
                        if *remaining > 0 {
                            *remaining -= 1;
                            continue;
                        }
                    }
                }
                if kind == "tool_result" {
                    if let Some(index) = call_id.as_ref().and_then(|id| tool_calls.get(id)).copied()
                    {
                        messages[index].tool_result = Some(text);
                        continue;
                    }
                }
                let uuid = format!("codex:{session_id}:{position}");
                if let Some(call_id) = call_id {
                    if kind == "tool_call" {
                        tool_calls.insert(call_id, messages.len());
                    }
                }
                messages.push(IndexedMessage {
                    uuid: uuid.clone(),
                    parent_uuid: parent_uuid.clone(),
                    message_type: kind,
                    text,
                    raw_json: value.to_string(),
                    position,
                    attachment,
                    tool_result: None,
                    tool_result_note: None,
                });
                parent_uuid = Some(uuid);
            }
            NormalizedRecord::State => skipped += 1,
            NormalizedRecord::Malformed(error) => {
                malformed += 1;
                errors.push((position, error));
            }
        }
    }
    let title = titles
        .get(&session_id)
        .map(String::as_str)
        .filter(|title| !title.trim().is_empty())
        .or_else(|| {
            messages
                .iter()
                .find(|message| message.message_type == "user")
                .map(|m| m.text.as_str())
        });
    let indexed = replace_indexed_session(
        connection,
        IndexedSession {
            id: &session_id,
            project: &project,
            source_path: path,
            updated_at: updated_at.as_deref(),
            title,
        },
        &messages,
        &errors,
    )?;
    Ok((session_id, lines.len(), indexed, malformed, skipped))
}

fn read_rollout(path: &Path) -> Result<String, TranscriptError> {
    let file = fs::File::open(path)?;
    let mut contents = String::new();
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".jsonl.zst"))
    {
        zstd::stream::read::Decoder::new(file)?.read_to_string(&mut contents)?;
    } else {
        let mut file = file;
        file.read_to_string(&mut contents)?;
    }
    Ok(contents)
}

fn session_metadata(value: &Value) -> Option<(String, String, Option<String>)> {
    if value.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = value.get("payload")?;
    let id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .or_else(|| payload.get("id").and_then(Value::as_str))?;
    let project = payload
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or("(unknown project)")
        .to_owned();
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some((id.to_owned(), project, timestamp))
}

enum NormalizedRecord {
    Visible {
        kind: String,
        text: String,
        attachment: Option<String>,
        call_id: Option<String>,
    },
    State,
    Malformed(String),
}

fn normalized_record(value: &Value) -> NormalizedRecord {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return NormalizedRecord::Malformed("rollout record has no string type".to_owned());
    };
    let Some(payload) = value.get("payload") else {
        return NormalizedRecord::State;
    };
    match kind {
        "event_msg" => normalize_event(payload),
        "response_item" => normalize_response_item(payload),
        _ => NormalizedRecord::State,
    }
}

fn normalize_event(payload: &Value) -> NormalizedRecord {
    match payload.get("type").and_then(Value::as_str) {
        Some("user_message") => visible(
            "user",
            payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            event_attachments(payload),
            None,
        ),
        Some("agent_message") => visible(
            "assistant",
            payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            None,
            None,
        ),
        Some("item_completed") => payload
            .get("item")
            .map(normalize_completed_item)
            .unwrap_or(NormalizedRecord::State),
        _ => NormalizedRecord::State,
    }
}

fn normalize_completed_item(item: &Value) -> NormalizedRecord {
    match item.get("type").and_then(Value::as_str) {
        Some("user_message") => visible(
            "user",
            collect_content(item.get("content").unwrap_or(&Value::Null)),
            event_attachments(item),
            None,
        ),
        Some("agent_message") => visible(
            "assistant",
            collect_content(item.get("content").unwrap_or(&Value::Null)),
            None,
            None,
        ),
        Some("function_call") => function_call(item),
        Some("function_call_output") => function_output(item),
        _ => NormalizedRecord::State,
    }
}

fn normalize_response_item(payload: &Value) -> NormalizedRecord {
    let Some(kind) = payload.get("type").and_then(Value::as_str) else {
        return NormalizedRecord::State;
    };
    match kind {
        "message" => {
            let Some(role) = payload.get("role").and_then(Value::as_str) else {
                return NormalizedRecord::State;
            };
            let message_type = match role {
                "user" => "user",
                "assistant" => "assistant",
                _ => return NormalizedRecord::State,
            };
            visible(
                message_type,
                collect_content(payload.get("content").unwrap_or(&Value::Null)),
                None,
                None,
            )
        }
        "function_call" => function_call(payload),
        "function_call_output" => function_output(payload),
        _ => NormalizedRecord::State,
    }
}

type DuplicateKey = (String, String);

fn is_response_record(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("response_item")
}

fn duplicate_key(kind: &str, text: &str, call_id: Option<&str>) -> DuplicateKey {
    let identity = call_id
        .map(|id| format!("call:{id}"))
        .unwrap_or_else(|| format!("text:{text}"));
    (kind.to_owned(), identity)
}

fn response_duplicate_budget(
    records: &[(usize, &str, Result<Value, serde_json::Error>)],
) -> HashMap<DuplicateKey, usize> {
    let mut counts = HashMap::<DuplicateKey, (usize, usize)>::new();
    for (_, _, result) in records {
        let Ok(value) = result else { continue };
        let normalized = normalized_record(value);
        let NormalizedRecord::Visible {
            kind,
            text,
            call_id,
            ..
        } = normalized
        else {
            continue;
        };
        let is_response = is_response_record(value);
        let key = duplicate_key(&kind, &text, call_id.as_deref());
        let entry = counts.entry(key).or_default();
        if is_response {
            entry.1 += 1;
        } else if value.get("type").and_then(Value::as_str) == Some("event_msg") {
            entry.0 += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(key, (events, responses))| {
            let duplicates = events.min(responses);
            (duplicates > 0).then_some((key, duplicates))
        })
        .collect()
}

fn function_call(value: &Value) -> NormalizedRecord {
    let name = value.get("name").and_then(Value::as_str).unwrap_or("tool");
    let arguments = value
        .get("arguments")
        .map(|value| match value {
            Value::String(text) => text.clone(),
            value => value.to_string(),
        })
        .unwrap_or_default();
    visible(
        "tool_call",
        format!("{name}\n{arguments}"),
        None,
        value
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
    )
}

fn function_output(value: &Value) -> NormalizedRecord {
    let text = value.get("output").map(collect_content).unwrap_or_default();
    visible(
        "tool_result",
        text,
        None,
        value
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
    )
}

fn event_attachments(value: &Value) -> Option<String> {
    let attachments = json!({
        "images": value.get("images"),
        "localImages": value.get("local_images"),
        "textElements": value.get("text_elements"),
    });
    (attachments.as_object().is_some_and(|object| {
        object
            .values()
            .any(|value| !value.is_null() && !value.as_array().is_some_and(Vec::is_empty))
    }))
    .then(|| attachments.to_string())
}

fn collect_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(collect_content)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| object.get("content").map(collect_content))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn visible(
    kind: &str,
    text: impl Into<String>,
    attachment: Option<String>,
    call_id: Option<String>,
) -> NormalizedRecord {
    NormalizedRecord::Visible {
        kind: kind.to_owned(),
        text: text.into(),
        attachment,
        call_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{ingest::list_sessions, spine};
    use std::io::Write;

    fn session_meta(id: &str, cwd: &str) -> Value {
        json!({
            "timestamp": "2026-10-22T11:00:00Z",
            "type": "session_meta",
            "payload": {
                "session_id": id,
                "id": id,
                "cwd": cwd,
                "timestamp": "2026-10-22T11:00:00Z",
                "originator": "codex-cli",
                "cli_version": "0.163.0",
                "source": "cli"
            }
        })
    }

    #[test]
    fn native_rollout_records_are_indexed_for_shared_read_and_search() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex");
        let rollout = home.join("sessions/2026/10/22/rollout-2026-10-22T11-00-00-00000000-0000-4000-8000-000000000001.jsonl");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let mut file = fs::File::create(&rollout).unwrap();
        writeln!(
            file,
            "{}",
            session_meta("00000000-0000-4000-8000-000000000001", "C:/work/demo")
        )
        .unwrap();
        writeln!(file, "{}", json!({"timestamp":"2026-10-22T11:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"locate the blue widget"}})).unwrap();
        writeln!(file, "{}", json!({"timestamp":"2026-10-22T11:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I found it."}]}})).unwrap();
        writeln!(file, "{}", json!({"timestamp":"2026-10-22T11:00:03Z","type":"future_event","payload":{"opaque":"retained in rollout"}})).unwrap();
        fs::write(home.join("session_index.jsonl"), format!("{}\n", json!({"id":"00000000-0000-4000-8000-000000000001","thread_name":"Widget notes","updated_at":"2026-10-22T11:00:03Z"}))).unwrap();

        let report = ingest(&home, &temp.path().join("appdata/codex")).unwrap();
        assert_eq!(report.sessions, 1);
        assert_eq!(report.messages_indexed, 2);
        let summaries = list_sessions(&temp.path().join("appdata/codex")).unwrap();
        assert_eq!(
            summaries[0].session_id,
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(summaries[0].title.as_deref(), Some("Widget notes"));
        assert_eq!(summaries[0].project, "C:/work/demo");
        let page = spine::thread_page(
            &temp.path().join("appdata/codex"),
            &summaries[0].session_id,
            0,
            10,
        )
        .unwrap();
        assert_eq!(
            page.iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            ["locate the blue widget", "I found it."]
        );
        assert_eq!(
            spine::search(&temp.path().join("appdata/codex"), "blue widget", 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(fs::read_to_string(rollout).unwrap().lines().count(), 4);
    }

    #[test]
    fn native_event_and_response_pairs_index_once_and_keep_repeated_turns() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex");
        let id = "00000000-0000-4000-8000-000000000001";
        let rollout = home.join(format!(
            "sessions/2026/10/22/rollout-2026-10-22T11-00-00-{id}.jsonl"
        ));
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let repeated = "same request twice";
        let records = [
            session_meta(id, "C:/work/demo"),
            json!({"type":"event_msg","payload":{"type":"user_message","message":repeated}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":repeated}]}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":repeated}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":repeated}]}}),
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"answer once"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer once"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"private instruction"}]}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"function_call","name":"lookup","arguments":"{}","call_id":"call-1"}}}),
            json!({"type":"response_item","payload":{"type":"function_call","name":"lookup","arguments":"{}","call_id":"call-1"}}),
            json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"function_call_output","output":"found","call_id":"call-1"}}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","output":"found","call_id":"call-1"}}),
        ];
        fs::write(
            &rollout,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        let app_data = temp.path().join("appdata/codex");
        let report = ingest(&home, &app_data).unwrap();
        assert_eq!(report.messages_indexed, 4);
        let summaries = list_sessions(&app_data).unwrap();
        let page = spine::thread_page(&app_data, &summaries[0].session_id, 0, 10).unwrap();
        assert_eq!(page.len(), 4);
        assert_eq!(
            page.iter()
                .map(|message| message.message_type.as_str())
                .collect::<Vec<_>>(),
            ["user", "user", "assistant", "tool_call"]
        );
        assert_eq!(page[0].text, repeated);
        assert_eq!(page[1].text, repeated);
        assert_eq!(page[2].text, "answer once");
        assert!(!page
            .iter()
            .any(|message| message.text.contains("private instruction")));
        assert_eq!(page[3].tool_result.as_deref(), Some("found"));
        assert!(serde_json::from_str::<Value>(&page[3].raw_json).is_ok());
    }

    #[test]
    fn archive_import_creates_a_fresh_codex_rollout_with_selected_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex");
        let project = temp.path().join("selected-project");
        let archive_destination = temp.path().join("archives");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&archive_destination).unwrap();
        let rollout = home.join("sessions/2026/10/22/rollout-2026-10-22T11-00-00-00000000-0000-4000-8000-000000000001.jsonl");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let original_id = "00000000-0000-4000-8000-000000000001";
        fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n{}\n",
                session_meta(original_id, "C:/work/old"),
                json!({"timestamp":"2026-10-22T11:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"archive me"}}),
                json!({"timestamp":"2026-10-22T11:00:01Z","type":"turn_context","payload":{"cwd":"C:/work/old","policy":"preserve me"}}),
                json!({"timestamp":"2026-10-22T11:00:02Z","type":"future_event","payload":{"opaque":"preserve this"}})
            ),
        )
        .unwrap();
        fs::write(
            home.join("session_index.jsonl"),
            format!("{}\n", json!({"id":original_id,"thread_name":"Native title","updated_at":"2026-10-22T11:00:02Z"})),
        )
        .unwrap();
        let app_data = temp.path().join("appdata/codex");
        ingest(&home, &app_data).unwrap();

        let exported = export_archive(&home, &app_data, original_id, &archive_destination).unwrap();
        assert!(matches!(
            &exported.action,
            super::super::archive::ArchiveAction::Exported
        ));
        let imported = import_archive(&home, Path::new(&exported.path), &project).unwrap();
        assert_ne!(imported.session_id, original_id);
        assert!(Path::new(&imported.path)
            .canonicalize()
            .unwrap()
            .starts_with(home.join("sessions").canonicalize().unwrap()));
        let imported_rollout = fs::read_to_string(&imported.path).unwrap();
        let imported_meta = imported_rollout
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|value| value.get("type").and_then(Value::as_str) == Some("session_meta"))
            .unwrap();
        assert_eq!(imported_meta["payload"]["id"], imported.session_id);
        assert_eq!(imported_meta["payload"]["session_id"], imported.session_id);
        assert!(imported_meta["payload"]["cwd"]
            .as_str()
            .unwrap()
            .ends_with("selected-project"));
        let imported_context = imported_rollout
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|value| value.get("type").and_then(Value::as_str) == Some("turn_context"))
            .unwrap();
        assert!(imported_context["payload"]["cwd"]
            .as_str()
            .unwrap()
            .ends_with("selected-project"));
        assert_eq!(imported_context["payload"]["policy"], "preserve me");
        assert!(imported_rollout.contains("archive me"));
        assert!(imported_rollout.contains("preserve this"));
        let index_entries = fs::read_to_string(home.join("session_index.jsonl")).unwrap();
        let registered = index_entries
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|entry| entry["id"] == imported.session_id)
            .unwrap();
        assert_eq!(registered["thread_name"], "Native title-1");
    }

    #[test]
    fn ingest_prunes_cache_entries_when_native_rollouts_are_removed() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex");
        let rollout = home.join(
            "sessions/2026/10/22/rollout-2026-10-22T11-00-00-00000000-0000-4000-8000-000000000001.jsonl",
        );
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        fs::create_dir_all(home.join("archived_sessions")).unwrap();
        fs::write(
            &rollout,
            format!(
                "{}\n{}\n",
                session_meta("00000000-0000-4000-8000-000000000001", "C:/work/demo"),
                json!({"timestamp":"2026-10-22T11:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"remove me"}}),
            ),
        )
        .unwrap();
        let app_data = temp.path().join("appdata/codex");
        ingest(&home, &app_data).unwrap();
        assert_eq!(list_sessions(&app_data).unwrap().len(), 1);

        fs::remove_file(rollout).unwrap();
        ingest(&home, &app_data).unwrap();
        assert!(list_sessions(&app_data).unwrap().is_empty());
    }

    #[test]
    fn compressed_rollouts_are_indexed_and_archived_byte_for_byte() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("codex");
        let project = temp.path().join("selected-project");
        let archive_destination = temp.path().join("archives");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&archive_destination).unwrap();
        let original_id = "00000000-0000-4000-8000-000000000001";
        let rollout = home.join("sessions/2026/10/22/rollout-2026-10-22T11-00-00-00000000-0000-4000-8000-000000000001.jsonl.zst");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let contents = format!(
            "{}\n{}\n{}\n",
            session_meta(original_id, "C:/work/old"),
            json!({"timestamp":"2026-10-22T11:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"compressed archive search term"}}),
            json!({"timestamp":"2026-10-22T11:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"compressed archive reply"}}),
        );
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 0).unwrap();
        encoder.write_all(contents.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        fs::write(&rollout, &compressed).unwrap();
        fs::write(
            home.join("session_index.jsonl"),
            format!(
                "{}\n",
                json!({"id":original_id,"thread_name":"Compressed native chat","updated_at":"2026-10-22T11:00:02Z"})
            ),
        )
        .unwrap();

        let app_data = temp.path().join("appdata/codex");
        let report = ingest(&home, &app_data).unwrap();
        assert_eq!(report.sessions, 1);
        assert_eq!(report.messages_indexed, 2);
        assert_eq!(
            spine::search(&app_data, "compressed archive search", 10)
                .unwrap()
                .len(),
            1
        );

        let exported = export_archive(&home, &app_data, original_id, &archive_destination).unwrap();
        let archived_rollout = Path::new(&exported.path).join("rollout.jsonl.zst");
        assert_eq!(fs::read(archived_rollout).unwrap(), compressed);
        let imported = import_archive(&home, Path::new(&exported.path), &project).unwrap();
        assert!(imported.path.ends_with(".jsonl"));
        let imported_contents = fs::read_to_string(&imported.path).unwrap();
        assert!(imported_contents.contains("compressed archive reply"));
        assert!(imported_contents.contains(&imported.session_id));
        let refreshed = ingest(&home, &app_data).unwrap();
        assert_eq!(refreshed.sessions, 2);
        assert_eq!(
            spine::search(&app_data, "compressed archive search", 10)
                .unwrap()
                .len(),
            2
        );
    }
}
