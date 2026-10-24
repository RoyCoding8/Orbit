use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::KiroHarness;
use crate::{
    harness::{
        atomic,
        claude::{
            skills::{self, SkillEntry},
            SkillOverride,
        },
        Harness, ResourceKind,
    },
    ledger,
};

pub fn inventory(harness: &KiroHarness, project: &Path) -> Result<Vec<SkillEntry>, String> {
    let mut entries = BTreeMap::new();
    collect_skills(&harness.home().join("skills"), &mut entries)?;
    if project.is_dir() {
        collect_skills(&harness.project_root(project).join("skills"), &mut entries)?;
    }
    Ok(entries.into_values().collect())
}

pub(crate) fn list_global(harness: &KiroHarness) -> Result<Vec<SkillEntry>, String> {
    let mut entries = BTreeMap::new();
    collect_skills(&harness.home().join("skills"), &mut entries)?;
    Ok(entries.into_values().collect())
}

pub fn set_override(
    _harness: &KiroHarness,
    _project: &Path,
    _id: &str,
    _state: SkillOverride,
) -> Result<(), String> {
    Err("Kiro discovers valid skills automatically and has no native per-skill enable or disable setting".into())
}

pub fn edit_skill(
    harness: &KiroHarness,
    project: &Path,
    id: &str,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    let entry = inventory(harness, project)?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or("skill is no longer available for editing")?;
    if !entry.editable {
        return Err(
            "this Kiro skill is outside an editable global or selected-project skill directory"
                .into(),
        );
    }
    skills::validate_plain_tree(
        entry
            .path
            .parent()
            .ok_or("skill file has no parent directory")?,
    )?;
    ledger::write_text(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "edit",
        &entry.path,
        expected_content,
        content,
    )
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("did not hold the expected state") {
            "Skill changed since this editor opened. Refresh skills and try again.".into()
        } else {
            message
        }
    })
}

pub fn install(harness: &KiroHarness, source: &Path) -> Result<String, String> {
    install_to(harness, &harness.home().join("skills"), source)
}

pub fn install_project(
    harness: &KiroHarness,
    project: &Path,
    source: &Path,
) -> Result<String, String> {
    if !project.is_dir() {
        return Err("project directory is unavailable".into());
    }
    install_to(
        harness,
        &harness.project_root(project).join("skills"),
        source,
    )
}

pub fn uninstall(harness: &KiroHarness, project: &Path, id: &str) -> Result<(), String> {
    let entry = inventory(harness, project)?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or("skill is no longer available for removal")?;
    if !entry.editable {
        return Err(
            "this Kiro skill is outside an editable global or selected-project skill directory"
                .into(),
        );
    }
    let source = entry
        .path
        .parent()
        .ok_or("skill file has no parent directory")?;
    crate::harness::skill_store::uninstall_tree(harness, id, "uninstall", source)
}

pub fn restore(
    harness: &KiroHarness,
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
    if skill_id(&target.join("SKILL.md")) != id {
        return Err("uninstall target does not match the skill identity".into());
    }
    if !owned_target(harness, project, &target) {
        return Err(
            "uninstall target is outside Kiro global and selected-project skill roots".into(),
        );
    }
    skills::ensure_backup_path(harness.config_root(), backup)?;
    skills::ensure_backup_path(harness.config_root(), &recorded_backup)?;
    if skills::canonical_existing_dir(backup)? != skills::canonical_existing_dir(&recorded_backup)?
    {
        return Err("backup is not the recorded uninstall for this skill".into());
    }
    let expected = row
        .before_checksum
        .as_deref()
        .ok_or("uninstall has no checksum")?;
    crate::harness::skill_store::restore_tree(harness, id, "restore", backup, &target, expected)
}

