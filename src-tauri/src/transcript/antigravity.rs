use super::{
    archive::{ArchiveAction, ArchiveReport},
    catalog::Metadata,
    ingest,
};
use crate::harness::{antigravity::AntigravityHarness, claude::skills::ensure_plain_components};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, time::Duration};

mod native;
mod writes;
pub use writes::import_archive;
use writes::store_root;
pub(super) use writes::{delete_chat, stage_conversion};
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const MAX_STEPS: usize = 100_000;

fn readonly(path: &Path) -> Result<Connection, String> {
    ensure_plain_components(path)?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    Ok(connection)
}

pub(super) fn fingerprint(path: &Path) -> Option<String> {
    Some(format!(
        "antigravity-v1|{}|{}",
        ingest::file_fingerprint(path, None)?,
        ingest::file_fingerprint(&path.with_extension("db-wal"), None).unwrap_or_default()
    ))
}

pub(super) fn metadata(harness: &AntigravityHarness) -> Result<Vec<Metadata>, String> {
    let mut entries = Vec::new();
    for (store, root) in ["app", "cli"].into_iter().zip(harness.data_roots()) {
        let catalog = root.join("conversation_summaries.db");
        if !catalog.exists() {
            continue;
        }
        let connection = readonly(&catalog)?;
        let mut query = connection.prepare("SELECT conversation_id, title, workspace_uris, last_modified_time FROM conversation_summaries").map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, title, uris, updated_at) = row.map_err(|e| e.to_string())?;
            if uuid::Uuid::parse_str(&id).is_err() {
                continue;
            }
            let path = root.join("conversations").join(format!("{id}.db"));
            if !path.is_file() {
                continue;
            }
            ensure_plain_components(&path)?;
            let project = uris
                .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
                .into_iter()
                .flatten()
                .find_map(|uri| reqwest::Url::parse(&uri).ok()?.to_file_path().ok())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            entries.push(Metadata {
                id: format!("antigravity:{store}:{id}"),
                project,
                title,
                updated_at,
                fingerprint: fingerprint(&path),
                path,
            });
        }
    }
    Ok(entries)
}

fn validate(harness: &AntigravityHarness, id: &str, path: &Path) -> Result<Connection, String> {
    let mut parts = id.split(':');
    if parts.next() != Some("antigravity") {
        return Err("Invalid Antigravity chat ID".into());
    }
    let store = parts.next().ok_or("Missing chat store")?;
    let native_id = parts.next().ok_or("Missing native chat ID")?;
    if parts.next().is_some() || uuid::Uuid::parse_str(native_id).is_err() {
        return Err("Invalid native chat ID".into());
    }
    let roots = harness.data_roots();
    let root = match store {
        "app" => &roots[0],
        "cli" => &roots[1],
        _ => return Err("Unknown Antigravity chat store".into()),
    };
    let expected = root.join("conversations").join(format!("{native_id}.db"));
    ensure_plain_components(&expected)?;
    if expected.canonicalize().map_err(|e| e.to_string())?
        != path.canonicalize().map_err(|e| e.to_string())?
    {
        return Err("Chat source is outside its native store".into());
    }
    let connection = readonly(path)?;
    let identity: String = connection
        .query_row(
            "SELECT cascade_id FROM trajectory_meta LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if identity != native_id {
        return Err("Chat identity changed. Refresh chats.".into());
    }
    Ok(connection)
}

