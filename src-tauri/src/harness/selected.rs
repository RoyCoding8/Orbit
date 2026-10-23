use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    claude::{self, config::SettingsLayer, ClaudeCodeHarness, SkillOverride},
    codex::{self, Client, CodexHarness},
    Harness, HarnessId, InventoryKind, ResourceRef,
};
use crate::{history, ledger, transcript};

pub enum SelectedHarness {
    Claude(ClaudeCodeHarness),
    Codex(CodexHarness),
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookDraft {
    pub layer: SettingsLayer,
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    pub edit_id: Option<String>,
}

impl SelectedHarness {
    pub fn from_env(client: Option<Client>) -> Self {
        match client.unwrap_or_default() {
            Client::ClaudeCode => Self::Claude(ClaudeCodeHarness::from_env()),
            Client::Codex => Self::Codex(CodexHarness::from_env()),
        }
    }

    pub fn mcp_list(&self, app: &Path) -> Result<Vec<claude::mcp::ServerView>, String> {
        match self {
            Self::Claude(h) => h.mcp_servers(app).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::mcp::list(h),
        }
    }

    pub fn mcp_save(&self, name: &str, entry: Value, app: &Path) -> Result<(), String> {
        match self {
            Self::Claude(h) => h.mcp_save(name, entry, app).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::mcp::save(h, name, entry),
        }
    }

    pub fn mcp_set_enabled(&self, name: &str, enabled: bool, app: &Path) -> Result<(), String> {
        match self {
            Self::Claude(h) => if enabled {
                h.mcp_enable(name, app)
            } else {
                h.mcp_disable(name, app)
            }
            .map_err(|e| e.to_string()),
            Self::Codex(h) => codex::mcp::set_enabled(h, name, enabled),
        }
    }

    pub fn mcp_delete(&self, name: &str, app: &Path) -> Result<(), String> {
        match self {
            Self::Claude(h) => h.mcp_delete(name, app).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::mcp::delete(h, name),
        }
    }

    pub fn mcp_test(&self, name: &str) -> Result<claude::mcp::BenchResult, String> {
        match self {
            Self::Claude(h) => h.mcp_test(name),
            Self::Codex(h) => codex::mcp::test(h, name),
        }
    }

    pub fn config_load(&self, project: &Path) -> Result<claude::config::ConfigSnapshot, String> {
        match self {
            Self::Claude(h) => claude::config::load_config(h, project).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::config::load_config(h, project),
        }
    }

    pub fn config_apply(
        &self,
        project: &Path,
        layer: SettingsLayer,
        key: &str,
        value: Value,
    ) -> Result<(), String> {
        match self {
            Self::Claude(h) => claude::config::apply_value(h, project, layer, key, value)
                .map_err(|e| e.to_string()),
            Self::Codex(h) => codex::config::apply_value(h, project, layer, key, value),
        }
    }

    pub fn config_undo(
        &self,
        project: &Path,
        layer: SettingsLayer,
        key: &str,
    ) -> Result<(), String> {
        match self {
            Self::Claude(h) => ledger::undo(
                h.config_root(),
                super::ResourceKind::Settings,
                key,
                &format!("config-{}", layer.label()),
                &layer.settings_path(h.config_root(), project),
            ),
            Self::Codex(h) => codex::config::undo(h, project, layer, key),
        }
    }

    pub fn hooks_list(
        &self,
        project: &Path,
        app: &Path,
    ) -> Result<Vec<claude::hooks::HookView>, String> {
        match self {
            Self::Claude(h) => h.hooks_list(project, app).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::hooks::list(h, project),
        }
    }

    pub fn hooks_save(&self, project: &Path, draft: HookDraft) -> Result<(), String> {
        let HookDraft {
            layer,
            event,
            matcher,
            command,
            edit_id,
        } = draft;
        match self {
            Self::Claude(h) => h
                .hooks_save(project, layer, &event, matcher, command, edit_id.as_deref())
                .map_err(|e| e.to_string()),
            Self::Codex(h) => codex::hooks::save(
                h,
                project,
                layer,
                &event,
                matcher,
                command,
                edit_id.as_deref(),
            ),
        }
    }

    pub fn hooks_set_enabled(
        &self,
        project: &Path,
        app: &Path,
        layer: SettingsLayer,
        id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        match self {
            Self::Claude(h) => if enabled {
                h.hooks_enable(project, app, id)
            } else {
                h.hooks_disable(project, app, layer, id)
            }
            .map_err(|e| e.to_string()),
            Self::Codex(h) => codex::hooks::set_enabled(h, project, layer, id, enabled),
        }
    }

    pub fn hooks_delete(
        &self,
        project: &Path,
        app: &Path,
        layer: SettingsLayer,
        id: &str,
    ) -> Result<(), String> {
        match self {
            Self::Claude(h) => h
                .hooks_delete(project, app, layer, id)
                .map_err(|e| e.to_string()),
            Self::Codex(h) => codex::hooks::delete(h, project, layer, id),
        }
    }

    pub fn skills_inventory(
        &self,
        project: &Path,
    ) -> Result<Vec<claude::skills::SkillEntry>, String> {
        match self {
            Self::Claude(h) => claude::skills::inventory(h),
            Self::Codex(h) => codex::skills::inventory(h, project),
        }
    }

