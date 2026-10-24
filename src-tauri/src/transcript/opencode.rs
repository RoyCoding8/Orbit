use super::{
    archive::{ArchiveAction, ArchiveReport},
    ingest::{
        open_database, prune_vanished_sessions, replace_indexed_session, IndexedMessage,
        IndexedSession, IngestReport,
    },
    TranscriptError,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::env;
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{mpsc, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

use crate::harness::process_lifetime::ChildTree;

const DATA_FILE: &str = "session.json";
const MANIFEST_FILE: &str = "manifest.json";
const ARCHIVE_VERSION: u32 = 1;
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
const CLI_OUTPUT_LIMIT: usize = 64 * 1024 * 1024;
const SESSION_PAGE_SIZE: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: u32,
    source_session_id: String,
    data_file: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeSessionListItem {
    id: String,
    title: Option<String>,
    #[serde(skip)]
    directory: PathBuf,
}

pub(super) fn stage_conversion(root: &Path, id: &str, bytes: &[u8]) -> Result<(), String> {
    let manifest = Manifest {
        version: ARCHIVE_VERSION,
        source_session_id: id.into(),
        data_file: DATA_FILE.into(),
        sha256: format!("{:x}", Sha256::digest(bytes)),
    };
    super::conversion::stage_archive(root, DATA_FILE, bytes, &manifest)
}

pub fn ingest(app_data_dir: &Path, _project: &Path) -> Result<IngestReport, TranscriptError> {
    ingest_with_environment(app_data_dir, &CliEnvironment::default())
}

fn ingest_with_environment(
    app_data_dir: &Path,
    environment: &CliEnvironment,
) -> Result<IngestReport, TranscriptError> {
    let sessions = list_native_sessions(None, environment, SESSION_PAGE_SIZE)
        .map_err(|error| TranscriptError::Io(std::io::Error::other(error)))?;
    let mut connection = open_database(app_data_dir)?;
    let mut seen_paths = Vec::with_capacity(sessions.len());
    let mut report = IngestReport {
        sessions: 0,
        lines_read: 0,
        messages_indexed: 0,
        malformed_lines: 0,
        skipped_files: 0,
        skipped_state_lines: 0,
    };
    for session in sessions {
        let id = format!("opencode:{}", session.id);
        let source = PathBuf::from(format!("opencode://session/{}", session.id));
        seen_paths.push(source.to_string_lossy().into_owned());
        match export_native(&session.id, Some(&session.directory), environment) {
            Ok(bytes) => match index_export(&mut connection, &bytes, &id, &source) {
                Ok((indexed, count)) => {
                    report.sessions += 1;
                    report.messages_indexed += indexed;
                    report.lines_read += count;
                }
                Err(_) => report.skipped_files += 1,
            },
            Err(_) => report.skipped_files += 1,
        }
    }
    prune_vanished_sessions(&mut connection, &seen_paths)?;
    Ok(report)
}

pub fn export_archive(
    _config_root: &Path,
    app_data_dir: &Path,
    session_id: &str,
    destination: &Path,
) -> Result<ArchiveReport, String> {
    let connection = open_database(app_data_dir).map_err(|error| error.to_string())?;
    let directory: String = connection
        .query_row(
            "SELECT project FROM sessions WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("Could not find the indexed OpenCode project: {error}"))?;
    export_archive_with_environment(
        session_id,
        destination,
        Some(Path::new(&directory)),
        &CliEnvironment::default(),
    )
}

fn export_archive_with_environment(
    session_id: &str,
    destination: &Path,
    project: Option<&Path>,
    environment: &CliEnvironment,
) -> Result<ArchiveReport, String> {
    let native_id = native_session_id(session_id)?;
    let bytes = export_native(&native_id, project, environment)?;
    let session: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("OpenCode returned invalid session JSON: {error}"))?;
    let actual_id = session
        .get("info")
        .and_then(|info| info.get("id"))
        .and_then(Value::as_str)
        .ok_or("OpenCode export has no session id")?;
    if actual_id != native_id {
        return Err("OpenCode exported a different session than requested".into());
    }
    let archive = unique_archive_directory(destination, &native_id)?;
    fs::create_dir(&archive)
        .map_err(|error| format!("could not create archive {}: {error}", archive.display()))?;
    write_new(&archive.join(DATA_FILE), &bytes)?;
    let manifest = Manifest {
        version: ARCHIVE_VERSION,
        source_session_id: native_id.clone(),
        data_file: DATA_FILE.into(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    if let Err(error) = write_new(&archive.join(MANIFEST_FILE), &manifest_bytes) {
        let _ = fs::remove_file(archive.join(DATA_FILE));
        let _ = fs::remove_dir(&archive);
        return Err(error);
    }
    Ok(ArchiveReport {
        action: ArchiveAction::Exported,
        source_session_id: session_id.into(),
        session_id: session_id.into(),
        files: 1,
        path: archive.to_string_lossy().into_owned(),
    })
}

pub fn import_archive(
    _config_root: &Path,
    source: &Path,
    project: &Path,
) -> Result<ArchiveReport, String> {
    import_archive_with_environment(source, project, &CliEnvironment::default())
}

fn import_archive_with_environment(
    source: &Path,
    project: &Path,
    environment: &CliEnvironment,
) -> Result<ArchiveReport, String> {
    let manifest_path = source.join(MANIFEST_FILE);
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| format!("could not read OpenCode archive manifest: {error}"))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid OpenCode archive manifest: {error}"))?;
    if manifest.version != ARCHIVE_VERSION || manifest.data_file != DATA_FILE {
        return Err("unsupported OpenCode archive format".into());
    }
    let native_bytes = fs::read(source.join(&manifest.data_file))
        .map_err(|error| format!("could not read archived session: {error}"))?;
    let digest = format!("{:x}", Sha256::digest(&native_bytes));
    if digest != manifest.sha256 {
        return Err("OpenCode archive checksum does not match session bytes".into());
    }
    let mut transfer: Value = serde_json::from_slice(&native_bytes)
        .map_err(|error| format!("archived native session is invalid JSON: {error}"))?;
    let imported_id = transfer
        .get("info")
        .and_then(|info| info.get("id"))
        .and_then(Value::as_str)
        .ok_or("archived session has no id")?
        .to_owned();
    if imported_id != manifest.source_session_id {
        return Err("archive manifest session id does not match native export".into());
    }
    let selected_directory = project
        .canonicalize()
        .map_err(|error| format!("could not resolve selected project directory: {error}"))?;
    let selected_location = json!({ "directory": selected_directory.to_string_lossy() });
    transfer["info"]["location"] = selected_location.clone();
    transfer["location"] = selected_location;
    let existing_sessions = list_native_sessions(Some(project), environment, SESSION_PAGE_SIZE)?;
    let collision_ids = existing_sessions
        .iter()
        .map(|item| item.id.clone())
        .collect::<HashSet<_>>();
    let imported_title = transfer
        .get("info")
        .and_then(|info| info.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("Imported session");
    let used_titles = existing_sessions
        .iter()
        .filter_map(|item| item.title.as_deref())
        .collect::<HashSet<_>>();
    let title_suffix = if used_titles.contains(imported_title) {
        let mut suffix = 1usize;
        while used_titles.contains(format!("{imported_title}-{suffix}").as_str()) {
            suffix += 1;
        }
        Some(suffix)
    } else {
        None
    };
    let mut target_id = if collision_ids.contains(&imported_id) {
        format!("ses_{}", Uuid::new_v4().simple())
    } else {
        imported_id.clone()
    };
    // Message and tool-part IDs are globally unique in the native store. Remap
    // them on every import so a distinct session archive cannot collide.
    remap_native_ids(&mut transfer, &target_id)?;
    if let Some(suffix) = title_suffix {
        let title = transfer["info"]["title"]
            .as_str()
            .unwrap_or("Imported session")
            .to_owned();
        transfer["info"]["title"] = Value::String(format!("{title}-{suffix}"));
    }
    let default_temp_dir = env::temp_dir();
    let transfer_file = environment
        .temp_dir
        .as_deref()
        .unwrap_or(&default_temp_dir)
        .join(format!(
            "orbit-opencode-import-{}.json",
            Uuid::new_v4().simple()
        ));
    let transfer_bytes = serde_json::to_vec(&transfer).map_err(|error| error.to_string())?;
    write_new(&transfer_file, &transfer_bytes)?;
    let transfer_file_arg = transfer_file.to_string_lossy().into_owned();
    let project_arg = project.to_string_lossy().into_owned();
    let args = [
        "session",
        "import",
        transfer_file_arg.as_str(),
        "--standalone",
        "--directory",
        project_arg.as_str(),
    ];
    let mut output = native_cli(&args, Some(project), environment);
    if output.as_ref().is_ok_and(is_identity_collision) && target_id == imported_id {
        let replacement_id = format!("ses_{}", Uuid::new_v4().simple());
        transfer["info"]["id"] = Value::String(replacement_id.clone());
        remap_session_reference_fields(
            &mut transfer["info"],
            &imported_id,
            &replacement_id,
            &HashMap::new(),
            &HashMap::new(),
        );
        target_id = replacement_id;
        let transfer_bytes = serde_json::to_vec(&transfer).map_err(|error| error.to_string())?;
        fs::write(&transfer_file, transfer_bytes)
            .map_err(|error| format!("could not update temporary native session: {error}"))?;
        output = native_cli(&args, Some(project), environment);
    }
    let _ = fs::remove_file(&transfer_file);
    let output = output?;
    if !output.status.success() {
        return Err(cli_error(&output));
    }
    let imported = parse_cli_json(&output.stdout).and_then(|value| {
        value
            .get("id")
            .or_else(|| value.get("data").and_then(|data| data.get("id")))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let target_id = imported.unwrap_or(target_id);
    let imported_export = export_native(&target_id, Some(project), environment)?;
    verify_import_directory(&imported_export, project)?;
    Ok(ArchiveReport {
        action: ArchiveAction::Imported,
        source_session_id: session_id_from_manifest(&manifest),
        session_id: format!("opencode:{target_id}"),
        files: 1,
        path: source.to_string_lossy().into_owned(),
    })
}

fn index_export(
    connection: &mut Connection,
    bytes: &[u8],
    indexed_id: &str,
    source_path: &Path,
) -> Result<(usize, usize), String> {
    let transfer: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let info = transfer
        .get("info")
        .and_then(Value::as_object)
        .ok_or("OpenCode session export has no info object")?;
    let native_id = info
        .get("id")
        .and_then(Value::as_str)
        .ok_or("OpenCode session export has no session id")?;
    let project = info
        .get("location")
        .and_then(|value| value.get("directory"))
        .and_then(Value::as_str)
        .or_else(|| info.get("projectID").and_then(Value::as_str))
        .unwrap_or("(unknown project)");
    let updated_at = info
        .get("time")
        .and_then(|value| value.get("updated"))
        .and_then(Value::as_i64)
        .map(|value| value.to_string());
    let title = info.get("title").and_then(Value::as_str);
    let records = transfer
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("OpenCode session export has no messages array")?;
    let mut messages = Vec::with_capacity(records.len());
    for (position, record) in records.iter().enumerate() {
        let Some(object) = record.as_object() else {
            continue;
        };
        let message_type = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let uuid = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{native_id}:message:{position}"));
        let parent_uuid = object
            .get("parentID")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let text = message_text(record);
        messages.push(IndexedMessage {
            uuid,
            parent_uuid,
            message_type: message_type.into(),
            text,
            raw_json: serde_json::to_string(record).unwrap_or_else(|_| "{}".into()),
            position,
            attachment: None,
            tool_result: None,
            tool_result_note: None,
        });
    }
    let indexed = replace_indexed_session(
        connection,
        IndexedSession {
            id: indexed_id,
            project,
            source_path,
            updated_at: updated_at.as_deref(),
            title,
        },
        &messages,
        &[],
    )
    .map_err(|error| error.to_string())?;
    Ok((indexed, records.len()))
}

fn message_text(record: &Value) -> String {
    if let Some(text) = record.get("text").and_then(Value::as_str) {
        return text.to_owned();
    }
    record
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| {
            let kind = part.get("type").and_then(Value::as_str)?;
            match kind {
                "text" | "reasoning" => part.get("text").and_then(Value::as_str).map(str::to_owned),
                "tool" => {
                    let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let output = part
                        .get("state")
                        .and_then(|state| state.get("output"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    Some(if output.is_empty() {
                        format!("[{name}]")
                    } else {
                        format!("[{name}] {output}")
                    })
                }
                _ => None,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn remap_native_ids(transfer: &mut Value, target_session: &str) -> Result<(), String> {
    let old_session = transfer
        .get("info")
        .and_then(|info| info.get("id"))
        .and_then(Value::as_str)
        .ok_or("native session has no id")?
        .to_owned();
    let messages = transfer
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .ok_or("native session messages must be an array")?;
    let mut message_ids = HashMap::<String, String>::new();
    let mut tool_ids = HashMap::<String, String>::new();
    let namespace = Uuid::new_v4().simple().to_string();
    for (position, message) in messages.iter_mut().enumerate() {
        let object = message
            .as_object_mut()
            .ok_or("native message is not an object")?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or("native message has no id; collision-safe import is unsupported")?
            .to_owned();
        if message_ids
            .insert(id, format!("msg_{namespace}_{position:08}"))
            .is_some()
        {
            return Err("native export contains duplicate message IDs; collision-safe import is unsupported".into());
        }
        if object.get("type").and_then(Value::as_str) == Some("assistant") {
            if let Some(parts) = object.get_mut("content").and_then(Value::as_array_mut) {
                for part in parts
                    .iter_mut()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool"))
                {
                    let part_id = part
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or(
                            "assistant tool part has no id; collision-safe import is unsupported",
                        )?
                        .to_owned();
                    tool_ids
                        .entry(part_id)
                        .or_insert_with(|| format!("call_{}", Uuid::new_v4().simple()));
                }
            }
        }
    }
    let message_ids_ref = &message_ids;
    for message in messages.iter_mut() {
        let object = message.as_object_mut().expect("validated message object");
        if let Some(old) = object.get("id").and_then(Value::as_str) {
            object.insert("id".into(), Value::String(message_ids_ref[old].clone()));
        }
        for key in ["parentID", "messageID"] {
            if let Some(old) = object.get(key).and_then(Value::as_str) {
                if let Some(new) = message_ids_ref.get(old) {
                    object.insert(key.into(), Value::String(new.clone()));
                }
            }
        }
        if let Some(parts) = object.get_mut("content").and_then(Value::as_array_mut) {
            for part in parts
                .iter_mut()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool"))
            {
                if let Some(old) = part.get("id").and_then(Value::as_str) {
                    if let Some(new) = tool_ids.get(old) {
                        part["id"] = Value::String(new.clone());
                    }
                }
            }
        }
    }
    transfer["info"]["id"] = Value::String(target_session.to_owned());
    remap_session_reference_fields(
        &mut transfer["info"],
        &old_session,
        target_session,
        &message_ids,
        &tool_ids,
    );
    Ok(())
}

fn remap_session_reference_fields(
    info: &mut Value,
    old_session: &str,
    new_session: &str,
    messages: &HashMap<String, String>,
    tools: &HashMap<String, String>,
) {
    if let Some(parent) = info.get("parentID").and_then(Value::as_str) {
        if parent == old_session {
            info["parentID"] = Value::String(new_session.to_owned());
        }
    }
    if let Some(fork) = info.get_mut("fork").and_then(Value::as_object_mut) {
        if fork.get("sessionID").and_then(Value::as_str) == Some(old_session) {
            fork.insert("sessionID".into(), Value::String(new_session.to_owned()));
        }
        if let Some(message_id) = fork
            .get("messageID")
            .and_then(Value::as_str)
            .and_then(|id| messages.get(id))
        {
            fork.insert("messageID".into(), Value::String(message_id.clone()));
        }
        if let Some(boundary) = fork.get_mut("boundary").and_then(Value::as_object_mut) {
            if let Some(message_id) = boundary
                .get("messageID")
                .and_then(Value::as_str)
                .and_then(|id| messages.get(id))
            {
                boundary.insert("messageID".into(), Value::String(message_id.clone()));
            }
        }
    }
    if let Some(revert) = info.get_mut("revert").and_then(Value::as_object_mut) {
        if let Some(message_id) = revert
            .get("messageID")
            .and_then(Value::as_str)
            .and_then(|id| messages.get(id))
        {
            revert.insert("messageID".into(), Value::String(message_id.clone()));
        }
        if let Some(part_id) = revert
            .get("partID")
            .and_then(Value::as_str)
            .and_then(|id| tools.get(id))
        {
            revert.insert("partID".into(), Value::String(part_id.clone()));
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct CliEnvironment {
    set: Vec<(OsString, OsString)>,
    remove: Vec<OsString>,
    temp_dir: Option<PathBuf>,
}

fn list_native_sessions(
    project: Option<&Path>,
    environment: &CliEnvironment,
    page_size: usize,
) -> Result<Vec<NativeSessionListItem>, String> {
    let scopes = if let Some(project) = project {
        vec![(
            format!("directory={}", project.to_string_lossy()),
            project.to_path_buf(),
        )]
    } else {
        let projects = native_api(&["project.list"], None, environment)?;
        projects
            .as_array()
            .ok_or("OpenCode project list is not an array")?
            .iter()
            .map(|project| -> Result<_, String> {
                let id = project["id"].as_str().ok_or("OpenCode project has no ID")?;
                let directory = project["canonical"]
                    .as_str()
                    .ok_or("OpenCode project has no canonical directory")?;
                Ok((format!("project={id}"), PathBuf::from(directory)))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut sessions = Vec::new();
    let mut seen = HashSet::new();
    for (scope, directory) in scopes {
        let limit = format!("limit={page_size}");
        let mut cursor = String::new();
        let mut cursors = HashSet::new();
        loop {
            let mut args = vec!["session.list", "--param", &scope, "--param", &limit];
            let cursor_param = format!("cursor={cursor}");
            if !cursor.is_empty() {
                args.extend(["--param", &cursor_param]);
            }
            let page = native_api(&args, None, environment)?;
            let records = page["data"]
                .as_array()
                .ok_or("OpenCode session list has no data array")?;
            for record in records {
                let mut session: NativeSessionListItem = serde_json::from_value(record.clone())
                    .map_err(|error| format!("OpenCode session metadata is invalid: {error}"))?;
                session.directory = record["location"]["directory"]
                    .as_str()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| directory.clone());
                if seen.insert(session.id.clone()) {
                    sessions.push(session);
                }
            }
            let Some(next) = page["cursor"]["next"].as_str().filter(|s| !s.is_empty()) else {
                break;
            };
            if records.is_empty() {
                break;
            }
            if !cursors.insert(next.to_owned()) {
                return Err("OpenCode returned a repeated session page cursor".into());
            }
            cursor = next.to_owned();
        }
    }
    Ok(sessions)
}

fn native_api(
    args: &[&str],
    project: Option<&Path>,
    environment: &CliEnvironment,
) -> Result<Value, String> {
    let mut command = vec!["api"];
    command.extend_from_slice(args);
    command.push("--standalone");
    let output = native_cli(&command, project, environment)?;
    if !output.status.success() {
        return Err(cli_error(&output));
    }
    parse_cli_json(&output.stdout).ok_or_else(|| "OpenCode API returned invalid JSON".to_owned())
}

fn export_native(
    id: &str,
    project: Option<&Path>,
    environment: &CliEnvironment,
) -> Result<Vec<u8>, String> {
    let output = native_cli(
        &["session", "export", id, "--standalone"],
        project,
        environment,
    )?;
    if !output.status.success() {
        return Err(cli_error(&output));
    }
    let bytes = output.stdout;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("OpenCode session export returned invalid JSON: {error}"))?;
    if value
        .get("info")
        .and_then(|info| info.get("id"))
        .and_then(Value::as_str)
        != Some(id)
    {
        return Err("OpenCode session export returned an unexpected session".into());
    }
    Ok(bytes)
}

pub(super) fn session_has_children(
    id: &str,
    project: &Path,
    environment: &CliEnvironment,
) -> Result<bool, String> {
    validate_native_session_id(id)?;
    let parent = format!("parentID={id}");
    let value = native_api(
        &["session.list", "--param", &parent, "--param", "limit=1"],
        Some(project),
        environment,
    )?;
    let children = value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .ok_or_else(|| {
            "OpenCode child-session query returned an unexpected response.".to_owned()
        })?;
    Ok(!children.is_empty())
}

pub(super) fn delete_native_session(
    id: &str,
    project: &Path,
    environment: &CliEnvironment,
) -> Result<(), String> {
    validate_native_session_id(id)?;
    if session_has_children(id, project, environment)? {
        return Err("OpenCode deletion refused: this chat has child sessions.".into());
    }
    let output = native_cli(
        &["session", "delete", id, "--standalone"],
        Some(project),
        environment,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(cli_error(&output))
    }
}

fn validate_native_session_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("OpenCode session ID is invalid.".into());
    }
    Ok(())
}

fn native_cli(
    args: &[&str],
    directory: Option<&Path>,
    environment: &CliEnvironment,
) -> Result<Output, String> {
    let executable = native_executable()?;
    let mut command = Command::new(executable);
    command.args(args);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    for name in &environment.remove {
        command.env_remove(name);
    }
    for (name, value) in &environment.set {
        command.env(name, value);
    }
    run_bounded_cli(command)
}

pub(super) fn native_executable() -> Result<PathBuf, String> {
    static EXECUTABLE: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    EXECUTABLE
        .get_or_init(resolve_and_check_native_executable)
        .clone()
}

fn resolve_and_check_native_executable() -> Result<PathBuf, String> {
    let executable = resolve_native_executable()?;
    let mut command = Command::new(&executable);
    command.args(["--version", "--standalone"]);
    let output = run_bounded_cli(command)?;
    if !output.status.success() {
        return Err(cli_error(&output));
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let major = version.split_whitespace().find_map(|token| {
        token
            .trim_start_matches('v')
            .split('.')
            .next()?
            .parse::<u32>()
            .ok()
    });
    if major != Some(2) {
        return Err(
            "OpenCode V2 CLI is required; upgrade the installed OpenCode CLI to major version 2"
                .into(),
        );
    }
    Ok(executable)
}

#[cfg(windows)]
fn resolve_native_executable() -> Result<PathBuf, String> {
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let direct = directory.join("opencode.exe");
        if direct.is_file() {
            return direct
                .canonicalize()
                .map_err(|error| format!("could not resolve OpenCode executable: {error}"));
        }
        for shim_name in ["opencode.cmd", "opencode.ps1"] {
            let shim = directory.join(shim_name);
            if !shim.is_file() {
                continue;
            }
            if let Some(executable) = executable_from_npm_shim(&shim) {
                return Ok(executable);
            }
        }
    }
    Err(
        "could not locate OpenCode CLI on PATH; install OpenCode V2 and make it available to Orbit"
            .into(),
    )
}

#[cfg(not(windows))]
fn resolve_native_executable() -> Result<PathBuf, String> {
    Ok(PathBuf::from("opencode"))
}

#[cfg(windows)]
fn executable_from_npm_shim(shim: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(shim).ok()?;
    let directory = shim.parent()?.canonicalize().ok()?;
    for line in text.lines() {
        let line = line.trim();
        let command = if let Some(command) = line.strip_prefix('"') {
            let Some((command, _)) = command.split_once('"') else {
                continue;
            };
            command
        } else if let Some(command) = line.strip_prefix("& \"") {
            let Some((command, _)) = command.split_once('"') else {
                continue;
            };
            command
        } else {
            continue;
        };
        let Some(relative) = command
            .strip_prefix("%dp0%\\")
            .or_else(|| command.strip_prefix("%~dp0%\\"))
            .or_else(|| command.strip_prefix("$basedir/"))
            .or_else(|| command.strip_prefix("$basedir\\"))
        else {
            continue;
        };
        let relative = Path::new(relative);
        if !relative.components().all(|part| {
            matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) || relative.extension().and_then(|value| value.to_str()) != Some("exe")
        {
            continue;
        }
        let candidate = directory.join(relative).canonicalize().ok()?;
        if candidate.starts_with(&directory) && candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn run_bounded_cli(mut command: Command) -> Result<Output, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildTree::spawn(&mut command)
        .map_err(|error| format!("could not start OpenCode V2 CLI: {error}"))?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or("OpenCode CLI stdout is unavailable")?;
    let stderr = child
        .child_mut()
        .stderr
        .take()
        .ok_or("OpenCode CLI stderr is unavailable")?;
    let stdout = spawn_bounded_reader(stdout);
    let stderr = spawn_bounded_reader(stderr);
    let deadline = Instant::now() + CLI_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .child_mut()
            .try_wait()
            .map_err(|error| format!("could not check OpenCode CLI process: {error}"))?
        {
            child.cleanup();
            break status;
        }
        if Instant::now() >= deadline {
            child.cleanup();
            return Err("OpenCode CLI exceeded its 30 second time limit".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = receive_bounded_output(stdout)?;
    let stderr = receive_bounded_output(stderr)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn spawn_bounded_reader<R: Read + Send + 'static>(
    mut reader: R,
) -> mpsc::Receiver<Result<(Vec<u8>, bool), String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut truncated = false;
        let mut buffer = [0; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let remaining = CLI_OUTPUT_LIMIT.saturating_sub(output.len());
                    let captured = count.min(remaining);
                    output.extend_from_slice(&buffer[..captured]);
                    truncated |= captured != count;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ =
                        sender.send(Err(format!("could not read OpenCode CLI output: {error}")));
                    return;
                }
            }
        }
        let _ = sender.send(Ok((output, truncated)));
    });
    receiver
}

fn receive_bounded_output(
    receiver: mpsc::Receiver<Result<(Vec<u8>, bool), String>>,
) -> Result<Vec<u8>, String> {
    let (bytes, truncated) = receiver
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "OpenCode CLI output did not close after process exit".to_owned())??;
    if truncated {
        return Err("OpenCode CLI output exceeded the 64 MB limit".into());
    }
    Ok(bytes)
}

fn parse_cli_json(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    serde_json::from_str(text).ok()
}

fn cli_error(output: &Output) -> String {
    let status = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".into());
    let raw = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    let reason = if raw.contains("already exists") || raw.contains("duplicate") {
        "native session identity collision"
    } else if raw.contains("not found") {
        "requested session was not found"
    } else if raw.contains("bad request") || raw.contains("invalid session") {
        "native session data was rejected"
    } else {
        "native command failed"
    };
    format!("OpenCode CLI failed (exit {status}): {reason}")
}

fn is_identity_collision(output: &Output) -> bool {
    let raw = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    raw.contains("already exists") || raw.contains("session id") && raw.contains("exists")
}

fn verify_import_directory(bytes: &[u8], selected_project: &Path) -> Result<(), String> {
    let transfer: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("OpenCode imported session export is invalid JSON: {error}"))?;
    let actual = transfer
        .get("info")
        .and_then(|info| info.get("location"))
        .and_then(|location| location.get("directory"))
        .and_then(Value::as_str)
        .ok_or("OpenCode imported session has no native location directory")?;
    let expected = selected_project
        .canonicalize()
        .map_err(|error| format!("could not resolve selected project directory: {error}"))?;
    let actual = Path::new(actual)
        .canonicalize()
        .map_err(|error| format!("could not resolve imported session directory: {error}"))?;
    if actual != expected {
        return Err(format!(
            "OpenCode imported the session under a different project directory: {} (expected {})",
            actual.display(),
            expected.display()
        ));
    }
    Ok(())
}

fn native_session_id(id: &str) -> Result<String, String> {
    id.strip_prefix("opencode:")
        .or_else(|| id.strip_prefix("opencode/"))
        .or_else(|| id.starts_with("ses_").then_some(id))
        .map(str::to_owned)
        .ok_or_else(|| "session ID is not an OpenCode V2 session".into())
}

fn unique_archive_directory(destination: &Path, session_id: &str) -> Result<PathBuf, String> {
    if !destination.is_dir() {
        return Err("archive destination must be an existing directory".into());
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for collision in 0..1000 {
        let suffix = if collision == 0 {
            String::new()
        } else {
            format!("-{collision}")
        };
        let path = destination.join(format!("opencode-{session_id}-{timestamp}{suffix}"));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("could not allocate a unique archive directory".into())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "could not create {} without overwriting: {error}",
                path.display()
            )
        })?;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .map_err(|error| {
            let _ = fs::remove_file(path);
            format!("could not finish {}: {error}", path.display())
        })
}

fn session_id_from_manifest(manifest: &Manifest) -> String {
    format!("opencode:{}", manifest.source_session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn fixture() -> Value {
        json!({
            "info": {"id":"ses_original","title":"Fixture","fork":{"sessionID":"ses_original","boundary":{"type":"through","messageID":"msg_user"}},"revert":{"messageID":"msg_assistant","partID":"call_original"}},
            "messages": [
                {"id":"msg_user","type":"user","text":"hello"},
                {"id":"msg_assistant","parentID":"msg_user","type":"assistant","content":[{"type":"text","text":"world"},{"type":"tool","id":"call_original","name":"read","state":{"output":"ok"}}]}
            ]
        })
    }

    #[test]
    fn collision_remap_changes_only_native_identity_fields_and_preserves_message_links() {
        let mut transfer = fixture();
        let original_text = transfer["messages"][1]["content"][0]["text"].clone();
        remap_native_ids(&mut transfer, "ses_new").unwrap();
        assert_eq!(transfer["info"]["id"], "ses_new");
        assert_ne!(transfer["messages"][0]["id"], "msg_user");
        assert_eq!(
            transfer["messages"][1]["parentID"],
            transfer["messages"][0]["id"]
        );
        assert_eq!(
            transfer["info"]["fork"]["boundary"]["messageID"],
            transfer["messages"][0]["id"]
        );
        assert_eq!(
            transfer["info"]["revert"]["messageID"],
            transfer["messages"][1]["id"]
        );
        assert_eq!(transfer["info"]["fork"]["sessionID"], "ses_new");
        assert_eq!(transfer["messages"][1]["content"][0]["text"], original_text);
        assert_ne!(transfer["messages"][1]["content"][1]["id"], "call_original");
        assert_eq!(
            transfer["info"]["revert"]["partID"],
            transfer["messages"][1]["content"][1]["id"]
        );
        assert!(
            transfer["messages"][0]["id"].as_str().unwrap()
                < transfer["messages"][1]["id"].as_str().unwrap()
        );
    }

    #[test]
    fn native_user_and_assistant_fixture_indexes_as_visible_conversation() {
        let temp = crate::test_support::tempdir().unwrap();
        let mut connection = open_database(temp.path()).unwrap();
        let bytes = serde_json::to_vec(&fixture()).unwrap();
        let source = Path::new("opencode://session/ses_original");
        let (indexed, count) =
            index_export(&mut connection, &bytes, "opencode:ses_original", source).unwrap();
        assert_eq!(indexed, 2);
        assert_eq!(count, 2);
        let page =
            super::super::spine::thread_page(temp.path(), "opencode:ses_original", 0, 10).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].text, "hello");
        assert!(page[1].text.contains("world"));
        assert!(page[1].text.contains("ok"));
        let _ = connection;
    }

    #[test]
    #[ignore = "requires installed OpenCode V2 CLI"]
    fn native_v2_cli_accepts_isolated_api_fixture_and_roundtrips_archive_collision() {
        let temp = crate::test_support::tempdir().unwrap();
        let config = temp.path().join("config");
        let data = temp.path().join("data");
        let cache = temp.path().join("cache");
        let state = temp.path().join("state");
        let home = temp.path().join("home");
        let native_db = temp.path().join("opencode.sqlite");
        let project_a = temp.path().join("project-a");
        let project_b = temp.path().join("project-b");
        let project_c = temp.path().join("project-c");
        let app_data = temp.path().join("orbit-data");
        let archives = temp.path().join("archives");
        let temp_files = temp.path().join("temp");
        for directory in [
            &config,
            &data,
            &cache,
            &state,
            &home,
            &project_a,
            &project_b,
            &project_c,
            &app_data,
            &archives,
            &temp_files,
        ] {
            fs::create_dir_all(directory).unwrap();
        }

        let mut environment = CliEnvironment {
            temp_dir: Some(temp_files),
            ..CliEnvironment::default()
        };
        for (name, value) in [
            ("XDG_CONFIG_HOME", config.as_os_str()),
            ("XDG_DATA_HOME", data.as_os_str()),
            ("XDG_CACHE_HOME", cache.as_os_str()),
            ("XDG_STATE_HOME", state.as_os_str()),
            ("HOME", home.as_os_str()),
            ("USERPROFILE", home.as_os_str()),
            ("APPDATA", home.as_os_str()),
            ("LOCALAPPDATA", data.as_os_str()),
            ("OPENCODE_DB", native_db.as_os_str()),
        ] {
            environment.set.push((name.into(), value.to_os_string()));
        }
        environment.remove.extend(
            [
                "OPENCODE_CONFIG",
                "OPENCODE_CONFIG_DIR",
                "OPENCODE_SERVER_PASSWORD",
                "OPENCODE_SERVER_USERNAME",
            ]
            .into_iter()
            .map(OsString::from),
        );

        let api_password = "orbit-native-acceptance-only";
        let mut server_environment = environment.clone();
        server_environment
            .set
            .push(("OPENCODE_SERVER_PASSWORD".into(), api_password.into()));
        server_environment
            .remove
            .retain(|name| name != "OPENCODE_SERVER_PASSWORD");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let mut server_command = Command::new(native_executable().unwrap());
        server_command.args(["serve", "--hostname", "127.0.0.1", "--port"]);
        server_command.arg(port.to_string());
        for name in &server_environment.remove {
            server_command.env_remove(name);
        }
        for (name, value) in &server_environment.set {
            server_command.env(name, value);
        }
        server_command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut server = ChildTree::spawn(&mut server_command).unwrap();

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let base_url = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if server.child_mut().try_wait().unwrap().is_some() {
                panic!("OpenCode V2 API exited before accepting the fixture");
            }
            if client
                .get(format!("{base_url}/api/session"))
                .basic_auth("opencode", Some(api_password))
                .send()
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            assert!(Instant::now() < deadline, "OpenCode V2 API did not start");
            thread::sleep(Duration::from_millis(100));
        }

        let create_session = |title: &str, directory: &Path| {
            let response = client
                .post(format!("{base_url}/api/session"))
                .basic_auth("opencode", Some(api_password))
                .json(&json!({
                    "title": title,
                    "location": {"directory": directory.to_string_lossy()}
                }))
                .send()
                .unwrap();
            assert!(
                response.status().is_success(),
                "native session create failed"
            );
            response.json::<Value>().unwrap()["data"]["id"]
                .as_str()
                .expect("native API returns a session id")
                .to_owned()
        };
        let source_id = create_session("Orbit V2 fixture", &project_a);
        let duplicate_title_id = create_session("Orbit V2 fixture", &project_b);
        for index in 0..3 {
            create_session(&format!("List pagination fixture {index}"), &project_c);
        }
        let source_info = client
            .get(format!("{base_url}/api/session/{source_id}"))
            .basic_auth("opencode", Some(api_password))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .unwrap()["data"]
            .clone();
        client
            .delete(format!("{base_url}/api/session/{source_id}"))
            .basic_auth("opencode", Some(api_password))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap();
        server.cleanup();

        let fixture_path = temp.path().join("native-fixture.json");
        fs::write(&fixture_path, serde_json::to_vec(&json!({
            "location": source_info["location"],
            "info": source_info,
            "messages": [
                {"id":"msg_fixture_0001", "time":{"created":1}, "type":"user", "text":"Orbit V2 native fixture message 0"},
                {"id":"msg_fixture_0002", "time":{"created":2,"completed":3}, "type":"assistant", "agent":"build", "model":{"providerID":"openai", "id":"fixture-only"}, "finish":"stop", "cost":0, "tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}}, "content":[{"type":"text", "text":"Orbit V2 native fixture message 1"}]}
            ]
        })).unwrap()).unwrap();
        let seeded = native_cli(
            &[
                "session",
                "import",
                &fixture_path.to_string_lossy(),
                "--standalone",
                "--directory",
                &project_a.to_string_lossy(),
            ],
            Some(&project_a),
            &environment,
        )
        .unwrap();
        assert!(
            seeded.status.success(),
            "fixture import: {}\n{}",
            String::from_utf8_lossy(&seeded.stderr),
            fs::read_to_string(&fixture_path).unwrap()
        );

        assert_eq!(
            list_native_sessions(Some(&project_c), &environment, 2)
                .unwrap()
                .len(),
            3,
            "adapter follows the page cursor beyond the requested two-session page"
        );
        let report = ingest_with_environment(&app_data, &environment).unwrap();
        assert_eq!(report.sessions, 5);
        assert_eq!(
            report.messages_indexed,
            2,
            "native export {}",
            String::from_utf8_lossy(
                &export_native(&source_id, Some(&project_a), &environment).unwrap()
            )
        );
        let page =
            super::super::spine::thread_page(&app_data, &format!("opencode:{source_id}"), 0, 10)
                .unwrap();
        assert_eq!(page.len(), 2);
        assert!(page[0].text.contains("native fixture message 0"));
        assert!(page[1].text.contains("native fixture message 1"));

        let exported = export_archive_with_environment(
            &format!("opencode:{source_id}"),
            &archives,
            Some(&project_a),
            &environment,
        )
        .unwrap();
        let archive = PathBuf::from(&exported.path);
        let original_bytes = fs::read(archive.join(DATA_FILE)).unwrap();
        let imported = import_archive_with_environment(&archive, &project_b, &environment).unwrap();
        assert_ne!(imported.session_id, format!("opencode:{source_id}"));
        assert_eq!(
            fs::read(archive.join(DATA_FILE)).unwrap(),
            original_bytes,
            "archive data remains byte-exact after import"
        );
        let imported_id = native_session_id(&imported.session_id).unwrap();
        let imported_bytes = export_native(&imported_id, Some(&project_b), &environment).unwrap();
        verify_import_directory(&imported_bytes, &project_b).unwrap();
        let imported_export: Value = serde_json::from_slice(&imported_bytes).unwrap();
        assert_eq!(imported_export["info"]["title"], "Orbit V2 fixture-1");
        let messages = imported_export["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["type"], "user");
        assert_eq!(messages[1]["type"], "assistant");
        assert_eq!(messages[0]["text"], "Orbit V2 native fixture message 0");
        assert_eq!(
            messages[1]["content"][0]["text"],
            "Orbit V2 native fixture message 1"
        );
        assert_ne!(imported_id, duplicate_title_id);
        let report = ingest_with_environment(&app_data, &environment).unwrap();
        assert_eq!(report.sessions, 6);
        let cached = super::super::ingest::list_sessions(&app_data).unwrap();
        assert_eq!(cached.len(), 6);
        assert!(cached
            .iter()
            .any(|session| session.session_id == format!("opencode:{source_id}")));
        assert!(cached
            .iter()
            .any(|session| session.session_id == imported.session_id));
        let all = list_native_sessions(None, &environment, 2).unwrap();
        assert_eq!(
            all.len(),
            6,
            "global discovery includes all three native projects"
        );
        assert!(all.iter().any(|session| session.id == source_id));
        assert!(all.iter().any(|session| session.id == imported_id));
        assert!(!session_has_children(&imported_id, &project_b, &environment).unwrap());
        delete_native_session(&imported_id, &project_b, &environment).unwrap();
        assert!(export_native(&imported_id, Some(&project_b), &environment).is_err());
        let restored = import_archive_with_environment(&archive, &project_b, &environment).unwrap();
        let restored_id = native_session_id(&restored.session_id).unwrap();
        let restored_bytes = export_native(&restored_id, Some(&project_b), &environment).unwrap();
        let restored: Value = serde_json::from_slice(&restored_bytes).unwrap();
        assert_eq!(
            restored["messages"][0]["text"],
            "Orbit V2 native fixture message 0"
        );
        let body = json!({"title":"child fixture", "parentID":restored_id, "location":{"directory":project_b}}).to_string();
        let child = native_api(
            &["POST", "/api/session", "--data", &body],
            Some(&project_b),
            &environment,
        )
        .unwrap();
        assert_eq!(child["data"]["parentID"], restored_id);
        assert!(list_native_sessions(None, &environment, SESSION_PAGE_SIZE)
            .unwrap()
            .iter()
            .any(|session| child["data"]["id"] == session.id));
        assert!(session_has_children(&restored_id, &project_b, &environment).unwrap());
        assert!(
            delete_native_session(&restored_id, &project_b, &environment)
                .unwrap_err()
                .contains("child")
        );
        assert!(export_native(&restored_id, Some(&project_b), &environment).is_ok());
    }
}
