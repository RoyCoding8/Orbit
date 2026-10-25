pub mod harness;
pub mod history;
pub mod ledger;
pub mod transcript;

#[cfg(test)]
mod test_support;

use harness::{selected::SelectedHarness, Client, Harness as _};
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{Manager, State};

#[derive(Clone, Default)]
struct ProjectRoots(Arc<Mutex<HashSet<PathBuf>>>);

impl ProjectRoots {
    fn grant(&self, path: &Path) -> Result<PathBuf, String> {
        let root = canonical_project_directory(path)?;
        self.0
            .lock()
            .map_err(|_| "project authorization state is unavailable".to_string())?
            .insert(root.clone());
        Ok(root)
    }

    fn resolve(&self, requested: Option<String>) -> Result<PathBuf, String> {
        let path = match requested {
            Some(path) => PathBuf::from(path),
            None => std::env::current_dir().map_err(|error| error.to_string())?,
        };
        let canonical = canonical_project_directory(&path)?;
        let roots = self
            .0
            .lock()
            .map_err(|_| "project authorization state is unavailable".to_string())?;
        if roots.contains(&canonical) {
            Ok(canonical)
        } else {
            Err(format!(
                "project directory is not authorized for this session: {}",
                canonical.display()
            ))
        }
    }
}

fn canonical_project_directory(path: &Path) -> Result<PathBuf, String> {
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "could not resolve project directory {}: {error}",
            path.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "project directory does not exist: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn app_data_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|_| "Could not resolve app data directory".to_string())
}

async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn mcp_list(
    app: tauri::AppHandle,
    client: Option<Client>,
) -> Result<Vec<harness::claude::mcp::ServerView>, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.mcp_list(&app_data_dir(&app)?)
    })
    .await
}

#[tauri::command]
async fn mcp_save(
    app: tauri::AppHandle,
    name: String,
    entry: Value,
    client: Option<Client>,
) -> Result<(), String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.mcp_save(&name, entry, &app_data_dir(&app)?)
    })
    .await
}

#[tauri::command]
async fn mcp_disable(
    app: tauri::AppHandle,
    name: String,
    client: Option<Client>,
) -> Result<(), String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.mcp_set_enabled(&name, false, &app_data_dir(&app)?)
    })
    .await
}

#[tauri::command]
async fn mcp_enable(
    app: tauri::AppHandle,
    name: String,
    client: Option<Client>,
) -> Result<(), String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.mcp_set_enabled(&name, true, &app_data_dir(&app)?)
    })
    .await
}

#[tauri::command]
async fn mcp_delete(
    app: tauri::AppHandle,
    name: String,
    client: Option<Client>,
) -> Result<(), String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.mcp_delete(&name, &app_data_dir(&app)?)
    })
    .await
}

#[tauri::command]
async fn mcp_test(
    name: String,
    client: Option<Client>,
) -> Result<harness::claude::mcp::BenchResult, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.mcp_test(&name)
    })
    .await
}

#[tauri::command]
async fn hooks_list(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<Vec<harness::claude::hooks::HookView>, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.hooks_list(&project, &app_data_dir(&app)?)
    })
    .await
}

#[tauri::command]
async fn hooks_sources(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<Vec<harness::kiro::hooks::HookSource>, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let project = roots.resolve(project_root)?;
        SelectedHarness::from_env(client).hooks_sources(&project)
    })
    .await
}

#[tauri::command]
async fn hooks_save(
    draft: harness::HookDraft,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.hooks_save(&project, draft)
    })
    .await
}

#[tauri::command]
async fn hooks_disable(
    app: tauri::AppHandle,
    layer: harness::claude::config::SettingsLayer,
    id: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.hooks_set_enabled(&project, &app_data_dir(&app)?, layer, &id, false)
    })
    .await
}

#[tauri::command]
async fn hooks_enable(
    app: tauri::AppHandle,
    id: String,
    layer: Option<harness::claude::config::SettingsLayer>,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.hooks_set_enabled(
            &project,
            &app_data_dir(&app)?,
            layer.unwrap_or(harness::claude::config::SettingsLayer::User),
            &id,
            true,
        )
    })
    .await
}

