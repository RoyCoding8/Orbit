use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    antigravity::{self, AntigravityHarness},
    claude::{self, config::SettingsLayer, ClaudeCodeHarness, SkillOverride},
    codex::{self, CodexHarness},
    cursor::{self, CursorHarness},
    junie::{self, JunieHarness},
    kiro::{self, KiroHarness},
    muse::{self, MuseHarness},
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
    Antigravity(AntigravityHarness),
    Muse(MuseHarness),
    Junie(JunieHarness),
    Cursor(CursorHarness),
}

macro_rules! dispatch_harness {
    ($self:expr, $h:ident => $body:expr) => {
        match $self {
            Self::Claude($h) => $body,
            Self::Codex($h) => $body,
            Self::Kiro($h) => $body,
            Self::Pi($h) => $body,
            Self::Antigravity($h) => $body,
            Self::Opencode($h) => $body,
            Self::Muse($h) => $body,
            Self::Junie($h) => $body,
            Self::Cursor($h) => $body,
        }
    };
    (
        $self:expr,
        claude($ch:ident) => $claude:expr,
        other($module:ident, $oh:ident) => $other:expr $(,)?
    ) => {
        match $self {
            Self::Claude($ch) => $claude,
            Self::Codex($oh) => {
                use codex as $module;
                $other
            }
            Self::Kiro($oh) => {
                use kiro as $module;
                $other
            }
            Self::Pi($oh) => {
                use pi as $module;
                $other
            }
            Self::Antigravity($oh) => {
                use antigravity as $module;
                $other
            }
            Self::Opencode($oh) => {
                use opencode as $module;
                $other
            }
            Self::Muse($oh) => {
                use muse as $module;
                $other
            }
            Self::Junie($oh) => {
                use junie as $module;
                $other
            }
            Self::Cursor($oh) => {
                use cursor as $module;
                $other
            }
        }
    };
    (
        $self:expr,
        module($module:ident, $h:ident) => $body:expr $(,)?
    ) => {
        match $self {
            Self::Claude($h) => {
                use claude as $module;
                $body
            }
            Self::Codex($h) => {
                use codex as $module;
                $body
            }
            Self::Kiro($h) => {
                use kiro as $module;
                $body
            }
            Self::Pi($h) => {
                use pi as $module;
                $body
            }
            Self::Antigravity($h) => {
                use antigravity as $module;
                $body
            }
            Self::Opencode($h) => {
                use opencode as $module;
                $body
            }
            Self::Muse($h) => {
                use muse as $module;
                $body
            }
            Self::Junie($h) => {
                use junie as $module;
                $body
            }
            Self::Cursor($h) => {
                use cursor as $module;
                $body
            }
        }
    };
    (
        hooks $self:expr,
        claude($ch:ident) => $claude:expr,
        supported($module:ident, $oh:ident) => $supported:expr $(,)?
    ) => {
        match $self {
            Self::Claude($ch) => $claude,
            Self::Codex($oh) => {
                use codex as $module;
                $supported
            }
            Self::Kiro($oh) => {
                use kiro as $module;
                $supported
            }
            Self::Antigravity($oh) => {
                use antigravity as $module;
                $supported
            }
            Self::Muse($oh) => {
                use muse as $module;
                $supported
            }
            Self::Junie($oh) => {
                use junie as $module;
                $supported
            }
            Self::Cursor($oh) => {
                use cursor as $module;
                $supported
            }
            Self::Opencode(_) => Err(opencode::hooks::UNSUPPORTED.into()),
            Self::Pi(_) => Err(pi::HOOKS_UNSUPPORTED.into()),
        }
    };
}

impl SelectedHarness {
    pub fn from_env(client: Option<Client>) -> Self {
        match client.unwrap_or_default() {
            Client::ClaudeCode => Self::Claude(ClaudeCodeHarness::from_env()),
            Client::Codex => Self::Codex(CodexHarness::from_env()),
            Client::Kiro => Self::Kiro(KiroHarness::from_env()),
            Client::Pi => Self::Pi(PiHarness::from_env()),
            Client::Antigravity => Self::Antigravity(AntigravityHarness::from_env()),
            Client::Opencode => Self::Opencode(OpenCodeHarness::from_env()),
            Client::Muse => Self::Muse(MuseHarness::from_env()),
            Client::Junie => Self::Junie(JunieHarness::from_env()),
            Client::Cursor => Self::Cursor(CursorHarness::from_env()),
        }
    }

