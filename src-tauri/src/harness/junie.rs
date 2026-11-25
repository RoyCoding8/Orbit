use std::path::{Path, PathBuf};

use super::{
    json_mcp, simple, Harness, HarnessId, InventoryKind, ResourceRef, Result as HarnessResult,
    JUNIE,
};

pub(crate) use super::json_mcp as mcp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JunieHarness {
    config_root: PathBuf,
    home: PathBuf,
}

impl JunieHarness {
    pub fn new(config_root: PathBuf, home: PathBuf) -> Self {
        Self { config_root, home }
    }

    pub fn from_env() -> Self {
        let home = simple::home_dir();
        let config_root = simple::env_path("JUNIE_HOME").unwrap_or_else(|| home.join(".junie"));
        Self::new(config_root, home)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
}

impl Harness for JunieHarness {
    fn id(&self) -> HarnessId {
        JUNIE
    }

    fn config_root(&self) -> &Path {
        &self.config_root
    }

    fn settings_path(&self) -> PathBuf {
        self.config_root.join("config.json")
    }

    fn mcp_path(&self) -> PathBuf {
        self.config_root.join("mcp").join("mcp.json")
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

    use super::JunieHarness;
    use crate::harness::{
        claude::config::{ConfigSnapshot, SettingsLayer},
        simple, Harness,
    };

    fn layer_path(
        harness: &JunieHarness,
        project: &Path,
        layer: SettingsLayer,
    ) -> Result<(PathBuf, &'static str), String> {
        match layer {
            SettingsLayer::User => Ok((harness.settings_path(), "junie-user-settings")),
            SettingsLayer::Project => Ok((
                project.join(".junie").join("config.json"),
                "junie-project-settings",
            )),
            SettingsLayer::Local => {
                Err("Junie CLI settings have user and project scopes only.".into())
            }
        }
    }

    pub fn load_config(harness: &JunieHarness, project: &Path) -> Result<ConfigSnapshot, String> {
        simple::load_config_layers(&[
            ("user", harness.settings_path()),
            ("project", project.join(".junie").join("config.json")),
        ])
    }

    pub fn apply_value(
        harness: &JunieHarness,
        project: &Path,
        layer: SettingsLayer,
        key: &str,
        value: Value,
    ) -> Result<(), String> {
        let (path, action) = layer_path(harness, project, layer)?;
        simple::apply_setting(harness, action, &path, key, value, None)
    }

    pub fn undo(
        harness: &JunieHarness,
        project: &Path,
        layer: SettingsLayer,
        key: &str,
    ) -> Result<(), String> {
        let (path, action) = layer_path(harness, project, layer)?;
        simple::undo_setting(harness, action, &path, key)
    }
}

pub mod skills {
    use std::path::{Path, PathBuf};

    use super::JunieHarness;
    use crate::harness::{
        claude::{skills::SkillEntry, SkillOverride},
        simple, Harness,
    };

    pub fn owned_roots(harness: &JunieHarness, project: &Path) -> Vec<PathBuf> {
        vec![
            harness.config_root().join("skills"),
            project.join(".junie").join("skills"),
        ]
    }

    pub fn owned(harness: &JunieHarness, project: &Path, file: &Path) -> bool {
        simple::skill_in_roots(&owned_roots(harness, project), file)
    }

    pub fn inventory(harness: &JunieHarness, project: &Path) -> Result<Vec<SkillEntry>, String> {
        simple::skill_inventory(&[
            (harness.config_root().join("skills"), true),
            (project.join(".junie").join("skills"), true),
        ])
    }

    pub fn set_override(
        _: &JunieHarness,
        _: &Path,
        _: &str,
        _: SkillOverride,
    ) -> Result<(), String> {
        simple::unsupported_skill_override("Junie CLI")
    }

    pub fn edit_skill(
        harness: &JunieHarness,
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

    pub fn install(harness: &JunieHarness, source: &Path) -> Result<String, String> {
        simple::install_skill(harness, &harness.config_root().join("skills"), source)
    }

    pub fn uninstall(harness: &JunieHarness, project: &Path, key: &str) -> Result<(), String> {
        simple::uninstall_skill(harness, inventory(harness, project)?, key)
    }

    pub fn restore(
        harness: &JunieHarness,
        project: &Path,
        key: &str,
        backup: &Path,
    ) -> Result<(), String> {
        simple::restore_skill(harness, &owned_roots(harness, project), key, backup)
    }
}

pub mod memories {
    use std::path::Path;