pub(super) fn index_session(
    harness: &AntigravityHarness,
    cache: &mut Connection,
    id: &str,
    path: &Path,
    project: &str,
) -> Result<ingest::IngestReport, String> {
    let connection = validate(harness, id, path)?;
    let stamp = fingerprint(path);
    let (title, updated_at): (Option<String>, Option<String>) = cache
        .query_row(
            "SELECT title, updated_at FROM sessions WHERE session_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let turns = read_turns(&connection)?;
    let messages = turns
        .iter()
        .enumerate()
        .map(|(i, turn)| {
            let role = turn["role"].as_str().unwrap_or("assistant");
            let text = turn["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let images = turn["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|block| block["type"] == "image")
                .count();
            ingest::IndexedMessage {
                uuid: format!("{id}:{i}"),
                parent_uuid: i.checked_sub(1).map(|n| format!("{id}:{n}")),
                message_type: role.into(),
                text,
                raw_json: json!({"type":role,"message":turn}).to_string(),
                position: i,
                attachment: (images > 0).then(|| {
                    format!(
                        "{images} inline image(s), preserved in the native backup and conversion"
                    )
                }),
                tool_result: None,
                tool_result_note: None,
            }
        })
        .collect::<Vec<_>>();
    let count = ingest::replace_indexed_session(
        cache,
        ingest::IndexedSession {
            id,
            project,
            source_path: path,
            updated_at: updated_at.as_deref(),
            title: title.as_deref(),
            fingerprint: stamp.as_deref(),
        },
        &messages,
        &[],
    )
    .map_err(|e| e.to_string())?;
    Ok(ingest::IngestReport {
        sessions: 1,
        messages_indexed: count,
        lines_read: count,
        ..Default::default()
    })
}

pub fn export_archive(
    harness: &AntigravityHarness,
    cache: &Path,
    id: &str,
    destination: &Path,
) -> Result<ArchiveReport, String> {
    let catalog = ingest::open_database(cache).map_err(|e| e.to_string())?;
    let (path, project, title): (String, String, Option<String>) = catalog
        .query_row(
            "SELECT source_path, project, title FROM sessions WHERE session_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    let source = validate(harness, id, Path::new(&path))?;
    let source_fingerprint = fingerprint(Path::new(&path));
    ensure_plain_components(destination)?;
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    let stage = tempfile::Builder::new()
        .prefix(".antigravity-export-")
        .tempdir_in(destination)
        .map_err(|e| e.to_string())?;
    let database = stage.path().join("session.db");
    let mut target = Connection::open(&database).map_err(|e| e.to_string())?;
    {
        let backup =
            rusqlite::backup::Backup::new(&source, &mut target).map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if backup.step(256).map_err(|e| e.to_string())? == rusqlite::backup::StepResult::Done {
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err("Antigravity database stayed busy. Try exporting again.".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    drop(target);
    let bytes = fs::read(&database).map_err(|e| e.to_string())?;
    let files = writes::snapshot_sidecars(harness, id, stage.path())?;
    let artifact_bytes = files
        .keys()
        .try_fold(0u64, |total, path| {
            fs::metadata(stage.path().join(path)).map(|m| total + m.len())
        })
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 + artifact_bytes > 256 * 1024 * 1024 {
        return Err("Antigravity backup exceeds the size limit".into());
    }
    if source_fingerprint != fingerprint(Path::new(&path)) {
        return Err("Chat changed during backup. Try again.".into());
    }
    let manifest = json!({"format":"orbit-antigravity", "version":2, "sourceSessionId":id, "sourceFingerprint":source_fingerprint,"title":title.unwrap_or_default(), "workingDirectory":project, "sha256":format!("{:x}",Sha256::digest(bytes)),"files":files, "note":"Native database, annotations, and brain artifacts. External files and installation-wide runtime state are not bundled."});
    fs::write(
        stage.path().join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let output = destination.join(format!("antigravity-{}", uuid::Uuid::new_v4()));
    fs::rename(stage.path(), &output).map_err(|e| e.to_string())?;
    Ok(ArchiveReport {
        action: ArchiveAction::Exported,
        source_session_id: id.into(),
        session_id: id.into(),
        files: files.len() + 2,
        path: output.to_string_lossy().into_owned(),
    })
}

pub(super) fn archive_turns(root: &Path, manifest: &Value) -> Result<Vec<Value>, String> {
    if manifest["format"] != "orbit-antigravity"
        || !matches!(manifest["version"].as_u64(), Some(1 | 2))
    {
        return Err("Unsupported Antigravity backup format".into());
    }
    let path = root.join("session.db");
    ensure_plain_components(&path)?;
    if fs::metadata(&path).map_err(|e| e.to_string())?.len() > 256 * 1024 * 1024 {
        return Err("Antigravity backup exceeds the size limit".into());
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    if manifest["sha256"].as_str() != Some(&format!("{:x}", Sha256::digest(bytes))) {
        return Err("Antigravity backup checksum does not match".into());
    }
    read_turns(&readonly(&path)?)
}

fn read_turns(connection: &Connection) -> Result<Vec<Value>, String> {
    let mut query = connection
        .prepare("SELECT step_type, step_format, step_payload FROM steps ORDER BY idx LIMIT 100001")
        .map_err(|e| e.to_string())?;
    let mut rows = query.query([]).map_err(|e| e.to_string())?;
    let mut turns = Vec::new();
    let mut i = 0;
    let mut total_bytes = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if i >= MAX_STEPS {
            return Err("Chat exceeds the supported step limit".into());
        }
        i += 1;
        let kind: i64 = row.get(0).map_err(|e| e.to_string())?;
        if !matches!(kind, 14 | 15) {
            continue;
        }
        let format: i64 = row.get(1).map_err(|e| e.to_string())?;
        if format != 0 {
            return Err(format!("Unsupported Antigravity step format {format}"));
        }
        let payload = row
            .get_ref(2)
            .map_err(|e| e.to_string())?
            .as_blob()
            .map_err(|e| e.to_string())?;
        total_bytes += payload.len();
        if total_bytes > 256 * 1024 * 1024 {
            return Err("Chat exceeds the supported decoding size limit".into());
        }
        let step = fields(payload)?;
        let Some(body) = step
            .iter()
            .find(|(n, _)| *n == if kind == 14 { 19 } else { 20 })
            .map(|(_, bytes)| *bytes)
        else {
            return Err("Antigravity message is missing its native body".into());
        };
        let parts = fields(body)?;
        let mut content = Vec::new();
        if kind == 14 {
            let items = parts
                .iter()
                .filter(|(n, _)| *n == 3)
                .map(|(_, b)| fields(b))
                .collect::<Result<Vec<_>, _>>()?;
            let texts = items
                .iter()
                .flat_map(|item| item.iter())
                .filter(|(n, _)| *n == 1)
                .map(|(_, b)| text(b))
                .collect::<Result<Vec<_>, _>>()?;
            let query = if texts.is_empty() {
                string_field(&parts, 1)?.or(string_field(&parts, 2)?)
            } else {
                Some(texts.join("\n"))
            };
            if let Some(value) = query.filter(|s| !s.is_empty()) {
                content.push(json!({"type":"text","text":value}));
            }
            for (_, bytes) in parts.iter().filter(|(n, _)| *n == 5) {
                let image = fields(bytes)?;
                if let (Some(data), Some(mime)) =
                    (string_field(&image, 1)?, string_field(&image, 2)?)
                {
                    image_block(&mut content, &mime, &data)?;
                }
            }
        } else if let Some(value) = string_field(&parts, 8)?
            .filter(|s| !s.is_empty())
            .or(string_field(&parts, 1)?)
        {
            if !value.is_empty() {
                content.push(json!({"type":"text","text":value}));
            }
        }
        for (_, bytes) in parts
            .iter()
            .filter(|(n, _)| *n == if kind == 14 { 9 } else { 20 })
        {
            let media = fields(bytes)?;
            if let (Some(mime), Some((_, data))) = (
                string_field(&media, 1)?,
                media.iter().find(|(n, _)| *n == 2),
            ) {
                image_block(&mut content, &mime, &STANDARD.encode(data))?;
            }
        }
        if !content.is_empty() {
            turns.push(json!({"role":if kind == 14 {"user"} else {"assistant"},"content":content}));
        }
    }
    Ok(turns)
}

fn image_block(content: &mut Vec<Value>, mime: &str, data: &str) -> Result<(), String> {
    if matches!(
        mime,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) {
        STANDARD
            .decode(data)
            .map_err(|_| "Invalid Antigravity inline image encoding")?;
        content
            .push(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}}));
    }
    Ok(())
}

fn text(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| "Invalid UTF-8 in Antigravity message".into())
}
fn string_field(parts: &[(u64, &[u8])], number: u64) -> Result<Option<String>, String> {
    parts
        .iter()
        .rev()
        .find(|(n, _)| *n == number)
        .map(|(_, b)| text(b))
        .transpose()
}

fn varint(bytes: &[u8], offset: &mut usize) -> Result<u64, String> {
    let mut value = 0;
    for shift in (0..70).step_by(7) {
        let byte = *bytes
            .get(*offset)
            .ok_or("Truncated Antigravity protobuf value")?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return Err("Antigravity protobuf integer overflow".into());
        }
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("Invalid Antigravity protobuf integer".into())
}

fn fields(bytes: &[u8]) -> Result<Vec<(u64, &[u8])>, String> {
    if bytes.len() > MAX_PAYLOAD {
        return Err("Antigravity step exceeds the supported payload limit".into());
    }
    let mut offset = 0;
    let mut fields = Vec::new();
    while offset < bytes.len() {
        let tag = varint(bytes, &mut offset)?;
        if tag >> 3 == 0 {
            return Err("Invalid Antigravity protobuf field".into());
        }
        let length = match tag & 7 {
            0 => {
                varint(bytes, &mut offset)?;
                continue;
            }
            1 => 8,
            2 => usize::try_from(varint(bytes, &mut offset)?)
                .map_err(|_| "Protobuf field length overflow")?,
            5 => 4,
            _ => return Err("Unsupported Antigravity protobuf wire type".into()),
        };
        let end = offset
            .checked_add(length)
            .ok_or("Protobuf field length overflow")?;
        let value = bytes
            .get(offset..end)
            .ok_or("Truncated Antigravity protobuf field")?;
        if tag & 7 == 2 {
            if fields.len() >= 100_000 {
                return Err("Antigravity protobuf has too many fields".into());
            }
            fields.push((tag >> 3, value));
        }
        offset = end;
    }
    Ok(fields)
}

#[cfg(test)]
mod tests;
