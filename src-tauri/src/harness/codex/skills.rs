use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table};

use super::CodexHarness;
use crate::{
    harness::{atomic, claude::skills::SkillEntry, claude::SkillOverride, Harness, ResourceKind},
    ledger,
};

pub fn inventory(harness: &CodexHarness, project: &Path) -> Result<Vec<SkillEntry>, String> {
    let project = canonical_directory(project)?;
    let response = super::app_server::request(
        harness,
        &project,
        "skills/list",
        json!({"cwds": [project.to_string_lossy()], "forceReload": true}),
    )?;
    map_native_inventory(harness, &project, &response)
}

fn map_native_inventory(
    harness: &CodexHarness,
    project: &Path,
    response: &Value,
) -> Result<Vec<SkillEntry>, String> {
    let data = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Codex returned an invalid skills/list response")?;
    let project_entry = data
        .iter()
        .find(|entry| {
            entry
                .get("cwd")
                .and_then(Value::as_str)
                .is_some_and(|cwd| same_path(Path::new(cwd), project))
        })
        .ok_or("Codex returned no skill inventory for the selected project")?;
    let skills = project_entry
        .get("skills")
        .and_then(Value::as_array)
        .ok_or("Codex returned an invalid skill list")?;
    let mut entries = BTreeMap::<PathBuf, SkillEntry>::new();
    for native in skills {
        let (Some(name), Some(description), Some(path), Some(enabled), Some(scope)) = (
            native.get("name").and_then(Value::as_str),
            native.get("description").and_then(Value::as_str),
            native.get("path").and_then(Value::as_str),
            native.get("enabled").and_then(Value::as_bool),
            native.get("scope").and_then(Value::as_str),
        ) else {
            continue;
        };
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.file_name().is_some_and(|name| name == "SKILL.md") {
            continue;
        }
        let identity = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let content = read_skill_content(&path);
        let content_readable = content.is_some();
        let plugin_owned = native
            .get("pluginId")
            .is_some_and(|plugin_id| !plugin_id.is_null());
        entries
            .entry(identity.clone())
            .or_insert_with(|| SkillEntry {
                id: skill_id(&identity),
                name: name.to_owned(),
                description: Some(description.to_owned()),
                path: path.clone(),
                content: content.unwrap_or_default(),
                state: if enabled {
                    SkillOverride::On
                } else {
                    SkillOverride::Off
                },
                editable: content_readable
                    && !plugin_owned
                    && editable_path(harness, project, &path, scope),
            });
    }
    Ok(entries.into_values().collect())
}

fn read_skill_content(path: &Path) -> Option<String> {
    if !plain_file(path) {
        return None;
    }
    crate::harness::claude::skills::read_plain_file(path).ok()
}

fn editable_path(harness: &CodexHarness, project: &Path, target: &Path, scope: &str) -> bool {
    match scope {
        "repo" => path_is_within(target, &project.join(".agents").join("skills")),
        "user" => {
            path_is_within(target, &harness.home().join(".agents").join("skills"))
                || (path_is_within(target, &harness.config_root().join("skills"))
                    && !path_is_within(
                        target,
                        &harness.config_root().join("skills").join(".system"),
                    ))
        }
        _ => false,
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

pub fn set_override(
    harness: &CodexHarness,
    project: &Path,
    id: &str,
    state: SkillOverride,
) -> Result<(), String> {
    let enabled = match state {
        SkillOverride::On => true,
        SkillOverride::Off => false,
        SkillOverride::NameOnly | SkillOverride::UserInvocableOnly => {
            return Err("Codex supports only enabled and disabled skill states".into())
        }
    };
    let entry = inventory(harness, project)?
        .into_iter()
        .find(|skill| skill.id == id)
        .ok_or("skill is no longer available")?;
    let path = entry.path;
    super::store::mutate(
        harness,
        ResourceKind::Settings,
        id,
        "override",
        &harness.settings_path(),
        move |document| set_config_enabled(document, &path, enabled),
    )
    .map_err(|error| format!("failed updating Codex skill settings: {error}"))?;
    Ok(())
}

pub fn edit_skill(
    harness: &CodexHarness,
    project: &Path,
    id: &str,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    let entry = inventory(harness, project)?
        .into_iter()
        .find(|skill| skill.id == id)
        .ok_or("skill is no longer available for editing")?;
    if !entry.editable {
        return Err("this Codex skill is managed by the system and cannot be edited here".into());
    }
    edit_skill_target(harness, id, &entry.path, expected_content, content)
}

fn edit_skill_target(
    harness: &CodexHarness,
    id: &str,
    path: &Path,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    let skill_dir = path.parent().ok_or("skill file has no parent directory")?;
    crate::harness::claude::skills::validate_plain_tree(skill_dir)?;
    let backup = ledger::backup_path(harness.config_root(), id, "edit");
    let expected = Sha256::digest(expected_content.as_bytes()).into();
    ledger::write(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "edit",
        path,
        atomic::RecordedWrite::Restore(expected, content.as_bytes()),
        Some(&backup),
    )
    .map(|_| ())
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("did not hold the expected state") {
            format!("Skill {id} changed since this editor opened. Refresh skills and try again.")
        } else {
            message
        }
    })
}