    use super::JunieHarness;
    use crate::harness::{
        claude::memories::{MemoryCatalog, MemoryGroup, MemoryRole, MemorySource},
        fs_guard,
        simple::{self, MemoryCandidate},
        Harness,
    };

    pub fn allowed_instruction_path(harness: &JunieHarness, project: &Path, path: &Path) -> bool {
        let Some(target) = fs_guard::canonical_or_missing_tail(path) else {
            return false;
        };
        let cfg = fs_guard::canonical_or_missing_tail(harness.config_root())
            .unwrap_or_else(|| harness.config_root().to_path_buf());
        let proj =
            fs_guard::canonical_or_missing_tail(project).unwrap_or_else(|| project.to_path_buf());
        if target == cfg.join("AGENTS.md")
            || target == proj.join("AGENTS.md")
            || target == proj.join(".junie").join("guidelines.md")
        {
            return true;
        }
        let rules_dir = proj.join(".junie").join("rules");
        target.parent() == Some(rules_dir.as_path())
            && target.extension().is_some_and(|e| e == "md")
    }

    pub fn catalog(harness: &JunieHarness, project: &Path) -> Result<MemoryCatalog, String> {
        let mut candidates = vec![
            MemoryCandidate {
                path: harness.config_root().join("AGENTS.md"),
                group_id: "user",
                source: MemorySource::UserInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "Always active in user scope".into(),
                editable: true,
            },
            MemoryCandidate {
                path: project.join("AGENTS.md"),
                group_id: "project",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "Always active in project scope".into(),
                editable: true,
            },
            MemoryCandidate {
                path: project.join(".junie").join("guidelines.md"),
                group_id: "project",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "Junie project guidelines".into(),
                editable: true,
            },
        ];
        for path in simple::collect_dir_files(&project.join(".junie").join("rules"), &["md"])? {
            candidates.push(MemoryCandidate {
                path,
                group_id: "project",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "Junie project rule".into(),
                editable: true,
            });
        }
        simple::build_memory_catalog(
            "junie",
            project,
            Some(harness.home()),
            harness.config_root(),
            candidates,
            vec![
                MemoryGroup {
                    id: "user".into(),
                    label: "User instructions".into(),
                    role: MemorySource::UserInstructions,
                },
                MemoryGroup {
                    id: "project".into(),
                    label: "Project instructions and rules".into(),
                    role: MemorySource::ProjectInstructions,
                },
            ],
        )
    }

    pub fn edit(
        harness: &JunieHarness,
        project: &Path,
        id: &str,
        expected: &str,
        content: &str,
    ) -> Result<(), String> {
        simple::edit_memory(harness, catalog(harness, project)?, id, expected, content)
    }

    pub fn delete(
        harness: &JunieHarness,
        project: &Path,
        id: &str,
        expected: &str,
    ) -> Result<(), String> {
        simple::delete_memory(harness, &catalog(harness, project)?, id, expected)
    }
}

pub mod hooks {
    use std::path::Path;

    use super::JunieHarness;
    use crate::harness::{
        claude::{config::SettingsLayer, hooks::HookView},
        simple::{self, HookFormat, HookSchema},
        Harness, HookDraft,
    };

    pub const EVENTS: [&str; 10] = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "SubagentStart",
        "SubagentStop",
        "PreCompact",
        "Notification",
        "Stop",
        "SessionEnd",
    ];

    const SCHEMA: HookSchema = HookSchema {
        action: "junie-hooks",
        events: &EVENTS,
        format: HookFormat::MatcherGroups,
        version_field: None,
    };

    pub fn list(harness: &JunieHarness, _project: &Path) -> Result<Vec<HookView>, String> {
        simple::list_hooks(&[(SettingsLayer::User, harness.settings_path())], SCHEMA)
    }

