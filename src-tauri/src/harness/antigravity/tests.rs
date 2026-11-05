use super::*;
use crate::{
    harness::{claude::config::SettingsLayer, json_store, HookDraft},
    history,
};
use serde_json::json;
use std::fs;

fn write(path: impl AsRef<Path>, bytes: &str) {
    fs::create_dir_all(path.as_ref().parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn config_mcp_and_grouped_hooks_keep_native_shapes_and_restorable_history() {
    let temp = crate::test_support::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let h = AntigravityHarness::new(temp.path().to_owned());
    write(
        h.settings_path(),
        r#"{"theme":"dark","unrelated":{"enabled":true}}"#,
    );
    config::apply_value(&h, &project, SettingsLayer::User, "theme", json!("light")).unwrap();
    assert_eq!(config::load_config(&h, &project).unwrap().layers.len(), 1);
    config::undo(&h, &project, SettingsLayer::User, "theme").unwrap();
    assert_eq!(
        json_store::read_value(&h.settings_path()).unwrap()["theme"],
        "dark"
    );
    assert!(config::apply_value(
        &h,
        &project,
        SettingsLayer::Project,
        "theme",
        json!("light")
    )
    .is_err());
    write(
        h.mcp_path(),
        r#"{"mcpServers":{"docs":{"serverUrl":"https://example.test/mcp?api_key=private","headers":{"Authorization":"Bearer secret"},"timeoutSeconds":19}}}"#,
    );
    let listed = mcp::list(&h).unwrap();
    assert!(!listed[0].entry.to_string().contains("private"));
    let mut edited = listed[0].entry.clone();
    let url = edited.as_object_mut().unwrap().remove("serverUrl").unwrap();
    edited["url"] = url;
    edited["timeoutSeconds"] = json!(23);
    mcp::save(&h, "docs", edited).unwrap();
    let saved = json_store::read_value(&h.mcp_path()).unwrap();
    assert_eq!(
        saved["mcpServers"]["docs"]["url"],
        "https://example.test/mcp?api_key=private"
    );
    assert_eq!(
        saved["mcpServers"]["docs"]["headers"]["Authorization"],
        "Bearer secret"
    );
    assert_eq!(saved["mcpServers"]["docs"]["timeoutSeconds"], 23);
    let hook_path = h.config_root().join("hooks.json");
    write(
        &hook_path,
        r#"{"team/shared":{"PostToolUse":[{"matcher":"run_command","hooks":[{"type":"command","command":"first","timeout":10},{"type":"command","command":"second"}]}]}}"#,
    );
    let hooks = hooks::list(&h, &project).unwrap();
    hooks::save(
        &h,
        &project,
        HookDraft {
            layer: SettingsLayer::User,
            event: "PostToolUse".into(),
            matcher: Some("run_command".into()),
            command: "edited".into(),
            edit_id: Some(hooks[0].id.clone()),
            source_id: None,
        },
    )
    .unwrap();
    let doc = json_store::read_value(&hook_path).unwrap();
    assert_eq!(
        doc["team/shared"]["PostToolUse"][0]["hooks"][0],
        json!({"type":"command","command":"edited","timeout":10})
    );
    assert_eq!(
        doc["team/shared"]["PostToolUse"][0]["hooks"][1]["command"],
        "second"
    );
    let hooks = hooks::list(&h, &project).unwrap();
    hooks::set_enabled(&h, &project, SettingsLayer::User, &hooks[0].id, false).unwrap();
    assert!(hooks::list(&h, &project)
        .unwrap()
        .iter()
        .all(|hook| !hook.enabled));
    hooks::delete(&h, &project, SettingsLayer::User, &hooks[0].id).unwrap();
    assert_eq!(
        hooks::list(&h, &project).unwrap()[0].commands,
        vec!["second"]
    );
    let versions = history::load(&h, &project).unwrap();
    let version = versions
        .versions
        .iter()
        .find(|v| v.target == hook_path && v.restorable)
        .unwrap();
    history::restore(
        &h,
        &project,
        &version.id,
        version.current_checksum.as_deref(),
    )
    .unwrap();
    assert_eq!(hooks::list(&h, &project).unwrap().len(), 2);
}

#[test]
fn discovery_resolves_manifests_and_filters_and_keeps_inherited_files_readonly() {
    let temp = crate::test_support::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let repo = temp.path().join("repo");
    let project = repo.join("nested");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&project).unwrap();
    let h = AntigravityHarness::new(home);
    let skill = "---\nname: fixture\ndescription: Fixture skill\n---\nContent";
    write(repo.join(".agents/skills/ancestor/SKILL.md"), skill);
    write(project.join("_agent/skills/local/SKILL.md"), skill);
    write(h.config_root().join("skills/hidden/SKILL.md"), skill);
    write(h.home().join(".gemini/skills/hidden/SKILL.md"), skill);
    let shared = temp.path().join("shared");
    write(
        shared.join(".agents/skills.json"),
        r#"{"entries":[{"path":"skills"}]}"#,
    );
    write(shared.join("skills/keep/SKILL.md"), skill);
    write(shared.join("skills/drop/SKILL.md"), skill);
    write(project.join(".agents/skills.json"),&json!({"exclude":["hidden"],"inherits":[{"path":shared.join(".agents/skills.json"),"include_only":["keep"]}]}).to_string());
    let inventory = skills::inventory(&h, &project).unwrap();
    let folders = inventory
        .iter()
        .map(|entry| {
            entry
                .path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(folders.len(), 4);
    for expected in ["ancestor", "local", "keep", "hidden"] {
        assert!(folders.iter().any(|name| name == expected));
    }
    let local = inventory
        .iter()
        .find(|s| s.path.ends_with("local/SKILL.md"))
        .unwrap();
    assert!(local.editable);
    let inherited = inventory
        .iter()
        .find(|s| s.path.ends_with("keep/SKILL.md"))
        .unwrap();
    assert!(!inherited.editable);
    assert!(skills::edit_skill(&h, &project, &inherited.id, skill, "edited").is_err());
    let rule = "---\ntrigger: manual\n---\nApply this explicitly.";
    write(project.join(".agents/rules/testing.md"), rule);
    write(
        project.join("AGENTS.md"),
        "Instructions [testing](.agents/rules/testing.md)",
    );
    let catalog = memories::catalog(&h, &project).unwrap();
    let rule_file = catalog
        .files
        .iter()
        .find(|f| f.name == "testing.md")
        .unwrap();
    assert_eq!(
        rule_file.load_behavior,
        "Loaded when explicitly requested in Antigravity"
    );
    memories::delete(&h, &project, &rule_file.id, rule).unwrap();
    assert!(!project.join(".agents/rules/testing.md").exists());
    assert!(!fs::read_to_string(project.join("AGENTS.md"))
        .unwrap()
        .contains("]("));
    assert!(history::load(&h, &project)
        .unwrap()
        .versions
        .iter()
        .any(|v| v.restorable));
}