pub fn install(harness: &CodexHarness, source: &Path) -> Result<String, String> {
    crate::harness::claude::skills::ensure_plain_directory(source)?;
    crate::harness::claude::skills::validate_plain_tree(source)?;
    let source_file = source.join("SKILL.md");
    crate::harness::claude::skills::ensure_plain_file_if_present(&source_file)?;
    if !source_file.is_file() {
        return Err("source folder must contain SKILL.md".into());
    }
    let content = crate::harness::claude::skills::read_plain_file(&source_file)?;
    validate_codex_metadata(&content)?;

    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid skill folder name")?;
    validate_component(name)?;
    let root = harness.home().join(".agents").join("skills");
    if !root.exists() {
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    }
    crate::harness::claude::skills::ensure_plain_directory(&root)?;
    let root = canonical_directory(&root)?;
    let destination = root.join(name);
    crate::harness::claude::skills::ensure_plain_parent(&destination)?;
    if crate::harness::claude::skills::path_exists_no_follow(&destination)? {
        return Err(format!("skill already exists: {name}"));
    }

    let id = skill_id(&identity_path(&destination.join("SKILL.md"))?);
    let guard = atomic::lock_mutations(&destination).map_err(|error| error.to_string())?;
    if crate::harness::claude::skills::path_exists_no_follow(&destination)? {
        return Err(format!("skill already exists: {name}"));
    }
    let checksum = crate::harness::claude::skills::safe_directory_checksum(source)?;
    let plan = atomic::MutationPlan {
        target: destination.clone(),
        backup: None,
        before_checksum: None,
        after_checksum: Some(checksum.clone()),
    };
    let op = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        &id,
        "install",
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    if let Err(error) = crate::harness::claude::skills::copy_dir(source, &destination) {
        return Err(format!(
            "reserved install {op} failed; inspect the partial destination at {}: {error}",
            destination.display()
        ));
    }
    let installed = crate::harness::claude::skills::safe_directory_checksum(&destination)?;
    if installed != checksum {
        return Err(format!(
            "reserved install {op} checksum differs from source; inspect the destination at {}",
            destination.display()
        ));
    }
    ledger::complete_mutation(harness.config_root(), &op, &guard)
        .map_err(|error| error.to_string())?;
    Ok(id)
}

pub fn uninstall(harness: &CodexHarness, project: &Path, id: &str) -> Result<(), String> {
    let entry = inventory(harness, project)?
        .into_iter()
        .find(|skill| skill.id == id)
        .ok_or("skill is no longer available for removal")?;
    if !entry.editable {
        return Err("this Codex skill is managed by the system and cannot be removed here".into());
    }
    let source = entry
        .path
        .parent()
        .ok_or("skill file has no parent directory")?
        .to_path_buf();
    uninstall_target(harness, id, &source)
}

