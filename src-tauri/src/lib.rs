pub mod harness;
pub mod history;
pub mod ledger;
pub mod transcript;

use harness::Harness as _;
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
async fn mcp_list(app: tauri::AppHandle) -> Result<Vec<harness::claude::mcp::ServerView>, String> {
    run_blocking(move || {
        harness::claude::ClaudeCodeHarness::from_env()
            .mcp_servers(&app_data_dir(&app)?)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn mcp_save(app: tauri::AppHandle, name: String, entry: Value) -> Result<(), String> {
    run_blocking(move || {
        harness::claude::ClaudeCodeHarness::from_env()
            .mcp_save(&name, entry, &app_data_dir(&app)?)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn mcp_disable(app: tauri::AppHandle, name: String) -> Result<(), String> {
    run_blocking(move || {
        harness::claude::ClaudeCodeHarness::from_env()
            .mcp_disable(&name, &app_data_dir(&app)?)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn mcp_enable(app: tauri::AppHandle, name: String) -> Result<(), String> {
    run_blocking(move || {
        harness::claude::ClaudeCodeHarness::from_env()
            .mcp_enable(&name, &app_data_dir(&app)?)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn mcp_delete(app: tauri::AppHandle, name: String) -> Result<(), String> {
    run_blocking(move || {
        harness::claude::ClaudeCodeHarness::from_env()
            .mcp_delete(&name, &app_data_dir(&app)?)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn mcp_test(name: String) -> Result<harness::claude::mcp::BenchResult, String> {
    run_blocking(move || harness::claude::ClaudeCodeHarness::from_env().mcp_test(&name)).await
}

#[tauri::command]
async fn hooks_list(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
) -> Result<Vec<harness::claude::hooks::HookView>, String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project_root = project_roots.resolve(project_root)?;
        harness::claude::ClaudeCodeHarness::from_env()
            .hooks_list(&project_root, &app_data_dir(&app)?)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn hooks_save(
    project_roots: State<'_, ProjectRoots>,
    layer: harness::claude::config::SettingsLayer,
    event: String,
    matcher: Option<String>,
    command: String,
    edit_id: Option<String>,
    project_root: Option<String>,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project_root = project_roots.resolve(project_root)?;
        harness::claude::ClaudeCodeHarness::from_env()
            .hooks_save(
                &project_root,
                layer,
                &event,
                matcher,
                command,
                edit_id.as_deref(),
            )
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn hooks_disable(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
    layer: harness::claude::config::SettingsLayer,
    id: String,
    project_root: Option<String>,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project_root = project_roots.resolve(project_root)?;
        harness::claude::ClaudeCodeHarness::from_env()
            .hooks_disable(&project_root, &app_data_dir(&app)?, layer, &id)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn hooks_enable(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
    id: String,
    project_root: Option<String>,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project_root = project_roots.resolve(project_root)?;
        harness::claude::ClaudeCodeHarness::from_env()
            .hooks_enable(&project_root, &app_data_dir(&app)?, &id)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn hooks_delete(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
    layer: harness::claude::config::SettingsLayer,
    id: String,
    project_root: Option<String>,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project_root = project_roots.resolve(project_root)?;
        harness::claude::ClaudeCodeHarness::from_env()
            .hooks_delete(&project_root, &app_data_dir(&app)?, layer, &id)
            .map_err(|error| error.to_string())
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
async fn skills_inventory() -> Result<Vec<harness::claude::skills::SkillEntry>, String> {
    run_blocking(move || {
        harness::claude::skills::inventory(&harness::claude::ClaudeCodeHarness::from_env())
    })
    .await
}

#[tauri::command]
async fn memory_catalog(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
) -> Result<harness::claude::memories::MemoryCatalog, String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project = project_roots.resolve(project_root)?;
        harness::claude::memories::catalog(
            &harness::claude::ClaudeCodeHarness::from_env(),
            &project,
        )
    })
    .await
}

#[tauri::command]
async fn memory_edit(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    id: String,
    expected_content: String,
    content: String,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project = project_roots.resolve(project_root)?;
        harness::claude::memories::edit(
            &harness::claude::ClaudeCodeHarness::from_env(),
            &project,
            &id,
            &expected_content,
            &content,
        )
    })
    .await
}

#[tauri::command]
async fn memory_delete(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    id: String,
    expected_content: String,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project = project_roots.resolve(project_root)?;
        harness::claude::memories::delete(
            &harness::claude::ClaudeCodeHarness::from_env(),
            &project,
            &id,
            &expected_content,
        )
    })
    .await
}

#[tauri::command]
async fn history_load(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
) -> Result<history::HistorySnapshot, String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project = project_roots.resolve(project_root)?;
        history::load(&harness::claude::ClaudeCodeHarness::from_env(), &project)
    })
    .await
}

#[tauri::command]
async fn history_restore(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
    id: String,
    expected_current_checksum: Option<String>,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project = project_roots.resolve(project_root)?;
        history::restore(
            &harness::claude::ClaudeCodeHarness::from_env(),
            &project,
            &id,
            expected_current_checksum.as_deref(),
        )
    })
    .await
}

#[tauri::command]
async fn history_delete(id: String) -> Result<(), String> {
    run_blocking(move || history::delete(&harness::claude::ClaudeCodeHarness::from_env(), &id))
        .await
}

#[tauri::command]
async fn history_cleanup_legacy() -> Result<history::CleanupReport, String> {
    run_blocking(move || history::cleanup_legacy(&harness::claude::ClaudeCodeHarness::from_env()))
        .await
}

#[tauri::command]
async fn skill_set_override(
    id: String,
    state: harness::claude::SkillOverride,
) -> Result<(), String> {
    run_blocking(move || {
        harness::claude::skills::set_override(
            &harness::claude::ClaudeCodeHarness::from_env(),
            &id,
            state,
        )
    })
    .await
}

#[tauri::command]
async fn skill_edit(id: String, expected_content: String, content: String) -> Result<(), String> {
    run_blocking(move || {
        harness::claude::skills::edit_skill(
            &harness::claude::ClaudeCodeHarness::from_env(),
            &id,
            &expected_content,
            &content,
        )
    })
    .await
}

#[tauri::command]
async fn skill_install(source: String) -> Result<String, String> {
    run_blocking(move || {
        harness::claude::skills::install(
            &harness::claude::ClaudeCodeHarness::from_env(),
            std::path::Path::new(&source),
        )
    })
    .await
}

#[tauri::command]
async fn skill_uninstall(id: String) -> Result<(), String> {
    run_blocking(move || {
        harness::claude::skills::uninstall(&harness::claude::ClaudeCodeHarness::from_env(), &id)
    })
    .await
}

#[tauri::command]
async fn skill_restore(id: String, backup: String) -> Result<(), String> {
    run_blocking(move || {
        let config_root = harness::claude::ClaudeCodeHarness::from_env();
        let root = ledger::backup_root(config_root.config_root());
        let backup_path = std::path::Path::new(&backup);
        if !backup_path.starts_with(&root) {
            return Err("backup path is outside the app backup root".into());
        }
        let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
        let canonical_backup = backup_path.canonicalize().map_err(|e| e.to_string())?;
        if !canonical_backup.starts_with(canonical_root) {
            return Err("backup path is outside the app backup root".into());
        }
        harness::claude::skills::restore(&config_root, &id, backup_path)
    })
    .await
}

#[tauri::command]
async fn skills_ledger() -> Result<Vec<ledger::LedgerRow>, String> {
    run_blocking(move || {
        ledger::read_applied(harness::claude::ClaudeCodeHarness::from_env().config_root())
    })
    .await
}

#[tauri::command]
async fn transcript_ingest(
    app: tauri::AppHandle,
) -> Result<transcript::ingest::IngestReport, String> {
    run_blocking(move || {
        let config_root = harness::claude::ClaudeCodeHarness::from_env()
            .config_root()
            .to_path_buf();
        transcript::ingest::ingest_projects(&config_root, &app_data_dir(&app)?)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_list_sessions(
    app: tauri::AppHandle,
) -> Result<Vec<transcript::SessionSummary>, String> {
    run_blocking(move || {
        transcript::ingest::list_sessions(&app_data_dir(&app)?).map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_thread_page(
    app: tauri::AppHandle,
    session_id: String,
    offset: usize,
    limit: usize,
) -> Result<Vec<transcript::TranscriptMessage>, String> {
    run_blocking(move || {
        transcript::spine::thread_page(&app_data_dir(&app)?, &session_id, offset, limit)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_message_page_offset(
    app: tauri::AppHandle,
    session_id: String,
    uuid: String,
    page_size: usize,
) -> Result<Option<usize>, String> {
    run_blocking(move || {
        transcript::spine::message_page_offset(&app_data_dir(&app)?, &session_id, &uuid, page_size)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_search(
    app: tauri::AppHandle,
    query: String,
    limit: usize,
) -> Result<Vec<transcript::SearchHit>, String> {
    run_blocking(move || {
        transcript::spine::search(&app_data_dir(&app)?, &query, limit)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_export_markdown(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<String, String> {
    run_blocking(move || {
        transcript::spine::export_markdown(&app_data_dir(&app)?, &session_id)
            .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn transcript_export_directory(
    app: tauri::AppHandle,
    session_id: String,
    destination: String,
) -> Result<transcript::archive::ArchiveReport, String> {
    run_blocking(move || {
        let harness = harness::claude::ClaudeCodeHarness::from_env();
        transcript::archive::export_archive(
            &app_data_dir(&app)?,
            &harness.config_root().join("projects"),
            &session_id,
            Path::new(&destination),
        )
    })
    .await
}

#[tauri::command]
async fn transcript_import_directory(
    app: tauri::AppHandle,
    project_roots: State<'_, ProjectRoots>,
    project_root: String,
    source: String,
) -> Result<transcript::archive::ArchiveReport, String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project = project_roots.resolve(Some(project_root))?;
        let harness = harness::claude::ClaudeCodeHarness::from_env();
        let report = transcript::archive::import_archive(
            &harness.config_root().join("projects"),
            Path::new(&source),
            &project,
        )?;
        transcript::ingest::ingest_projects(harness.config_root(), &app_data_dir(&app)?)
            .map_err(|error| format!("Chat imported, but indexing failed: {error}"))?;
        Ok(report)
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

#[tauri::command]
async fn config_load(
    project_roots: State<'_, ProjectRoots>,
    project_root: Option<String>,
) -> Result<harness::claude::config::ConfigSnapshot, String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project_root = project_roots.resolve(project_root)?;
        harness::claude::config::load_config(
            &harness::claude::ClaudeCodeHarness::from_env(),
            &project_root,
        )
        .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn config_apply(
    project_roots: State<'_, ProjectRoots>,
    layer: harness::claude::config::SettingsLayer,
    key: String,
    value: serde_json::Value,
    project_root: Option<String>,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project_root = project_roots.resolve(project_root)?;
        harness::claude::config::apply_value(
            &harness::claude::ClaudeCodeHarness::from_env(),
            &project_root,
            layer,
            &key,
            value,
        )
        .map_err(|error| error.to_string())
    })
    .await
}

#[tauri::command]
async fn config_undo(
    project_roots: State<'_, ProjectRoots>,
    layer: harness::claude::config::SettingsLayer,
    key: String,
    project_root: Option<String>,
) -> Result<(), String> {
    let project_roots = project_roots.inner().clone();
    run_blocking(move || {
        let project_root = project_roots.resolve(project_root)?;
        let harness = harness::claude::ClaudeCodeHarness::from_env();
        let path = layer.settings_path(harness.config_root(), &project_root);
        ledger::undo(
            harness.config_root(),
            harness::ResourceKind::Settings,
            &key,
            &format!("config-{}", layer.label()),
            &path,
        )
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
            transcript_thread_page,
            transcript_message_page_offset,
            transcript_search,
            transcript_export_markdown,
            transcript_export_directory,
            transcript_import_directory
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
        let directory = tempfile::tempdir().unwrap();
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
        let directory = tempfile::tempdir().unwrap();
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
