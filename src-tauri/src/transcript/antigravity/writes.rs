use super::*;
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

static WRITES: Mutex<()> = Mutex::new(());
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

pub(super) fn store_root(harness: &AntigravityHarness, store: &str) -> Result<PathBuf, String> {
    match store {
        "app" => Ok(harness.data_roots()[0].clone()),
        "cli" => Ok(harness.data_roots()[1].clone()),
        _ => Err("Unknown Antigravity chat store".into()),
    }
}

fn identity(id: &str) -> Result<(&str, &str), String> {
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 3
        || parts[0] != "antigravity"
        || !matches!(parts[1], "app" | "cli")
        || uuid::Uuid::parse_str(parts[2]).is_err()
    {
        return Err("Invalid Antigravity chat identity".into());
    }
    Ok((parts[1], parts[2]))
}

fn ensure_closed() -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("tasklist");
        command.args(["/FO", "CSV", "/NH"]);
        command
    } else {
        let mut command = std::process::Command::new("ps");
        command.args(["-A", "-o", "comm="]);
        command
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .output()
        .map_err(|e| format!("Could not check whether Antigravity is closed: {e}"))?;
    if !output.status.success() {
        return Err("Could not check whether Antigravity is closed. No chats were changed.".into());
    }
    let running = String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let name = if cfg!(windows) {
            line.trim_start_matches('"').split('"').next().unwrap_or("")
        } else {
            line.rsplit('/').next().unwrap_or("").trim()
        };
        matches!(
            name.to_ascii_lowercase().as_str(),
            "agy" | "agy.exe" | "antigravity" | "antigravity.exe"
        )
    });
    if running {
        return Err("Close the Antigravity App and all agy sessions before importing, restoring, converting into, or deleting Antigravity chats.".into());
    }
    Ok(())
}

fn digest(path: &Path) -> Result<String, String> {
    ensure_plain_components(path)?;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hash.finalize()))
}

fn collect(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
    total: &mut u64,
) -> Result<(), String> {
    ensure_plain_components(directory)?;
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        ensure_plain_components(&path)?;
        if path.is_dir() {
            collect(root, &path, files, total)?;
        } else if path.is_file() {
            *total += fs::metadata(&path).map_err(|e| e.to_string())?.len();
            if *total > MAX_ARCHIVE_BYTES || files.len() >= 10_000 {
                return Err("Antigravity chat artifacts exceed the backup limit".into());
            }
            files.insert(
                path.strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/"),
                digest(&path)?,
            );
        } else {
            return Err("Unsupported Antigravity artifact file".into());
        }
    }
    Ok(())
}

fn sidecars(root: &Path, id: &str) -> Result<BTreeMap<String, String>, String> {
    let mut files = BTreeMap::new();
    collect(root, &root.join("brain").join(id), &mut files, &mut 0)?;
    let annotations = root.join("annotations").join(format!("{id}.pbtxt"));
    ensure_plain_components(&annotations)?;
    if annotations.exists() {
        files.insert(format!("annotations/{id}.pbtxt"), digest(&annotations)?);
    }
    Ok(files)
}

pub(super) fn snapshot_sidecars(
    harness: &AntigravityHarness,
    id: &str,
    stage: &Path,
) -> Result<BTreeMap<String, String>, String> {
    let (store, native_id) = identity(id)?;
    let root = store_root(harness, store)?;
    let files = sidecars(&root, native_id)?;
    for (relative, checksum) in &files {
        let destination = stage.join(relative);
        fs::create_dir_all(destination.parent().ok_or("No artifact parent")?)
            .map_err(|e| e.to_string())?;
        fs::copy(root.join(relative), &destination).map_err(|e| e.to_string())?;
        if digest(&destination)? != *checksum {
            return Err("Chat artifacts changed during backup. Try again.".into());
        }
    }
    if sidecars(&root, native_id)? != files {
        return Err("Chat artifacts changed during backup. Try again.".into());
    }
    Ok(files)
}