fn uninstall_target(harness: &CodexHarness, id: &str, source: &Path) -> Result<(), String> {
    crate::harness::claude::skills::validate_plain_tree(source)?;
    let backup = ledger::backup_path(harness.config_root(), id, "uninstall");
    crate::harness::claude::skills::ensure_plain_parent(&backup)?;
    if crate::harness::claude::skills::path_exists_no_follow(&backup)? {
        return Err("skill uninstall backup already exists".into());
    }
    std::fs::create_dir_all(backup.parent().ok_or("backup path has no parent")?)
        .map_err(|error| error.to_string())?;
    let guard = atomic::lock_mutations(source).map_err(|error| error.to_string())?;
    let checksum = crate::harness::claude::skills::safe_directory_checksum(source)?;
    let plan = atomic::MutationPlan {
        target: source.to_path_buf(),
        backup: Some(backup.clone()),
        before_checksum: Some(checksum.clone()),
        after_checksum: None,
    };
    let op = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "uninstall",
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(source, &backup)
        .map_err(|error| format!("reserved uninstall {op} did not complete: {error}"))?;
    let moved = crate::harness::claude::skills::safe_directory_checksum(&backup)?;
    if moved != checksum {
        return Err(format!(
            "reserved uninstall {op} backup checksum differs from source"
        ));
    }
    ledger::complete_mutation(harness.config_root(), &op, &guard).map_err(|error| error.to_string())
}

pub fn restore(
    harness: &CodexHarness,
    project: &Path,
    id: &str,
    backup: &Path,
) -> Result<(), String> {
    let row = ledger::read_applied(harness.config_root())?
        .into_iter()
        .rev()
        .find(|row| row.resource_kind == "skill" && row.id == id && row.action == "uninstall")
        .ok_or_else(|| format!("no completed uninstall is available to restore skill {id}"))?;
    let target = PathBuf::from(
        row.target
            .as_deref()
            .ok_or("uninstall has no target path")?,
    );
    let recorded_backup = PathBuf::from(
        row.backup_path
            .as_deref()
            .ok_or("uninstall has no backup path")?,
    );
    if skill_id(&skill_file_path(&target)?) != id {
        return Err("uninstall target does not match the skill identity".into());
    }
    if !is_owned_target(harness, project, &target)? {
        return Err(
            "uninstall target is outside Codex user and selected-project skill roots".into(),
        );
    }
    crate::harness::claude::skills::ensure_backup_path(harness.config_root(), backup)?;
    crate::harness::claude::skills::ensure_backup_path(harness.config_root(), &recorded_backup)?;
    if crate::harness::claude::skills::canonical_existing_dir(backup)?
        != crate::harness::claude::skills::canonical_existing_dir(&recorded_backup)?
    {
        return Err("backup is not the recorded uninstall for this skill".into());
    }
    let expected = row
        .before_checksum
        .as_deref()
        .ok_or("uninstall has no checksum")?;
    let actual = crate::harness::claude::skills::safe_directory_checksum(backup)?;
    ledger::verify_directory_checksum(&actual, expected)
        .map_err(|error| format!("cannot restore skill {id}: {error}"))?;
    crate::harness::claude::skills::ensure_plain_parent(&target)?;
    if crate::harness::claude::skills::path_exists_no_follow(&target)? {
        return Err("skill already exists at its original path".into());
    }
    let guard = atomic::lock_mutations(&target).map_err(|error| error.to_string())?;
    if crate::harness::claude::skills::path_exists_no_follow(&target)? {
        return Err("skill already exists at its original path".into());
    }
    let plan = atomic::MutationPlan {
        target: target.clone(),
        backup: Some(recorded_backup.clone()),
        before_checksum: None,
        after_checksum: Some(expected.to_owned()),
    };
    let op = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "restore",
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    crate::harness::claude::skills::copy_dir(backup, &target).map_err(|error| {
        format!("reserved restore {op} failed; inspect the partial destination: {error}")
    })?;
    let restored = crate::harness::claude::skills::safe_directory_checksum(&target)?;
    ledger::verify_directory_checksum(&restored, expected)
        .map_err(|error| format!("reserved restore {op} failed verification: {error}"))?;
    ledger::complete_mutation(harness.config_root(), &op, &guard).map_err(|error| error.to_string())
}

