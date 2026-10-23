use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use super::OpenCodeHarness;
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

/// Enumerate OpenCode V2's documented skill roots and grouped skill folders.
pub fn inventory(harness: &OpenCodeHarness, project: &Path) -> Result<Vec<SkillEntry>, String> {
    let mut entries = BTreeMap::<String, SkillEntry>::new();
    let scope = project_scope(project)?;
    let selected = scope
        .last()
        .ok_or("project path resolved to no directories")?;
    for (folder, global_label, project_label) in [
        (".claude", "global-claude", "project-claude"),
        (".agents", "global-agents", "project-agents"),
    ] {
        collect_root(
            &harness.home().join(folder).join("skills"),
            global_label,
            true,
            &mut entries,
        )?;
        for directory in &scope {
            let root = directory.join(folder).join("skills");
            let path_scope = directory.to_string_lossy().replace(['\\', ':'], "_");
            collect_root(
                &root,
                &format!("{project_label}-{path_scope}"),
                directory == selected,
                &mut entries,
            )?;
        }
    }
    collect_root(
        &harness.config_root().join("skills"),
        "global-opencode",
        true,
        &mut entries,
    )?;
    for directory in &scope {
        let path_scope = directory.to_string_lossy().replace(['\\', ':'], "_");
        collect_root(
            &directory.join(".opencode/skills"),
            &format!("project-opencode-{path_scope}"),
            directory == selected,
            &mut entries,
        )?;
    }
    Ok(entries.into_values().collect())
}

fn collect_root(
    root: &Path,
    label: &str,
    editable: bool,
    entries: &mut BTreeMap<String, SkillEntry>,
) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    if skills::ensure_plain_components(root).is_err() {
        return Ok(());
    }
    crate::harness::claude::skills::ensure_plain_directory(root)?;
    collect_root_markdown(root, label, editable, entries)?;
    collect_directories(root, root, label, editable, 0, entries)
}

fn collect_root_markdown(
    root: &Path,
    label: &str,
    editable: bool,
    entries: &mut BTreeMap<String, SkillEntry>,
) -> Result<(), String> {
    let mut files = fs::read_dir(root)
        .map_err(|error| format!("failed reading {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    files.sort_by_key(|entry| entry.file_name());
    for entry in files {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.')
            || Path::new(&name).extension().and_then(|ext| ext.to_str()) != Some("md")
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
            continue;
        }
        let content = skills::read_plain_file(&path)?;
        let skill_name = Path::new(&name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("skill filename is not valid UTF-8")?;
        let id = format!("{label}/{name}");
        let frontmatter = skills::parse_frontmatter(&content, skill_name);
        entries.insert(
            id.clone(),
            SkillEntry {
                id,
                name: frontmatter.name.unwrap_or_else(|| skill_name.to_owned()),
                description: frontmatter.description,
                path,
                content,
                state: SkillOverride::On,
                editable,
            },
        );
    }
    Ok(())
}

fn collect_directories(
    root: &Path,
    directory: &Path,
    label: &str,
    editable: bool,
    depth: usize,
    entries: &mut BTreeMap<String, SkillEntry>,
) -> Result<(), String> {
    let skill_file = directory.join("SKILL.md");
    if depth > 0 && skill_file.is_file() {
        let content = skills::read_plain_file(&skill_file)?;
        let relative = directory
            .strip_prefix(root)
            .map_err(|_| "skill path escaped its root")?;
        let suffix = if relative.as_os_str().is_empty() {
            String::new()
        } else {
            format!("/{}", relative.to_string_lossy().replace('\\', "/"))
        };
        let id = format!("{label}{suffix}");
        let fallback_name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| id.clone());
        let frontmatter = skills::parse_frontmatter(&content, &fallback_name);
        entries.insert(
            id.clone(),
            SkillEntry {
                id,
                name: frontmatter.name.unwrap_or_else(|| fallback_name.clone()),
                description: frontmatter.description,
                path: skill_file,
                content,
                state: SkillOverride::On,
                editable,
            },
        );
        return Ok(());
    }
    if depth >= 6 {
        return Ok(());
    }
    let directories = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("failed reading {}: {error}", directory.display())),
    };
    for entry in directories {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|error| error.to_string())?;
        if atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() {
            continue;
        }
        collect_directories(root, &child, label, editable, depth + 1, entries)?;
    }
    Ok(())
}

