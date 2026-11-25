use std::path::{Path, PathBuf};

use super::{
    json_mcp, simple, Harness, HarnessId, InventoryKind, ResourceRef, Result as HarnessResult,
    CURSOR,
};

pub(crate) use super::json_mcp as mcp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorHarness {
    config_root: PathBuf,
    home: PathBuf,
}

impl CursorHarness {
    pub fn new(config_root: PathBuf, home: PathBuf) -> Self {
        Self { config_root, home }
    }

    fn xdg_cursor_dir() -> Option<PathBuf> {
        if cfg!(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd"
        )) {
            simple::env_path("XDG_CONFIG_HOME").map(|d| d.join("cursor"))
        } else {
            None
        }
    }

    pub fn from_env() -> Self {
        let home = simple::home_dir();
        let config_root = simple::env_path("CURSOR_CONFIG_DIR")
            .or_else(Self::xdg_cursor_dir)
            .unwrap_or_else(|| home.join(".cursor"));
        Self::new(config_root, home)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
}

impl Harness for CursorHarness {
    fn id(&self) -> HarnessId {
        CURSOR
    }

    fn config_root(&self) -> &Path {
        &self.config_root
    }

    fn settings_path(&self) -> PathBuf {
        self.config_root.join("cli-config.json")
    }

    fn mcp_path(&self) -> PathBuf {
        self.config_root.join("mcp.json")
    }

    fn user_home(&self) -> Option<&Path> {
        Some(&self.home)
    }

    fn list(&self, kind: InventoryKind) -> HarnessResult<Vec<ResourceRef>> {
        simple::list_inventory(
            self,
            skills::inventory(self, &self.home),
            json_mcp::list(self),
            kind,
        )
    }
}

pub mod config {
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use super::CursorHarness;
    use crate::harness::{
        claude::config::{ConfigSnapshot, SettingsLayer},
        simple, Harness,
    };

    type LayerTarget = (PathBuf, &'static str, Option<(&'static str, i64)>);

    fn layer_path(
        harness: &CursorHarness,
        project: &Path,
        layer: SettingsLayer,
    ) -> Result<LayerTarget, String> {
        match layer {
            SettingsLayer::User => Ok((
                harness.settings_path(),
                "cursor-user-settings",
                Some(("version", 1)),
            )),
            SettingsLayer::Project => Ok((
                project.join(".cursor").join("cli.json"),
                "cursor-project-settings",
                None,
            )),
            SettingsLayer::Local => {
                Err("Cursor CLI settings have user and project scopes only.".into())
            }
        }
    }

    pub fn load_config(harness: &CursorHarness, project: &Path) -> Result<ConfigSnapshot, String> {
        simple::load_config_layers(&[
            ("user", harness.settings_path()),
            ("project", project.join(".cursor").join("cli.json")),
        ])
    }

    pub fn apply_value(
        harness: &CursorHarness,
        project: &Path,
        layer: SettingsLayer,
        key: &str,
        value: Value,
    ) -> Result<(), String> {
        if layer == SettingsLayer::Project && key.trim() != "permissions" {
            return Err(
                "Cursor CLI project cli.json only supports the `permissions` setting.".into(),
            );
        }
        let (path, action, version) = layer_path(harness, project, layer)?;
        simple::apply_setting(harness, action, &path, key, value, version)
    }

    pub fn undo(
        harness: &CursorHarness,
        project: &Path,
        layer: SettingsLayer,
        key: &str,
    ) -> Result<(), String> {
        let (path, action, _) = layer_path(harness, project, layer)?;
        simple::undo_setting(harness, action, &path, key)
    }
}

pub mod skills {
    use std::path::{Path, PathBuf};

    use super::CursorHarness;
    use crate::harness::{
        claude::{skills::SkillEntry, SkillOverride},
        simple, Harness,
    };

    pub fn owned_roots(harness: &CursorHarness, project: &Path) -> Vec<PathBuf> {
        vec![
            harness.config_root().join("skills"),
            project.join(".cursor").join("skills"),
        ]
    }

    pub fn owned(harness: &CursorHarness, project: &Path, file: &Path) -> bool {
        simple::skill_in_roots(&owned_roots(harness, project), file)
    }

    pub fn inventory(harness: &CursorHarness, project: &Path) -> Result<Vec<SkillEntry>, String> {
        simple::skill_inventory(&[
            (harness.config_root().join("skills"), true),
            (harness.config_root().join("skills-cursor"), false),
            (project.join(".cursor").join("skills"), true),
        ])
    }

    pub fn set_override(
        _: &CursorHarness,
        _: &Path,
        _: &str,
        _: SkillOverride,
    ) -> Result<(), String> {
        simple::unsupported_skill_override("Cursor CLI")
    }