fn verified_archive(root: &Path, manifest: &Value) -> Result<BTreeMap<String, String>, String> {
    let (_, id) = identity(
        manifest["sourceSessionId"]
            .as_str()
            .ok_or("Backup is missing its source identity")?,
    )?;
    let db = readonly(&root.join("session.db"))?;
    let objects: Vec<(String,String)> = db.prepare("SELECT type,name FROM sqlite_master WHERE type IN ('table','view','trigger') ORDER BY name").map_err(|e| e.to_string())?.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?.collect::<Result<_,_>>().map_err(|e|e.to_string())?;
    let expected = [
        "battle_mode_infos",
        "executor_metadata",
        "gen_metadata",
        "parent_references",
        "steps",
        "trajectory_meta",
        "trajectory_metadata_blob",
    ];
    if objects
        .iter()
        .map(|(kind, name)| (kind.as_str(), name.as_str()))
        .ne(expected.map(|name| ("table", name)))
    {
        return Err("Unsupported Antigravity database schema. No chat was imported.".into());
    }
    archive_turns(root, manifest)?;
    let stored: String = db
        .query_row("SELECT cascade_id FROM trajectory_meta", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if stored != id {
        return Err("Antigravity backup identity does not match its database".into());
    }
    let files: BTreeMap<String, String> = match manifest.get("files") {
        Some(files) => {
            serde_json::from_value(files.clone()).map_err(|_| "Invalid backup artifact list")?
        }
        None if manifest["version"] == 1 => BTreeMap::new(),
        None => return Err("Backup is missing its artifact list".into()),
    };
    let mut total = fs::metadata(root.join("session.db"))
        .map_err(|e| e.to_string())?
        .len();
    for (relative, checksum) in &files {
        let path = Path::new(relative);
        if relative.contains(['\\', ':'])
            || path
                .components()
                .any(|p| !matches!(p, std::path::Component::Normal(_)))
            || !(relative.starts_with(&format!("brain/{id}/"))
                || relative == &format!("annotations/{id}.pbtxt"))
        {
            return Err("Unsafe Antigravity backup artifact path".into());
        }
        let path = root.join(path);
        ensure_plain_components(&path)?;
        total += fs::metadata(&path)
            .map_err(|e| format!("Backup artifact is missing: {e}"))?
            .len();
        if total > MAX_ARCHIVE_BYTES || files.len() > 10_000 {
            return Err("Antigravity backup exceeds the import limit".into());
        }
        if digest(&path)? != *checksum {
            return Err("Antigravity artifact checksum does not match".into());
        }
    }
    Ok(files)
}

pub fn import_archive(
    harness: &AntigravityHarness,
    source: &Path,
    project: &Path,
) -> Result<ArchiveReport, String> {
    let _lock = WRITES
        .lock()
        .map_err(|_| "Antigravity chat lock is unavailable")?;
    ensure_plain_components(source)?;
    let manifest_path = source.join("manifest.json");
    ensure_plain_components(&manifest_path)?;
    if fs::metadata(&manifest_path)
        .map_err(|e| e.to_string())?
        .len()
        > 4 * 1024 * 1024
    {
        return Err("Antigravity manifest exceeds the size limit".into());
    }
    let manifest: Value =
        serde_json::from_slice(&fs::read(manifest_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let files = verified_archive(source, &manifest)?;
    let project = project
        .canonicalize()
        .map_err(|e| format!("Choose an available working directory: {e}"))?;
    if !project.is_dir() {
        return Err("The working directory must be a directory".into());
    }
    let source_id = manifest["sourceSessionId"]
        .as_str()
        .ok_or("Missing source identity")?;
    let (store, original_id) = identity(source_id)?;
    let root = store_root(harness, store)?;
    ensure_closed()?;
    let native = native::Native::start(harness, store)?;
    let conversations = root.join("conversations");
    ensure_plain_components(&conversations)?;
    fs::create_dir_all(&conversations).map_err(|e| e.to_string())?;
    let original = conversations.join(format!("{original_id}.db"));
    ensure_plain_components(&original)?;
    // Opaque runtime records may refer to the original identity. Never relabel a native backup.
    if original.exists()
        || root.join("brain").join(original_id).exists()
        || root
            .join("annotations")
            .join(format!("{original_id}.pbtxt"))
            .exists()
    {
        return Err("This Antigravity chat identity already exists. Orbit will not overwrite it or relabel opaque runtime records. Import it into another installation, or delete the existing chat after saving a backup.".into());
    }
    let existing = metadata(harness)?;
    let title = manifest["title"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported chat");
    let mut name = title.to_string();
    let mut suffix = 1;
    while existing
        .iter()
        .any(|m| m.title.as_deref() == Some(name.as_str()))
    {
        name = format!("{title}-{suffix}");
        suffix += 1;
    }
    let stage = tempfile::Builder::new()
        .prefix(".orbit-chat-")
        .tempdir_in(&root)
        .map_err(|e| e.to_string())?;
    let db_path = stage.path().join("session.db");
    fs::copy(source.join("session.db"), &db_path).map_err(|e| e.to_string())?;
    if digest(&db_path)? != manifest["sha256"].as_str().unwrap_or("") {
        return Err("Backup changed during import".into());
    }
    let original_project = manifest["workingDirectory"]
        .as_str()
        .and_then(|p| Path::new(p).canonicalize().ok());
    if original_project.as_deref() != Some(project.as_path()) {
        let db = Connection::open(&db_path).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA trusted_schema=OFF")
            .map_err(|e| e.to_string())?;
        let old: Vec<u8> = db
            .query_row(
                "SELECT data FROM trajectory_metadata_blob WHERE id='main'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("Unsupported native chat metadata: {e}"))?;
        let uri = reqwest::Url::from_directory_path(&project)
            .map_err(|_| "Invalid workspace path")?
            .to_string();
        let metadata = replace_workspace(&old, &uri)?;
        db.execute(
            "UPDATE trajectory_metadata_blob SET data=?1 WHERE id='main'",
            [metadata],
        )
        .map_err(|e| e.to_string())?;
    }
    let mut created = Vec::new();
    let result = (|| {
        for relative in files
            .keys()
            .chain(std::iter::once(&"session.db".to_string()))
        {
            let target = if relative == "session.db" {
                original.clone()
            } else {
                root.join(relative)
            };
            ensure_plain_components(&target)?;
            fs::create_dir_all(target.parent().ok_or("Missing import parent")?)
                .map_err(|e| e.to_string())?;
            if relative == "session.db" {
                fs::hard_link(&db_path, &target)
                    .map_err(|e| format!("Could not publish the native chat database: {e}"))?;
                created.push(target);
                continue;
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|e| e.to_string())?;
            created.push(target.clone());
            let input = source.join(relative);
            let mut input = fs::File::open(input).map_err(|e| e.to_string())?;
            std::io::copy(&mut input, &mut output)
                .and_then(|_| output.sync_all())
                .map_err(|e| e.to_string())?;
            if let Some(expected) = files.get(relative) {
                if digest(&target)? != *expected {
                    return Err("Backup artifacts changed during import".into());
                }
            }
        }
        native.call("LoadTrajectory", json!({"cascadeId":original_id}))?;
        let catalog =
            Connection::open(root.join("conversation_summaries.db")).map_err(|e| e.to_string())?;
        catalog
            .busy_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        let (_, _, stamp) = super::super::archive::current_codex_timestamp();
        let uri = reqwest::Url::from_directory_path(&project)
            .map_err(|_| "Invalid imported workspace")?
            .to_string();
        let count: i64 = readonly(&original)?
            .query_row("SELECT count(*) FROM steps", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        catalog.execute("INSERT INTO conversation_summaries(conversation_id,title,workspace_uris,last_modified_time,last_user_input_time,step_count) VALUES(?1,?2,?3,?4,?4,?5) ON CONFLICT(conversation_id) DO UPDATE SET title=excluded.title,workspace_uris=excluded.workspace_uris,last_modified_time=excluded.last_modified_time,last_user_input_time=excluded.last_user_input_time,step_count=excluded.step_count",rusqlite::params![original_id,name,json!([uri]).to_string(),stamp,count]).map_err(|e|e.to_string())?;
        drop(catalog);
        native.call(
            "UpdateConversationAnnotations",
            json!({"cascadeId":original_id,"annotations":{"title":name},"mergeAnnotations":true}),
        )?;
        Ok(ArchiveReport {
            action: ArchiveAction::Imported,
            source_session_id: source_id.into(),
            session_id: source_id.into(),
            files: files.len() + 1,
            path: original.to_string_lossy().into_owned(),
        })
    })();
    if result.is_err() {
        // Keep any files already accepted by the native service; they remain discoverable and recoverable.
        if readonly(&root.join("conversation_summaries.db"))
            .ok()
            .and_then(|c| {
                c.query_row(
                    "SELECT 1 FROM conversation_summaries WHERE conversation_id=?1",
                    [original_id],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
            })
            .is_none()
        {
            drop(native);
            for path in created.into_iter().rev() {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_dir_all(root.join("brain").join(original_id));
        } else {
            return result.map_err(|error|format!("The chat was copied into Antigravity, but finishing the import failed: {error}. Refresh chats before trying again. The source archive is unchanged."));
        }
    }
    result
}

fn replace_workspace(bytes: &[u8], uri: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let start = offset;
        let tag = varint(bytes, &mut offset)?;
        match tag & 7 {
            0 => {
                varint(bytes, &mut offset)?;
            }
            1 => offset = offset.checked_add(8).ok_or("Invalid native metadata")?,
            2 => {
                let size = usize::try_from(varint(bytes, &mut offset)?)
                    .map_err(|_| "Invalid native metadata")?;
                offset = offset.checked_add(size).ok_or("Invalid native metadata")?;
            }
            5 => offset = offset.checked_add(4).ok_or("Invalid native metadata")?,
            _ => return Err("Unsupported native metadata wire type".into()),
        }
        let field = bytes
            .get(start..offset)
            .ok_or("Truncated native metadata")?;
        if !matches!(tag >> 3, 1 | 7 | 18) {
            output.extend_from_slice(field);
        }
    }
    output.extend(proto_bytes(1, &proto_bytes(1, uri.as_bytes())));
    output.extend(proto_bytes(7, uri.as_bytes()));
    Ok(output)
}

pub(crate) fn delete_chat(
    harness: &AntigravityHarness,
    cache: &Path,
    id: &str,
    source: &Path,
    destination: &Path,
) -> Result<super::super::deletion::DeleteReport, String> {
    let _lock = WRITES
        .lock()
        .map_err(|_| "Antigravity chat lock is unavailable")?;
    ensure_closed()?;
    let (store, native_id) = identity(id)?;
    drop(validate(harness, id, source)?);
    let root = store_root(harness, store)?;
    let native = native::Native::start(harness, store)?;
    native.call("LoadTrajectory", json!({"cascadeId":native_id}))?;
    let summaries = native.call("GetAllCascadeTrajectories", json!({}))?;
    let summary = summaries["trajectorySummaries"]
        .get(native_id)
        .ok_or("Antigravity did not report the loaded chat's status. No chat was deleted.")?;
    if summary["status"] != "CASCADE_RUN_STATUS_IDLE"
        || summary["hasActiveChildren"] == true
        || summary["hasRunningSubagents"] == true
        || summary["notFullyIdle"] == true
        || summary["backgroundTaskCount"].as_u64().unwrap_or(0) > 0
    {
        return Err(
            "The Antigravity chat is busy or has active child sessions. No chat was deleted."
                .into(),
        );
    }
    let catalog = readonly(&root.join("conversation_summaries.db"))?;
    let related: i64 = catalog.query_row("SELECT count(*) FROM conversation_summaries WHERE parent_conversation_id=?1 OR (conversation_id=?1 AND (parent_conversation_id<>'' OR battle_id<>'' OR not_fully_idle<>0))",[native_id],|r|r.get(0)).map_err(|e| e.to_string())?;
    if related > 0 {
        return Err("This chat is active or linked to child, parent, or battle chats. Orbit refused to delete it independently.".into());
    }
    drop(catalog);
    let archive = export_archive(harness, cache, id, destination)?;
    let removal = (|| {
        let backup = Path::new(&archive.path);
        let manifest: Value = serde_json::from_slice(
            &fs::read(backup.join("manifest.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let files = verified_archive(backup, &manifest)?;
        if manifest["sourceSessionId"] != id
            || manifest["sourceFingerprint"].as_str() != fingerprint(source).as_deref()
            || sidecars(&root, native_id)? != files
        {
            return Err("Chat changed after backup. No chat was deleted.".to_string());
        }
        native.call("DeleteCascadeTrajectory", json!({"cascadeId":native_id}))?;
        if source.exists() {
            return Err("Antigravity did not remove the native chat database. The backup remains available.".into());
        }
        Ok(())
    })();
    removal.map_err(|error| format!("{error} Native archive saved at {}.", archive.path))?;
    Ok(super::super::deletion::DeleteReport {
        path: archive.path,
        deleted_sessions: vec![id.into()],
        warning: None,
    })
}

fn proto_number(mut value: u64) -> Vec<u8> {
    let mut result = Vec::new();
    while value >= 128 {
        result.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    result.push(value as u8);
    result
}

pub(super) fn proto_bytes(number: u64, data: &[u8]) -> Vec<u8> {
    [
        proto_number(number * 8 + 2),
        proto_number(data.len() as u64),
        data.to_vec(),
    ]
    .concat()
}

pub(super) fn proto_scalar(number: u64, value: u64) -> Vec<u8> {
    [proto_number(number * 8), proto_number(value)].concat()
}

pub(crate) fn stage_conversion(
    root: &Path,
    title: &str,
    cwd: &str,
    turns: &[Value],
) -> Result<(), String> {
    let id = uuid::Uuid::new_v4().to_string();
    let db_path = root.join("session.db");
    let db = Connection::open(&db_path).map_err(|e| e.to_string())?;
    db.execute_batch(include_str!("schema.sql"))
        .map_err(|e| e.to_string())?;
    db.execute(
        "INSERT INTO trajectory_meta VALUES(?1,?2,4,17)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), id],
    )
    .map_err(|e| e.to_string())?;
    let uri = reqwest::Url::from_directory_path(cwd)
        .map_err(|_| "Invalid converted workspace")?
        .to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let metadata = replace_workspace(
        &[
            proto_bytes(6, id.as_bytes()),
            proto_bytes(2, &proto_scalar(1, now)),
        ]
        .concat(),
        &uri,
    )?;
    db.execute(
        "INSERT INTO trajectory_metadata_blob VALUES('main',?1)",
        [metadata],
    )
    .map_err(|e| e.to_string())?;
    let transaction = db.unchecked_transaction().map_err(|e| e.to_string())?;
    for (i, turn) in turns.iter().enumerate() {
        let kind = if turn["role"] == "user" { 14 } else { 15 };
        let mut body = Vec::new();
        let mut texts = Vec::new();
        for block in turn["content"]
            .as_array()
            .ok_or("Invalid converted content")?
        {
            if block["type"] == "text" {
                let text = block["text"].as_str().ok_or("Invalid converted text")?;
                if kind == 14 {
                    body.extend(proto_bytes(3, &proto_bytes(1, text.as_bytes())));
                } else {
                    texts.push(text);
                }
            } else if block["type"] == "image" {
                let image = &block["source"];
                let mime = image["media_type"]
                    .as_str()
                    .ok_or("Missing converted image type")?;
                let data = STANDARD
                    .decode(image["data"].as_str().ok_or("Missing converted image")?)
                    .map_err(|e| e.to_string())?;
                body.extend(proto_bytes(
                    if kind == 14 { 9 } else { 20 },
                    &[proto_bytes(1, mime.as_bytes()), proto_bytes(2, &data)].concat(),
                ));
            }
        }
        if kind == 15 && !texts.is_empty() {
            body.extend(proto_bytes(1, texts.join("\n").as_bytes()));
        }
        let metadata = [
            proto_bytes(1, &proto_scalar(1, now)),
            proto_bytes(8, &proto_scalar(1, now)),
        ]
        .concat();
        let payload = [
            proto_scalar(1, kind),
            proto_scalar(4, 3),
            proto_bytes(5, &metadata),
            proto_bytes(if kind == 14 { 19 } else { 20 }, &body),
        ]
        .concat();
        transaction.execute("INSERT INTO steps(idx,step_type,status,metadata,step_payload)VALUES(?1,?2,3,?3,?4)",rusqlite::params![i,kind,metadata,payload]).map_err(|e|e.to_string())?;
    }
    transaction.commit().map_err(|e| e.to_string())?;
    drop(db);
    fs::write(root.join("manifest.json"),serde_json::to_vec_pretty(&json!({"format":"orbit-antigravity","version":2,"sourceSessionId":format!("antigravity:cli:{id}"),"title":title,"workingDirectory":cwd,"sha256":digest(&db_path)?,"files":{}})).map_err(|e|e.to_string())?).map_err(|e|e.to_string())
}