    pub fn mcp_list(&self, app: &Path) -> Result<Vec<claude::mcp::ServerView>, String> {
        dispatch_harness!(
            self,
            claude(h) => h.mcp_servers(app).map_err(|e| e.to_string()),
            other(m, h) => m::mcp::list(h),
        )
    }

    pub fn mcp_save(&self, name: &str, entry: Value, app: &Path) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => h.mcp_save(name, entry, app).map_err(|e| e.to_string()),
            other(m, h) => m::mcp::save(h, name, entry),
        )
    }

    pub fn mcp_set_enabled(&self, name: &str, enabled: bool, app: &Path) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => if enabled {
                h.mcp_enable(name, app)
            } else {
                h.mcp_disable(name, app)
            }
            .map_err(|e| e.to_string()),
            other(m, h) => m::mcp::set_enabled(h, name, enabled),
        )
    }

    pub fn mcp_delete(&self, name: &str, app: &Path) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => h.mcp_delete(name, app).map_err(|e| e.to_string()),
            other(m, h) => m::mcp::delete(h, name),
        )
    }

    pub fn mcp_test(&self, name: &str) -> Result<claude::mcp::BenchResult, String> {
        dispatch_harness!(
            self,
            claude(h) => h.mcp_test(name),
            other(m, h) => m::mcp::test(h, name),
        )
    }

    pub fn config_load(&self, project: &Path) -> Result<claude::config::ConfigSnapshot, String> {
        dispatch_harness!(
            self,
            claude(h) => claude::config::load_config(h, project).map_err(|e| e.to_string()),
            other(m, h) => m::config::load_config(h, project),
        )
    }

    pub fn config_apply(
        &self,
        project: &Path,
        layer: SettingsLayer,
        key: &str,
        value: Value,
    ) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => claude::config::apply_value(h, project, layer, key, value)
                .map_err(|e| e.to_string()),
            other(m, h) => m::config::apply_value(h, project, layer, key, value),
        )
    }

    pub fn config_undo(
        &self,
        project: &Path,
        layer: SettingsLayer,
        key: &str,
    ) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => ledger::undo(
                h.config_root(),
                super::ResourceKind::Settings,
                key,
                &format!("config-{}", layer.label()),
                &layer.settings_path(h.config_root(), project),
            ),
            other(m, h) => m::config::undo(h, project, layer, key),
        )
    }

    pub fn hooks_list(
        &self,
        project: &Path,
        app: &Path,
    ) -> Result<Vec<claude::hooks::HookView>, String> {
        dispatch_harness!(
            hooks self,
            claude(h) => h.hooks_list(project, app).map_err(|e| e.to_string()),
            supported(m, h) => m::hooks::list(h, project),
        )
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
            Self::Antigravity(h) => antigravity::hooks::save(h, project, draft),
            Self::Muse(h) => muse::hooks::save(h, project, draft),
            Self::Junie(h) => junie::hooks::save(h, project, draft),
            Self::Cursor(h) => cursor::hooks::save(h, project, draft),
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
        dispatch_harness!(
            hooks self,
            claude(h) => if enabled {
                h.hooks_enable(project, app, id)
            } else {
                h.hooks_disable(project, app, layer, id)
            }
            .map_err(|e| e.to_string()),
            supported(m, h) => m::hooks::set_enabled(h, project, layer, id, enabled),
        )
    }

    pub fn hooks_delete(
        &self,
        project: &Path,
        app: &Path,
        layer: SettingsLayer,
        id: &str,
    ) -> Result<(), String> {
        dispatch_harness!(
            hooks self,
            claude(h) => h
                .hooks_delete(project, app, layer, id)
                .map_err(|e| e.to_string()),
            supported(m, h) => m::hooks::delete(h, project, layer, id),
        )
    }

    pub fn skills_inventory(
        &self,
        project: &Path,
    ) -> Result<Vec<claude::skills::SkillEntry>, String> {
        dispatch_harness!(
            self,
            claude(h) => claude::skills::inventory(h),
            other(m, h) => m::skills::inventory(h, project),
        )
    }

    pub fn skill_set_override(
        &self,
        project: &Path,
        id: &str,
        state: SkillOverride,
    ) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => claude::skills::set_override(h, id, state),
            other(m, h) => m::skills::set_override(h, project, id, state),
        )
    }

    pub fn skill_edit(
        &self,
        project: &Path,
        id: &str,
        expected: &str,
        content: &str,
    ) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => claude::skills::edit_skill(h, id, expected, content),
            other(m, h) => m::skills::edit_skill(h, project, id, expected, content),
        )
    }

    pub fn skill_install(&self, source: &Path) -> Result<String, String> {
        dispatch_harness!(self, module(m, h) => m::skills::install(h, source))
    }

    pub fn skill_uninstall(&self, project: &Path, id: &str) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => claude::skills::uninstall(h, id),
            other(m, h) => m::skills::uninstall(h, project, id),
        )
    }

    pub fn skill_restore(&self, project: &Path, id: &str, backup: &Path) -> Result<(), String> {
        dispatch_harness!(
            self,
            claude(h) => {
                claude::skills::ensure_backup_path(h.config_root(), backup)?;
                claude::skills::restore(h, id, backup)
            },
            other(m, h) => m::skills::restore(h, project, id, backup),
        )
    }

    pub fn memory_catalog(
        &self,
        project: &Path,
    ) -> Result<claude::memories::MemoryCatalog, String> {
        dispatch_harness!(self, module(m, h) => m::memories::catalog(h, project))
    }

    pub fn memory_edit(
        &self,
        project: &Path,
        id: &str,
        expected: &str,
        content: &str,
    ) -> Result<(), String> {
        dispatch_harness!(self, module(m, h) => m::memories::edit(h, project, id, expected, content))
    }

    pub fn memory_delete(&self, project: &Path, id: &str, expected: &str) -> Result<(), String> {
        dispatch_harness!(self, module(m, h) => m::memories::delete(h, project, id, expected))
    }

    /// Each client owns its cache. Native session IDs and shared query code stay unchanged.
    pub fn transcript_cache(&self, app: &Path) -> PathBuf {
        match self {
            Self::Claude(_) => app.to_path_buf(),
            Self::Codex(_) => app.join("codex"),
            Self::Kiro(_) => app.join("kiro"),
            Self::Pi(_) => app.join("pi"),
            Self::Antigravity(_) => app.join("antigravity"),
            Self::Opencode(_) => app.join("opencode"),
            Self::Muse(_) => app.join("muse"),
            Self::Junie(_) => app.join("junie"),
            Self::Cursor(_) => app.join("cursor"),
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
            Self::Antigravity(h) => transcript::antigravity::export_archive(
                h,
                &self.transcript_cache(app),
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
            Self::Muse(_) | Self::Junie(_) | Self::Cursor(_) => {
                Err("Local transcripts are not supported for this agent yet.".into())
            }
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
            Self::Antigravity(h) => transcript::antigravity::import_archive(h, source, project),
            Self::Pi(h) => transcript::pi::import_archive(h.config_root(), source, project),
            Self::Opencode(h) => {
                transcript::opencode::import_archive(h.config_root(), source, project)
            }
            Self::Muse(_) | Self::Junie(_) | Self::Cursor(_) => {
                Err("Local transcripts are not supported for this agent yet.".into())
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
        dispatch_harness!(self, h => h.id())
    }
    fn config_root(&self) -> &Path {
        dispatch_harness!(self, h => h.config_root())
    }
    fn settings_path(&self) -> PathBuf {
        dispatch_harness!(self, h => h.settings_path())
    }
    fn mcp_path(&self) -> PathBuf {
        dispatch_harness!(self, h => h.mcp_path())
    }
    fn user_home(&self) -> Option<&Path> {
        dispatch_harness!(self, h => h.user_home())
    }
    fn list(&self, kind: InventoryKind) -> super::Result<Vec<ResourceRef>> {
        dispatch_harness!(self, h => h.list(kind))
    }
}
