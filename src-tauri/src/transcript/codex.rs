use super::archive::{export_codex_archive, import_codex_archive, ArchiveReport};
use super::ingest::{replace_indexed_session, IndexedMessage, IndexedSession};
use super::TranscriptError;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(super) const MAX_ROLLOUT_BYTES: u64 = 256 * 1024 * 1024;

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

pub(super) fn collect_rollouts(
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), TranscriptError> {
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

pub(super) fn read_titles(path: &Path) -> HashMap<String, String> {
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

pub(super) fn ingest_rollout(
    connection: &mut Connection,
    path: &Path,
    titles: &HashMap<String, String>,
    fingerprint: Option<&str>,
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
            fingerprint,
        },
        &messages,
        &errors,
    )?;
    Ok((session_id, lines.len(), indexed, malformed, skipped))
}

fn read_rollout(path: &Path) -> Result<String, TranscriptError> {
    read_rollout_bounded(path, MAX_ROLLOUT_BYTES)
}

pub(super) fn read_rollout_bounded(path: &Path, max_bytes: u64) -> Result<String, TranscriptError> {
    let mut bytes = Vec::new();
    rollout_reader(path)?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(TranscriptError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Codex rollout exceeds the {max_bytes}-byte uncompressed size limit."),
        )));
    }
    String::from_utf8(bytes).map_err(|error| {
        TranscriptError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

pub(super) fn rollout_reader(path: &Path) -> Result<Box<dyn Read>, TranscriptError> {
    let file = fs::File::open(path)?;
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".jsonl.zst"))
    {
        Ok(Box::new(zstd::stream::read::Decoder::new(file)?))
    } else {
        Ok(Box::new(file))
    }
}

