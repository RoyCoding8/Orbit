//! Shared resource types and configuration adapters for coding harnesses.

pub mod atomic;
pub mod claude;
pub mod codex;
pub(crate) mod credentials;
pub(crate) mod instruction_store;
pub(crate) mod json_store;
pub mod kiro;
pub(crate) mod memory_links;
pub mod opencode;
pub mod pi;
pub(crate) mod process_lifetime;
pub mod selected;
pub(crate) mod skill_store;

use std::path::PathBuf;

/// Resource kinds tracked by the adapter contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// Named markdown unit with frontmatter, e.g. Claude `skills/<name>/SKILL.md`.
    Skill,
    /// Per-project auto memory, e.g. `projects/<slug>/memory/MEMORY.md`.
    Memory,
    /// User instruction file, e.g. `CLAUDE.md`.
    GlobalInstructions,
    ProjectInstructions,
    /// A settings-layer document, e.g. `settings.json` or `.claude.json`.
    Settings,
    /// MCP server entry, e.g. the `mcpServers` map in Claude's `.claude.json`.
    McpServer,
}

/// Which harness a resource belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HarnessId(pub &'static str);

pub const CODEX: HarnessId = HarnessId("codex");

pub const CLAUDE_CODE: HarnessId = HarnessId("claude-code");
pub const KIRO: HarnessId = HarnessId("kiro");
pub const OPENCODE: HarnessId = HarnessId("opencode");
pub const PI: HarnessId = HarnessId("pi");

#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Client {
    #[default]
    ClaudeCode,
    Codex,
    Kiro,
    Opencode,
    Pi,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookDraft {
    pub layer: claude::config::SettingsLayer,
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    pub edit_id: Option<String>,
    pub source_id: Option<String>,
}

/// A resource found inside one harness's configuration tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRef {
    pub kind: ResourceKind,
    pub id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub enum InventoryKind {
    Skill,
    McpServer,
}

/// Errors the adapter surfaces to callers.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("failed reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed parsing {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed writing {path}: {source}")]
    WriteJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("mutation of {path} may have landed: {message}")]
    Mutation { path: PathBuf, message: String },
    #[error("resource kind {0} is not supported by this adapter arm yet")]
    UnsupportedKind(String),
}

pub type Result<T> = std::result::Result<T, HarnessError>;

/// The per-harness adapter boundary. Each arm implements this trait without
/// touching its callers.
pub trait Harness {
    fn id(&self) -> HarnessId;

    /// Root of the harness's configuration directory (Claude: `CLAUDE_CONFIG_DIR`).
    fn config_root(&self) -> &std::path::Path;

    /// List every resource of one kind the harness currently has on disk.
    fn list(&self, kind: InventoryKind) -> Result<Vec<ResourceRef>>;

    /// The settings file the harness rewrites itself (Claude: `settings.json`).
    fn settings_path(&self) -> PathBuf;

    fn mcp_path(&self) -> PathBuf {
        self.settings_path()
    }

    fn user_home(&self) -> Option<&std::path::Path> {
        None
    }
}

/// The default state file is beside the config directory. CLAUDE_CONFIG_DIR
/// moves it inside the override, so both locations must be resolved together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessPaths {
    /// Root of the harness's configuration directory.
    pub config_root: PathBuf,
    /// The continuously rewritten global state file (`~/.claude.json`).
    pub claude_json: PathBuf,
}

/// Resolve both harness locations from an optional `CLAUDE_CONFIG_DIR`
/// override and a fallback home directory. Pure so tests can drive it. A
/// whitespace-only override counts as unset; other values are trimmed.
pub fn resolve_paths(override_dir: Option<&str>, home: &std::path::Path) -> HarnessPaths {
    match override_dir.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some(dir) => {
            let config_root = PathBuf::from(dir);
            HarnessPaths {
                claude_json: config_root.join(".claude.json"),
                config_root,
            }
        }
        None => HarnessPaths {
            config_root: home.join(".claude"),
            claude_json: home.join(".claude.json"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_config_dir_moves_both_the_root_and_the_state_file() {
        let home = std::path::Path::new(r"C:\Users\fixture");
        let paths = resolve_paths(Some(r"D:\fixture\claude-alt"), home);
        assert_eq!(
            paths.config_root,
            std::path::PathBuf::from(r"D:\fixture\claude-alt")
        );
        assert_eq!(
            paths.claude_json,
            std::path::PathBuf::from(r"D:\fixture\claude-alt\.claude.json")
        );
    }

    #[test]
    fn missing_or_blank_override_keeps_the_default_layout() {
        let home = std::path::Path::new(r"C:\Users\fixture");
        let expected = HarnessPaths {
            config_root: std::path::PathBuf::from(r"C:\Users\fixture\.claude"),
            claude_json: std::path::PathBuf::from(r"C:\Users\fixture\.claude.json"),
        };
        assert_eq!(resolve_paths(None, home), expected);
        assert_eq!(resolve_paths(Some(""), home), expected);
        assert_eq!(resolve_paths(Some("   "), home), expected);
    }

    #[test]
    fn a_set_override_is_trimmed_before_use() {
        let home = std::path::Path::new(r"C:\Users\fixture");
        let paths = resolve_paths(Some("  D:\\fixture\\cfg  "), home);
        assert_eq!(
            paths.config_root,
            std::path::PathBuf::from(r"D:\fixture\cfg")
        );
    }
}
