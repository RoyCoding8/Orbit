use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    claude::{self, config::SettingsLayer, ClaudeCodeHarness, SkillOverride},
    codex::{self, CodexHarness},
    kiro::{self, KiroHarness},
    opencode::{self, OpenCodeHarness},
    pi::{self, PiHarness},
    Client, Harness, HarnessId, HookDraft, InventoryKind, ResourceRef,
};
use crate::{history, ledger, transcript};

pub enum SelectedHarness {
    Claude(ClaudeCodeHarness),
    Codex(CodexHarness),
    Kiro(KiroHarness),
    Opencode(OpenCodeHarness),
    Pi(PiHarness),
}

impl SelectedHarness {
    pub fn from_env(client: Option<Client>) -> Self {
        match client.unwrap_or_default() {
            Client::ClaudeCode => Self::Claude(ClaudeCodeHarness::from_env()),
            Client::Codex => Self::Codex(CodexHarness::from_env()),
            Client::Kiro => Self::Kiro(KiroHarness::from_env()),
            Client::Pi => Self::Pi(PiHarness::from_env()),
            Client::Opencode => Self::Opencode(OpenCodeHarness::from_env()),
        }
    }

    pub fn mcp_list(&self, app: &Path) -> Result<Vec<claude::mcp::ServerView>, String> {
        match self {
            Self::Claude(h) => h.mcp_servers(app).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::mcp::list(h),
            Self::Kiro(h) => kiro::mcp::list(h),
            Self::Pi(h) => pi::mcp::list(h),
            Self::Opencode(h) => opencode::mcp::list(h),
        }
    }

    pub fn mcp_save(&self, name: &str, entry: Value, app: &Path) -> Result<(), String> {
        match self {
            Self::Claude(h) => h.mcp_save(name, entry, app).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::mcp::save(h, name, entry),
            Self::Kiro(h) => kiro::mcp::save(h, name, entry),
            Self::Pi(h) => pi::mcp::save(h, name, entry),
            Self::Opencode(h) => opencode::mcp::save(h, name, entry),
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
            Self::Kiro(h) => kiro::mcp::set_enabled(h, name, enabled),
            Self::Pi(h) => pi::mcp::set_enabled(h, name, enabled),
            Self::Opencode(h) => opencode::mcp::set_enabled(h, name, enabled),
        }
    }

    pub fn mcp_delete(&self, name: &str, app: &Path) -> Result<(), String> {
        match self {
            Self::Claude(h) => h.mcp_delete(name, app).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::mcp::delete(h, name),
            Self::Kiro(h) => kiro::mcp::delete(h, name),
            Self::Pi(h) => pi::mcp::delete(h, name),
            Self::Opencode(h) => opencode::mcp::delete(h, name),
        }
    }

    pub fn mcp_test(&self, name: &str) -> Result<claude::mcp::BenchResult, String> {
        match self {
            Self::Claude(h) => h.mcp_test(name),
            Self::Codex(h) => codex::mcp::test(h, name),
            Self::Kiro(h) => kiro::mcp::test(h, name),
            Self::Pi(h) => pi::mcp::test(h, name),
            Self::Opencode(h) => opencode::mcp::test(h, name),
        }
    }

    pub fn config_load(&self, project: &Path) -> Result<claude::config::ConfigSnapshot, String> {
        match self {
            Self::Claude(h) => claude::config::load_config(h, project).map_err(|e| e.to_string()),
            Self::Codex(h) => codex::config::load_config(h, project),
            Self::Kiro(h) => kiro::config::load_config(h, project),
            Self::Pi(h) => pi::config::load_config(h, project),
            Self::Opencode(h) => opencode::config::load_config(h, project),
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
            Self::Kiro(h) => kiro::config::apply_value(h, project, layer, key, value),
            Self::Pi(h) => pi::config::apply_value(h, project, layer, key, value),
            Self::Opencode(h) => opencode::config::apply_value(h, project, layer, key, value),
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
            Self::Kiro(h) => kiro::config::undo(h, project, layer, key),
            Self::Pi(h) => pi::config::undo(h, project, layer, key),
            Self::Opencode(h) => opencode::config::undo(h, project, layer, key),
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
            Self::Kiro(h) => kiro::hooks::list(h, project),
            Self::Opencode(_) => Err(opencode::hooks::UNSUPPORTED.into()),
            Self::Pi(_) => Err(pi::HOOKS_UNSUPPORTED.into()),
        }
    }

    pub fn hooks_sources(&self, project: &Path) -> Result<Vec<kiro::hooks::HookSource>, String> {
        match self {
            Self::Kiro(h) => kiro::hooks::sources(h, project),
            _ => Ok(Vec::new()),
        }
    }

