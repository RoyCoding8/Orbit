use std::path::{Path, PathBuf};

use super::{
    simple, Harness, HarnessId, InventoryKind, ResourceRef, Result as HarnessResult, MUSE,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuseHarness {
    config_root: PathBuf,
    home: PathBuf,
}

impl MuseHarness {
    pub fn new(config_root: PathBuf, home: PathBuf) -> Self {
        Self { config_root, home }
    }

    pub fn from_env() -> Self {
        let home = simple::home_dir();
        let config_root = simple::env_path("MUSE_CONFIG_DIR")
            .or_else(|| simple::env_path("XDG_CONFIG_HOME").map(|d| d.join("muse")))
            .unwrap_or_else(|| home.join(".config").join("muse"));
        Self::new(config_root, home)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
}

impl Harness for MuseHarness {
    fn id(&self) -> HarnessId {
        MUSE
    }

    fn config_root(&self) -> &Path {
        &self.config_root
    }

    fn settings_path(&self) -> PathBuf {
        self.config_root.join("settings.json")
    }

    fn user_home(&self) -> Option<&Path> {
        Some(&self.home)
    }

    fn list(&self, kind: InventoryKind) -> HarnessResult<Vec<ResourceRef>> {
        simple::list_inventory(
            self,
            skills::inventory(self, &self.home),
            mcp::list(self),
            kind,
        )
    }
}

pub mod config {
    use std::path::Path;

    use serde_json::Value;

    use super::MuseHarness;
    use crate::harness::{
        claude::config::{ConfigSnapshot, SettingsLayer},
        simple, Harness,
    };

    const ACTION: &str = "muse-settings";

    pub fn load_config(harness: &MuseHarness, _project: &Path) -> Result<ConfigSnapshot, String> {
        simple::load_config_layers(&[("user", harness.settings_path())])
    }

    pub fn apply_value(
        harness: &MuseHarness,
        _project: &Path,
        layer: SettingsLayer,
        key: &str,
        value: Value,
    ) -> Result<(), String> {
        if layer != SettingsLayer::User {
            return Err("Muse Code settings are user-scoped only.".into());
        }
        simple::apply_setting(
            harness,
            ACTION,
            &harness.settings_path(),
            key,
            value,
            Some(("schema_version", 1)),
        )
    }

    pub fn undo(
        harness: &MuseHarness,
        _project: &Path,
        layer: SettingsLayer,
        key: &str,
    ) -> Result<(), String> {
        if layer != SettingsLayer::User {
            return Err("Muse Code settings are user-scoped only.".into());
        }
        simple::undo_setting(harness, ACTION, &harness.settings_path(), key)
    }
}

pub mod mcp {
    use serde_json::{Map, Value};

    use super::MuseHarness;
    use crate::harness::{
        claude::mcp::{self as cmcp, BenchResult, HttpEntry, ServerView, StdioEntry},
        credentials, json_store, Harness, ResourceKind,
    };

    const SERVERS: &str = "mcp_servers";

    fn transport_kind(entry: &Map<String, Value>) -> Result<&'static str, String> {
        if let Some(t) = entry
            .get("transport")
            .or_else(|| entry.get("type"))
            .and_then(Value::as_str)
        {
            return match t {
                "stdio" => Ok("stdio"),
                "streamable_http" | "http" => Ok("http"),
                _ => Err("Muse Code MCP transport must be `stdio` or `streamable_http`".into()),
            };
        }
        match (entry.contains_key("command"), entry.contains_key("url")) {
            (true, false) => Ok("stdio"),
            (false, true) => Ok("http"),
            (true, true) => Err("MCP server cannot contain both `command` and `url`".into()),
            (false, false) => Err("MCP server must have a `command` or `url`".into()),
        }
    }

    fn validate_name(name: &str) -> Result<(), String> {
        if name.trim().is_empty() || name.chars().any(char::is_control) {
            return Err(
                "MCP server name must be non-empty and contain no control characters".into(),
            );
        }
        Ok(())
    }

    pub fn list(harness: &MuseHarness) -> Result<Vec<ServerView>, String> {
        let doc = json_store::read_value(&harness.settings_path())?;
        let Some(servers) = doc.get(SERVERS).and_then(Value::as_object) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(servers.len());
        for (name, raw) in servers {
            let obj = raw
                .as_object()
                .ok_or("MCP server entry must be an object")?;
            let kind = transport_kind(obj)?;
            let enabled = obj.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let mut entry = raw.clone();
            entry
                .as_object_mut()
                .expect("object")
                .insert("type".into(), Value::String(kind.into()));
            out.push(ServerView {
                name: name.clone(),
                kind: kind.into(),
                entry: cmcp::mask_entry(&entry),
                enabled,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn save(harness: &MuseHarness, name: &str, entry: Value) -> Result<(), String> {
        validate_name(name)?;
        let mut obj = entry
            .as_object()
            .cloned()
            .ok_or("MCP server entry must be an object")?;
        let kind = transport_kind(&obj)?;
        obj.remove("type");
        let native_transport = if kind == "stdio" {
            "stdio"
        } else {
            "streamable_http"
        };
        obj.insert("transport".into(), Value::String(native_transport.into()));
        if let Some(mode) = obj.get("mode").and_then(Value::as_str) {
            if !matches!(mode, "required" | "optional") {
                return Err("Muse Code MCP `mode` must be `required` or `optional`".into());
            }
        }
        if let Some(enabled) = obj.get("enabled") {
            if !enabled.is_boolean() {
                return Err("`enabled` must be a boolean".into());
            }
        }
        let mut candidate = Value::Object(obj);
        match kind {
            "stdio" => {
                let parsed: StdioEntry = serde_json::from_value(candidate.clone())
                    .map_err(|e| format!("Invalid stdio MCP server: {e}"))?;
                if parsed.command.trim().is_empty() {
                    return Err("stdio MCP command cannot be empty".into());
                }
            }
            _ => {
                let parsed: HttpEntry = serde_json::from_value(candidate.clone())
                    .map_err(|e| format!("Invalid HTTP MCP server: {e}"))?;
                if parsed.url.trim().is_empty() {
                    return Err("HTTP MCP URL cannot be empty".into());
                }
            }
        }
        let id = name.to_owned();
        json_store::mutate_json(
            harness,
            ResourceKind::McpServer,
            name,
            "muse-mcp-save",
            &harness.settings_path(),
            move |doc| {
                let root = doc
                    .as_object_mut()
                    .ok_or("settings.json must contain a JSON object")?;
                root.entry("schema_version".to_owned())
                    .or_insert_with(|| Value::from(1));
                let servers = root
                    .entry(SERVERS)
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .ok_or("`mcp_servers` must be a JSON object")?;
                let previous = servers.get(&id).cloned();
                if let Some(prev) = &previous {
                    cmcp::preserve_masked_credentials(prev, &mut candidate)
                        .map_err(|e| e.to_string())?;
                }
                if credentials::contains_mask(&candidate) {
                    return Err("the edited entry still holds masked placeholders; re-enter the credential values".into());
                }
                if let Some(obj) = candidate.as_object_mut() {
                    if !obj.contains_key("enabled") {
                        let prev_enabled = previous
                            .as_ref()
                            .and_then(|p| p.get("enabled").cloned())
                            .unwrap_or(Value::Bool(true));
                        obj.insert("enabled".into(), prev_enabled);
                    }
                }
                servers.insert(id, candidate);
                Ok(())
            },
        )
    }

    pub fn set_enabled(harness: &MuseHarness, name: &str, enabled: bool) -> Result<(), String> {
        validate_name(name)?;
        json_store::mutate_json(
            harness,
            ResourceKind::McpServer,
            name,
            "muse-mcp-enabled",
            &harness.settings_path(),
            |doc| {
                let root = doc
                    .as_object_mut()
                    .ok_or("settings.json must contain a JSON object")?;
                root.entry("schema_version".to_owned())
                    .or_insert_with(|| Value::from(1));
                let entry = root
                    .get_mut(SERVERS)
                    .and_then(Value::as_object_mut)
                    .and_then(|s| s.get_mut(name))
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
                entry.insert("enabled".into(), Value::Bool(enabled));
                Ok(())
            },
        )
    }

    pub fn delete(harness: &MuseHarness, name: &str) -> Result<(), String> {
        validate_name(name)?;
        json_store::mutate_json(
            harness,
            ResourceKind::McpServer,
            name,
            "muse-mcp-delete",
            &harness.settings_path(),
            |doc| {
                let root = doc
                    .as_object_mut()
                    .ok_or("settings.json must contain a JSON object")?;
                root.entry("schema_version".to_owned())
                    .or_insert_with(|| Value::from(1));
                let servers = root
                    .get_mut(SERVERS)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
                if servers.remove(name).is_none() {
                    return Err(format!("MCP server `{name}` was not found"));
                }
                Ok(())
            },
        )
    }

    pub fn test(harness: &MuseHarness, name: &str) -> Result<BenchResult, String> {
        let doc = json_store::read_value(&harness.settings_path())?;
        let entry = doc
            .get(SERVERS)
            .and_then(Value::as_object)
            .and_then(|s| s.get(name))
            .cloned()
            .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
        let obj = entry
            .as_object()
            .ok_or("MCP server entry must be an object")?;
        if obj.get("enabled").and_then(Value::as_bool) == Some(false) {
            return Err("MCP server is disabled".into());
        }
        match transport_kind(obj)? {
            "stdio" => cmcp::test_stdio(
                &serde_json::from_value::<StdioEntry>(entry)
                    .map_err(|e| format!("Invalid stdio MCP server: {e}"))?,
            ),
            _ => cmcp::test_http(
                &serde_json::from_value::<HttpEntry>(entry)
                    .map_err(|e| format!("Invalid HTTP MCP server: {e}"))?,
            ),
        }
    }
}

pub mod skills {
    use std::path::{Path, PathBuf};

    use super::MuseHarness;
    use crate::harness::{
        claude::{skills::SkillEntry, SkillOverride},
        simple, Harness,
    };

    pub fn owned_roots(harness: &MuseHarness, project: &Path) -> Vec<PathBuf> {
        vec![
            harness.config_root().join("skills"),
            project.join(".agents").join("skills"),
        ]
    }

    pub fn owned(harness: &MuseHarness, project: &Path, file: &Path) -> bool {
        simple::skill_in_roots(&owned_roots(harness, project), file)
    }

    pub fn inventory(harness: &MuseHarness, project: &Path) -> Result<Vec<SkillEntry>, String> {
        simple::skill_inventory(&[
            (harness.config_root().join("skills"), true),
            (harness.home().join(".agents").join("skills"), false),
            (project.join(".agents").join("skills"), true),
        ])
    }

    pub fn set_override(
        _: &MuseHarness,
        _: &Path,
        _: &str,
        _: SkillOverride,
    ) -> Result<(), String> {
        simple::unsupported_skill_override("Muse Code")
    }

    pub fn edit_skill(
        harness: &MuseHarness,
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

    pub fn install(harness: &MuseHarness, source: &Path) -> Result<String, String> {
        simple::install_skill(harness, &harness.config_root().join("skills"), source)
    }

    pub fn uninstall(harness: &MuseHarness, project: &Path, key: &str) -> Result<(), String> {
        simple::uninstall_skill(harness, inventory(harness, project)?, key)
    }

    pub fn restore(
        harness: &MuseHarness,
        project: &Path,
        key: &str,
        backup: &Path,
    ) -> Result<(), String> {
        simple::restore_skill(harness, &owned_roots(harness, project), key, backup)
    }
}

pub mod memories {
    use std::path::Path;

    use super::MuseHarness;
    use crate::harness::{
        claude::memories::{MemoryCatalog, MemoryGroup, MemoryRole, MemorySource},
        fs_guard,
        simple::{self, MemoryCandidate},
        Harness,
    };

    pub fn allowed_instruction_path(_harness: &MuseHarness, project: &Path, path: &Path) -> bool {
        let Some(target) = fs_guard::canonical_or_missing_tail(path) else {
            return false;
        };
        let proj =
            fs_guard::canonical_or_missing_tail(project).unwrap_or_else(|| project.to_path_buf());
        let direct = [
            proj.join("AGENTS.md"),
            proj.join("CLAUDE.md"),
            proj.join(".agents").join("AGENTS.md"),
            proj.join(".claude").join("CLAUDE.md"),
        ];
        if direct.iter().any(|p| p == &target) {
            return true;
        }
        let mem_dir = proj.join(".agents").join("memory");
        target.parent() == Some(mem_dir.as_path()) && target.extension().is_some_and(|e| e == "md")
    }

    pub fn catalog(harness: &MuseHarness, project: &Path) -> Result<MemoryCatalog, String> {
        let mut candidates = Vec::new();
        for rel in [
            "AGENTS.md",
            "CLAUDE.md",
            ".agents/AGENTS.md",
            ".claude/CLAUDE.md",
        ] {
            candidates.push(MemoryCandidate {
                path: project.join(rel),
                group_id: "project",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "Always active in project scope".into(),
                editable: true,
            });
        }
        let mem_dir = project.join(".agents").join("memory");
        for path in simple::collect_dir_files(&mem_dir, &["md"])? {
            let is_index = path.file_name().is_some_and(|n| n == "MEMORY.md");
            candidates.push(MemoryCandidate {
                path,
                group_id: "memory",
                source: MemorySource::ProjectAutoMemory,
                role: if is_index {
                    MemoryRole::Index
                } else {
                    MemoryRole::Topic
                },
                load_behavior: if is_index {
                    "Project memory index".into()
                } else {
                    "Project memory topic".into()
                },
                editable: true,
            });
        }
        simple::build_memory_catalog(
            "muse",
            project,
            Some(harness.home()),
            harness.config_root(),
            candidates,
            vec![
                MemoryGroup {
                    id: "project".into(),
                    label: "Project instructions".into(),
                    role: MemorySource::ProjectInstructions,
                },
                MemoryGroup {
                    id: "memory".into(),
                    label: "Project memory".into(),
                    role: MemorySource::ProjectAutoMemory,
                },
            ],
        )
    }

    pub fn edit(
        harness: &MuseHarness,
        project: &Path,
        id: &str,
        expected: &str,
        content: &str,
    ) -> Result<(), String> {
        simple::edit_memory(harness, catalog(harness, project)?, id, expected, content)
    }

    pub fn delete(
        harness: &MuseHarness,
        project: &Path,
        id: &str,
        expected: &str,
    ) -> Result<(), String> {
        simple::delete_memory(harness, &catalog(harness, project)?, id, expected)
    }
}

pub mod hooks {
    use std::path::{Path, PathBuf};

    use super::MuseHarness;
    use crate::harness::{
        claude::{config::SettingsLayer, hooks::HookView},
        simple::{self, HookFormat, HookSchema},
        Harness, HookDraft,
    };

    pub const EVENTS: [&str; 17] = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "PostToolUse",
        "PostToolUseFailure",
        "PostToolBatch",
        "PreLLMCall",
        "PostLLMCall",
        "PreCompact",
        "PostCompact",
        "SubagentStart",
        "SubagentStop",
        "Notification",
        "Stop",
        "StopFailure",
        "SessionEnd",
    ];

    fn schema(layer: SettingsLayer) -> HookSchema {
        HookSchema {
            action: "muse-hooks",
            events: &EVENTS,
            format: HookFormat::MatcherGroups,
            version_field: if layer == SettingsLayer::User {
                Some(("schema_version", 1))
            } else {
                None
            },
        }
    }

    fn path(
        harness: &MuseHarness,
        project: &Path,
        layer: SettingsLayer,
    ) -> Result<PathBuf, String> {
        match layer {
            SettingsLayer::User => Ok(harness.settings_path()),
            SettingsLayer::Project => Ok(project.join(".muse").join("hooks.json")),
            SettingsLayer::Local => Err("Muse Code hooks have user and project scopes only".into()),
        }
    }

    pub fn list(harness: &MuseHarness, project: &Path) -> Result<Vec<HookView>, String> {
        let mut out = simple::list_hooks(
            &[(SettingsLayer::User, harness.settings_path())],
            schema(SettingsLayer::User),
        )?;
        out.extend(simple::list_hooks(
            &[(
                SettingsLayer::Project,
                project.join(".muse").join("hooks.json"),
            )],
            schema(SettingsLayer::Project),
        )?);
        Ok(out)
    }

    pub fn save(harness: &MuseHarness, project: &Path, draft: HookDraft) -> Result<(), String> {
        let target = path(harness, project, draft.layer)?;
        simple::save_hook(harness, &target, schema(draft.layer), draft)
    }

    pub fn set_enabled(
        _: &MuseHarness,
        _: &Path,
        _: SettingsLayer,
        _: &str,
        _: bool,
    ) -> Result<(), String> {
        simple::unsupported_toggle("Muse Code")
    }

    pub fn delete(
        harness: &MuseHarness,
        project: &Path,
        layer: SettingsLayer,
        id: &str,
    ) -> Result<(), String> {
        let target = path(harness, project, layer)?;
        simple::delete_hook(harness, &target, layer, schema(layer), id)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::harness::{claude::config::SettingsLayer, json_store, HookDraft};

    #[test]
    fn settings_and_mcp_preserve_schema_version_and_mask_secrets() {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        fs::create_dir_all(&project).unwrap();
        let harness = MuseHarness::new(home.join(".config/muse"), home.clone());

        config::apply_value(
            &harness,
            &project,
            SettingsLayer::User,
            "env",
            json!({"API_TOKEN": "sk-secret-123"}),
        )
        .unwrap();
        assert!(config::apply_value(
            &harness,
            &project,
            SettingsLayer::Project,
            "model",
            json!("x")
        )
        .is_err());

        let raw = json_store::read_value(&harness.settings_path()).unwrap();
        assert_eq!(raw["schema_version"], 1);
        let snap = config::load_config(&harness, &project).unwrap();
        assert_eq!(snap.layers[0].values["env"]["API_TOKEN"], "••••••••");

        let url = ["https:", "", "mcp.example.test"].join("/");
        mcp::save(
            &harness,
            "remote",
            json!({"type": "http", "url": url, "headers": {"Authorization": "Bearer secret"}, "mode": "optional"}),
        )
        .unwrap();
        let listed = mcp::list(&harness).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, "http");
        assert!(listed[0].enabled);
        assert_eq!(listed[0].entry["headers"]["Authorization"], "••••••••");
        let saved = json_store::read_value(&harness.settings_path()).unwrap();
        assert_eq!(saved["schema_version"], 1);
        assert_eq!(
            saved["mcp_servers"]["remote"]["transport"],
            "streamable_http"
        );

        mcp::set_enabled(&harness, "remote", false).unwrap();
        assert!(!mcp::list(&harness).unwrap()[0].enabled);
        mcp::delete(&harness, "remote").unwrap();
        assert!(mcp::list(&harness).unwrap().is_empty());
    }

    #[test]
    fn skills_memories_and_hooks_follow_native_layout() {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        fs::create_dir_all(home.join(".config/muse/skills/owned")).unwrap();
        fs::create_dir_all(home.join(".agents/skills/shared")).unwrap();
        fs::create_dir_all(project.join(".agents/memory")).unwrap();
        fs::write(
            home.join(".config/muse/skills/owned/SKILL.md"),
            "---\nname: owned\ndescription: Owned skill\n---\nbody",
        )
        .unwrap();
        fs::write(
            home.join(".agents/skills/shared/SKILL.md"),
            "---\nname: shared\ndescription: Shared skill\n---\nbody",
        )
        .unwrap();
        fs::write(project.join("AGENTS.md"), "# Agents").unwrap();
        fs::write(project.join(".agents/memory/MEMORY.md"), "# Memory").unwrap();
        fs::write(project.join(".agents/memory/topic.md"), "# Topic").unwrap();
        let harness = MuseHarness::new(home.join(".config/muse"), home);

        let inv = skills::inventory(&harness, &project).unwrap();
        assert_eq!(inv.len(), 2);
        assert!(inv.iter().find(|s| s.name == "owned").unwrap().editable);
        assert!(!inv.iter().find(|s| s.name == "shared").unwrap().editable);

        let cat = memories::catalog(&harness, &project).unwrap();
        assert_eq!(cat.files.len(), 3);

        hooks::save(
            &harness,
            &project,
            HookDraft {
                layer: SettingsLayer::User,
                event: "PreLLMCall".into(),
                matcher: Some("*".into()),
                command: "echo pre".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .unwrap();
        hooks::save(
            &harness,
            &project,
            HookDraft {
                layer: SettingsLayer::Project,
                event: "PostToolBatch".into(),
                matcher: None,
                command: "echo batch".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .unwrap();
        let list = hooks::list(&harness, &project).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            json_store::read_value(&harness.settings_path()).unwrap()["schema_version"],
            1
        );
        hooks::delete(&harness, &project, SettingsLayer::User, &list[0].id).unwrap();
        assert_eq!(hooks::list(&harness, &project).unwrap().len(), 1);
    }

    #[test]
    fn mcp_save_preserves_masked_credentials_and_rejects_fresh_placeholders() {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let harness = MuseHarness::new(home.join(".config/muse"), home);
        let url = ["https:", "", "mcp.example.test"].join("/");
        mcp::save(
            &harness,
            "remote",
            json!({"type": "http", "url": url, "headers": {"Authorization": "Bearer real-token"}, "mode": "optional"}),
        )
        .unwrap();
        mcp::save(
            &harness,
            "remote",
            json!({"type": "http", "url": url, "headers": {"Authorization": "••••••••"}, "mode": "required"}),
        )
        .unwrap();
        let raw = json_store::read_value(&harness.settings_path()).unwrap();
        assert_eq!(
            raw["mcp_servers"]["remote"]["headers"]["Authorization"],
            "Bearer real-token"
        );
        assert_eq!(raw["mcp_servers"]["remote"]["mode"], "required");
        assert!(mcp::save(
            &harness,
            "fresh",
            json!({"type": "http", "url": url, "headers": {"Authorization": "••••••••"}}),
        )
        .unwrap_err()
        .contains("masked placeholders"));
        assert!(
            json_store::read_value(&harness.settings_path()).unwrap()["mcp_servers"]
                .get("fresh")
                .is_none()
        );
    }
}
