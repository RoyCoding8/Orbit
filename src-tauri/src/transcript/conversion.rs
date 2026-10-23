use super::{archive, ingest::open_database};
use crate::harness::{selected::SelectedHarness, Harness};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const MAX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Role {
    User,
    Assistant,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Text(String),
    Image { mime: String, data: String },
}

#[derive(Debug, Clone)]
struct Turn {
    role: Role,
    content: Vec<Content>,
}

#[derive(Default, Debug)]
struct Conversation {
    cwd: String,
    title: String,
    turns: Vec<Turn>,
    notes: BTreeSet<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub working_directory: String,
    pub title: String,
    pub messages: usize,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionReport {
    pub session_id: String,
    pub working_directory: String,
    pub messages: usize,
    pub notes: Vec<String>,
}

pub fn preview(source: &SelectedHarness, app: &Path, session: &str) -> Result<Preview, String> {
    let (_, chat) = snapshot(source, app, session)?;
    Ok(Preview {
        working_directory: chat.cwd,
        title: chat.title,
        messages: chat.turns.len(),
        notes: chat.notes.into_iter().collect(),
    })
}

pub fn convert(
    source: &SelectedHarness,
    target: &SelectedHarness,
    app: &Path,
    session: &str,
    project: Option<&Path>,
) -> Result<ConversionReport, String> {
    if source.id() == target.id() {
        return Err("Choose a different destination agent. Use native archives to copy a chat within one agent.".into());
    }
    if matches!(target, SelectedHarness::Kiro(_)) {
        return Err("Kiro does not expose a verified transcript import API. Kiro v2 chats can be converted to other agents, but Kiro cannot be a conversion destination.".into());
    }
    let (stage, chat) = snapshot(source, app, session)?;
    let project = project.unwrap_or_else(|| Path::new(&chat.cwd));
    let project = project.canonicalize().map_err(|error| {
        format!(
            "The destination working directory is unavailable. Choose another directory. {error}"
        )
    })?;
    if !project.is_dir() {
        return Err("The destination working directory must be a directory.".into());
    }
    let destination = stage.path().join("converted");
    fs::create_dir(&destination).map_err(|error| error.to_string())?;
    encode(target, &chat, &destination)?;
    let report = target.transcript_import(app, &destination, &project)?;
    Ok(ConversionReport {
        session_id: report.session_id,
        working_directory: native_path(&project),
        messages: chat.turns.len(),
        notes: chat.notes.into_iter().collect(),
    })
}

fn snapshot(
    source: &SelectedHarness,
    app: &Path,
    session: &str,
) -> Result<(tempfile::TempDir, Conversation), String> {
    fs::create_dir_all(app).map_err(|error| error.to_string())?;
    let stage = tempfile::Builder::new()
        .prefix("chat-conversion-")
        .tempdir_in(app)
        .map_err(|error| error.to_string())?;
    let report = source.transcript_export(app, session, stage.path())?;
    let mut chat = decode(source, Path::new(&report.path))?;
    if chat.cwd.trim().is_empty() {
        return Err(
            "The source chat has no working directory. Index its native session again.".into(),
        );
    }
    if chat.turns.is_empty() {
        return Err("The source chat has no conversation messages to convert.".into());
    }
    if chat
        .turns
        .first()
        .is_some_and(|turn| turn.role == Role::Assistant)
    {
        chat.turns.insert(
            0,
            Turn {
                role: Role::User,
                content: vec![Content::Text(
                    "[Imported conversation history follows.]".into(),
                )],
            },
        );
        chat.notes.insert("A history marker was inserted because the available conversation began with an assistant message.".into());
    }
    if chat.title.trim().is_empty() {
        let connection = open_database(&source.transcript_cache(app)).map_err(|e| e.to_string())?;
        chat.title = connection
            .query_row(
                "SELECT title FROM sessions WHERE session_id = ?1",
                [session],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "Converted chat".into());
    }
    chat.notes.insert(
        "Agent settings, permissions, signed reasoning, and runtime state are not transferred."
            .into(),
    );
    Ok((stage, chat))
}

fn decode(source: &SelectedHarness, root: &Path) -> Result<Conversation, String> {
    let manifest = read_json(&root.join(if matches!(source, SelectedHarness::Kiro(_)) {
        "orbit-backup.json"
    } else {
        "manifest.json"
    }))?;
    match source {
        SelectedHarness::Claude(_) => {
            let file = required(&manifest, "sessionFile")?;
            decode_claude(&read_lines(&root.join(file))?)
        }
        SelectedHarness::Codex(_) => {
            let file = required(&manifest, "rolloutFile")?;
            let contents = archive::read_codex_rollout(&root.join(file))?;
            let mut records = parse_lines(&contents)?;
            embed_codex_attachments(root, &manifest, &mut records)?;
            let mut chat = decode_codex(&records)?;
            chat.title = manifest["title"].as_str().unwrap_or("").into();
            Ok(chat)
        }
        SelectedHarness::Pi(_) => decode_pi(&read_lines(&root.join("session.jsonl"))?),
        SelectedHarness::Opencode(_) => decode_opencode(&read_json(&root.join("session.json"))?),
        SelectedHarness::Kiro(_) => {
            let id = required(&manifest, "sourceSessionId")?.trim_start_matches("kiro-v2:");
            let header = read_json(&root.join(format!("{id}.json")))?;
            decode_kiro(&header, &read_lines(&root.join(format!("{id}.jsonl")))?)
        }
    }
}

fn decode_claude(records: &[Value]) -> Result<Conversation, String> {
    let mut chat = Conversation::default();
    let mut title = super::SessionTitle::default();
    for record in records {
        title.observe(record);
        if let Some(cwd) = record["cwd"].as_str() {
            chat.cwd = cwd.into();
        }
    }
    chat.title = title.finish().unwrap_or_default();
    let eligible: Vec<_> = records
        .iter()
        .filter(|r| r["isSidechain"] != true)
        .cloned()
        .collect();
    let leaf = eligible
        .iter()
        .rev()
        .find(|r| matches!(r["type"].as_str(), Some("user" | "assistant")) && r["uuid"].is_string())
        .and_then(|r| r["uuid"].as_str());
    for record in branch(&eligible, "uuid", "parentUuid", leaf, &mut chat.notes)? {
        let role = record["type"].as_str().unwrap_or("");
        let message = record.get("message").unwrap_or(&record["content"]);
        chat.message(role, message.get("content").unwrap_or(message));
    }
    Ok(chat)
}

fn decode_pi(records: &[Value]) -> Result<Conversation, String> {
    let header = records.first().ok_or("Pi session is empty")?;
    if header["type"] != "session" || header["version"] != 3 {
        return Err("Only Pi version 3 sessions can be converted.".into());
    }
    let mut chat = Conversation {
        cwd: required(header, "cwd")?.into(),
        ..Default::default()
    };
    for record in records {
        if record["type"] == "session_info" {
            chat.title = record["name"].as_str().unwrap_or("").into();
        }
    }
    let entries = &records[1..];
    let leaf = entries.last().and_then(|r| r["id"].as_str());
    for entry in branch(entries, "id", "parentId", leaf, &mut chat.notes)? {
        match entry["type"].as_str() {
            Some("message") => {
                let message = &entry["message"];
                if message["role"] == "toolResult" {
                    chat.tool("result", message);
                } else {
                    chat.message(message["role"].as_str().unwrap_or(""), &message["content"]);
                }
            }
            Some("compaction" | "branch_summary") => chat.summary(&entry["summary"]),
            Some("custom_message") => chat.message("user", &entry["content"]),
            _ => {}
        }
    }
    Ok(chat)
}

fn decode_codex(records: &[Value]) -> Result<Conversation, String> {
    let mut chat = Conversation::default();
    let mut echo = None;
    for record in records {
        let payload = &record["payload"];
        match record["type"].as_str() {
            Some("session_meta") => chat.cwd = required(payload, "cwd")?.into(),
            Some("response_item") if payload["type"] == "message" => {
                codex_message(
                    &mut chat,
                    &mut echo,
                    true,
                    payload["role"].as_str().unwrap_or(""),
                    &payload["content"],
                );
            }
            Some("response_item") => {
                codex_item(&mut chat, payload);
                echo = None;
            }
            Some("compacted") => {
                echo = None;
                if let Some(history) = payload["replacement_history"].as_array() {
                    chat.turns.clear();
                    for item in history {
                        codex_item(&mut chat, item);
                    }
                } else {
                    chat.summary(&payload["message"]);
                }
            }
            Some("event_msg") => match payload["type"].as_str() {
                Some("user_message" | "agent_message") => {
                    let role = if payload["type"] == "user_message" {
                        "user"
                    } else {
                        "assistant"
                    };
                    let mut content = vec![
                        json!({"type":"input_text","text":payload["message"].as_str().unwrap_or("")}),
                    ];
                    if let Some(images) = payload["images"].as_array() {
                        for url in images.iter().filter_map(Value::as_str) {
                            content.push(json!({"type":"input_image","image_url":url}));
                        }
                    }
                    if let Some(references) = payload["local_images"].as_array() {
                        for reference in references.iter().filter_map(Value::as_str) {
                            content.push(json!({"type":"input_image","image_url":reference}));
                        }
                    }
                    codex_message(&mut chat, &mut echo, false, role, &Value::Array(content));
                }
                Some("thread_rolled_back") => {
                    echo = None;
                    let count = payload["num_turns"]
                        .as_u64()
                        .ok_or("Codex rollback has no turn count")?;
                    for _ in 0..count {
                        if let Some(position) =
                            chat.turns.iter().rposition(|turn| turn.role == Role::User)
                        {
                            chat.turns.truncate(position);
                        }
                    }
                }
                Some("item_completed") => {
                    let item = &payload["item"];
                    match item["type"].as_str() {
                        Some("user_message" | "agent_message") => {
                            let role = if item["type"] == "user_message" {
                                "user"
                            } else {
                                "assistant"
                            };
                            codex_message(&mut chat, &mut echo, false, role, &item["content"]);
                        }
                        _ => {
                            codex_item(&mut chat, item);
                            echo = None;
                        }
                    }
                }
                Some("task_started" | "task_complete" | "turn_started" | "turn_complete") => {
                    echo = None
                }
                Some("compaction" | "context_compaction") => {
                    echo = None;
                    chat.notes
                        .insert("Codex compaction control records were omitted.".into());
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(chat)
}

fn embed_codex_attachments(
    root: &Path,
    manifest: &Value,
    records: &mut [Value],
) -> Result<(), String> {
    let assets: HashMap<_, _> = manifest["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| Some((item["referenceHash"].as_str()?, item["path"].as_str()?)))
        .collect();
    for record in records {
        if record["type"] != "event_msg" {
            continue;
        }
        let Some(images) = record["payload"]["local_images"].as_array_mut() else {
            continue;
        };
        for image in images {
            let Some(reference) = image.as_str() else {
                continue;
            };
            let hash = format!("{:x}", Sha256::digest(reference.as_bytes()));
            let Some(file) = assets.get(hash.as_str()) else {
                continue;
            };
            let path = root.join(file);
            let mime = match path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                "gif" => "image/gif",
                _ => continue,
            };
            if fs::metadata(&path).map_err(|e| e.to_string())?.len() > MAX_BYTES {
                return Err("A source image exceeds the conversion size limit.".into());
            }
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            *image = json!(format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
        }
    }
    Ok(())
}

fn codex_message(
    chat: &mut Conversation,
    echo: &mut Option<(bool, String, usize)>,
    response: bool,
    role: &str,
    value: &Value,
) {
    let key = echo_key(role, value);
    let mut content = Vec::new();
    chat.block(value, &mut content);
    if let Some((was_response, previous_key, index)) = echo.take() {
        if was_response != response && previous_key == key && index + 1 == chat.turns.len() {
            let previous = &mut chat.turns[index].content;
            let prompt = key.strip_prefix(&format!("{role}\n")).unwrap_or("");
            if response {
                for attachment in previous
                    .iter()
                    .filter(|block| !matches!(block, Content::Text(text) if text == prompt))
                {
                    if !content.contains(attachment) {
                        content.push(attachment.clone());
                    }
                }
                *previous = content;
            } else {
                for attachment in content
                    .into_iter()
                    .filter(|block| !matches!(block, Content::Text(text) if text == prompt))
                {
                    if !previous.contains(&attachment) {
                        previous.push(attachment);
                    }
                }
            }
            return;
        }
    }
    let start = chat.turns.len();
    match role {
        "user" => chat.push(Role::User, content),
        "assistant" => chat.push(Role::Assistant, content),
        _ => chat.message(role, value),
    }
    if chat.turns.len() == start + 1 {
        *echo = Some((response, key, start));
    }
}

fn codex_item(chat: &mut Conversation, item: &Value) {
    match item["type"].as_str() {
        Some("message") => chat.message(item["role"].as_str().unwrap_or(""), &item["content"]),
        Some("function_call" | "custom_tool_call") => chat.tool("call", item),
        Some("function_call_output" | "custom_tool_call_output") => chat.tool("result", item),
        Some("reasoning") => {
            chat.notes
                .insert("Private or signed reasoning was omitted.".into());
        }
        Some("compaction" | "context_compaction") => {
            chat.notes
                .insert("Codex compaction control records were omitted.".into());
        }
        _ => {}
    }
}

fn echo_key(role: &str, content: &Value) -> String {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    format!("{role}\n{text}")
}

fn decode_opencode(transfer: &Value) -> Result<Conversation, String> {
    let info = &transfer["info"];
    let cwd = info["location"]["directory"]
        .as_str()
        .or(info["directory"].as_str())
        .ok_or("OpenCode session has no working directory")?;
    let mut chat = Conversation {
        cwd: cwd.into(),
        title: info["title"].as_str().unwrap_or("").into(),
        ..Default::default()
    };
    let records = transfer["messages"]
        .as_array()
        .ok_or("OpenCode export has no messages")?;
    let reverted = info["revert"]["messageID"].as_str();
    for original in records {
        let is_reverted = reverted.is_some() && original["id"].as_str() == reverted;
        let partial = if is_reverted {
            chat.notes
                .insert("OpenCode's reverted timeline tail was excluded.".into());
            let Some(part) = info["revert"]["partID"].as_str() else {
                break;
            };
            let Some(parts) = original["content"].as_array() else {
                return Err("OpenCode's partial revert boundary cannot be resolved. Finish or reset the revert in OpenCode before converting.".into());
            };
            let Some(boundary) = parts
                .iter()
                .position(|item| item["id"].as_str() == Some(part))
            else {
                return Err("OpenCode's partial revert boundary is absent from its export. Finish or reset the revert before converting.".into());
            };
            let mut record = original.clone();
            record["content"] = Value::Array(parts[..boundary].to_vec());
            Some(record)
        } else {
            None
        };
        let record = partial.as_ref().unwrap_or(original);
        if record["type"] == "user" {
            let mut content = Vec::new();
            chat.block(&record["text"], &mut content);
            if let Some(files) = record["files"].as_array() {
                for file in files {
                    chat.image(file, &mut content);
                }
            }
            chat.push(Role::User, content);
        } else {
            chat.message(record["type"].as_str().unwrap_or(""), &record["content"]);
        }
        if is_reverted {
            break;
        }
    }
    Ok(chat)
}

fn decode_kiro(header: &Value, records: &[Value]) -> Result<Conversation, String> {
    let mut chat = Conversation {
        cwd: required(header, "cwd")?.into(),
        title: header["title"].as_str().unwrap_or("").into(),
        ..Default::default()
    };
    let mut chunks = HashMap::<(Role, String), usize>::new();
    for record in records {
        let role = match record["kind"].as_str() {
            Some("Prompt") => Role::User,
            Some("AssistantMessage") => Role::Assistant,
            _ => continue,
        };
        let data = &record["data"];
        let mut content = Vec::new();
        chat.block(&data["content"], &mut content);
        if content.is_empty() {
            continue;
        }
        if let Some(id) = data["message_id"].as_str() {
            let key = (role, id.to_owned());
            if let Some(index) = chunks.get(&key) {
                if let (Some(Content::Text(tail)), Some(Content::Text(head))) =
                    (chat.turns[*index].content.last_mut(), content.first())
                {
                    tail.push_str(head);
                    content.remove(0);
                }
                chat.turns[*index].content.extend(content);
                continue;
            }
            chunks.insert(key, chat.turns.len());
        }
        chat.push(role, content);
    }
    chat.notes.insert("Kiro conversion reads v2 ACP prompt and assistant content. Other ACP events and v3 server-owned sessions are not converted.".into());
    Ok(chat)
}

fn branch<'a>(
    records: &'a [Value],
    id_key: &str,
    parent_key: &str,
    leaf: Option<&str>,
    notes: &mut BTreeSet<String>,
) -> Result<Vec<&'a Value>, String> {
    let by_id: HashMap<_, _> = records
        .iter()
        .filter_map(|r| r[id_key].as_str().map(|id| (id, r)))
        .collect();
    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = leaf;
    while let Some(id) = cursor {
        if !seen.insert(id) {
            return Err("Source chat contains a cycle in its branch links.".into());
        }
        let Some(record) = by_id.get(id) else {
            notes.insert("The selected branch starts after an unavailable ancestor; only its available history was converted.".into());
            break;
        };
        ordered.push(*record);
        cursor = record[parent_key].as_str();
    }
    ordered.reverse();
    if seen.len() < by_id.len() {
        notes.insert("Only the latest active branch was converted. Alternate branches remain in the source chat.".into());
    }
    Ok(ordered)
}

impl Conversation {
    fn push(&mut self, role: Role, content: Vec<Content>) {
        if role == Role::Assistant
            && content
                .iter()
                .any(|block| matches!(block, Content::Image { .. }))
        {
            let (images, text): (Vec<_>, Vec<_>) = content
                .into_iter()
                .partition(|block| matches!(block, Content::Image { .. }));
            if !text.is_empty() {
                self.turns.push(Turn {
                    role,
                    content: text,
                });
            }
            let mut content = vec![Content::Text("[Historical assistant attachments]".into())];
            content.extend(images);
            self.turns.push(Turn {
                role: Role::User,
                content,
            });
            self.notes.insert("Assistant images were moved to a labeled user attachment message for destination compatibility.".into());
        } else if !content.is_empty() {
            self.turns.push(Turn { role, content });
        }
    }

    fn message(&mut self, role: &str, value: &Value) {
        let role = match role {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "" => return,
            _ => {
                self.notes.insert("Source system and developer instructions were omitted; the destination agent uses its own instructions.".into());
                return;
            }
        };
        let mut content = Vec::new();
        self.block(value, &mut content);
        self.push(role, content);
    }

    fn summary(&mut self, value: &Value) {
        if let Some(summary) = value.as_str().filter(|s| !s.is_empty()) {
            self.push(
                Role::Assistant,
                vec![Content::Text(format!(
                    "[Historical conversation summary]\n{summary}"
                ))],
            );
            self.notes
                .insert("Compaction and branch summaries were retained as historical text.".into());
        }
    }

    fn tool(&mut self, kind: &str, value: &Value) {
        let mut content = Vec::new();
        self.historical_tool(kind, value, &mut content);
        self.push(Role::Assistant, content);
    }

    fn historical_tool(&mut self, kind: &str, value: &Value, output: &mut Vec<Content>) {
        output.push(Content::Text(format!("[Historical tool {kind}]\n{value}")));
        self.notes.insert(
            "Tool calls and results were retained as historical text, not executable tool records."
                .into(),
        );
    }

    fn block(&mut self, value: &Value, output: &mut Vec<Content>) {
        match value {
            Value::String(text) if !text.is_empty() => output.push(Content::Text(text.clone())),
            Value::Array(blocks) => {
                for block in blocks {
                    self.block(block, output);
                }
            }
            Value::Object(_) => match value["type"].as_str().unwrap_or("") {
                "" if value["kind"] == "Text" => {
                    if value["data"].is_string() {
                        self.block(&value["data"], output);
                    } else {
                        self.block(&value["data"]["text"], output);
                    }
                }
                "text" | "input_text" | "output_text" => self.block(&value["text"], output),
                "thinking" | "reasoning" | "redacted_thinking" => {
                    self.notes
                        .insert("Private or signed reasoning was omitted.".into());
                }
                "tool_use" | "toolCall" | "tool_call" => {
                    self.historical_tool("call", value, output)
                }
                "tool_result" | "toolResult" | "tool" => {
                    self.historical_tool("result", value, output)
                }
                "image" | "input_image" | "file" => self.image(value, output),
                _ => {
                    output.push(Content::Text(format!("[Historical content]\n{value}")));
                    self.notes.insert(
                        "Unrecognized content was retained as historical JSON text.".into(),
                    );
                }
            },
            _ => {}
        }
    }

    fn image(&mut self, value: &Value, output: &mut Vec<Content>) {
        let mime = value["mimeType"]
            .as_str()
            .or(value["mime"].as_str())
            .or(value["source"]["media_type"].as_str());
        let data = value["data"].as_str().or(value["source"]["data"].as_str());
        if let (Some(mime), Some(data)) = (mime, data) {
            if mime.starts_with("image/") {
                output.push(Content::Image {
                    mime: mime.into(),
                    data: data.into(),
                });
                return;
            }
        }
        let url = value["image_url"]
            .as_str()
            .or(value["url"].as_str())
            .or(value["source"]["url"].as_str());
        if let Some(url) = url {
            if let Some((mime, data)) = url
                .strip_prefix("data:")
                .and_then(|s| s.split_once(";base64,"))
            {
                if mime.starts_with("image/") {
                    output.push(Content::Image {
                        mime: mime.into(),
                        data: data.into(),
                    });
                    return;
                }
            }
        }
        output.push(Content::Text(format!(
            "[Historical attachment reference]\n{value}"
        )));
        self.notes.insert("External attachments were retained as references. Only inline images are embedded in converted chats.".into());
    }
}

fn encode(target: &SelectedHarness, chat: &Conversation, root: &Path) -> Result<(), String> {
    let id = Uuid::new_v4().to_string();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis() as u64;
    let (_, _, stamp) = archive::current_codex_timestamp();
    match target {
        SelectedHarness::Claude(_) => {
            let mut parent: Option<String> = None;
            let mut records = Vec::new();
            for turn in &chat.turns {
                let uuid = Uuid::new_v4().to_string();
                let content: Vec<_> = turn.content.iter().map(|block| match block {
                    Content::Text(text) => json!({"type":"text","text":text}),
                    Content::Image { mime, data } => json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}}),
                }).collect();
                let mut message = json!({"role":turn.role.name(),"content":content});
                if turn.role == Role::Assistant {
                    message["id"] = json!(format!("msg_{}", Uuid::new_v4().simple()));
                    message["type"] = json!("message");
                    message["model"] = json!("orbit-imported");
                    message["stop_reason"] = json!("end_turn");
                    message["stop_sequence"] = Value::Null;
                    message["usage"] = json!({"input_tokens":0,"output_tokens":0});
                }
                records.push(
                    json!({"type":turn.role.name(),"uuid":uuid,"parentUuid":parent,
                    "sessionId":id,"cwd":chat.cwd,"timestamp":stamp,"isSidechain":false,
                    "userType":"external","version":"2.0.0","message":message}),
                );
                parent = Some(uuid);
            }
            records.push(json!({"type":"custom-title","sessionId":id,"customTitle":chat.title}));
            archive::stage_claude_conversion(root, &id, &jsonl(&records)?)
        }
        SelectedHarness::Codex(_) => {
            let mut records = vec![json!({"timestamp":stamp,"type":"session_meta","payload":{
                "id":id,"session_id":id,"timestamp":stamp,"cwd":chat.cwd,"originator":"orbit","cli_version":"0.0.0",
                "source":"cli"}})];
            for turn in &chat.turns {
                let content: Vec<_> = turn.content.iter().map(|block| match block {
                    Content::Text(text) => json!({"type":if turn.role == Role::User {"input_text"} else {"output_text"},"text":text}),
                    Content::Image { mime, data } => json!({"type":"input_image","image_url":format!("data:{mime};base64,{data}")}),
                }).collect();
                records.push(json!({"timestamp":stamp,"type":"response_item","payload":{
                    "type":"message","role":turn.role.name(),"content":content}}));
                let text = turn
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let payload = if turn.role == Role::User {
                    let images: Vec<_> = turn
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            Content::Image { mime, data } => {
                                Some(format!("data:{mime};base64,{data}"))
                            }
                            _ => None,
                        })
                        .collect();
                    json!({"type":"user_message","message":text,"images":images,"local_images":[]})
                } else {
                    json!({"type":"agent_message","message":text})
                };
                records.push(json!({"timestamp":stamp,"type":"event_msg","payload":payload}));
            }
            archive::stage_codex_conversion(root, &id, &chat.title, &jsonl(&records)?)
        }
        SelectedHarness::Pi(_) => {
            let mut records = vec![
                json!({"type":"session","version":3,"id":id,"timestamp":stamp,"cwd":chat.cwd}),
            ];
            let mut parent: Option<String> = None;
            for turn in &chat.turns {
                let entry_id = Uuid::new_v4().simple().to_string();
                let content: Vec<_> = turn
                    .content
                    .iter()
                    .map(|block| match block {
                        Content::Text(text) => json!({"type":"text","text":text}),
                        Content::Image { mime, data } => {
                            json!({"type":"image","mimeType":mime,"data":data})
                        }
                    })
                    .collect();
                let mut message =
                    json!({"role":turn.role.name(),"content":content,"timestamp":millis});
                if turn.role == Role::Assistant {
                    message["api"] = json!("openai-responses");
                    message["provider"] = json!("orbit");
                    message["model"] = json!("imported");
                    message["usage"] = json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,
                        "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}});
                    message["stopReason"] = json!("stop");
                }
                records.push(json!({"type":"message","id":entry_id,"parentId":parent,"timestamp":stamp,"message":message}));
                parent = Some(entry_id);
            }
            records.push(json!({"type":"session_info","id":Uuid::new_v4().simple().to_string(),"parentId":parent,"timestamp":stamp,"name":chat.title}));
            super::pi::stage_conversion(root, &id, &jsonl(&records)?)
        }
        SelectedHarness::Opencode(_) => {
            let native_id = format!("ses_{}", Uuid::new_v4().simple());
            let messages: Vec<_> = chat.turns.iter().enumerate().map(|(i,turn)| {
                let id = format!("msg_{}_{i:08}", Uuid::new_v4().simple());
                let text = turn.content.iter().filter_map(|block| match block {Content::Text(text)=>Some(text.as_str()), _=>None}).collect::<Vec<_>>().join("\n");
                if turn.role == Role::User {
                    let files: Vec<_> = turn.content.iter().filter_map(|block| match block {
                        Content::Image { mime, data } => Some(json!({"mime":mime,"data":data,"source":{"type":"inline"}})), _=>None,
                    }).collect();
                    json!({"id":id,"type":"user","time":{"created":millis+i as u64},"text":text,"files":files})
                } else {
                    json!({"id":id,"type":"assistant","time":{"created":millis+i as u64,"completed":millis+i as u64},
                        "agent":"build","model":{"providerID":"orbit","id":"imported"},"content":[{"type":"text","text":text}]})
                }
            }).collect();
            let transfer = json!({"info":{"id":native_id,"projectID":"converted","cost":0,
                "tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},
                "title":chat.title,"location":{"directory":chat.cwd},
                "time":{"created":millis,"updated":millis}},"messages":messages});
            super::opencode::stage_conversion(
                root,
                &native_id,
                &serde_json::to_vec(&transfer).map_err(|e| e.to_string())?,
            )
        }
        SelectedHarness::Kiro(_) => Err("Kiro transcript import is unavailable.".into()),
    }
}