    pub fn hooks_save(&self, project: &Path, draft: HookDraft) -> Result<(), String> {
        match self {
            Self::Claude(h) => h
                .hooks_save(
                    project,
                    draft.layer,
                    &draft.event,
                    draft.matcher,
                    draft.command,
                    draft.edit_id.as_deref(),
                )
                .map_err(|e| e.to_string()),
            Self::Codex(h) => codex::hooks::save(
                h,
                project,
                draft.layer,
                &draft.event,
                draft.matcher,
                draft.command,
                draft.edit_id.as_deref(),
            ),
            Self::Kiro(h) => kiro::hooks::save(h, project, draft),
            Self::Opencode(_) => Err(opencode::hooks::UNSUPPORTED.into()),
            Self::Pi(_) => Err(pi::HOOKS_UNSUPPORTED.into()),
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
            Self::Kiro(h) => kiro::hooks::set_enabled(h, project, layer, id, enabled),
            Self::Opencode(_) => Err(opencode::hooks::UNSUPPORTED.into()),
            Self::Pi(_) => Err(pi::HOOKS_UNSUPPORTED.into()),
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
            Self::Kiro(h) => kiro::hooks::delete(h, project, layer, id),
            Self::Opencode(_) => Err(opencode::hooks::UNSUPPORTED.into()),
            Self::Pi(_) => Err(pi::HOOKS_UNSUPPORTED.into()),
        }
    }

    pub fn skills_inventory(
        &self,
        project: &Path,
    ) -> Result<Vec<claude::skills::SkillEntry>, String> {
        match self {
            Self::Claude(h) => claude::skills::inventory(h),
            Self::Codex(h) => codex::skills::inventory(h, project),
            Self::Kiro(h) => kiro::skills::inventory(h, project),
            Self::Pi(h) => pi::skills::inventory(h, project),
            Self::Opencode(h) => opencode::skills::inventory(h, project),
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
            Self::Kiro(h) => kiro::skills::set_override(h, project, id, state),
            Self::Pi(h) => pi::skills::set_override(h, project, id, state),
            Self::Opencode(h) => opencode::skills::set_override(h, project, id, state),
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
            Self::Kiro(h) => kiro::skills::edit_skill(h, project, id, expected, content),
            Self::Pi(h) => pi::skills::edit_skill(h, project, id, expected, content),
            Self::Opencode(h) => opencode::skills::edit_skill(h, project, id, expected, content),
        }
    }

    pub fn skill_install(&self, source: &Path) -> Result<String, String> {
        match self {
            Self::Claude(h) => claude::skills::install(h, source),
            Self::Codex(h) => codex::skills::install(h, source),
            Self::Kiro(h) => kiro::skills::install(h, source),
            Self::Pi(h) => pi::skills::install(h, source),
            Self::Opencode(h) => opencode::skills::install(h, source),
        }
    }

    pub fn skill_uninstall(&self, project: &Path, id: &str) -> Result<(), String> {
        match self {
            Self::Claude(h) => claude::skills::uninstall(h, id),
            Self::Codex(h) => codex::skills::uninstall(h, project, id),
            Self::Kiro(h) => kiro::skills::uninstall(h, project, id),
            Self::Pi(h) => pi::skills::uninstall(h, project, id),
            Self::Opencode(h) => opencode::skills::uninstall(h, project, id),
        }
    }

    pub fn skill_restore(&self, project: &Path, id: &str, backup: &Path) -> Result<(), String> {
        match self {
            Self::Claude(h) => {
                claude::skills::ensure_backup_path(h.config_root(), backup)?;
                claude::skills::restore(h, id, backup)
            }
            Self::Codex(h) => codex::skills::restore(h, project, id, backup),
            Self::Kiro(h) => kiro::skills::restore(h, project, id, backup),
            Self::Pi(h) => pi::skills::restore(h, project, id, backup),
            Self::Opencode(h) => opencode::skills::restore(h, project, id, backup),
        }
    }

    pub fn memory_catalog(
        &self,
        project: &Path,
    ) -> Result<claude::memories::MemoryCatalog, String> {
        match self {
            Self::Claude(h) => claude::memories::catalog(h, project),
            Self::Codex(h) => codex::memories::catalog(h, project),
            Self::Kiro(h) => kiro::memories::catalog(h, project),
            Self::Pi(h) => pi::memories::catalog(h, project),
            Self::Opencode(h) => opencode::memories::catalog(h, project),
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
            Self::Kiro(h) => kiro::memories::edit(h, project, id, expected, content),
            Self::Pi(h) => pi::memories::edit(h, project, id, expected, content),
            Self::Opencode(h) => opencode::memories::edit(h, project, id, expected, content),
        }
    }