fn set_config_enabled(
    document: &mut DocumentMut,
    skill_file: &Path,
    enabled: bool,
) -> Result<(), String> {
    let skills = document
        .entry("skills")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or("Codex skills settings must be a table")?;
    let config = skills
        .entry("config")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    let desired = if enabled { None } else { Some(false) };
    let mut matched = false;
    match config {
        Item::ArrayOfTables(entries) => {
            for entry in entries.iter_mut() {
                if config_entry_matches(entry.get("path").and_then(Item::as_str), skill_file) {
                    matched = true;
                    match desired {
                        Some(value) => {
                            entry["enabled"] = value.into();
                        }
                        None => {
                            entry.remove("enabled");
                        }
                    }
                }
            }
        }
        Item::Value(toml_edit::Value::Array(entries)) => {
            for entry in entries.iter_mut() {
                let path = entry
                    .as_inline_table()
                    .and_then(|table| table.get("path"))
                    .and_then(toml_edit::Value::as_str);
                if config_entry_matches(path, skill_file) {
                    matched = true;
                    let table = entry
                        .as_inline_table_mut()
                        .ok_or("skills.config entries must be tables")?;
                    match desired {
                        Some(value) => {
                            table.insert("enabled", value.into());
                        }
                        None => {
                            table.remove("enabled");
                        }
                    }
                }
            }
        }
        _ => return Err("skills.config must be an array of tables".into()),
    }
    if !matched && !enabled {
        let mut entry = InlineTable::new();
        entry.insert("path", skill_file.to_string_lossy().to_string().into());
        entry.insert("enabled", false.into());
        match config {
            Item::ArrayOfTables(entries) => {
                let mut table = Table::new();
                table["path"] = skill_file.to_string_lossy().to_string().into();
                table["enabled"] = false.into();
                entries.push(table);
            }
            Item::Value(toml_edit::Value::Array(entries)) => {
                entries.push(toml_edit::Value::InlineTable(entry))
            }
            _ => return Err("skills.config must be an array of tables".into()),
        }
    }
    Ok(())
}

fn config_entry_matches(path: Option<&str>, skill_file: &Path) -> bool {
    path.and_then(skill_config_path)
        .is_some_and(|configured| same_path(&configured, skill_file))
}

fn skill_config_path(path: &str) -> Option<PathBuf> {
    let path = PathBuf::from(path);
    path.file_name()
        .is_some_and(|name| name == "SKILL.md")
        .then(|| std::fs::canonicalize(&path).ok())
        .flatten()
}

fn validate_codex_metadata(content: &str) -> Result<(), String> {
    let metadata = crate::harness::claude::skills::parse_frontmatter(content, "");
    if metadata.name.as_deref().is_none_or(str::is_empty)
        || metadata.description.as_deref().is_none_or(str::is_empty)
    {
        return Err("Codex skills require nonempty name and description frontmatter".into());
    }
    Ok(())
}

fn skill_id(skill_file: &Path) -> String {
    format!(
        "skill-{}",
        atomic::hex(&Sha256::digest(skill_file.to_string_lossy().as_bytes()))
    )
}

fn identity_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("skill file has no parent directory")?;
    if parent.exists() {
        let canonical = std::fs::canonicalize(parent).map_err(|error| error.to_string())?;
        Ok(canonical.join(path.file_name().ok_or("skill file has no name")?))
    } else {
        Ok(path.to_path_buf())
    }
}

fn skill_file_path(skill_dir: &Path) -> Result<PathBuf, String> {
    let parent = skill_dir
        .parent()
        .ok_or("skill directory has no parent directory")?;
    let name = skill_dir.file_name().ok_or("skill directory has no name")?;
    Ok(std::fs::canonicalize(parent)
        .map_err(|error| error.to_string())?
        .join(name)
        .join("SKILL.md"))
}

