//! Native Pi coding-agent configuration and resources.

use std::path::{Path, PathBuf};

use super::{
    Harness, HarnessError, HarnessId, InventoryKind, ResourceKind, ResourceRef, Result, PI,
};

pub mod config;
pub mod mcp;
pub mod memories;
pub mod skills;

pub const HOOKS_UNSUPPORTED: &str = "Pi hooks are implemented by TypeScript or JavaScript extensions; declarative hook management is unavailable";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiHarness {
    config_root: PathBuf,
    home: PathBuf,
}

impl PiHarness {
    pub fn new(config_root: PathBuf, home: PathBuf) -> Self {
        Self { config_root, home }
    }

    pub fn from_env() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let config_root = std::env::var_os("PI_CODING_AGENT_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".pi").join("agent"));
        Self::new(config_root, home)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn project_root(&self, project: &Path) -> PathBuf {
        project.join(".pi")
    }

    pub fn project_settings_path(&self, project: &Path) -> PathBuf {
        self.project_root(project).join("settings.json")
    }
}

impl Harness for PiHarness {
    fn id(&self) -> HarnessId {
        PI
    }

    fn config_root(&self) -> &Path {
        &self.config_root
    }

    fn settings_path(&self) -> PathBuf {
        self.config_root.join("settings.json")
    }

    fn mcp_path(&self) -> PathBuf {
        self.config_root.join("mcp.json")
    }

    fn user_home(&self) -> Option<&Path> {
        Some(&self.home)
    }

    fn list(&self, kind: InventoryKind) -> Result<Vec<ResourceRef>> {
        let resources = match kind {
            InventoryKind::Skill => skills::list_global(self).map(|items| {
                items
                    .into_iter()
                    .map(|item| ResourceRef {
                        kind: ResourceKind::Skill,
                        id: item.id,
                        path: item.path,
                    })
                    .collect()
            }),
            InventoryKind::McpServer => mcp::list(self).map(|items| {
                items
                    .into_iter()
                    .map(|item| ResourceRef {
                        kind: ResourceKind::McpServer,
                        id: item.name,
                        path: self.mcp_path(),
                    })
                    .collect()
            }),
        };
        resources.map_err(|message| HarnessError::Mutation {
            path: self.config_root.clone(),
            message,
        })
    }
}