    pub fn memory_delete(&self, project: &Path, id: &str, expected: &str) -> Result<(), String> {
        match self {
            Self::Claude(h) => claude::memories::delete(h, project, id, expected),
            Self::Codex(h) => codex::memories::delete(h, project, id, expected),
            Self::Kiro(h) => kiro::memories::delete(h, project, id, expected),
            Self::Pi(h) => pi::memories::delete(h, project, id, expected),
            Self::Opencode(h) => opencode::memories::delete(h, project, id, expected),
        }
    }

    /// Each client owns its cache. Native session IDs and shared query code stay unchanged.
    pub fn transcript_cache(&self, app: &Path) -> PathBuf {
        match self {
            Self::Claude(_) => app.to_path_buf(),
            Self::Codex(_) => app.join("codex"),
            Self::Kiro(_) => app.join("kiro"),
            Self::Pi(_) => app.join("pi"),
            Self::Opencode(_) => app.join("opencode"),
        }
    }

    pub fn transcript_ingest(
        &self,
        app: &Path,
        project: &Path,
    ) -> Result<transcript::ingest::IngestReport, String> {
        match self {
            Self::Claude(h) => {
                transcript::ingest::ingest_projects(h.config_root(), app).map_err(|e| e.to_string())
            }
            Self::Codex(h) => {
                transcript::codex::ingest(h.config_root(), &self.transcript_cache(app))
                    .map_err(|e| e.to_string())
            }
            Self::Pi(h) => {
                transcript::pi::ingest(h.config_root(), &self.transcript_cache(app), project)
                    .map_err(|e| e.to_string())
            }
            Self::Kiro(h) => transcript::kiro::ingest(h.config_root(), &self.transcript_cache(app))
                .map_err(|e| e.to_string()),
            Self::Opencode(_) => transcript::opencode::ingest(&self.transcript_cache(app), project)
                .map_err(|e| e.to_string()),
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
            Self::Pi(h) => transcript::pi::export_archive(
                h.config_root(),
                &self.transcript_cache(app),
                session,
                destination,
            ),
            Self::Kiro(h) => transcript::kiro::export_archive(
                h.config_root(),
                &self.transcript_cache(app),
                session,
                destination,
            ),
            Self::Opencode(h) => transcript::opencode::export_archive(
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
            Self::Kiro(h) => transcript::kiro::import_archive(h.config_root(), source, project),
            Self::Pi(h) => transcript::pi::import_archive(h.config_root(), source, project),
            Self::Opencode(h) => {
                transcript::opencode::import_archive(h.config_root(), source, project)
            }
        }?;
        transcript::catalog::refresh(self, app, project)
            .map_err(|error| format!("Chat imported, but discovery failed: {error}"))?;
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
            Self::Kiro(h) => h.id(),
            Self::Pi(h) => h.id(),
            Self::Opencode(h) => h.id(),
        }
    }
    fn config_root(&self) -> &Path {
        match self {
            Self::Claude(h) => h.config_root(),
            Self::Codex(h) => h.config_root(),
            Self::Kiro(h) => h.config_root(),
            Self::Pi(h) => h.config_root(),
            Self::Opencode(h) => h.config_root(),
        }
    }
    fn settings_path(&self) -> PathBuf {
        match self {
            Self::Claude(h) => h.settings_path(),
            Self::Codex(h) => h.settings_path(),
            Self::Kiro(h) => h.settings_path(),
            Self::Pi(h) => h.settings_path(),
            Self::Opencode(h) => h.settings_path(),
        }
    }
    fn mcp_path(&self) -> PathBuf {
        match self {
            Self::Claude(h) => h.mcp_path(),
            Self::Codex(h) => h.mcp_path(),
            Self::Kiro(h) => h.mcp_path(),
            Self::Pi(h) => h.mcp_path(),
            Self::Opencode(h) => h.mcp_path(),
        }
    }
    fn user_home(&self) -> Option<&Path> {
        match self {
            Self::Claude(h) => h.user_home(),
            Self::Codex(h) => h.user_home(),
            Self::Kiro(h) => h.user_home(),
            Self::Pi(h) => h.user_home(),
            Self::Opencode(h) => h.user_home(),
        }
    }
    fn list(&self, kind: InventoryKind) -> super::Result<Vec<ResourceRef>> {
        match self {
            Self::Claude(h) => h.list(kind),
            Self::Codex(h) => h.list(kind),
            Self::Kiro(h) => h.list(kind),
            Self::Pi(h) => h.list(kind),
            Self::Opencode(h) => h.list(kind),
        }
    }
}