fn install_to(harness: &KiroHarness, root: &Path, source: &Path) -> Result<String, String> {
    skills::ensure_plain_directory(source)?;
    skills::validate_plain_tree(source)?;
    let source_file = source.join("SKILL.md");
    skills::ensure_plain_file_if_present(&source_file)?;
    if !source_file.is_file() {
        return Err("source folder must contain SKILL.md".into());
    }
    let content = skills::read_plain_file(&source_file)?;
    validate_metadata(
        &content,
        source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
    )?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid skill folder name")?;
    validate_component(name)?;
    skills::ensure_plain_parent(&root.join(name))?;
    if !root.exists() {
        std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    }
    skills::ensure_plain_directory(root)?;
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    let destination = root.join(name);
    if skills::path_exists_no_follow(&destination)? {
        return Err(format!("skill already exists: {name}"));
    }
    let id = skill_id(&destination.join("SKILL.md"));
    crate::harness::skill_store::install_tree(harness, &id, "install", source, &destination)?;
    Ok(id)
}

fn collect_skills(root: &Path, out: &mut BTreeMap<PathBuf, SkillEntry>) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    skills::ensure_plain_directory(root)?;
    for entry in std::fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let dir = entry.path();
        skills::ensure_plain_directory(&dir)?;
        let path = dir.join("SKILL.md");
        if !path.is_file() {
            continue;
        }
        skills::validate_plain_tree(&dir)?;
        let content = skills::read_plain_file(&path)?;
        let metadata =
            skills::parse_frontmatter(&content, entry.file_name().to_string_lossy().as_ref());
        let identity = std::fs::canonicalize(&path).unwrap_or(path);
        let id = skill_id(&identity);
        out.entry(identity.clone()).or_insert(SkillEntry {
            id,
            name: metadata.name.unwrap_or_default(),
            description: metadata.description,
            path: identity,
            content,
            state: SkillOverride::On,
            editable: true,
        });
    }
    Ok(())
}

fn validate_metadata(content: &str, folder_name: &str) -> Result<(), String> {
    let metadata = skills::parse_frontmatter(content, folder_name);
    let name = metadata.name.ok_or("skill metadata requires a name")?;
    let description = metadata
        .description
        .ok_or("skill metadata requires a description")?;
    validate_component(&name)?;
    if name != folder_name {
        return Err("skill frontmatter name must match its folder name".into());
    }
    if description.trim().is_empty() || description.chars().count() > 1024 {
        return Err("skill description must contain 1 to 1024 characters".into());
    }
    Ok(())
}

fn validate_component(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Err(
            "Kiro skill names must use lowercase letters, digits, and hyphens, up to 64 characters"
                .into(),
        )
    } else {
        Ok(())
    }
}

fn skill_id(path: &Path) -> String {
    format!(
        "skill-{}",
        atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
    )
}

fn owned_target(harness: &KiroHarness, project: &Path, target: &Path) -> bool {
    let target = target
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok());
    let global = std::fs::canonicalize(harness.home().join("skills")).ok();
    let workspace = if project.is_dir() {
        std::fs::canonicalize(harness.project_root(project).join("skills")).ok()
    } else {
        None
    };
    target.is_some() && (target == global || target == workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Fixture skill\n---\nInstructions.\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn inventory_edit_uninstall_and_restore_use_native_skill_directories() {
        let root = crate::test_support::tempdir().unwrap();
        let home = root.path().join("home/.kiro");
        let harness = KiroHarness::new(home.clone());
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let installed = skill(&root.path().join("source"), "fixture-skill");
        let id = install(&harness, &installed).unwrap();
        let inventory = inventory(&harness, &project).unwrap();
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].id, id);
        edit_skill(&harness, &project, &id, &inventory[0].content, "changed\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join("skills/fixture-skill/SKILL.md")).unwrap(),
            "changed\n"
        );

        uninstall(&harness, &project, &id).unwrap();
        assert!(!home.join("skills/fixture-skill").exists());
        let row = ledger::read_applied(harness.config_root())
            .unwrap()
            .into_iter()
            .find(|row| row.id == id && row.action == "uninstall")
            .unwrap();
        restore(
            &harness,
            &project,
            &id,
            Path::new(row.backup_path.as_deref().unwrap()),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join("skills/fixture-skill/SKILL.md")).unwrap(),
            "changed\n"
        );
    }

    #[test]
    fn native_skill_toggle_is_reported_as_unsupported() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));
        let error = set_override(&harness, root.path(), "fixture", SkillOverride::Off).unwrap_err();
        assert!(error.contains("no native per-skill enable or disable"));
    }
}
