//! OpenCode V2's native JSONC configuration and project resources.

use std::path::{Path, PathBuf};

use super::{Harness, HarnessId, InventoryKind, ResourceKind, ResourceRef, Result, OPENCODE};

pub mod config;
pub mod hooks;
pub mod mcp;
pub mod memories;
pub mod skills;

#[derive(Debug, Clone)]
pub struct OpenCodeHarness {
    config_root: PathBuf,
    home: PathBuf,
}

impl OpenCodeHarness {
    pub fn new(config_root: PathBuf, home: PathBuf) -> Self {
        Self { config_root, home }
    }

    pub fn from_env() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let config_root = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|root| root.join("opencode"))
            .unwrap_or_else(|| home.join(".config").join("opencode"));
        Self::new(config_root, home)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn global_config_path(&self) -> PathBuf {
        config::config_path(&self.config_root)
    }
}

impl Harness for OpenCodeHarness {
    fn id(&self) -> HarnessId {
        OPENCODE
    }

    fn config_root(&self) -> &Path {
        &self.config_root
    }

    fn settings_path(&self) -> PathBuf {
        self.global_config_path()
    }

    fn user_home(&self) -> Option<&Path> {
        Some(&self.home)
    }

    fn list(&self, kind: InventoryKind) -> Result<Vec<ResourceRef>> {
        let result = match kind {
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
        result.map_err(|message| super::HarnessError::Mutation {
            path: self.config_root.clone(),
            message,
        })
    }
}
