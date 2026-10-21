//! The Claude Code arm of the adapter.
//!
//! Every file access resolves through `CLAUDE_CONFIG_DIR` so a test fixture
//! replaces the real home. At runtime with no override set, the root falls
//! back to `%USERPROFILE%\.claude`.

use std::path::{Path, PathBuf};

pub mod mcp;

use super::{
    Harness, HarnessError, HarnessId, HarnessPaths, ResourceKind, ResourceRef, Result, CLAUDE_CODE,
};

pub mod config;
mod credentials;
pub mod hooks;
mod process_lifetime;
pub mod skills;

/// Environment variable Claude Code honors to relocate its config directory.
pub const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// A setting for controlling how one skill is surfaced by Claude Code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillOverride {
    On,
    NameOnly,
    UserInvocableOnly,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeHarness {
    paths: HarnessPaths,
}

impl ClaudeCodeHarness {
    pub fn new(paths: HarnessPaths) -> Self {
        Self { paths }
    }

    /// Build from the environment, resolving `CLAUDE_CONFIG_DIR` against a
    /// home-directory fallback.
    pub fn from_env() -> Self {
        let override_dir = std::env::var(CONFIG_DIR_ENV).ok();
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_default();
        Self::new(super::resolve_paths(override_dir.as_deref(), &home))
    }

    /// The global state file. Under `CLAUDE_CONFIG_DIR` it lives inside the
    /// override directory; the default layout puts it beside the config dir.
    pub fn claude_json_path(&self) -> PathBuf {
        self.paths.claude_json.clone()
    }
}

impl Harness for ClaudeCodeHarness {
    fn id(&self) -> HarnessId {
        CLAUDE_CODE
    }

    fn config_root(&self) -> &Path {
        &self.paths.config_root
    }

    fn list(&self, kind: ResourceKind) -> Result<Vec<ResourceRef>> {
        match kind {
            ResourceKind::Skill => list_skill_dirs(&self.paths.config_root.join("skills")),
            ResourceKind::Memory => list_project_memories(&self.paths.config_root),
            ResourceKind::GlobalInstructions => {
                let path = self.paths.config_root.join("CLAUDE.md");
                if path.exists() {
                    Ok(vec![ResourceRef {
                        kind: ResourceKind::GlobalInstructions,
                        id: "CLAUDE.md".into(),
                        path,
                    }])
                } else {
                    Ok(Vec::new())
                }
            }
            ResourceKind::Settings => Ok(Vec::new()),
            ResourceKind::McpServer => list_map_keys(
                &self.claude_json_path(),
                "mcpServers",
                ResourceKind::McpServer,
            ),
        }
    }

    fn settings_path(&self) -> PathBuf {
        self.paths.config_root.join("settings.json")
    }
}