pub(super) fn stage_archive(
    root: &Path,
    filename: &str,
    bytes: &[u8],
    manifest: &impl Serialize,
) -> Result<(), String> {
    let manifest = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    for (name, data) in [(filename, bytes), ("manifest.json", manifest.as_slice())] {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(name))
            .map_err(|e| e.to_string())?;
        file.write_all(data)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn jsonl(records: &[Value]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for record in records {
        serde_json::to_writer(&mut bytes, record).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_str(&read_text(path)?).map_err(|e| e.to_string())
}

fn read_text(path: &Path) -> Result<String, String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("The source chat exceeds the 256 MiB conversion limit.".into());
    }
    fs::read_to_string(path).map_err(|e| e.to_string())
}

fn read_lines(path: &Path) -> Result<Vec<Value>, String> {
    parse_lines(&read_text(path)?)
}

fn parse_lines(contents: &str) -> Result<Vec<Value>, String> {
    if contents.len() as u64 > MAX_BYTES {
        return Err("The source chat exceeds the conversion size limit.".into());
    }
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(i, line)| {
            serde_json::from_str(line)
                .map_err(|e| format!("Source chat contains invalid JSON at record {}: {e}", i + 1))
        })
        .collect()
}

fn required<'a>(object: &'a Value, field: &str) -> Result<&'a str, String> {
    object[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Source chat is missing {field}."))
}

fn native_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    value
        .strip_prefix(r"\\?\UNC\")
        .map(|s| format!(r"\\{s}"))
        .unwrap_or_else(|| value.strip_prefix(r"\\?\").unwrap_or(&value).into())
}

#[cfg(test)]
mod tests;