#[tauri::command]
async fn hooks_delete(
    app: tauri::AppHandle,
    layer: harness::claude::config::SettingsLayer,
    id: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.hooks_delete(&project, &app_data_dir(&app)?, layer, &id)
    })
    .await
}

#[tauri::command]
async fn hooks_test(
    command: String,
    input: Value,
) -> Result<harness::claude::hooks::HookTestResult, String> {
    run_blocking(move || {
        harness::claude::hooks::test_command(&command, &input, std::time::Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn skills_inventory(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<Vec<harness::claude::skills::SkillEntry>, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.skills_inventory(&project)
    })
    .await
}

#[tauri::command]
async fn memory_catalog(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<harness::claude::memories::MemoryCatalog, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.memory_catalog(&project)
    })
    .await
}

#[tauri::command]
async fn memory_edit(
    id: String,
    expected_content: String,
    content: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.memory_edit(&project, &id, &expected_content, &content)
    })
    .await
}

#[tauri::command]
async fn memory_delete(
    id: String,
    expected_content: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.memory_delete(&project, &id, &expected_content)
    })
    .await
}

#[tauri::command]
async fn history_load(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<history::HistorySnapshot, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.history_load(&project)
    })
    .await
}

#[tauri::command]
async fn history_restore(
    id: String,
    expected_current_checksum: Option<String>,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        history::restore(
            &harness,
            &project,
            &id,
            expected_current_checksum.as_deref(),
        )
    })
    .await
}

#[tauri::command]
async fn history_delete(id: String, client: Option<Client>) -> Result<(), String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        history::delete(&harness, &id)
    })
    .await
}

#[tauri::command]
async fn history_cleanup_legacy(client: Option<Client>) -> Result<history::CleanupReport, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        history::cleanup_legacy(&harness)
    })
    .await
}

#[tauri::command]
async fn skill_set_override(
    id: String,
    state: harness::claude::SkillOverride,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.skill_set_override(&project, &id, state)
    })
    .await
}

#[tauri::command]
async fn skill_edit(
    id: String,
    expected_content: String,
    content: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.skill_edit(&project, &id, &expected_content, &content)
    })
    .await
}

#[tauri::command]
async fn skill_install(source: String, client: Option<Client>) -> Result<String, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.skill_install(Path::new(&source))
    })
    .await
}

#[tauri::command]
async fn skill_uninstall(
    id: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.skill_uninstall(&project, &id)
    })
    .await
}

#[tauri::command]
async fn skill_restore(
    id: String,
    backup: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.skill_restore(&project, &id, Path::new(&backup))
    })
    .await
}

#[tauri::command]
async fn skills_ledger(client: Option<Client>) -> Result<Vec<ledger::LedgerRow>, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let rows = ledger::read_applied(harness.config_root())?;
        Ok(ledger::rows_with_saved_payloads(rows))
    })
    .await
}

#[tauri::command]
async fn transcript_ingest(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<transcript::ingest::IngestReport, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.transcript_ingest(&app_data_dir(&app)?, &roots.resolve(project_root)?)
    })
    .await
}

#[tauri::command]
async fn transcript_list_sessions(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    refresh: Option<bool>,
    client: Option<Client>,
) -> Result<SessionCatalog, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let app_dir = app_data_dir(&app)?;
        let mut warnings = Vec::new();
        if refresh.unwrap_or(false) {
            let report = harness
                .transcript_ingest(&app_dir, &roots.resolve(project_root)?)
                .map_err(|error| {
                    format!(
                        "Could not read chats from {}: {error}",
                        harness.config_root().display()
                    )
                })?;
            if report.skipped_files > 0 {
                warnings.push(format!("{} unreadable chat files", report.skipped_files));
            }
            if report.malformed_lines > 0 {
                warnings.push(format!("{} malformed records", report.malformed_lines));
            }
        }
        Ok(SessionCatalog {
            warnings,
            sessions: transcript::ingest::list_sessions(&harness.transcript_cache(&app_dir))
                .map_err(|e| e.to_string())?,
            source: match &harness {
                SelectedHarness::Opencode(_) => "OpenCode v2 native session service".into(),
                _ => harness.config_root().to_string_lossy().into_owned(),
            },
        })
    })
    .await
}

