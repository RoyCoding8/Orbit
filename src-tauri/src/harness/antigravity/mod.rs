use std::path::{Path, PathBuf};

use super::{
    Harness, HarnessError, HarnessId, InventoryKind, ResourceKind, ResourceRef, Result, ANTIGRAVITY,
};

pub mod config;
mod discovery;
pub mod hooks;
pub mod memories;
pub mod skills;
pub(crate) use super::json_mcp as mcp;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct AntigravityHarness {
    home: PathBuf,
    config: PathBuf,
}

impl AntigravityHarness {
    pub fn new(home: PathBuf) -> Self {
        let home = home.canonicalize().unwrap_or(home);
        Self {
            config: home.join(".gemini/config"),
            home,
        }
    }

    pub fn from_env() -> Self {
        Self::new(
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .unwrap_or_default(),
        )
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn data_roots(&self) -> [PathBuf; 2] {
        [
            self.home.join(".gemini/antigravity"),
            self.home.join(".gemini/antigravity-cli"),
        ]
    }

    pub fn project_root(&self, project: &Path) -> PathBuf {
        project.join(".agents")
    }
}

impl Harness for AntigravityHarness {
    fn id(&self) -> HarnessId {
        ANTIGRAVITY
    }
    fn config_root(&self) -> &Path {
        &self.config
    }
    fn settings_path(&self) -> PathBuf {
        self.home.join(".gemini/antigravity-cli/settings.json")
    }
    fn mcp_path(&self) -> PathBuf {
        self.config.join("mcp_config.json")
    }
    fn user_home(&self) -> Option<&Path> {
        Some(&self.home)
    }
    fn list(&self, kind: InventoryKind) -> Result<Vec<ResourceRef>> {
        let resources = match kind {
            InventoryKind::Skill => skills::inventory(self, &self.home).map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| ResourceRef {
                        kind: ResourceKind::Skill,
                        id: entry.id,
                        path: entry.path,
                    })
                    .collect()
            }),
            InventoryKind::McpServer => super::json_mcp::list(self).map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| ResourceRef {
                        kind: ResourceKind::McpServer,
                        id: entry.name,
                        path: self.mcp_path(),
                    })
                    .collect()
            }),
        };
        resources.map_err(|message| HarnessError::Mutation {
            path: self.config.clone(),
            message,
        })
    }
}
