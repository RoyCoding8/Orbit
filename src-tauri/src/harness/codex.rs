//! Codex's native configuration and resources. Paths follow CODEX_HOME.

use std::path::{Path, PathBuf};

use super::{Harness, HarnessId, InventoryKind, ResourceKind, ResourceRef, Result, CODEX};

pub(crate) mod app_server;
pub mod config;
pub mod hooks;
pub mod mcp;
pub mod memories;
pub mod skills;
pub(crate) mod store;

#[derive(Debug, Clone)]
pub struct CodexHarness {
    config_root: PathBuf,
    home: PathBuf,
    sqlite_home: Option<PathBuf>,
}

impl CodexHarness {
    pub fn new(config_root: PathBuf, home: PathBuf) -> Self {
        Self {
            config_root,
            home,
            sqlite_home: None,
        }
    }

    pub fn from_env() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let config_root = std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let mut harness = Self::new(config_root, home);
        harness.sqlite_home = std::env::var_os("CODEX_SQLITE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        harness
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
}

impl Harness for CodexHarness {
    fn id(&self) -> HarnessId {
        CODEX
    }

    fn config_root(&self) -> &Path {
        &self.config_root
    }

    fn settings_path(&self) -> PathBuf {
        self.config_root.join("config.toml")
    }

    fn user_home(&self) -> Option<&Path> {
        Some(&self.home)
    }

    fn list(&self, kind: InventoryKind) -> Result<Vec<ResourceRef>> {
        let resources = match kind {
            InventoryKind::Skill => {
                let project =
                    std::env::current_dir().map_err(|source| super::HarnessError::Read {
                        path: PathBuf::from("."),
                        source,
                    })?;
                skills::inventory(self, &project).map(|items| {
                    items
                        .into_iter()
                        .map(|item| ResourceRef {
                            kind: ResourceKind::Skill,
                            id: item.id,
                            path: item.path,
                        })
                        .collect()
                })
            }
            InventoryKind::McpServer => mcp::list(self).map(|items| {
                items
                    .into_iter()
                    .map(|item| ResourceRef {
                        kind: ResourceKind::McpServer,
                        id: item.name,
                        path: self.settings_path(),
                    })
                    .collect()
            }),
        };
        resources.map_err(|message| super::HarnessError::Mutation {
            path: self.config_root.clone(),
            message,
        })
    }
}