#[derive(serde::Serialize)]
struct SessionCatalog {
    sessions: Vec<transcript::SessionSummary>,
    source: String,
    warnings: Vec<String>,
}

#[tauri::command]
async fn transcript_delete(
    app: tauri::AppHandle,
    session_id: String,
    backup_directory: String,
    client: Option<Client>,
) -> Result<transcript::deletion::DeleteReport, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let app_dir = app_data_dir(&app)?;
        let mut report = transcript::deletion::delete(
            &harness, &app_dir, &session_id, Path::new(&backup_directory),
        )?;
        if let Err(error) = transcript::ingest::remove_sessions(
            &harness.transcript_cache(&app_dir), &report.deleted_sessions,
        ) {
            report.warning = Some(format!("The chat was deleted, but its search index could not refresh: {error}. Refresh chats to rebuild it."));
        }
        Ok(report)
    }).await
}

#[tauri::command]
async fn transcript_thread_page(
    app: tauri::AppHandle,
    session_id: String,
    offset: usize,
    limit: usize,
    client: Option<Client>,
) -> Result<Vec<transcript::TranscriptMessage>, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let cache = harness.transcript_cache(&app_data_dir(&app)?);
        transcript::spine::thread_page(&cache, &session_id, offset, limit)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_message_page_offset(
    app: tauri::AppHandle,
    session_id: String,
    uuid: String,
    page_size: usize,
    client: Option<Client>,
) -> Result<Option<usize>, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let cache = harness.transcript_cache(&app_data_dir(&app)?);
        transcript::spine::message_page_offset(&cache, &session_id, &uuid, page_size)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_search(
    app: tauri::AppHandle,
    query: String,
    limit: usize,
    client: Option<Client>,
) -> Result<Vec<transcript::SearchHit>, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let cache = harness.transcript_cache(&app_data_dir(&app)?);
        transcript::spine::search(&cache, &query, limit).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_export_markdown(
    app: tauri::AppHandle,
    session_id: String,
    client: Option<Client>,
) -> Result<String, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let cache = harness.transcript_cache(&app_data_dir(&app)?);
        transcript::spine::export_markdown(&cache, &session_id).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_export_directory(
    app: tauri::AppHandle,
    session_id: String,
    destination: String,
    client: Option<Client>,
) -> Result<transcript::archive::ArchiveReport, String> {
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        harness.transcript_export(&app_data_dir(&app)?, &session_id, Path::new(&destination))
    })
    .await
}

#[tauri::command]
async fn transcript_import_directory(
    app: tauri::AppHandle,
    source: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<transcript::archive::ArchiveReport, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.transcript_import(&app_data_dir(&app)?, Path::new(&source), &project)
    })
    .await
}

#[tauri::command]
async fn transcript_conversion_preview(
    app: tauri::AppHandle,
    session_id: String,
    client: Option<Client>,
) -> Result<transcript::conversion::Preview, String> {
    run_blocking(move || {
        transcript::conversion::preview(
            &SelectedHarness::from_env(client),
            &app_data_dir(&app)?,
            &session_id,
        )
    })
    .await
}

#[tauri::command]
async fn transcript_convert(
    app: tauri::AppHandle,
    session_id: String,
    target_client: Client,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<transcript::conversion::ConversionReport, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let project = project_root
            .map(|path| roots.resolve(Some(path)))
            .transpose()?;
        transcript::conversion::convert(
            &SelectedHarness::from_env(client),
            &SelectedHarness::from_env(Some(target_client)),
            &app_data_dir(&app)?,
            &session_id,
            project.as_deref(),
        )
    })
    .await
}

#[tauri::command]
async fn config_load(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<harness::claude::config::ConfigSnapshot, String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.config_load(&project)
    })
    .await
}

