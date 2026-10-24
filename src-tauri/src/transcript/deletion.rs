use super::ingest::open_database;
use crate::harness::{selected::SelectedHarness, Harness};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    /// Directory containing the native archive used to restore this session.
    pub path: String,
    pub deleted_sessions: Vec<String>,
    /// Set by the caller if its local search cache could not be purged.
    pub warning: Option<String>,
}

pub fn delete(
    harness: &SelectedHarness,
    app: &Path,
    session_id: &str,
    backup_directory: &Path,
) -> Result<DeleteReport, String> {
    if matches!(harness, SelectedHarness::Kiro(_)) {
        return Err("Kiro deletion is unavailable because Orbit cannot yet restore native Kiro V2 session archives.".into());
    }
    let cache = harness.transcript_cache(app);
    let (source, project) = indexed_source(&cache, session_id)?;
    let native_id = session_id.strip_prefix("opencode:").unwrap_or(session_id);
    if matches!(harness, SelectedHarness::Codex(_)) {
        uuid::Uuid::parse_str(session_id).map_err(|_| "Codex session ID is invalid.".to_owned())?;
    }
    if matches!(harness, SelectedHarness::Opencode(_))
        && super::opencode::session_has_children(
            native_id,
            Path::new(&project),
            &Default::default(),
        )?
    {
        return Err("This OpenCode chat has child sessions. Native deletion would also remove them, so Orbit refused to delete it.".into());
    }
    let archive = harness.transcript_export(app, session_id, backup_directory)?;
    let removal = (|| -> Result<(), String> {
        match harness {
            SelectedHarness::Claude(h) => super::archive::delete_exported_source(
                app,
                &h.config_root().join("projects"),
                session_id,
                Path::new(&archive.path),
            )
            .map(|_| ()),
            SelectedHarness::Codex(h) => {
                super::archive::verify_codex_backup(
                    h.config_root(),
                    &cache,
                    session_id,
                    Path::new(&archive.path),
                )?;
                crate::harness::codex::app_server::delete_session(h, session_id)
            }
            SelectedHarness::Pi(_) => {
                if indexed_source(&cache, session_id)? != (source.clone(), project.clone()) {
                    return Err("Pi session source changed after its backup was created.".into());
                }
                super::pi::delete_exported_source(
                    Path::new(&source),
                    session_id,
                    Path::new(&archive.path),
                )
            }
            SelectedHarness::Opencode(_) => super::opencode::delete_native_session(
                native_id,
                Path::new(&project),
                &Default::default(),
            ),
            SelectedHarness::Kiro(_) => unreachable!("Kiro rejected before archival"),
        }
    })();
    removal.map_err(|error| format!("{error} Native archive saved at {}.", archive.path))?;
    Ok(DeleteReport {
        path: archive.path,
        deleted_sessions: vec![session_id.to_owned()],
        warning: None,
    })
}

fn indexed_source(cache: &Path, session_id: &str) -> Result<(String, String), String> {
    let connection = open_database(cache).map_err(|error| error.to_string())?;
    connection
        .query_row(
            "SELECT source_path, project FROM sessions WHERE session_id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| format!("Could not find the indexed session: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{claude::ClaudeCodeHarness, resolve_paths};
    use std::fs;

    #[test]
    fn claude_delete_preserves_other_chats_restores_family_and_rejects_changed_sources() {
        for changed in [false, true] {
            let temp = crate::test_support::tempdir().unwrap();
            let app = temp.path().join("app");
            let config = temp.path().join("claude");
            let projects = config.join("projects");
            let bucket = projects.join("project-key");
            let destination = temp.path().join("destination");
            let backups = temp.path().join("backups");
            for path in [&bucket, &destination, &backups] {
                fs::create_dir_all(path).unwrap();
            }
            let id = "project-key/11111111-2222-4333-8444-555555555555.jsonl";
            let source = bucket.join("11111111-2222-4333-8444-555555555555.jsonl");
            let bytes = serde_json::json!({"uuid":"m1", "type":"user", "cwd":destination, "message":{"content":"hello"}}).to_string();
            fs::write(&source, &bytes).unwrap();
            let family = source.with_extension("");
            fs::create_dir_all(family.join("tool-results")).unwrap();
            fs::write(family.join("tool-results/result.txt"), b"preserved result").unwrap();
            let other = bucket.join("22222222-2222-4333-8444-555555555555.jsonl");
            fs::write(&other, b"unrelated").unwrap();
            open_database(&app)
                .unwrap()
                .execute(
                    "INSERT INTO sessions(session_id, project, source_path) VALUES (?1, ?2, ?3)",
                    rusqlite::params![id, destination.to_string_lossy(), source.to_string_lossy()],
                )
                .unwrap();
            let harness = SelectedHarness::Claude(ClaudeCodeHarness::new(resolve_paths(
                Some(config.to_str().unwrap()),
                temp.path(),
            )));
            if changed {
                let archive = harness.transcript_export(&app, id, &backups).unwrap();
                fs::write(&source, format!("{bytes}\n")).unwrap();
                assert!(super::super::archive::delete_exported_source(
                    &app,
                    &projects,
                    id,
                    Path::new(&archive.path)
                )
                .unwrap_err()
                .contains("changed after the backup"));
                assert_eq!(fs::read_to_string(&source).unwrap(), format!("{bytes}\n"));
                assert!(family.is_dir());
            } else {
                let archive = delete(&harness, &app, id, &backups).unwrap();
                assert_eq!(archive.deleted_sessions, [id]);
                assert!(!source.exists());
                assert!(!family.exists());
                let restored = harness
                    .transcript_import(&app, Path::new(&archive.path), &destination)
                    .unwrap();
                let record: serde_json::Value =
                    serde_json::from_str(fs::read_to_string(&restored.path).unwrap().trim())
                        .unwrap();
                assert_eq!(record["message"]["content"], "hello");
                assert_eq!(
                    fs::read(
                        Path::new(&restored.path)
                            .with_extension("")
                            .join("tool-results/result.txt")
                    )
                    .unwrap(),
                    b"preserved result"
                );
            }
            assert_eq!(fs::read(&other).unwrap(), b"unrelated");
        }
    }
}