/// Every direct child directory of `skills/` holding a `SKILL.md`.
fn list_skill_dirs(skills_root: &Path) -> Result<Vec<ResourceRef>> {
    let entries = match std::fs::read_dir(skills_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(HarnessError::Read {
                path: skills_root.to_path_buf(),
                source: err,
            });
        }
    };

    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| HarnessError::Read {
            path: skills_root.to_path_buf(),
            source: err,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("SKILL.md").is_file() {
            let id = entry.file_name().to_string_lossy().into_owned();
            found.push(ResourceRef {
                kind: ResourceKind::Skill,
                id,
                path,
            });
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

fn list_project_memories(config_root: &Path) -> Result<Vec<ResourceRef>> {
    let projects_root = config_root.join("projects");
    if std::fs::symlink_metadata(&projects_root).is_ok_and(|meta| is_link(&meta)) {
        return Ok(Vec::new());
    }
    let project_entries = match std::fs::read_dir(&projects_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(HarnessError::Read {
                path: projects_root.to_path_buf(),
                source: err,
            })
        }
    };

    let mut found = Vec::new();
    for project in project_entries {
        let Ok(project) = project else { continue };
        let project_dir = project.path();
        match std::fs::symlink_metadata(&project_dir) {
            Ok(meta) if meta.is_dir() && !is_link(&meta) => {}
            _ => continue,
        }
        let slug = project.file_name();
        let memory_dir = project_dir.join("memory");
        match std::fs::symlink_metadata(&memory_dir) {
            Ok(meta) if meta.is_dir() && !is_link(&meta) => {}
            _ => continue,
        }
        let Ok(memories) = list_markdown(&memory_dir, ResourceKind::Memory) else {
            continue;
        };
        for memory in memories {
            found.push(ResourceRef {
                kind: ResourceKind::Memory,
                id: format!("{}/{}", slug.to_string_lossy(), memory.id),
                path: memory.path,
            });
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

/// Every `*.md` file directly inside one directory, as one resource each. A
/// missing directory yields no resources; other read errors surface to the
/// caller, which decides whether to skip or report. An entry that is not a
/// plain file is skipped, so a link resolving outside the tree never enters
/// the inventory.
fn list_markdown(dir: &Path, kind: ResourceKind) -> Result<Vec<ResourceRef>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(HarnessError::Read {
                path: dir.to_path_buf(),
                source: err,
            })
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(_) => continue,
        };
        let is_plain_file = std::fs::symlink_metadata(&path)
            .map(|meta| meta.is_file() && !is_link(&meta))
            .unwrap_or(false);
        if is_plain_file && path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            found.push(ResourceRef {
                kind,
                id: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path,
            });
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// The keys of one top-level map in a JSON document, as one resource per key.
fn list_map_keys(path: &Path, map_key: &str, kind: ResourceKind) -> Result<Vec<ResourceRef>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(HarnessError::Read {
                path: path.to_path_buf(),
                source: err,
            })
        }
    };
    let doc: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|source| HarnessError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    let keys = doc
        .get(map_key)
        .and_then(|map| map.as_object())
        .map(|map| map.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(keys
        .into_iter()
        .map(|id| ResourceRef {
            kind,
            id,
            path: path.to_path_buf(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    /// Build a temp fixture standing in for `$HOME`: the config root lives at
    /// `<tmp>/claude/` and the state file beside it at `<tmp>/.claude.json`,
    /// mirroring the real default layout.
    struct Fixture {
        _dir: tempfile::TempDir,
        home: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().to_path_buf();
            fs::create_dir_all(home.join("claude")).unwrap();
            Self { _dir: dir, home }
        }

        fn paths(&self) -> HarnessPaths {
            HarnessPaths {
                config_root: self.home.join("claude"),
                claude_json: self.home.join(".claude.json"),
            }
        }

        fn harness(&self) -> ClaudeCodeHarness {
            ClaudeCodeHarness::new(self.paths())
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.home.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, contents).unwrap();
        }
    }

    fn skill_fixture(root: &Path, name: &str) {
        fs::create_dir_all(root.join("skills").join(name)).unwrap();
        fs::write(
            root.join("skills").join(name).join("SKILL.md"),
            "---\nname: x\n---\n",
        )
        .unwrap();
    }

    #[test]
    fn from_env_prefers_claude_config_dir_over_userprofile() {
        let fixture = Fixture::new();
        let override_dir = fixture
            .home
            .join("claude-overridden")
            .to_string_lossy()
            .into_owned();
        let paths = super::super::resolve_paths(Some(override_dir.as_str()), &fixture.home);
        assert_eq!(paths.config_root, fixture.home.join("claude-overridden"));
        assert_eq!(
            paths.claude_json,
            fixture.home.join("claude-overridden/.claude.json"),
            "the state file must live inside the override, not beside it"
        );
    }

    #[test]
    fn skills_listing_reports_each_skill_directory_with_its_own_path() {
        let fixture = Fixture::new();
        skill_fixture(&fixture.home.join("claude"), "alpha");
        skill_fixture(&fixture.home.join("claude"), "beta");
        fs::create_dir_all(
            fixture
                .home
                .join("claude")
                .join("skills")
                .join("not-a-skill"),
        )
        .unwrap();
        fs::write(
            fixture.home.join("claude").join("skills").join("loose.md"),
            "x",
        )
        .unwrap();

        let refs = fixture.harness().list(ResourceKind::Skill).unwrap();
        let ids: Vec<_> = refs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["alpha", "beta"]);
        assert!(refs[0].path.join("SKILL.md").is_file());
        assert_eq!(refs[0].kind, ResourceKind::Skill);
    }

    #[test]
    fn mcp_servers_listing_reads_the_sibling_claude_json() {
        let fixture = Fixture::new();
        fixture.write(
            ".claude.json",
            &serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "cx-agent": {"command": "redacted-cmd", "args": []},
                    "obscura": {"command": "redacted-cmd"}
                },
                "oauthAccount": {"redacted": "value"}
            }))
            .unwrap(),
        );

        let refs = fixture.harness().list(ResourceKind::McpServer).unwrap();
        let ids: Vec<_> = refs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["cx-agent", "obscura"]);
        assert!(refs
            .iter()
            .all(|r| r.path == fixture.harness().claude_json_path()));
    }

    #[test]
    fn memories_aggregate_per_project_with_project_qualified_ids() {
        let fixture = Fixture::new();
        fixture.write(
            "claude/projects/D--AI-rea-testing/memory/MEMORY.md",
            "# index\n",
        );
        fixture.write(
            "claude/projects/D--AI-rea-testing/memory/notes.md",
            "notes\n",
        );
        fixture.write("claude/projects/other-repo/memory/MEMORY.md", "# other\n");

        let refs = fixture.harness().list(ResourceKind::Memory).unwrap();
        let ids: Vec<_> = refs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "D--AI-rea-testing/MEMORY.md",
                "D--AI-rea-testing/notes.md",
                "other-repo/MEMORY.md"
            ],
            "same-named files in different projects must stay distinct"
        );
        assert_eq!(
            refs[0].path,
            fixture
                .home
                .join("claude/projects/D--AI-rea-testing/memory/MEMORY.md")
        );
    }

    #[test]
    fn memory_files_outside_the_projects_tree_are_not_inventoried() {
        let fixture = Fixture::new();
        fixture.write("claude/projects/D--AI/memory/MEMORY.md", "# real\n");
        fixture.write("claude/memory/MEMORY.md", "# wrong location\n");
        fixture.write("claude/projects/D--AI/memory/other.txt", "not markdown\n");
        fixture.write("claude/projects/D--AI/memory/nested/deep.md", "too deep\n");

        let refs = fixture.harness().list(ResourceKind::Memory).unwrap();
        let ids: Vec<_> = refs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            ["D--AI/MEMORY.md"],
            "the legacy top-level memory dir, non-markdown files, and \
             nested subdirectories are not memories"
        );
    }

    #[test]
    fn markdown_named_directory_does_not_hide_valid_memories() {
        let fixture = Fixture::new();
        fixture.write("claude/projects/D--AI/memory/MEMORY.md", "# index\n");
        fs::create_dir_all(fixture.home.join("claude/projects/D--AI/memory/poison.md")).unwrap();
        fixture.write("claude/projects/other/memory/notes.md", "notes\n");

        let refs = fixture.harness().list(ResourceKind::Memory).unwrap();
        let ids: Vec<_> = refs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            ["D--AI/MEMORY.md", "other/notes.md"],
            "the poisoned entry is skipped, the valid ones are not blanked"
        );
    }

    #[test]
    fn non_directory_project_entries_are_skipped() {
        let fixture = Fixture::new();
        fixture.write("claude/projects/D--AI/memory/MEMORY.md", "# index\n");
        fixture.write("claude/projects/not-a-dir", "x\n");

        let refs = fixture.harness().list(ResourceKind::Memory).unwrap();
        let ids: Vec<_> = refs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["D--AI/MEMORY.md"]);
    }

    #[cfg(any(unix, windows))]
    fn link_directory(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let link = link.to_string_lossy().replace('/', "\\");
            let target = target.to_string_lossy().replace('/', "\\");
            let result = std::process::Command::new("cmd")
                .args(["/D", "/C", "mklink", "/J", &link, &target])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn linked_project_and_memory_directories_are_not_followed() {
        for relative in ["projects", "projects/linked", "projects/linked/memory"] {
            let fixture = Fixture::new();
            fixture.write("outside/foreign/memory/root.md", "outside root");
            fixture.write("outside/memory/project.md", "outside project");
            fixture.write("outside/leaf.md", "outside memory");
            let link = fixture.home.join("claude").join(relative);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            link_directory(&fixture.home.join("outside"), &link);

            assert!(
                fixture
                    .harness()
                    .list(ResourceKind::Memory)
                    .unwrap()
                    .is_empty(),
                "followed {relative}"
            );
        }
    }

    #[test]
    fn empty_or_missing_directories_yield_no_resources_not_errors() {
        let fixture = Fixture::new();
        for kind in [
            ResourceKind::Skill,
            ResourceKind::Memory,
            ResourceKind::GlobalInstructions,
        ] {
            assert!(
                fixture.harness().list(kind).unwrap().is_empty(),
                "{kind:?} should be empty"
            );
        }
    }

    #[test]
    fn settings_and_state_paths_resolve_inside_the_fixture() {
        let fixture = Fixture::new();
        let harness = fixture.harness();
        assert_eq!(
            harness.settings_path(),
            fixture.home.join("claude/settings.json")
        );
        assert_eq!(
            harness.claude_json_path(),
            fixture.home.join(".claude.json")
        );
        assert_eq!(harness.id(), CLAUDE_CODE);
    }

    /// The default layout keeps the state file beside the config root; an
    /// override moves both inside the override, and a poison sibling of the
    /// override is never touched.
    #[test]
    fn override_state_file_lives_inside_the_override_not_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let override_dir = home.join("claude-alt");
        fs::create_dir_all(&override_dir).unwrap();
        fs::write(
            home.join(".claude.json"),
            r#"{"mcpServers":{"outside":{"command":"outside"}}}"#,
        )
        .unwrap();
        fs::write(
            override_dir.join(".claude.json"),
            r#"{"mcpServers":{"inside":{"command":"inside"}}}"#,
        )
        .unwrap();

        let harness =
            ClaudeCodeHarness::new(super::super::resolve_paths(override_dir.to_str(), home));
        assert_eq!(
            harness.claude_json_path(),
            override_dir.join(".claude.json")
        );
        assert_eq!(harness.config_root(), override_dir.as_path());
        let resources = harness.list(ResourceKind::McpServer).unwrap();
        assert_eq!(
            resources
                .iter()
                .map(|resource| resource.id.as_str())
                .collect::<Vec<_>>(),
            ["inside"]
        );

        let default_harness = ClaudeCodeHarness::new(super::super::resolve_paths(None, home));
        assert_eq!(
            default_harness.claude_json_path(),
            home.join(".claude.json")
        );
        let resources = default_harness.list(ResourceKind::McpServer).unwrap();
        assert_eq!(
            resources
                .iter()
                .map(|resource| resource.id.as_str())
                .collect::<Vec<_>>(),
            ["outside"]
        );
    }

    /// The trait contract: a full resource walk over one fixture tree resolves
    /// only inside the fixture, never outside it, and stays self-consistent.
    #[test]
    fn every_listed_resource_path_lives_inside_the_fixture_root() {
        let fixture = Fixture::new();
        skill_fixture(&fixture.home.join("claude"), "alpha");
        fixture.write("claude/projects/D--AI/memory/MEMORY.md", "# index\n");
        fixture.write("claude/CLAUDE.md", "# global\n");
        fixture.write(
            ".claude.json",
            &serde_json::to_string(&json!({"mcpServers": {"cx-agent": {"command": "x"}}, "projects": {"D:/fixture": {"allowedTools": []}}})).unwrap(),
        );

        let harness = fixture.harness();
        for kind in [
            ResourceKind::Skill,
            ResourceKind::Memory,
            ResourceKind::GlobalInstructions,
            ResourceKind::McpServer,
        ] {
            let refs = harness
                .list(kind)
                .unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            for r in refs {
                assert!(
                    r.path.starts_with(&fixture.home),
                    "{kind:?} resource escaped the fixture: {}",
                    r.path.display()
                );
            }
        }
    }

    #[test]
    fn settings_json_with_unknown_keys_round_trips_without_losing_them() {
        let fixture = Fixture::new();
        let settings = json!({
            "model": "sonnet",
            "hooks": {"Stop": [{"type": "command", "command": "echo hi", "timeout": 600}]},
            "enabledPlugins": {"code-review@anthropic": true},
            "skillOverrides": {"alpha": "off"}
        });
        fixture.write(
            "claude/settings.json",
            &serde_json::to_string_pretty(&settings).unwrap(),
        );

        let path = fixture.harness().settings_path();
        assert_eq!(super::super::atomic::read_json(&path).unwrap(), settings);

        super::super::atomic::commit(
            &path,
            vec![super::super::atomic::Patch::SetPath {
                path: super::super::atomic::p!["skillOverrides"],
                value: json!({"alpha": "on", "beta": "off"}),
            }],
            None,
        )
        .unwrap();
        let after = super::super::atomic::read_json(&path).unwrap();
        assert_eq!(after["model"], json!("sonnet"));
        assert_eq!(after["hooks"], settings["hooks"]);
        assert_eq!(after["enabledPlugins"], settings["enabledPlugins"]);
        assert_eq!(
            after["skillOverrides"],
            json!({"alpha": "on", "beta": "off"})
        );
    }

    #[test]
    fn account_state_is_excluded_from_mcp_listing() {
        let fixture = Fixture::new();
        fixture.write(
            ".claude.json",
            &serde_json::to_string(
                &json!({"oauthAccount": {"email": "x"}, "mcpServers": {"a": {}}}),
            )
            .unwrap(),
        );
        let refs = fixture.harness().list(ResourceKind::McpServer).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].id, "a");
    }
}