    pub fn edit_skill(
        harness: &CursorHarness,
        project: &Path,
        key: &str,
        expected: &str,
        content: &str,
    ) -> Result<(), String> {
        simple::edit_skill(
            harness,
            inventory(harness, project)?,
            key,
            expected,
            content,
        )
    }

    pub fn install(harness: &CursorHarness, source: &Path) -> Result<String, String> {
        simple::install_skill(harness, &harness.config_root().join("skills"), source)
    }

    pub fn uninstall(harness: &CursorHarness, project: &Path, key: &str) -> Result<(), String> {
        simple::uninstall_skill(harness, inventory(harness, project)?, key)
    }

    pub fn restore(
        harness: &CursorHarness,
        project: &Path,
        key: &str,
        backup: &Path,
    ) -> Result<(), String> {
        simple::restore_skill(harness, &owned_roots(harness, project), key, backup)
    }
}

pub mod memories {
    use std::path::Path;

    use super::CursorHarness;
    use crate::harness::{
        claude::memories::{MemoryCatalog, MemoryGroup, MemoryRole, MemorySource},
        fs_guard,
        simple::{self, MemoryCandidate},
        Harness,
    };

    pub fn allowed_instruction_path(_harness: &CursorHarness, project: &Path, path: &Path) -> bool {
        let Some(target) = fs_guard::canonical_or_missing_tail(path) else {
            return false;
        };
        let proj =
            fs_guard::canonical_or_missing_tail(project).unwrap_or_else(|| project.to_path_buf());
        if target == proj.join("AGENTS.md") || target == proj.join("CLAUDE.md") {
            return true;
        }
        let rules_dir = proj.join(".cursor").join("rules");
        target.parent() == Some(rules_dir.as_path())
            && target
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "md" | "mdc"))
    }

    pub fn catalog(harness: &CursorHarness, project: &Path) -> Result<MemoryCatalog, String> {
        let mut candidates = vec![
            MemoryCandidate {
                path: project.join("AGENTS.md"),
                group_id: "project",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "Always active in project scope".into(),
                editable: true,
            },
            MemoryCandidate {
                path: project.join("CLAUDE.md"),
                group_id: "project",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "Always active in project scope".into(),
                editable: true,
            },
        ];
        for path in
            simple::collect_dir_files(&project.join(".cursor").join("rules"), &["md", "mdc"])?
        {
            candidates.push(MemoryCandidate {
                path,
                group_id: "project",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "Cursor project rule".into(),
                editable: true,
            });
        }
        simple::build_memory_catalog(
            "cursor",
            project,
            Some(harness.home()),
            harness.config_root(),
            candidates,
            vec![MemoryGroup {
                id: "project".into(),
                label: "Project instructions and rules".into(),
                role: MemorySource::ProjectInstructions,
            }],
        )
    }

    pub fn edit(
        harness: &CursorHarness,
        project: &Path,
        id: &str,
        expected: &str,
        content: &str,
    ) -> Result<(), String> {
        simple::edit_memory(harness, catalog(harness, project)?, id, expected, content)
    }

    pub fn delete(
        harness: &CursorHarness,
        project: &Path,
        id: &str,
        expected: &str,
    ) -> Result<(), String> {
        simple::delete_memory(harness, &catalog(harness, project)?, id, expected)
    }
}

pub mod hooks {
    use std::path::{Path, PathBuf};

    use super::CursorHarness;
    use crate::harness::{
        claude::{config::SettingsLayer, hooks::HookView},
        simple::{self, HookFormat, HookSchema},
        Harness, HookDraft,
    };

    pub const EVENTS: [&str; 21] = [
        "sessionStart",
        "sessionEnd",
        "preToolUse",
        "postToolUse",
        "postToolUseFailure",
        "subagentStart",
        "subagentStop",
        "beforeShellExecution",
        "afterShellExecution",
        "beforeMCPExecution",
        "afterMCPExecution",
        "beforeReadFile",
        "afterFileEdit",
        "beforeSubmitPrompt",
        "preCompact",
        "stop",
        "afterAgentResponse",
        "afterAgentThought",
        "beforeTabFileRead",
        "afterTabFileEdit",
        "workspaceOpen",
    ];

    const SCHEMA: HookSchema = HookSchema {
        action: "cursor-hooks",
        events: &EVENTS,
        format: HookFormat::FlatList,
        version_field: Some(("version", 1)),
    };