fn project_scope(project: &Path) -> Result<Vec<PathBuf>, String> {
    skills::ensure_plain_directory(project)?;
    let selected = project.canonicalize().map_err(|error| error.to_string())?;
    let mut directories = Vec::new();
    let mut current = selected.as_path();
    loop {
        directories.push(current.to_path_buf());
        let marker = current.join(".git");
        if fs::symlink_metadata(&marker).is_ok() {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    directories.reverse();
    Ok(directories)
}

/// OpenCode V2 has permission rules for tools but no native per-skill toggle.
pub fn set_override(
    _harness: &OpenCodeHarness,
    _project: &Path,
    _id: &str,
    state: SkillOverride,
) -> Result<(), String> {
    if state == SkillOverride::On {
        Ok(())
    } else {
        Err("OpenCode V2 does not expose a per-skill enabled state".into())
    }
}

pub fn edit_skill(
    harness: &OpenCodeHarness,
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
        return Err("inherited project skills are read-only outside the selected project".into());
    }
    let path = entry.path;
    skills::ensure_plain_components(&path)?;
    skills::ensure_plain_file_if_present(&path)?;
    ledger::write_text(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "opencode-skill-edit",
        &path,
        expected_content,
        content,
    )
    .map_err(|error| error.to_string())
}

pub fn install(harness: &OpenCodeHarness, source: &Path) -> Result<String, String> {
    skills::ensure_plain_components(source)?;
    let source_metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if !atomic::is_link_or_reparse(&source_metadata)
        && source_metadata.is_file()
        && source.extension().and_then(|value| value.to_str()) == Some("md")
    {
        return install_markdown(harness, source);
    }
    skills::ensure_plain_directory(source)?;
    skills::validate_plain_tree(source)?;
    let skill_file = source.join("SKILL.md");
    skills::ensure_plain_file_if_present(&skill_file)?;
    if !skill_file.is_file() {
        return Err("source folder must contain SKILL.md".into());
    }
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid skill folder name")?;
    validate_name(name)?;
    let destination = harness.config_root().join("skills").join(name);
    if skills::path_exists_no_follow(&destination)? {
        return Err(format!("skill already exists: {name}"));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::harness::skill_store::install_tree(
        harness,
        name,
        "opencode-skill-install",
        source,
        &destination,
    )?;
    Ok(name.to_owned())
}

fn install_markdown(harness: &OpenCodeHarness, source: &Path) -> Result<String, String> {
    skills::ensure_plain_file_if_present(source)?;
    let name = source
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("invalid skill filename")?;
    validate_name(name)?;
    let destination = harness
        .config_root()
        .join("skills")
        .join(format!("{name}.md"));
    skills::ensure_plain_parent(&destination)?;
    if skills::path_exists_no_follow(&destination)? {
        return Err(format!("skill already exists: {name}"));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let contents = fs::read(source).map_err(|error| error.to_string())?;
    let checksum = atomic::hex(&Sha256::digest(&contents));
    let guard = atomic::lock_mutations(&destination).map_err(|error| error.to_string())?;
    if skills::path_exists_no_follow(&destination)? {
        return Err(format!("skill already exists: {name}"));
    }
    let plan = atomic::MutationPlan {
        target: destination.clone(),
        backup: None,
        before_checksum: None,
        after_checksum: Some(checksum.clone()),
    };
    let op = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        name,
        "opencode-skill-install",
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| format!("reserved install {op} could not create destination: {error}"))?;
    use std::io::Write;
    output
        .write_all(&contents)
        .map_err(|error| format!("reserved install {op} failed writing destination: {error}"))?;
    output
        .sync_all()
        .map_err(|error| format!("reserved install {op} failed syncing destination: {error}"))?;
    let actual = atomic::hex(&Sha256::digest(
        fs::read(&destination).map_err(|error| error.to_string())?,
    ));
    if actual != checksum {
        return Err(format!(
            "reserved install {op} checksum differs from source"
        ));
    }
    ledger::complete_mutation(harness.config_root(), &op, &guard)
        .map_err(|error| error.to_string())?;
    Ok(name.to_owned())
}

pub fn uninstall(harness: &OpenCodeHarness, project: &Path, id: &str) -> Result<(), String> {
    let entry = inventory(harness, project)?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or("skill is no longer available for removal")?;
    if !entry.editable {
        return Err("inherited project skills are read-only outside the selected project".into());
    }
    let source = if is_flat_markdown(&entry.path) {
        entry.path.clone()
    } else {
        entry
            .path
            .parent()
            .ok_or("skill file has no parent directory")?
            .to_path_buf()
    };
    let source_metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
    if atomic::is_link_or_reparse(&source_metadata) {
        return Err("refusing to remove a linked skill".into());
    }
    if source_metadata.is_dir() {
        return crate::harness::skill_store::uninstall_tree(
            harness,
            id,
            "opencode-skill-uninstall",
            &source,
        );
    } else if source_metadata.is_file() {
        skills::ensure_plain_file_if_present(&source)?;
    } else {
        return Err("skill is not a regular file or directory".into());
    }
    let backup = ledger::backup_path(harness.config_root(), id, "opencode-skill-uninstall");
    skills::ensure_plain_parent(&backup)?;
    if skills::path_exists_no_follow(&backup)? {
        return Err("skill uninstall backup already exists".into());
    }
    fs::create_dir_all(backup.parent().ok_or("backup path has no parent")?)
        .map_err(|error| error.to_string())?;
    let guard = atomic::lock_mutations(&source).map_err(|error| error.to_string())?;
    let checksum = checksum_path(&source)?;
    let plan = atomic::MutationPlan {
        target: source.clone(),
        backup: Some(backup.clone()),
        before_checksum: Some(checksum.clone()),
        after_checksum: None,
    };
    let op = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "opencode-skill-uninstall",
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(&source, &backup)
        .map_err(|error| format!("reserved uninstall {op} did not complete: {error}"))?;
    if checksum_path(&backup)? != checksum {
        return Err(format!(
            "reserved uninstall {op} backup checksum differs from the source"
        ));
    }
    ledger::complete_mutation(harness.config_root(), &op, &guard).map_err(|error| error.to_string())
}

pub fn restore(
    harness: &OpenCodeHarness,
    project: &Path,
    id: &str,
    backup: &Path,
) -> Result<(), String> {
    let backup_metadata = fs::symlink_metadata(backup).map_err(|error| error.to_string())?;
    if atomic::is_link_or_reparse(&backup_metadata)
        || !(backup_metadata.is_file() || backup_metadata.is_dir())
    {
        return Err("skill backup is not a plain file or directory".into());
    }
    if backup_metadata.is_dir() {
        skills::ensure_backup_path(harness.config_root(), backup)?;
        skills::validate_plain_tree(backup)?;
    } else {
        ensure_backup_file_path(harness.config_root(), backup)?;
        skills::ensure_plain_file_if_present(backup)?;
    }
    if id.trim().is_empty() || id.chars().any(char::is_control) {
        return Err("invalid skill identity".into());
    }
    let row = ledger::read_applied(harness.config_root())?
        .into_iter()
        .rev()
        .find(|row| {
            row.resource_kind == "skill" && row.id == id && row.action == "opencode-skill-uninstall"
        })
        .ok_or_else(|| format!("no completed uninstall is available to restore skill {id}"))?;
    let expected_checksum = row
        .before_checksum
        .as_deref()
        .ok_or("skill uninstall has no recorded source checksum")?;
    let recorded_backup = row
        .backup_path
        .as_deref()
        .ok_or("uninstall has no backup path")?;
    if Path::new(recorded_backup) != backup {
        return Err("skill backup does not match the completed uninstall record".into());
    }
    let destination = Path::new(
        row.target
            .as_deref()
            .ok_or("uninstall has no target path")?,
    )
    .to_path_buf();
    if !allowed_skill_path(harness, project, &destination) {
        return Err("recorded skill target is outside the OpenCode skill roots".into());
    }
    if skills::path_exists_no_follow(&destination)? {
        return Err(format!("skill already exists: {id}"));
    }
    skills::ensure_plain_parent(&destination)?;
    let checksum = checksum_path(backup)?;
    if checksum != expected_checksum {
        return Err("skill backup does not match the completed uninstall checksum".into());
    }
    let guard = atomic::lock_mutations(&destination).map_err(|error| error.to_string())?;
    let plan = atomic::MutationPlan {
        target: destination.clone(),
        backup: None,
        before_checksum: None,
        after_checksum: Some(checksum.clone()),
    };
    let op = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "opencode-skill-restore",
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    let copy_result = if backup_metadata.is_dir() {
        skills::copy_dir(backup, &destination)
    } else {
        copy_plain_file(backup, &destination)
    };
    if let Err(error) = copy_result {
        return Err(format!(
            "reserved restore {op} failed; inspect {}: {error}",
            destination.display()
        ));
    }
    if checksum_path(&destination)? != checksum {
        return Err(format!(
            "reserved restore {op} checksum differs from its backup"
        ));
    }
    ledger::complete_mutation(harness.config_root(), &op, &guard).map_err(|error| error.to_string())
}

pub(crate) fn allowed_skill_path(harness: &OpenCodeHarness, project: &Path, target: &Path) -> bool {
    if skills::ensure_plain_components(target).is_err() {
        return false;
    }
    let Some(normalized) = resolved_or_deleted_path(target) else {
        return false;
    };
    let Ok(scope) = project_scope(project) else {
        return false;
    };
    let Some(selected) = scope.last() else {
        return false;
    };
    let roots = [
        harness.config_root().join("skills"),
        harness.home().join(".claude/skills"),
        harness.home().join(".agents/skills"),
        selected.join(".opencode/skills"),
        selected.join(".claude/skills"),
        selected.join(".agents/skills"),
    ];
    roots.iter().any(|root| {
        if crate::harness::claude::skills::ensure_plain_directory(root).is_err() {
            return false;
        }
        let Ok(root) = root.canonicalize() else {
            return false;
        };
        normalized.starts_with(&root)
            && normalized.strip_prefix(&root).is_ok_and(|relative| {
                let components = relative.components().collect::<Vec<_>>();
                ((components.len() == 1 && normalized.extension().is_some_and(|ext| ext == "md"))
                    || ((2..=7).contains(&components.len()) && normalized.file_name().is_some_and(|name| name == "SKILL.md"))
                    || ((1..=6).contains(&components.len()) && !normalized.is_file()))
                    && components.iter().all(|component| matches!(component, std::path::Component::Normal(name) if !name.to_string_lossy().starts_with('.') && *name != std::ffi::OsStr::new("node_modules")))
            })
    })
}

fn resolved_or_deleted_path(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return path.canonicalize().ok();
    }
    let parent = path.parent()?.canonicalize().ok()?;
    Some(parent.join(path.file_name()?))
}

fn is_flat_markdown(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("md")
        && (path.file_name().and_then(|name| name.to_str()) != Some("SKILL.md")
            || path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                == Some("skills"))
}

fn checksum_path(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if atomic::is_link_or_reparse(&metadata) {
        return Err(format!("refusing linked skill path: {}", path.display()));
    }
    if metadata.is_dir() {
        skills::safe_directory_checksum(path)
    } else if metadata.is_file() {
        Ok(atomic::hex(&Sha256::digest(
            fs::read(path).map_err(|error| error.to_string())?,
        )))
    } else {
        Err(format!("unsupported skill path: {}", path.display()))
    }
}

fn copy_plain_file(source: &Path, destination: &Path) -> Result<(), String> {
    skills::ensure_plain_file_if_present(source)?;
    let contents = fs::read(source).map_err(|error| error.to_string())?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| error.to_string())?;
    use std::io::Write;
    output
        .write_all(&contents)
        .map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())
}