#[tauri::command]
async fn config_apply(
    layer: harness::claude::config::SettingsLayer,
    key: String,
    value: Value,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.config_apply(&project, layer, &key, value)
    })
    .await
}

#[tauri::command]
async fn config_undo(
    layer: harness::claude::config::SettingsLayer,
    key: String,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    client: Option<Client>,
) -> Result<(), String> {
    let roots = project_roots.inner().clone();
    run_blocking(move || {
        let harness = SelectedHarness::from_env(client);
        let project = roots.resolve(project_root)?;
        harness.config_undo(&project, layer, &key)
    })
    .await
}

#[tauri::command]
async fn project_choose(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let selected =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .map_err(|error| error.to_string())?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected = selected.into_path().map_err(|error| error.to_string())?;
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let canonical = project_roots.grant(&selected)?;
        Ok(Some(canonical.to_string_lossy().into_owned()))
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let launch_dir = std::env::current_dir()
        .and_then(|path| path.canonicalize())
        .expect("could not resolve application launch directory");
    let project_roots = ProjectRoots(Mutex::new(HashSet::from([launch_dir])).into());
    tauri::Builder::default()
        .manage(project_roots)
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            skills_inventory,
            memory_catalog,
            memory_edit,
            memory_delete,
            history_load,
            history_restore,
            history_delete,
            history_cleanup_legacy,
            skill_set_override,
            skill_edit,
            skill_install,
            skill_uninstall,
            skill_restore,
            skills_ledger,
            project_choose,
            config_load,
            config_apply,
            config_undo,
            hooks_list,
            hooks_sources,
            hooks_save,
            hooks_disable,
            hooks_enable,
            hooks_delete,
            hooks_test,
            mcp_list,
            mcp_save,
            mcp_disable,
            mcp_enable,
            mcp_delete,
            mcp_test,
            transcript_ingest,
            transcript_list_sessions,
            transcript_delete,
            transcript_thread_page,
            transcript_message_page_offset,
            transcript_search,
            transcript_export_markdown,
            transcript_export_directory,
            transcript_import_directory,
            transcript_conversion_preview,
            transcript_convert
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod project_root_tests {
    use super::ProjectRoots;
    use std::{fs, path::PathBuf};

    #[test]
    fn launch_directory_and_native_picker_grants_are_the_only_allowed_roots() {
        let directory = crate::test_support::tempdir().unwrap();
        let launch = directory.path().join("launch");
        let selected = directory.path().join("selected");
        let unknown = directory.path().join("unknown");
        for path in [&launch, &selected, &unknown] {
            fs::create_dir_all(path).unwrap();
        }
        let roots = ProjectRoots(
            std::sync::Mutex::new(std::collections::HashSet::from([launch
                .canonicalize()
                .unwrap()]))
            .into(),
        );

        assert_eq!(
            roots
                .resolve(Some(launch.to_string_lossy().into_owned()))
                .unwrap(),
            launch.canonicalize().unwrap()
        );
        assert!(roots
            .resolve(Some(unknown.to_string_lossy().into_owned()))
            .is_err());
        assert_eq!(
            roots.grant(&selected).unwrap(),
            selected.canonicalize().unwrap()
        );
        assert_eq!(
            roots
                .resolve(Some(selected.to_string_lossy().into_owned()))
                .unwrap(),
            selected.canonicalize().unwrap()
        );
    }

    #[test]
    fn authorization_compares_canonical_directory_paths() {
        let directory = crate::test_support::tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let alias = project.join("..").join("project");
        let canonical = project.canonicalize().unwrap();
        let roots = ProjectRoots(
            std::sync::Mutex::new(std::collections::HashSet::from([canonical.clone()])).into(),
        );

        assert_eq!(
            roots
                .resolve(Some(alias.to_string_lossy().into_owned()))
                .unwrap(),
            canonical
        );
        assert!(roots
            .resolve(Some(
                PathBuf::from("missing-project")
                    .to_string_lossy()
                    .into_owned()
            ))
            .is_err());
    }
}