    fn path(
        harness: &CursorHarness,
        project: &Path,
        layer: SettingsLayer,
    ) -> Result<PathBuf, String> {
        match layer {
            SettingsLayer::User => Ok(harness.config_root().join("hooks.json")),
            SettingsLayer::Project => Ok(project.join(".cursor").join("hooks.json")),
            SettingsLayer::Local => {
                Err("Cursor CLI hooks have user and project scopes only".into())
            }
        }
    }

    pub fn list(harness: &CursorHarness, project: &Path) -> Result<Vec<HookView>, String> {
        simple::list_hooks(
            &[
                (
                    SettingsLayer::User,
                    harness.config_root().join("hooks.json"),
                ),
                (
                    SettingsLayer::Project,
                    project.join(".cursor").join("hooks.json"),
                ),
            ],
            SCHEMA,
        )
    }

    pub fn save(harness: &CursorHarness, project: &Path, draft: HookDraft) -> Result<(), String> {
        let target = path(harness, project, draft.layer)?;
        simple::save_hook(harness, &target, SCHEMA, draft)
    }

    pub fn set_enabled(
        _: &CursorHarness,
        _: &Path,
        _: SettingsLayer,
        _: &str,
        _: bool,
    ) -> Result<(), String> {
        simple::unsupported_toggle("Cursor CLI")
    }

    pub fn delete(
        harness: &CursorHarness,
        project: &Path,
        layer: SettingsLayer,
        id: &str,
    ) -> Result<(), String> {
        let target = path(harness, project, layer)?;
        simple::delete_hook(harness, &target, layer, SCHEMA, id)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::harness::{claude::config::SettingsLayer, json_store, HookDraft};

    #[test]
    fn settings_version_project_permissions_only_and_hooks_version() {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        fs::create_dir_all(&project).unwrap();
        let harness = CursorHarness::new(home.join(".cursor"), home);

        config::apply_value(
            &harness,
            &project,
            SettingsLayer::User,
            "model",
            json!("claude-4"),
        )
        .unwrap();
        assert_eq!(
            json_store::read_value(&harness.settings_path()).unwrap()["version"],
            1
        );
        assert!(config::apply_value(
            &harness,
            &project,
            SettingsLayer::Project,
            "model",
            json!("x")
        )
        .is_err());
        config::apply_value(
            &harness,
            &project,
            SettingsLayer::Project,
            "permissions",
            json!({"allow": ["Read"]}),
        )
        .unwrap();

        hooks::save(
            &harness,
            &project,
            HookDraft {
                layer: SettingsLayer::User,
                event: "beforeShellExecution".into(),
                matcher: Some("git".into()),
                command: "echo check".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .unwrap();
        let raw_hooks = json_store::read_value(&harness.config_root().join("hooks.json")).unwrap();
        assert_eq!(raw_hooks["version"], 1);
        assert_eq!(
            raw_hooks["hooks"]["beforeShellExecution"][0]["command"],
            "echo check"
        );
        let listed = hooks::list(&harness, &project).unwrap();
        assert_eq!(listed.len(), 1);
        hooks::delete(&harness, &project, SettingsLayer::User, &listed[0].id).unwrap();
        assert!(hooks::list(&harness, &project).unwrap().is_empty());
    }

    #[test]
    fn skills_rules_mdc_and_mcp_follow_cursor_layout() {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        fs::create_dir_all(home.join(".cursor/skills/user-skill")).unwrap();
        fs::create_dir_all(home.join(".cursor/skills-cursor/builtin")).unwrap();
        fs::create_dir_all(project.join(".cursor/rules")).unwrap();
        fs::write(
            home.join(".cursor/skills/user-skill/SKILL.md"),
            "---\nname: user-skill\ndescription: User\n---\nbody",
        )
        .unwrap();
        fs::write(
            home.join(".cursor/skills-cursor/builtin/SKILL.md"),
            "---\nname: builtin\ndescription: Builtin\n---\nbody",
        )
        .unwrap();
        fs::write(project.join(".cursor/rules/style.mdc"), "# Style").unwrap();
        fs::write(project.join("CLAUDE.md"), "# Claude").unwrap();
        let harness = CursorHarness::new(home.join(".cursor"), home);

        let inv = skills::inventory(&harness, &project).unwrap();
        assert_eq!(inv.len(), 2);
        assert!(
            inv.iter()
                .find(|s| s.name == "user-skill")
                .unwrap()
                .editable
        );
        assert!(!inv.iter().find(|s| s.name == "builtin").unwrap().editable);
        assert_eq!(
            memories::catalog(&harness, &project).unwrap().files.len(),
            2
        );

        mcp::save(
            &harness,
            "docs",
            json!({"type": "stdio", "command": "npx", "args": ["-y", "docs"]}),
        )
        .unwrap();
        assert_eq!(mcp::list(&harness).unwrap().len(), 1);
    }
}