fn ensure_backup_file_path(config_root: &Path, backup: &Path) -> Result<(), String> {
    crate::harness::claude::skills::ensure_plain_components(backup)?;
    let metadata = fs::symlink_metadata(backup).map_err(|error| error.to_string())?;
    if atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err("skill backup is not a plain file".into());
    }
    let backup_root = ledger::backup_root(config_root);
    crate::harness::claude::skills::ensure_plain_directory(&backup_root)?;
    let actual_parent = backup
        .parent()
        .ok_or("backup has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let expected_parent = backup_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if actual_parent != expected_parent {
        return Err("backup is outside the harness backup directory".into());
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.chars().any(char::is_control)
    {
        Err("invalid skill directory name".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn inventory_finds_native_roots_and_grouped_skills_but_skips_hidden_and_node_modules() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".config/opencode");
        let project = temp.path().join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        let global_skill = config.join("skills/global-a/SKILL.md");
        let global_flat_skill = config.join("skills/global-flat.md");
        let agent_skill = home.join(".agents/skills/team/nested/SKILL.md");
        let agent_flat_skill = home.join(".agents/skills/flat-agent.md");
        let project_skill = project.join(".agents/skills/grouped/SKILL.md");
        let project_flat_skill = project.join(".agents/skills/flat-project.md");
        let hidden_skill = project.join(".agents/skills/.hidden/SKILL.md");
        let dependency_skill = project.join(".agents/skills/node_modules/pkg/SKILL.md");
        for path in [
            &global_skill,
            &global_flat_skill,
            &agent_skill,
            &agent_flat_skill,
            &project_skill,
            &project_flat_skill,
            &hidden_skill,
            &dependency_skill,
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "---\nname: sample\n---\nbody\n").unwrap();
        }
        let harness = OpenCodeHarness::new(config, home);

        let entries = inventory(&harness, &project).unwrap();
        let paths: Vec<_> = entries.iter().map(|entry| entry.path.clone()).collect();
        let canonical_paths: Vec<_> = paths
            .iter()
            .map(|path| path.canonicalize().unwrap())
            .collect();
        assert!(canonical_paths.contains(&global_skill.canonicalize().unwrap()));
        assert!(canonical_paths.contains(&global_flat_skill.canonicalize().unwrap()));
        assert!(canonical_paths.contains(&agent_skill.canonicalize().unwrap()));
        assert!(canonical_paths.contains(&agent_flat_skill.canonicalize().unwrap()));
        assert!(canonical_paths.contains(&project_skill.canonicalize().unwrap()));
        assert!(canonical_paths.contains(&project_flat_skill.canonicalize().unwrap()));
        assert!(!canonical_paths.contains(&hidden_skill.canonicalize().unwrap()));
        assert!(!canonical_paths.contains(&dependency_skill.canonicalize().unwrap()));
        assert!(entries.iter().all(|entry| entry.state == SkillOverride::On));
    }

    #[test]
    fn inherited_project_skills_are_read_only_and_flat_markdown_lifecycle_preserves_root() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".config/opencode");
        let repository = temp.path().join("repository");
        let project = repository.join("packages/app");
        fs::create_dir_all(repository.join(".git")).unwrap();
        fs::create_dir_all(&project).unwrap();
        let inherited = repository.join(".agents/skills/ancestor/SKILL.md");
        let local = project.join(".agents/skills/local/SKILL.md");
        let root_skill = config.join("skills/neighbor/SKILL.md");
        let flat = config.join("skills/standalone.md");
        for path in [&inherited, &local, &root_skill, &flat] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "---\nname: sample\n---\nbody\n").unwrap();
        }
        let harness = OpenCodeHarness::new(config.clone(), home);
        let entries = inventory(&harness, &project).unwrap();
        let inherited_entry = entries
            .iter()
            .find(|entry| entry.path == inherited.canonicalize().unwrap())
            .unwrap();
        assert!(!inherited_entry.editable);
        assert!(
            entries
                .iter()
                .find(|entry| entry.path == local.canonicalize().unwrap())
                .unwrap()
                .editable
        );
        assert!(
            edit_skill(&harness, &project, &inherited_entry.id, "body", "changed")
                .unwrap_err()
                .contains("read-only")
        );
        assert!(uninstall(&harness, &project, &inherited_entry.id)
            .unwrap_err()
            .contains("read-only"));

        let flat_entry = entries.iter().find(|entry| entry.path == flat).unwrap();
        assert!(flat_entry.editable);
        uninstall(&harness, &project, &flat_entry.id).unwrap();
        assert!(!flat.exists());
        assert!(config.join("skills").is_dir());
        assert!(root_skill.is_file());
        let uninstall_row = ledger::read_applied(&config)
            .unwrap()
            .into_iter()
            .find(|row| row.id == flat_entry.id && row.action == "opencode-skill-uninstall")
            .unwrap();
        let backup = PathBuf::from(uninstall_row.backup_path.unwrap());
        restore(&harness, &project, &flat_entry.id, &backup).unwrap();
        assert_eq!(
            fs::read_to_string(flat).unwrap(),
            "---\nname: sample\n---\nbody\n"
        );

        let source = temp.path().join("installed-flat.md");
        fs::write(&source, "# Flat skill\n").unwrap();
        assert_eq!(install(&harness, &source).unwrap(), "installed-flat");
        assert_eq!(
            fs::read_to_string(config.join("skills/installed-flat.md")).unwrap(),
            "# Flat skill\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn inventory_skips_skill_directories_symlinked_outside_the_selected_tree() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".config/opencode");
        let project = temp.path().join("project");
        let outside = temp.path().join("outside");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(outside.join("escape")).unwrap();
        fs::write(outside.join("escape/SKILL.md"), "outside").unwrap();
        let skill_root = project.join(".opencode/skills");
        fs::create_dir_all(&skill_root).unwrap();
        symlink(&outside, skill_root.join("linked")).unwrap();

        let harness = OpenCodeHarness::new(config, home);
        assert!(inventory(&harness, &project).unwrap().is_empty());
    }
}