    pub fn skill_set_override(
        &self,
        project: &Path,
        id: &str,
        state: SkillOverride,
    ) -> Result<(), String> {
        match self {
            Self::Claude(h) => claude::skills::set_override(h, id, state),
            Self::Codex(h) => codex::skills::set_override(h, project, id, state),
        }
    }

    pub fn skill_edit(
        &self,
        project: &Path,
        id: &str,
        expected: &str,
        content: &str,
    ) -> Result<(), String> {
        match self {
            Self::Claude(h) => claude::skills::edit_skill(h, id, expected, content),
            Self::Codex(h) => codex::skills::edit_skill(h, project, id, expected, content),
        }
    }

    pub fn skill_install(&self, source: &Path) -> Result<String, String> {
        match self {
            Self::Claude(h) => claude::skills::install(h, source),
            Self::Codex(h) => codex::skills::install(h, source),
        }
    }

    pub fn skill_uninstall(&self, project: &Path, id: &str) -> Result<(), String> {
        match self {
            Self::Claude(h) => claude::skills::uninstall(h, id),
            Self::Codex(h) => codex::skills::uninstall(h, project, id),
        }
    }

    pub fn skill_restore(&self, project: &Path, id: &str, backup: &Path) -> Result<(), String> {
        claude::skills::ensure_backup_path(self.config_root(), backup)?;
        match self {
            Self::Claude(h) => claude::skills::restore(h, id, backup),
            Self::Codex(h) => codex::skills::restore(h, project, id, backup),
        }
    }

    pub fn memory_catalog(
        &self,
        project: &Path,
    ) -> Result<claude::memories::MemoryCatalog, String> {
        match self {
            Self::Claude(h) => claude::memories::catalog(h, project),
            Self::Codex(h) => codex::memories::catalog(h, project),
        }
    }

    pub fn memory_edit(
        &self,
        project: &Path,
        id: &str,
        expected: &str,
        content: &str,
    ) -> Result<(), String> {
        match self {
            Self::Claude(h) => claude::memories::edit(h, project, id, expected, content),
            Self::Codex(h) => codex::memories::edit(h, project, id, expected, content),
        }
    }

    pub fn memory_delete(&self, project: &Path, id: &str, expected: &str) -> Result<(), String> {
        match self {
            Self::Claude(h) => claude::memories::delete(h, project, id, expected),
            Self::Codex(h) => codex::memories::delete(h, project, id, expected),
        }
    }

    /// Each client owns its cache. Native session IDs and shared query code stay unchanged.
    pub fn transcript_cache(&self, app: &Path) -> PathBuf {
        match self {
            Self::Claude(_) => app.to_path_buf(),
            Self::Codex(_) => app.join("codex"),
        }
    }

    pub fn transcript_ingest(
        &self,
        app: &Path,
    ) -> Result<transcript::ingest::IngestReport, String> {
        match self {
            Self::Claude(h) => {
                transcript::ingest::ingest_projects(h.config_root(), app).map_err(|e| e.to_string())
            }
            Self::Codex(h) => {
                transcript::codex::ingest(h.config_root(), &self.transcript_cache(app))
                    .map_err(|e| e.to_string())
            }
        }
    }

    pub fn transcript_export(
        &self,
        app: &Path,
        session: &str,
        destination: &Path,
    ) -> Result<transcript::archive::ArchiveReport, String> {
        match self {
            Self::Claude(h) => transcript::archive::export_archive(
                app,
                &h.config_root().join("projects"),
                session,
                destination,
            ),
            Self::Codex(h) => transcript::codex::export_archive(
                h.config_root(),
                &self.transcript_cache(app),
                session,
                destination,
            ),
        }
    }

    pub fn transcript_import(
        &self,
        app: &Path,
        source: &Path,
        project: &Path,
    ) -> Result<transcript::archive::ArchiveReport, String> {
        let report = match self {
            Self::Claude(h) => transcript::archive::import_archive(
                &h.config_root().join("projects"),
                source,
                project,
            ),
            Self::Codex(h) => transcript::codex::import_archive(h.config_root(), source, project),
        }?;
        self.transcript_ingest(app)
            .map_err(|error| format!("Chat imported, but indexing failed: {error}"))?;
        Ok(report)
    }

    pub fn history_load(&self, project: &Path) -> Result<history::HistorySnapshot, String> {
        history::load(self, project)
    }
}

impl Harness for SelectedHarness {
    fn id(&self) -> HarnessId {
        match self {
            Self::Claude(h) => h.id(),
            Self::Codex(h) => h.id(),
        }
    }
    fn config_root(&self) -> &Path {
        match self {
            Self::Claude(h) => h.config_root(),
            Self::Codex(h) => h.config_root(),
        }
    }
    fn settings_path(&self) -> PathBuf {
        match self {
            Self::Claude(h) => h.settings_path(),
            Self::Codex(h) => h.settings_path(),
        }
    }
    fn mcp_path(&self) -> PathBuf {
        match self {
            Self::Claude(h) => h.mcp_path(),
            Self::Codex(h) => h.mcp_path(),
        }
    }
    fn user_home(&self) -> Option<&Path> {
        match self {
            Self::Claude(h) => h.user_home(),
            Self::Codex(h) => h.user_home(),
        }
    }
    fn list(&self, kind: InventoryKind) -> super::Result<Vec<ResourceRef>> {
        match self {
            Self::Claude(h) => h.list(kind),
            Self::Codex(h) => h.list(kind),
        }
    }
}
