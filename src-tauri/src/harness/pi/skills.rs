use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::PiHarness;
use crate::{
    harness::{
        atomic,
        claude::skills::SkillEntry,
        claude::{self, SkillOverride},
        json_store, Harness, ResourceKind,
    },
    ledger,
};

pub fn inventory(harness: &PiHarness, project: &Path) -> Result<Vec<SkillEntry>, String> {
    let mut roots = vec![(harness.config_root().join("skills"), true)];
    if project.is_dir() {
        roots.push((harness.project_root(project).join("skills"), true));
        roots.push((harness.home().join(".agents/skills"), false));
        for ancestor in project_ancestors(project) {
            roots.push((ancestor.join(".agents/skills"), false));
        }
        for path in configured_paths(harness, project)? {
            roots.push((path, false));
        }
    } else {
        roots.push((harness.home().join(".agents/skills"), false));
    }
    let mut entries = BTreeMap::new();
    for (root, editable_root) in roots {
        collect(&root, editable_root, &mut entries, 0)?;
    }
    Ok(entries.into_values().collect())
}

pub(crate) fn list_global(harness: &PiHarness) -> Result<Vec<SkillEntry>, String> {
    let mut entries = BTreeMap::new();
    collect(&harness.config_root().join("skills"), true, &mut entries, 0)?;
    Ok(entries.into_values().collect())
}

pub fn set_override(
    _harness: &PiHarness,
    _project: &Path,
    _id: &str,
    _state: SkillOverride,
) -> Result<(), String> {
    Err("Pi discovers valid skills automatically and has no native per-skill enable or disable setting".into())
}

pub fn edit_skill(
    harness: &PiHarness,
    project: &Path,
    id: &str,
    expected: &str,
    content: &str,
) -> Result<(), String> {
    let entry = inventory(harness, project)?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or("skill is no longer available for editing")?;
    if !entry.editable {
        return Err(
            "this Pi skill is outside an editable global or selected-project skill directory"
                .into(),
        );
    }
    claude::skills::ensure_plain_components(&entry.path)?;
    claude::skills::ensure_plain_file_if_present(&entry.path)?;
    ledger::write_text(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "edit",
        &entry.path,
        expected,
        content,
    )
    .map_err(stale_skill)
}

pub fn install(harness: &PiHarness, source: &Path) -> Result<String, String> {
    claude::skills::ensure_plain_components(source)?;
    if source.is_file()
        && source
            .extension()
            .is_some_and(|extension| extension == "md")
    {
        let content = claude::skills::read_plain_file(source)?;
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid skill filename")?;
        validate_name(name)?;
        let target = harness.config_root().join("skills").join(name);
        claude::skills::ensure_plain_parent(&target)?;
        std::fs::create_dir_all(target.parent().ok_or("skill path has no parent")?)
            .map_err(|error| error.to_string())?;
        let target = target
            .parent()
            .unwrap()
            .canonicalize()
            .map_err(|error| error.to_string())?
            .join(name);
        let id = skill_id(&target);
        ledger::write(
            harness.config_root(),
            ResourceKind::Skill,
            &id,
            "install",
            &target,
            atomic::RecordedWrite::Create(content.as_bytes()),
            None,
        )
        .map_err(|error| error.to_string())?;
        return Ok(id);
    }
    claude::skills::ensure_plain_directory(source)?;
    claude::skills::validate_plain_tree(source)?;
    let skill_file = source.join("SKILL.md");
    if !skill_file.is_file() {
        return Err("source folder must contain SKILL.md".into());
    }
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid skill folder name")?;
    validate_name(name)?;
    let root = harness.config_root().join("skills");
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    claude::skills::ensure_plain_directory(&root)?;
    let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
    let target = root.join(name);
    claude::skills::ensure_plain_parent(&target)?;
    let id = skill_id(&target.join("SKILL.md"));
    crate::harness::skill_store::install_tree(harness, &id, "install", source, &target)?;
    Ok(id)
}

pub fn uninstall(harness: &PiHarness, project: &Path, id: &str) -> Result<(), String> {
    let entry = inventory(harness, project)?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or("skill is no longer available for removal")?;
    if !entry.editable {
        return Err(
            "this Pi skill is outside an editable global or selected-project skill directory"
                .into(),
        );
    }
    if entry.path.file_name().is_none_or(|name| name != "SKILL.md") {
        return json_store::delete_file(harness, ResourceKind::Skill, id, "uninstall", &entry.path);
    }
    let source = entry
        .path
        .parent()
        .ok_or("skill file has no parent directory")?
        .to_path_buf();
    crate::harness::skill_store::uninstall_tree(harness, id, "uninstall", &source)
}

