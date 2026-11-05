use super::*;
use crate::harness::{pi::PiHarness, selected::SelectedHarness};

const ID: &str = "c8eec658-4e5d-4b6c-b12d-97da715f2da3";

fn number(mut value: u64) -> Vec<u8> {
    let mut result = Vec::new();
    while value >= 128 {
        result.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    result.push(value as u8);
    result
}

fn field(n: u64, data: &[u8]) -> Vec<u8> {
    [number(n * 8 + 2), number(data.len() as u64), data.to_vec()].concat()
}

fn user() -> Vec<u8> {
    field(
        19,
        &[
            field(3, &field(1, b"Hello")),
            field(5, &[field(1, b"aW1hZ2U="), field(2, b"image/png")].concat()),
            field(99, b"future context"),
        ]
        .concat(),
    )
}

fn assistant() -> Vec<u8> {
    field(
        20,
        &[
            field(1, b"Original"),
            field(3, b"PRIVATE REASONING"),
            field(8, b"Edited reply"),
            field(16, b"RAW REASONING"),
        ]
        .concat(),
    )
}

fn fixture(
    h: &AntigravityHarness,
    project: &Path,
    store: usize,
) -> (Connection, std::path::PathBuf) {
    let root = &h.data_roots()[store];
    fs::create_dir_all(root.join("conversations")).unwrap();
    let catalog = Connection::open(root.join("conversation_summaries.db")).unwrap();
    catalog.execute_batch("CREATE TABLE conversation_summaries(conversation_id TEXT,title TEXT,workspace_uris TEXT,last_modified_time TEXT)").unwrap();
    let uris = json!([reqwest::Url::from_directory_path(project).unwrap().as_str()]).to_string();
    catalog.execute("INSERT INTO conversation_summaries VALUES(?1,'Fixture title',?2,'2026-11-04T12:00:00Z')", rusqlite::params![ID,uris]).unwrap();
    let path = root.join("conversations").join(format!("{ID}.db"));
    let native = Connection::open(&path).unwrap();
    native.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE trajectory_meta(trajectory_id TEXT,cascade_id TEXT); CREATE TABLE steps(idx INTEGER,step_type INTEGER,step_format INTEGER,step_payload BLOB)").unwrap();
    native
        .execute(
            "INSERT INTO trajectory_meta VALUES('different-trajectory',?1)",
            [ID],
        )
        .unwrap();
    native
        .execute("INSERT INTO steps VALUES(0,14,0,?1)", [user()])
        .unwrap();
    native
        .execute("INSERT INTO steps VALUES(1,15,0,?1)", [assistant()])
        .unwrap();
    native
        .execute(
            "INSERT INTO steps VALUES(2,101,0,?1)",
            [b"INTERNAL TOOL STATE".as_slice()],
        )
        .unwrap();
    (native, path)
}

#[test]
fn catalog_load_snapshot_and_conversion_preserve_supported_content_without_native_writes() {
    let temp = crate::test_support::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project with spaces");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();
    let h = AntigravityHarness::new(home.clone());
    let (app_db, path) = fixture(&h, &project, 0);
    let (_cli_db, _) = fixture(&h, &project, 1);
    let source = SelectedHarness::Antigravity(h.clone());
    let app = temp.path().join("orbit");
    super::super::catalog::refresh(&source, &app, &project).unwrap();
    let cache = source.transcript_cache(&app);
    let sessions = ingest::list_sessions(&cache).unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions
        .iter()
        .all(|s| !s.indexed && Path::new(&s.project) == project));
    assert_ne!(sessions[0].session_id, sessions[1].session_id);
    let id = format!("antigravity:app:{ID}");
    let report = super::super::catalog::ensure_loaded(&source, &app, &id).unwrap();
    assert_eq!(report.messages_indexed, 2);
    assert_eq!(
        ingest::list_sessions(&cache)
            .unwrap()
            .iter()
            .find(|s| s.session_id == id)
            .unwrap()
            .title
            .as_deref(),
        Some("Fixture title")
    );
    let cache_db = ingest::open_database(&cache).unwrap();
    let texts = cache_db
        .prepare("SELECT text FROM messages WHERE session_id=?1 ORDER BY position")
        .unwrap()
        .query_map([&id], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(texts, vec!["Hello", "Edited reply"]);
    let export = source
        .transcript_export(&app, &id, &temp.path().join("exports"))
        .unwrap();
    let root = Path::new(&export.path);
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let turns = archive_turns(root, &manifest).unwrap();
    assert_eq!(turns,json!([{"role":"user","content":[{"type":"text","text":"Hello"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"aW1hZ2U="}}]},{"role":"assistant","content":[{"type":"text","text":"Edited reply"}]}]).as_array().unwrap().clone());
    assert!(!serde_json::to_string(&turns).unwrap().contains("REASONING"));
    let snapshot = readonly(&root.join("session.db")).unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT count(*) FROM steps", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    let target = SelectedHarness::Pi(PiHarness::new(home.join(".pi/agent"), home));
    let converted = super::super::conversion::convert(&source, &target, &app, &id, None).unwrap();
    assert_eq!(converted.messages, 2);
    assert_eq!(
        Path::new(&converted.working_directory)
            .canonicalize()
            .unwrap(),
        project.canonicalize().unwrap()
    );
    assert!(converted
        .notes
        .iter()
        .any(|note| note.contains("tool steps")));
    assert_eq!(
        app_db
            .query_row("SELECT count(*) FROM steps", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert!(validate(&h, &id, &path).is_ok());
    let override_project = temp.path().join("override");
    fs::create_dir(&override_project).unwrap();
    let overridden =
        super::super::conversion::convert(&source, &target, &app, &id, Some(&override_project))
            .unwrap();
    assert_eq!(
        Path::new(&overridden.working_directory)
            .canonicalize()
            .unwrap(),
        override_project.canonicalize().unwrap()
    );
    cache_db
        .execute("UPDATE sessions SET project='' WHERE session_id=?1", [&id])
        .unwrap();
    assert!(
        super::super::conversion::convert(&source, &target, &app, &id, None)
            .unwrap_err()
            .contains("Choose a destination directory")
    );
    assert_eq!(
        super::super::conversion::convert(&source, &target, &app, &id, Some(&override_project))
            .unwrap()
            .messages,
        2
    );
    fs::write(root.join("session.db"), b"changed").unwrap();
    assert_eq!(
        archive_turns(root, &manifest).unwrap_err(),
        "Antigravity backup checksum does not match"
    );
}

#[test]
fn listing_does_not_decode_messages_and_loading_rejects_unknown_formats_and_bad_protobuf() {
    let temp = crate::test_support::tempdir().unwrap();
    let h = AntigravityHarness::new(temp.path().to_path_buf());
    let (native, path) = fixture(&h, temp.path(), 0);
    native
        .execute("UPDATE steps SET step_format=7 WHERE idx=0", [])
        .unwrap();
    let source = SelectedHarness::Antigravity(h.clone());
    let app = temp.path().join("orbit");
    super::super::catalog::refresh(&source, &app, temp.path()).unwrap();
    assert_eq!(
        ingest::list_sessions(&source.transcript_cache(&app))
            .unwrap()
            .len(),
        1
    );
    let id = format!("antigravity:app:{ID}");
    assert_eq!(
        super::super::catalog::ensure_loaded(&source, &app, &id).unwrap_err(),
        "Unsupported Antigravity step format 7"
    );
    native
        .execute(
            "UPDATE steps SET step_format=0,step_payload=?1 WHERE idx=0",
            [vec![0x9a, 0x01, 0xff]],
        )
        .unwrap();
    assert!(super::super::catalog::ensure_loaded(&source, &app, &id)
        .unwrap_err()
        .contains("Truncated"));
    native
        .execute("UPDATE trajectory_meta SET cascade_id='wrong'", [])
        .unwrap();
    assert!(validate(&h, &id, &path)
        .err()
        .unwrap()
        .contains("identity changed"));
}

#[test]
#[ignore = "Reads installed Antigravity stores without modifying native files"]
fn live_readonly_catalog_and_decode() {
    let h = AntigravityHarness::from_env();
    let entries = metadata(&h).unwrap();
    assert!(!entries.is_empty());
    let mut messages = 0;
    for entry in &entries {
        messages += read_turns(&validate(&h, &entry.id, &entry.path).unwrap())
            .unwrap()
            .len();
    }
    assert!(messages > 0);
    println!(
        "Antigravity live check: {} catalog chats, {messages} readable messages",
        entries.len()
    );
}

#[test]
fn incoming_archives_reject_artifact_traversal_missing_files_and_identity_changes_before_native_start(
) {
    let temp = crate::test_support::tempdir().unwrap();
    let stage = temp.path().join("archive");
    fs::create_dir(&stage).unwrap();
    stage_conversion(
        &stage,
        "Fixture",
        temp.path().to_str().unwrap(),
        &[json!({"role":"user","content":[{"type":"text","text":"hello"}]})],
    )
    .unwrap();
    let manifest_path = stage.join("manifest.json");
    let original: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let h = AntigravityHarness::new(temp.path().join("unused home"));
    for (path, expected) in [
        ("../outside", "Unsafe"),
        ("C:/outside", "Unsafe"),
        ("brain/wrong/file", "Unsafe"),
    ] {
        let mut manifest = original.clone();
        manifest["files"] = json!({path:"checksum"});
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(import_archive(&h, &stage, temp.path())
            .unwrap_err()
            .contains(expected));
    }
    let mut manifest = original.clone();
    let id = original["sourceSessionId"]
        .as_str()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap();
    manifest["files"] = json!({format!("brain/{id}/missing.txt"):"checksum"});
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(import_archive(&h, &stage, temp.path())
        .unwrap_err()
        .contains("missing"));
    let mut manifest = original.clone();
    manifest["sourceSessionId"] = json!(format!("antigravity:cli:{}", uuid::Uuid::new_v4()));
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(import_archive(&h, &stage, temp.path())
        .unwrap_err()
        .contains("identity does not match"));
    let db = Connection::open(stage.join("session.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER unexpected AFTER UPDATE ON trajectory_meta BEGIN SELECT 1; END;",
    )
    .unwrap();
    drop(db);
    let mut manifest = original;
    manifest["sha256"] = json!(format!(
        "{:x}",
        Sha256::digest(fs::read(stage.join("session.db")).unwrap())
    ));
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(import_archive(&h, &stage, temp.path())
        .unwrap_err()
        .contains("Unsupported Antigravity database schema"));
    assert!(!h.home().exists());
}

#[test]
#[ignore = "Requires agy 1.5.0; writes only temporary native stores and starts no model turns"]
fn live_isolated_native_import_replay_delete_restore_and_incoming_conversion() {
    let temp = crate::test_support::tempdir().unwrap();
    let home = temp.path().join("isolated home");
    let project = temp.path().join("project with spaces café");
    let override_project = temp.path().join("other project");
    for path in [&home, &project, &override_project] {
        fs::create_dir(path).unwrap();
    }
    let h = AntigravityHarness::new(home.clone());
    let selected = SelectedHarness::Antigravity(h.clone());
    let app = temp.path().join("orbit");
    let image = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
    let turns = vec![
        json!({"role":"user","content":[{"type":"text","text":"Orbit fixture user café"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":image}}]}),
        json!({"role":"assistant","content":[{"type":"text","text":"Orbit fixture assistant"},{"type":"text","text":"second block"}]}),
    ];
    let mut evidence = Vec::new();
    for store in ["cli", "app"] {
        let stage = temp.path().join(format!("seed-{store}"));
        fs::create_dir(&stage).unwrap();
        stage_conversion(&stage, "Fixture title", project.to_str().unwrap(), &turns).unwrap();
        let manifest_path = stage.join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["sourceSessionId"] = json!(manifest["sourceSessionId"]
            .as_str()
            .unwrap()
            .replace(":cli:", &format!(":{store}:")));
        fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let imported = selected.transcript_import(&app, &stage, &project).unwrap();
        let native_id = imported.session_id.rsplit(':').next().unwrap();
        let mut native = native::Native::start(&h, store).unwrap();
        let loaded = native
            .call("GetCascadeTrajectory", json!({"cascadeId":native_id}))
            .unwrap();
        assert_eq!(
            loaded["trajectory"]["steps"][0]["userInput"]["media"][0]["inlineData"],
            image
        );
        assert_eq!(
            loaded["trajectory"]["steps"][0]["userInput"]["media"][0]["mimeType"],
            "image/png"
        );
        let updates = native.replay(native_id, &project).unwrap();
        drop(native);
        let text: Vec<_> = updates
            .iter()
            .filter_map(|u| u["content"]["text"].as_str())
            .collect();
        assert_eq!(
            text,
            [
                "Orbit fixture user café",
                "Orbit fixture assistant\nsecond block"
            ]
        );
        let before = fs::read(&imported.path).unwrap();
        assert!(selected
            .transcript_import(&app, &stage, &project)
            .unwrap_err()
            .contains("already exists"));
        assert_eq!(fs::read(&imported.path).unwrap(), before);
        let root = store_root(&h, store).unwrap();
        let artifact = root.join("brain").join(native_id).join("task.md");
        fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        fs::write(&artifact, "fixture artifact").unwrap();
        let deleted = super::super::deletion::delete(
            &selected,
            &app,
            &imported.session_id,
            &temp.path().join("backups"),
        )
        .unwrap();
        assert!(!Path::new(&imported.path).exists());
        assert!(!artifact.exists());
        assert!(!metadata(&h)
            .unwrap()
            .iter()
            .any(|m| m.id == imported.session_id));
        let list = native::Native::start(&h, store)
            .unwrap()
            .sessions()
            .unwrap();
        assert!(!list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["sessionId"] == native_id));
        let restored = selected
            .transcript_import(&app, Path::new(&deleted.path), &override_project)
            .unwrap();
        assert_eq!(restored.session_id, imported.session_id);
        assert_eq!(fs::read_to_string(&artifact).unwrap(), "fixture artifact");
        let updates = native::Native::start(&h, store)
            .unwrap()
            .replay(native_id, &override_project)
            .unwrap();
        assert!(updates
            .iter()
            .any(|u| u["content"]["text"] == "Orbit fixture assistant\nsecond block"));
        assert_eq!(
            Path::new(
                &metadata(&h)
                    .unwrap()
                    .into_iter()
                    .find(|m| m.id == restored.session_id)
                    .unwrap()
                    .project
            )
            .canonicalize()
            .unwrap(),
            override_project.canonicalize().unwrap()
        );
        let pi = SelectedHarness::Pi(PiHarness::new(home.join(".pi/agent"), home.clone()));
        let outgoing =
            super::super::conversion::convert(&selected, &pi, &app, &restored.session_id, None)
                .unwrap();
        let incoming = super::super::conversion::convert(
            &pi,
            &selected,
            &app,
            &outgoing.session_id,
            Some(&project),
        )
        .unwrap();
        assert!(metadata(&h)
            .unwrap()
            .into_iter()
            .find(|m| m.id == incoming.session_id)
            .unwrap()
            .title
            .unwrap()
            .ends_with("-1"));
        let id = incoming.session_id.rsplit(':').next().unwrap();
        let updates = native::Native::start(&h, "cli")
            .unwrap()
            .replay(id, &project)
            .unwrap();
        assert!(updates
            .iter()
            .any(|u| u["content"]["text"] == "Orbit fixture user café"));
        assert!(updates
            .iter()
            .any(|u| u["content"]["text"] == "Orbit fixture assistant\nsecond block"));
        evidence.push(json!({"store":store,"nativeMessages":text.len(),"inlineImage":true,"duplicateRejected":true,"deleteAndRestore":true,"artifactRestored":true,"cwdOverride":true,"piRoundTrip":true}));
    }
    println!("Antigravity 1.5.0 native acceptance: {}", json!(evidence));
}