    pub fn save(harness: &JunieHarness, _project: &Path, draft: HookDraft) -> Result<(), String> {
        if draft.layer != SettingsLayer::User {
            return Err(
                "Junie CLI ignores project hooks for safety; configure user hooks only.".into(),
            );
        }
        simple::save_hook(harness, &harness.settings_path(), SCHEMA, draft)
    }

    pub fn set_enabled(
        _: &JunieHarness,
        _: &Path,
        _: SettingsLayer,
        _: &str,
        _: bool,
    ) -> Result<(), String> {
        simple::unsupported_toggle("Junie CLI")
    }

    pub fn delete(
        harness: &JunieHarness,
        _project: &Path,
        layer: SettingsLayer,
        id: &str,
    ) -> Result<(), String> {
        if layer != SettingsLayer::User {
            return Err(
                "Junie CLI ignores project hooks for safety; configure user hooks only.".into(),
            );
        }
        simple::delete_hook(harness, &harness.settings_path(), layer, SCHEMA, id)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::harness::{claude::config::SettingsLayer, HookDraft};

    #[test]
    fn config_precedence_credential_masking_and_user_only_hooks() {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        fs::create_dir_all(&project).unwrap();
        let harness = JunieHarness::new(home.join(".junie"), home);

        config::apply_value(
            &harness,
            &project,
            SettingsLayer::User,
            "byok",
            json!({"openai": {"apiKey": "sk-junie-secret"}}),
        )
        .unwrap();
        config::apply_value(
            &harness,
            &project,
            SettingsLayer::User,
            "model",
            json!("user-model"),
        )
        .unwrap();
        config::apply_value(
            &harness,
            &project,
            SettingsLayer::Project,
            "model",
            json!("project-model"),
        )
        .unwrap();
        let snap = config::load_config(&harness, &project).unwrap();
        assert_eq!(snap.effective["model"].value, json!("project-model"));
        assert_eq!(snap.effective["model"].source, "project");
        assert_eq!(
            snap.layers[0].values["byok"]["openai"]["apiKey"],
            "••••••••"
        );

        assert!(hooks::save(
            &harness,
            &project,
            HookDraft {
                layer: SettingsLayer::Project,
                event: "PreToolUse".into(),
                matcher: None,
                command: "echo nope".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .is_err());
        hooks::save(
            &harness,
            &project,
            HookDraft {
                layer: SettingsLayer::User,
                event: "PreToolUse".into(),
                matcher: Some("Bash".into()),
                command: "echo ok".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .unwrap();
        let list = hooks::list(&harness, &project).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].matcher.as_deref(), Some("Bash"));
    }

    #[test]
    fn mcp_skills_and_memories_follow_junie_paths() {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        fs::create_dir_all(home.join(".junie/skills/u-skill")).unwrap();
        fs::create_dir_all(project.join(".junie/skills/p-skill")).unwrap();
        fs::create_dir_all(project.join(".junie/rules")).unwrap();
        fs::write(
            home.join(".junie/skills/u-skill/SKILL.md"),
            "---\nname: u-skill\ndescription: User\n---\nbody",
        )
        .unwrap();
        fs::write(
            project.join(".junie/skills/p-skill/SKILL.md"),
            "---\nname: p-skill\ndescription: Project\n---\nbody",
        )
        .unwrap();
        fs::write(home.join(".junie/AGENTS.md"), "# User").unwrap();
        fs::write(project.join(".junie/guidelines.md"), "# Guide").unwrap();
        fs::write(project.join(".junie/rules/rust.md"), "# Rust").unwrap();
        let harness = JunieHarness::new(home.join(".junie"), home);

        mcp::save(
            &harness,
            "local",
            json!({"type": "stdio", "command": "npx", "args": ["-y", "srv"]}),
        )
        .unwrap();
        assert_eq!(mcp::list(&harness).unwrap().len(), 1);
        assert!(harness.mcp_path().ends_with("mcp/mcp.json"));
        assert_eq!(skills::inventory(&harness, &project).unwrap().len(), 2);
        assert_eq!(
            memories::catalog(&harness, &project).unwrap().files.len(),
            3
        );
    }
}