pub fn restore(harness: &PiHarness, project: &Path, id: &str, backup: &Path) -> Result<(), String> {
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
    let recorded = PathBuf::from(
        row.backup_path
            .as_deref()
            .ok_or("uninstall has no backup path")?,
    );
    let file_backup = !row
        .before_checksum
        .as_deref()
        .is_some_and(|checksum| checksum.starts_with("tree-v2-sha256:"));
    let identity = if file_backup {
        target.clone()
    } else {
        target.join("SKILL.md")
    };
    if skill_id(&identity) != id || !allowed_skill_path(harness, project, &identity) {
        return Err(
            "uninstall target is outside Pi global and selected-project skill roots".into(),
        );
    }
    let backup_root = ledger::backup_root(harness.config_root())
        .canonicalize()
        .map_err(|error| error.to_string())?;
    for path in [backup, recorded.as_path()] {
        claude::skills::ensure_plain_components(path)?;
        if path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .as_ref()
            != Some(&backup_root)
        {
            return Err("backup is outside the Pi backup directory".into());
        }
    }
    if std::fs::canonicalize(backup).map_err(|error| error.to_string())?
        != std::fs::canonicalize(&recorded).map_err(|error| error.to_string())?
    {
        return Err("backup is not the recorded uninstall for this skill".into());
    }
    let expected = row
        .before_checksum
        .as_deref()
        .ok_or("uninstall has no checksum")?;
    if file_backup {
        let content = claude::skills::read_plain_file(backup)?;
        if atomic::hex(&Sha256::digest(content.as_bytes())) != expected {
            return Err("saved skill file checksum does not match".into());
        }
        return ledger::write(
            harness.config_root(),
            ResourceKind::Skill,
            id,
            "restore",
            &target,
            atomic::RecordedWrite::Create(content.as_bytes()),
            None,
        )
        .map(|_| ())
        .map_err(|error| error.to_string());
    }
    crate::harness::skill_store::restore_tree(harness, id, "restore", backup, &target, expected)
}

pub(crate) fn allowed_skill_path(harness: &PiHarness, project: &Path, target: &Path) -> bool {
    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        project.join(target)
    };
    if claude::skills::ensure_plain_components(&target).is_err()
        || target
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return false;
    }
    let mut roots = vec![harness.config_root().join("skills")];
    if project.is_dir() {
        let Ok(project) = std::fs::canonicalize(project) else {
            return false;
        };
        roots.push(project.join(".pi/skills"));
    }
    roots.into_iter().any(|root| {
        let target = canonical_or_deleted(&target);
        let root = canonical_or_deleted(&root);
        target.strip_prefix(root).is_ok_and(|relative| {
            let parts: Vec<_> = relative.components().collect();
            parts
                .iter()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
                && !parts.is_empty()
        })
    })
}

fn collect(
    root: &Path,
    editable: bool,
    entries: &mut BTreeMap<PathBuf, SkillEntry>,
    depth: usize,
) -> Result<(), String> {
    if depth > 24 || !root.exists() {
        return Ok(());
    }
    claude::skills::ensure_plain_components(root)?;
    if root.is_file() {
        if root.extension().is_some_and(|extension| extension == "md") {
            add_skill(root, editable, entries)?;
        }
        return Ok(());
    }
    claude::skills::ensure_plain_directory(root)?;
    let skill_file = root.join("SKILL.md");
    if skill_file.is_file() {
        add_skill(&skill_file, editable, entries)?;
        return Ok(());
    }
    for item in std::fs::read_dir(root).map_err(|error| error.to_string())? {
        let item = item.map_err(|error| error.to_string())?;
        let path = item.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.is_dir() && !atomic::is_link_or_reparse(&metadata) {
            collect(&path, editable, entries, depth + 1)?;
        } else if depth == 0
            && metadata.is_file()
            && !atomic::is_link_or_reparse(&metadata)
            && path.extension().is_some_and(|extension| extension == "md")
        {
            add_skill(&path, editable, entries)?;
        }
    }
    Ok(())
}

fn add_skill(
    path: &Path,
    editable: bool,
    entries: &mut BTreeMap<PathBuf, SkillEntry>,
) -> Result<(), String> {
    let content = claude::skills::read_plain_file(path)?;
    let fallback = if path.file_name().is_some_and(|name| name == "SKILL.md") {
        path.parent().and_then(Path::file_name)
    } else {
        path.file_stem()
    }
    .unwrap_or_default()
    .to_string_lossy();
    let meta = claude::skills::parse_frontmatter(&content, &fallback);
    if meta
        .description
        .as_deref()
        .is_none_or(|description| description.trim().is_empty())
    {
        return Ok(());
    }
    let identity = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    entries.entry(identity.clone()).or_insert(SkillEntry {
        id: skill_id(&identity),
        name: meta.name.unwrap_or_default(),
        description: meta.description,
        path: identity,
        content,
        state: SkillOverride::On,
        editable,
    });
    Ok(())
}

fn configured_paths(harness: &PiHarness, project: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for (settings, base) in [
        (harness.settings_path(), harness.config_root().to_path_buf()),
        (harness.project_settings_path(project), project.join(".pi")),
    ] {
        let value = crate::harness::json_store::read_value(&settings)?;
        paths.extend(
            value
                .get("skills")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .filter(|value| {
                    !value.contains('*')
                        && !value.contains('?')
                        && !value.starts_with('!')
                        && !value.contains("${")
                })
                .map(|value| {
                    let path = PathBuf::from(value);
                    if path.is_absolute() {
                        path
                    } else {
                        base.join(path)
                    }
                }),
        );
    }
    Ok(paths)
}

