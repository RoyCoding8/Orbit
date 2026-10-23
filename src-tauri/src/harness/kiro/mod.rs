//! Native Kiro CLI configuration and resources.

use std::path::{Path, PathBuf};

use super::{
    Harness, HarnessError, HarnessId, InventoryKind, ResourceKind, ResourceRef, Result, KIRO,
};

pub mod config;
pub mod hooks;
pub mod mcp;
pub mod memories;
pub mod skills;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroHarness {
    home: PathBuf,
}

impl KiroHarness {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    pub fn from_env() -> Self {
        let user_home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let home = std::env::var_os("KIRO_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| user_home.join(".kiro"));
        Self::new(home)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn project_root(&self, project: &Path) -> PathBuf {
        project.join(".kiro")
    }

    pub fn project_settings_path(&self, project: &Path) -> PathBuf {
        self.project_root(project).join("settings").join("cli.json")
    }

    pub fn project_mcp_path(&self, project: &Path) -> PathBuf {
        self.project_root(project).join("settings").join("mcp.json")
    }
}

impl Harness for KiroHarness {
    fn id(&self) -> HarnessId {
        KIRO
    }

    fn config_root(&self) -> &Path {
        &self.home
    }

    fn settings_path(&self) -> PathBuf {
        self.home.join("settings").join("cli.json")
    }

    fn mcp_path(&self) -> PathBuf {
        self.home.join("settings").join("mcp.json")
    }

    fn user_home(&self) -> Option<&Path> {
        self.home.parent()
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
            InventoryKind::McpServer => mcp::list_global(self).map(|items| {
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
            path: self.home.clone(),
            message,
        })
    }
}