fn is_owned_target(harness: &CodexHarness, project: &Path, target: &Path) -> Result<bool, String> {
    let project = canonical_directory(project)?;
    let target = if target.exists() {
        std::fs::canonicalize(target).map_err(|error| error.to_string())?
    } else {
        let parent = target
            .parent()
            .ok_or("skill target has no parent directory")?;
        std::fs::canonicalize(parent)
            .map_err(|error| error.to_string())?
            .join(target.file_name().ok_or("skill target has no name")?)
    };
    let roots = [
        harness.home().join(".agents").join("skills"),
        harness.config_root().join("skills"),
        project.join(".agents").join("skills"),
    ];
    Ok(roots.iter().enumerate().any(|(index, root)| {
        path_is_within(&target, root)
            && (index != 1 || !path_is_within(&target, &root.join(".system")))
    }))
}

fn validate_component(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', ':'])
        || value.chars().any(char::is_control)
    {
        return Err("invalid skill folder name".into());
    }
    Ok(())
}

fn plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !atomic::is_link_or_reparse(&metadata))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || atomic::is_link_or_reparse(&metadata) {
        return Err(format!("expected a plain directory: {}", path.display()));
    }
    std::fs::canonicalize(path).map_err(|error| error.to_string())
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_skill(root: &Path, directory: &str, name: &str, description: &str) -> PathBuf {
        let skill = root.join(directory);
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nInstructions.\n"),
        )
        .unwrap();
        skill
    }

    fn native_skill(
        path: &Path,
        name: &str,
        scope: &str,
        enabled: bool,
        plugin: Option<&str>,
    ) -> Value {
        json!({
            "name": name,
            "description": "native description",
            "path": path.to_string_lossy(),
            "enabled": enabled,
            "scope": scope,
            "pluginId": plugin,
        })
    }

    #[test]
    fn native_inventory_preserves_duplicate_names_and_limits_editing_to_owned_paths() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let codex_home = temp.path().join("codex-home");
        let repo = temp.path().join("repo");
        let project = repo.join("packages/app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&codex_home).unwrap();
        let project_skill = write_skill(
            &project.join(".agents/skills"),
            "writer",
            "writer",
            "project copy",
        );
        let ancestor_skill = write_skill(
            &repo.join(".agents/skills"),
            "shared",
            "writer",
            "ancestor copy",
        );
        let user_skill = write_skill(
            &home.join(".agents/skills"),
            "writer",
            "writer",
            "user copy",
        );
        let legacy_skill = write_skill(
            &codex_home.join("skills"),
            "legacy",
            "legacy",
            "legacy copy",
        );
        let system_skill = write_skill(
            &codex_home.join("skills/.system"),
            "diagnostics",
            "diagnostics",
            "system copy",
        );
        let plugin_skill = write_skill(
            &home.join(".agents/skills"),
            "plugin-copy",
            "writer",
            "plugin copy",
        );
        let admin_skill = write_skill(
            &temp.path().join("admin/skills"),
            "policy",
            "policy",
            "admin copy",
        );
        let harness = CodexHarness::new(codex_home, home);
        let paths = [
            native_skill(
                &project_skill.join("SKILL.md"),
                "writer",
                "repo",
                true,
                None,
            ),
            native_skill(
                &ancestor_skill.join("SKILL.md"),
                "writer",
                "repo",
                true,
                None,
            ),
            native_skill(&user_skill.join("SKILL.md"), "writer", "user", false, None),
            native_skill(&legacy_skill.join("SKILL.md"), "legacy", "user", true, None),
            native_skill(
                &system_skill.join("SKILL.md"),
                "diagnostics",
                "system",
                true,
                None,
            ),
            native_skill(
                &plugin_skill.join("SKILL.md"),
                "writer",
                "user",
                true,
                Some("plugin-id"),
            ),
            native_skill(&admin_skill.join("SKILL.md"), "policy", "admin", true, None),
        ];
        let response = json!({"data":[{"cwd":project.to_string_lossy(),"errors":[{"message":"one malformed skill was skipped"}],"skills":paths}]});
        let skills = map_native_inventory(&harness, &project, &response).unwrap();
        assert_eq!(skills.len(), 7);
        let find = |path: &Path| {
            skills
                .iter()
                .find(|skill| same_path(&skill.path, path))
                .unwrap()
        };
        let project_entry = find(&project_skill.join("SKILL.md"));
        let ancestor_entry = find(&ancestor_skill.join("SKILL.md"));
        let user_entry = find(&user_skill.join("SKILL.md"));
        let legacy_entry = find(&legacy_skill.join("SKILL.md"));
        let system_entry = find(&system_skill.join("SKILL.md"));
        let plugin_entry = find(&plugin_skill.join("SKILL.md"));
        let admin_entry = find(&admin_skill.join("SKILL.md"));
        assert_ne!(project_entry.id, user_entry.id);
        assert_eq!(project_entry.name, user_entry.name);
        assert_eq!(user_entry.state, SkillOverride::Off);
        assert!(project_entry.editable);
        assert!(!ancestor_entry.editable);
        assert!(user_entry.editable);
        assert!(legacy_entry.editable);
        assert!(!system_entry.editable);
        assert!(!plugin_entry.editable);
        assert!(!admin_entry.editable);
        assert!(project_entry.content.contains("project copy"));
    }

    #[test]
    fn native_skill_overrides_match_only_the_skill_md_path() {
        let temp = tempdir().unwrap();
        let skill = write_skill(temp.path(), "writer", "writer", "fixture");
        let skill_file = skill.join("SKILL.md");
        let folder = skill.to_string_lossy().replace('\\', "\\\\");
        let mut document = format!("[[skills.config]]\npath = \"{folder}\"\nenabled = false\n")
            .parse::<DocumentMut>()
            .unwrap();
        set_config_enabled(&mut document, &skill_file, false).unwrap();
        {
            let entries = document["skills"]["config"].as_array_of_tables().unwrap();
            assert_eq!(entries.len(), 2);
            let mut entries = entries.iter();
            let folder_entry = entries.next().unwrap();
            let skill_entry = entries.next().unwrap();
            assert_eq!(folder_entry["enabled"].as_bool(), Some(false));
            assert_eq!(
                skill_entry["path"].as_str(),
                Some(skill_file.to_str().unwrap())
            );
            assert_eq!(skill_entry["enabled"].as_bool(), Some(false));
        }
        set_config_enabled(&mut document, &skill_file, true).unwrap();
        let entries = document["skills"]["config"].as_array_of_tables().unwrap();
        let mut entries = entries.iter();
        let folder_entry = entries.next().unwrap();
        let skill_entry = entries.next().unwrap();
        assert_eq!(folder_entry["enabled"].as_bool(), Some(false));
        assert!(skill_entry.get("enabled").is_none());
    }

    #[test]
    fn install_edit_uninstall_and_restore_use_codex_user_skill_root() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("repo");
        let source = write_skill(
            &temp.path().join("source"),
            "sample",
            "sample",
            "A sample skill",
        );
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let harness = CodexHarness::new(home.clone(), home.clone());
        let id = install(&harness, &source).unwrap();
        let installed = home.join(".agents/skills/sample/SKILL.md");
        assert!(installed.is_file());
        let content = std::fs::read_to_string(&installed).unwrap();
        edit_skill_target(
            &harness,
            &id,
            &installed,
            &content,
            &content.replace("Instructions.", "Updated."),
        )
        .unwrap();
        assert!(
            edit_skill_target(&harness, &id, &installed, &content, "stale content")
                .unwrap_err()
                .contains("changed since")
        );
        uninstall_target(&harness, &id, installed.parent().unwrap()).unwrap();
        let backup = ledger::latest_applied_row(
            harness.config_root(),
            ResourceKind::Skill,
            &id,
            "uninstall",
            installed.parent().unwrap(),
        )
        .unwrap()
        .unwrap()
        .backup_path
        .map(PathBuf::from)
        .unwrap();
        restore(&harness, &project, &id, &backup).unwrap();
        assert!(installed.is_file());
        assert!(std::fs::read_to_string(installed)
            .unwrap()
            .contains("Updated."));
    }
}