fn project_ancestors(project: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut cursor = Some(project);
    let mut found_git = false;
    while let Some(path) = cursor {
        dirs.push(path.to_path_buf());
        if path.join(".git").exists() {
            found_git = true;
            break;
        }
        cursor = path.parent();
    }
    if !found_git {
        dirs.truncate(1);
    }
    dirs.reverse();
    dirs
}

fn skill_id(path: &Path) -> String {
    format!(
        "skill-{}",
        atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
    )
}

fn canonical_or_deleted(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 128
        || name.bytes().any(|byte| byte == b'/' || byte == b'\\')
        || name == "."
        || name == ".."
    {
        Err(
            "Pi skill names must be a single non-empty path component of at most 128 characters"
                .into(),
        )
    } else {
        Ok(())
    }
}

fn stale_skill(error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    if message.contains("did not hold the expected state") {
        "Skill changed since this editor opened. Refresh skills and try again.".into()
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_skill_can_be_edited_uninstalled_and_restored_from_its_backup() {
        for flat in [false, true] {
            let root = crate::test_support::tempdir().unwrap();
            let harness = PiHarness::new(root.path().join(".pi/agent"), root.path().join("home"));
            let project = root.path().join("project");
            std::fs::create_dir_all(&project).unwrap();
            let source = root.path().join("source/example");
            std::fs::create_dir_all(&source).unwrap();
            let file = if flat {
                source.join("example.md")
            } else {
                source.join("SKILL.md")
            };
            std::fs::write(
                &file,
                "---\nname: example\ndescription: fixture\n---\noriginal\n",
            )
            .unwrap();

            let id = install(&harness, if flat { &file } else { &source }).unwrap();
            let entry = inventory(&harness, &project)
                .unwrap()
                .into_iter()
                .find(|entry| entry.id == id)
                .unwrap();
            assert_eq!(entry.name, "example");
            let updated = "---\nname: example\ndescription: Changed fixture\n---\nchanged\n";
            edit_skill(&harness, &project, &id, &entry.content, updated).unwrap();
            let stale = edit_skill(&harness, &project, &id, &entry.content, "wrong\n").unwrap_err();
            assert!(stale.contains("changed since this editor opened"));

            uninstall(&harness, &project, &id).unwrap();
            assert!(inventory(&harness, &project).unwrap().is_empty());
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
            let restored = inventory(&harness, &project).unwrap();
            assert_eq!(restored[0].content, updated);
        }
    }

    #[test]
    fn history_allowlist_accepts_native_skill_targets_and_rejects_traversal() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = PiHarness::new(root.path().join(".pi/agent"), root.path().join("home"));
        let project = root.path().join("project");
        std::fs::create_dir_all(project.join(".pi/skills")).unwrap();
        let skill = project.join(".pi/skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        let file = skill.join("SKILL.md");
        std::fs::write(&file, "instructions").unwrap();
        assert!(allowed_skill_path(&harness, &project, &file));
        assert!(allowed_skill_path(&harness, &project, &skill));
        assert!(!allowed_skill_path(
            &harness,
            &project,
            &project.join("README.md")
        ));
        assert!(!allowed_skill_path(
            &harness,
            &project,
            &project.join(".pi/skills/../settings.json")
        ));
    }

    #[test]
    fn project_and_custom_skill_roots_are_visible_with_only_native_roots_editable() {
        use serde_json::json;

        let root = crate::test_support::tempdir().unwrap();
        let home = root.path().join("home");
        let harness = PiHarness::new(home.join(".pi/agent"), home.clone());
        let project = root.path().join("project");
        std::fs::create_dir_all(project.join(".pi")).unwrap();
        std::fs::create_dir_all(project.join(".pi/skills/project-skill")).unwrap();
        std::fs::create_dir_all(home.join(".agents/skills/custom-skill")).unwrap();
        std::fs::create_dir_all(root.path().join("configured/configured-skill")).unwrap();
        std::fs::write(project.join(".pi/skills/README.md"), "Skill documentation").unwrap();
        std::fs::create_dir_all(harness.config_root()).unwrap();
        std::fs::write(
            harness.settings_path(),
            serde_json::to_vec(
                &json!({"skills":[root.path().join("configured/configured-skill").to_string_lossy()]}),
            )
            .unwrap(),
        )
        .unwrap();
        for (folder, name) in [
            (project.join(".pi/skills/project-skill"), "project-skill"),
            (home.join(".agents/skills/custom-skill"), "custom-skill"),
            (
                root.path().join("configured/configured-skill"),
                "configured-skill",
            ),
        ] {
            std::fs::write(
                folder.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: fixture\n---\n"),
            )
            .unwrap();
        }
        let entries = inventory(&harness, &project).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .find(|entry| entry.name == "project-skill")
                .unwrap()
                .editable
        );
        assert!(
            !entries
                .iter()
                .find(|entry| entry.name == "custom-skill")
                .unwrap()
                .editable
        );
        assert!(
            !entries
                .iter()
                .find(|entry| entry.name == "configured-skill")
                .unwrap()
                .editable
        );
    }
}
