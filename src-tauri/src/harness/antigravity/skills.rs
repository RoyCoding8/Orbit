use super::{discovery, AntigravityHarness};
use crate::{
    harness::{
        atomic,
        claude::{
            skills::{self, SkillEntry},
            SkillOverride,
        },
        skill_store, Harness, ResourceKind,
    },
    ledger,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn id(path: &Path) -> String {
    format!(
        "skill-{}",
        atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
    )
}

pub fn owned_roots(harness: &AntigravityHarness, project: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        harness.config_root().join("skills"),
        harness.home().join(".gemini/antigravity-cli/skills"),
        harness.home().join(".gemini/skills"),
    ];
    roots.extend(discovery::PROJECT_DIRS.map(|dir| project.join(dir).join("skills")));
    roots
}

pub fn owned(harness: &AntigravityHarness, project: &Path, file: &Path) -> bool {
    owned_roots(harness, project).iter().any(|root| {
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        file.strip_prefix(root).is_ok_and(|p| {
            p.components().count() == 2 && p.file_name().is_some_and(|n| n == "SKILL.md")
        })
    })
}

pub fn inventory(harness: &AntigravityHarness, project: &Path) -> Result<Vec<SkillEntry>, String> {
    discovery::files(harness, project, "skills")?
        .into_iter()
        .map(|path| {
            let content = skills::read_plain_file(&path)?;
            let name = path
                .parent()
                .and_then(Path::file_name)
                .unwrap_or_default()
                .to_string_lossy();
            let metadata = skills::parse_frontmatter(&content, &name);
            Ok(SkillEntry {
                id: id(&path),
                name: metadata.name.unwrap_or_else(|| name.into_owned()),
                description: metadata.description,
                editable: owned(harness, project, &path),
                path,
                content,
                state: SkillOverride::On,
            })
        })
        .collect()
}

pub fn set_override(
    _: &AntigravityHarness,
    _: &Path,
    _: &str,
    _: SkillOverride,
) -> Result<(), String> {
    Err("Antigravity has no verified per-skill enable switch. Remove an owned skill to keep a restorable copy in History.".into())
}

pub fn edit_skill(
    harness: &AntigravityHarness,
    project: &Path,
    key: &str,
    expected: &str,
    content: &str,
) -> Result<(), String> {
    let entry = entry(harness, project, key)?;
    ledger::write_text(
        harness.config_root(),
        ResourceKind::Skill,
        key,
        "edit",
        &entry.path,
        expected,
        content,
    )
    .map_err(|e| e.to_string())
}

pub fn install(harness: &AntigravityHarness, source: &Path) -> Result<String, String> {
    skills::validate_plain_tree(source)?;
    let content = skills::read_plain_file(&source.join("SKILL.md"))?;
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Skill folder has no name")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err("Skill folder names use lowercase letters, digits and hyphens.".into());
    }
    if skills::parse_frontmatter(&content, name)
        .description
        .is_none()
    {
        return Err("SKILL.md requires a description in its frontmatter.".into());
    }
    let root = harness.config_root().join("skills");
    skills::ensure_plain_parent(&root.join(name))?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let target = root.canonicalize().map_err(|e| e.to_string())?.join(name);
    let key = id(&target.join("SKILL.md"));
    skill_store::install_tree(harness, &key, "install", source, &target)?;
    Ok(key)
}

pub fn uninstall(harness: &AntigravityHarness, project: &Path, key: &str) -> Result<(), String> {
    let entry = entry(harness, project, key)?;
    skill_store::uninstall_tree(
        harness,
        key,
        "uninstall",
        entry.path.parent().ok_or("Skill has no folder")?,
    )
}

pub fn restore(
    harness: &AntigravityHarness,
    project: &Path,
    key: &str,
    backup: &Path,
) -> Result<(), String> {
    let row = ledger::read_applied(harness.config_root())?
        .into_iter()
        .rev()
        .find(|r| r.resource_kind == "skill" && r.id == key && r.action == "uninstall")
        .ok_or("No completed removal is available to restore")?;
    let target = PathBuf::from(row.target.ok_or("Removal has no target")?);
    if !owned(harness, project, &target.join("SKILL.md")) || id(&target.join("SKILL.md")) != key {
        return Err("Skill removal target does not belong to this project or user".into());
    }
    skills::ensure_backup_path(harness.config_root(), backup)?;
    let recorded = PathBuf::from(row.backup_path.ok_or("Removal has no backup")?);
    if backup.canonicalize().map_err(|e| e.to_string())?
        != recorded.canonicalize().map_err(|e| e.to_string())?
    {
        return Err("This is not the recorded skill backup".into());
    }
    skill_store::restore_tree(
        harness,
        key,
        "restore",
        backup,
        &target,
        &row.before_checksum.ok_or("Removal has no checksum")?,
    )
}

fn entry(harness: &AntigravityHarness, project: &Path, key: &str) -> Result<SkillEntry, String> {
    inventory(harness, project)?
        .into_iter()
        .find(|e| e.id == key && e.editable)
        .ok_or("Skill is unavailable or belongs to an inherited or shared folder".into())
}