fn session_metadata(value: &Value) -> Option<(String, String, Option<String>)> {
    if value.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = value.get("payload")?;
    let id = metadata_id(payload)?;
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

pub(super) fn metadata_id(payload: &Value) -> Option<&str> {
    payload
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| payload.get("session_id").and_then(Value::as_str))
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
            super::extract_text(item.get("content").unwrap_or(&Value::Null)),
            event_attachments(item),
            None,
        ),
        Some("agent_message") => visible(
            "assistant",
            super::extract_text(item.get("content").unwrap_or(&Value::Null)),
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
                super::extract_text(payload.get("content").unwrap_or(&Value::Null)),
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
    let text = value
        .get("output")
        .map(super::extract_text)
        .unwrap_or_default();
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
    use crate::harness::{codex::CodexHarness, selected::SelectedHarness};
    use crate::transcript::{
        catalog,
        ingest::{file_fingerprint, list_sessions, open_database, IngestReport},
        spine,
    };
    use rusqlite::params;
    use std::io::Write;

    fn index_codex(home: &Path, app: &Path) -> Result<IngestReport, String> {
        let harness = SelectedHarness::Codex(CodexHarness::new(home.into(), home.into()));
        catalog::index_all(&harness, app, home, |_| {})
    }

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
        let temp = crate::test_support::tempdir().unwrap();
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

        let app = temp.path().join("appdata");
        let cache = app.join("codex");
        let report = index_codex(&home, &app).unwrap();
        assert_eq!(report.sessions, 1);
        assert_eq!(report.messages_indexed, 2);
        let summaries = list_sessions(&cache).unwrap();
        assert_eq!(
            summaries[0].session_id,
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(summaries[0].title.as_deref(), Some("Widget notes"));
        assert_eq!(summaries[0].project, "C:/work/demo");
        let page = spine::thread_page(&cache, &summaries[0].session_id, 0, 10).unwrap();
        assert_eq!(
            page.iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            ["locate the blue widget", "I found it."]
        );
        assert_eq!(spine::search(&cache, "blue widget", 10).unwrap().len(), 1);
        assert_eq!(fs::read_to_string(&rollout).unwrap().lines().count(), 4);
        assert_eq!(index_codex(&home, &app).unwrap().messages_indexed, 0);
        for title in ["Native title", "Renamed native conversation"] {
            fs::write(
                home.join("session_index.jsonl"),
                format!(
                    "{}\n",
                    json!({
                        "id": "00000000-0000-4000-8000-000000000001", "thread_name": title,
                    })
                ),
            )
            .unwrap();
            let renamed = index_codex(&home, &app).unwrap();
            assert_eq!(
                renamed.lines_read, 0,
                "title changes do not re-read rollouts"
            );
            assert_eq!(renamed.messages_indexed, 0);
            assert_eq!(
                list_sessions(&cache).unwrap()[0].title.as_deref(),
                Some(title)
            );
            assert_eq!(index_codex(&home, &app).unwrap().messages_indexed, 0);
        }
    }

    #[test]
    fn native_event_and_response_pairs_index_once_and_keep_repeated_turns() {
        let temp = crate::test_support::tempdir().unwrap();
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

        let app = temp.path().join("appdata");
        let app_data = app.join("codex");
        let report = index_codex(&home, &app).unwrap();
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
    fn forked_rollouts_keep_their_own_identity_and_skip_unchanged_sources() {
        let temp = crate::test_support::tempdir().unwrap();
        let home = temp.path().join("codex");
        let app = temp.path().join("app");
        let cache = app.join("codex");
        let sessions = home.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let parent = "00000000-0000-4000-8000-000000000001";
        let child = "00000000-0000-4000-8000-000000000002";
        for (id, text) in [
            (parent, "parent conversation"),
            (child, "child conversation"),
        ] {
            let mut metadata = session_meta(id, "C:/work/demo");
            metadata["payload"]["session_id"] = json!(parent);
            fs::write(
                sessions.join(format!("rollout-2026-10-22-{id}.jsonl")),
                format!(
                    "{metadata}\n{}\n",
                    json!({"type":"event_msg","payload":{"type":"user_message","message":text}})
                ),
            )
            .unwrap();
        }
        fs::write(sessions.join(format!("rollout-2026-10-21-{parent}.jsonl")), format!("{}\n{}\n", session_meta(parent, "C:/work/demo"), json!({"type":"event_msg","payload":{"type":"user_message","message":"outdated parent conversation"}}))).unwrap();
        index_codex(&home, &app).unwrap();
        let summaries = list_sessions(&cache).unwrap();
        assert_eq!(
            summaries
                .iter()
                .map(|session| (session.session_id.as_str(), session.title.as_deref()))
                .collect::<Vec<_>>(),
            [
                (parent, Some("parent conversation")),
                (child, Some("child conversation"))
            ]
        );
        assert_eq!(
            spine::thread_page(&cache, parent, 0, 10).unwrap()[0].text,
            "parent conversation"
        );
        assert_eq!(
            spine::thread_page(&cache, child, 0, 10).unwrap()[0].text,
            "child conversation"
        );
        assert_eq!(index_codex(&home, &app).unwrap().lines_read, 0);
        super::super::ingest::remove_sessions(&cache, &[child.to_owned()]).unwrap();
        let child_path = sessions.join(format!("rollout-2026-10-22-{child}.jsonl"));
        {
            let connection = open_database(&cache).unwrap();
            connection.execute("INSERT INTO sessions(session_id, project, source_path, fingerprint) VALUES ('legacy-owner', 'C:/work/demo', ?1, ?2)",
                params![child_path.to_string_lossy(), file_fingerprint(&child_path, None)]).unwrap();
        }
        index_codex(&home, &app).unwrap();
        assert_eq!(
            list_sessions(&cache)
                .unwrap()
                .iter()
                .map(|session| session.session_id.as_str())
                .collect::<Vec<_>>(),
            [parent, child]
        );
    }

    #[test]
    fn archive_import_creates_a_fresh_codex_rollout_with_selected_cwd() {
        let temp = crate::test_support::tempdir().unwrap();
        let home = temp.path().join("codex");
        let project = temp.path().join("selected-project");
        let archive_destination = temp.path().join("archives");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&archive_destination).unwrap();
        let rollout = home.join("sessions/2026/10/22/rollout-2026-10-22T11-00-00-00000000-0000-4000-8000-000000000001.jsonl");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let original_id = "00000000-0000-4000-8000-000000000001";
        let parent_id = "00000000-0000-4000-8000-000000000099";
        let mut metadata = session_meta(original_id, "C:/work/old");
        metadata["payload"]["session_id"] = json!(parent_id);
        fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n{}\n",
                metadata,
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
        let app = temp.path().join("appdata");
        let app_data = app.join("codex");
        index_codex(&home, &app).unwrap();

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
        assert_eq!(imported_meta["payload"]["session_id"], parent_id);
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
        let temp = crate::test_support::tempdir().unwrap();
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
        let app = temp.path().join("appdata");
        let app_data = app.join("codex");
        index_codex(&home, &app).unwrap();
        assert_eq!(list_sessions(&app_data).unwrap().len(), 1);

        fs::remove_file(rollout).unwrap();
        index_codex(&home, &app).unwrap();
        assert!(list_sessions(&app_data).unwrap().is_empty());
    }

    #[test]
    fn compressed_rollouts_are_indexed_and_archived_byte_for_byte() {
        let temp = crate::test_support::tempdir().unwrap();
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

        let app = temp.path().join("appdata");
        let app_data = app.join("codex");
        let report = index_codex(&home, &app).unwrap();
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
        let refreshed = index_codex(&home, &app).unwrap();
        assert_eq!(refreshed.sessions, 2);
        assert_eq!(
            spine::search(&app_data, "compressed archive search", 10)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn compressed_rollout_exceeding_decompression_cap_is_rejected() {
        let temp = crate::test_support::tempdir().unwrap();
        let payload = "x".repeat(4096);
        let compressed = zstd::stream::encode_all(payload.as_bytes(), 1).unwrap();
        let path = temp.path().join("rollout-bomb.jsonl.zst");
        fs::write(&path, compressed).unwrap();
        assert_eq!(read_rollout_bounded(&path, 4096).unwrap().len(), 4096);
        let err = read_rollout_bounded(&path, 4095).unwrap_err();
        assert!(err.to_string().contains("uncompressed size limit"));
    }
}
