use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{ClaudeCodeHarness, SkillOverride};
use crate::{
    harness::{atomic, Harness, ResourceKind},
    ledger,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub path: PathBuf,
    pub content: String,
    pub state: SkillOverride,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
}

fn parse_frontmatter(content: &str, fallback_name: &str) -> Frontmatter {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return Frontmatter {
            name: Some(fallback_name.to_owned()),
            description: None,
        };
    }
    let mut yaml = Vec::new();
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        yaml.push(line);
    }
    let fallback = || Frontmatter {
        name: Some(fallback_name.to_owned()),
        description: None,
    };
    if !closed {
        return fallback();
    }
    let fields: BTreeMap<String, serde_yaml::Value> = match serde_yaml::from_str(&yaml.join("\n")) {
        Ok(fields) => fields,
        Err(_) => return fallback(),
    };
    Frontmatter {
        name: fields
            .get("name")
            .and_then(serde_yaml::Value::as_str)
            .map(str::to_owned)
            .or_else(|| Some(fallback_name.to_owned())),
        description: fields
            .get("description")
            .and_then(serde_yaml::Value::as_str)
            .map(str::to_owned),
    }
}

pub fn inventory(
    harness: &ClaudeCodeHarness,
) -> Result<(Vec<SkillEntry>, Vec<MemoryEntry>), String> {
    let refs = harness
        .list(ResourceKind::Skill)
        .map_err(|e| e.to_string())?;
    let settings = atomic::read_json(&harness.settings_path()).map_err(|e| e.to_string())?;
    let overrides = settings.get("skillOverrides").and_then(Value::as_object);
    let mut skills = Vec::with_capacity(refs.len());
    for resource in refs {
        if let Err(error) = validate_id(&resource.id) {
            eprintln!("Skipping skill {}: {error}", resource.id);
            continue;
        }
        let path = resource.path.join("SKILL.md");
        let content = match read_plain_file(&path) {
            Ok(content) => content,
            Err(error) => {
                eprintln!(
                    "Skipping skill {} at {}: {error}",
                    resource.id,
                    path.display()
                );
                continue;
            }
        };
        let metadata = parse_frontmatter(&content, &resource.id);
        let state = overrides
            .and_then(|map| map.get(&resource.id))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(SkillOverride::On);
        skills.push(SkillEntry {
            id: resource.id,
            name: metadata.name.unwrap_or_default(),
            description: metadata.description,
            path,
            content,
            state,
        });
    }
    let refs = harness
        .list(ResourceKind::Memory)
        .map_err(|e| e.to_string())?;
    let mut memories = Vec::with_capacity(refs.len());
    for resource in refs {
        match read_plain_file(&resource.path) {
            Ok(content) => memories.push(MemoryEntry {
                id: resource.id,
                path: resource.path,
                content,
            }),
            Err(error) => eprintln!(
                "Skipping memory {} at {}: {error}",
                resource.id,
                resource.path.display()
            ),
        }
    }
    Ok((skills, memories))
}

pub fn set_override(
    harness: &ClaudeCodeHarness,
    id: &str,
    state: SkillOverride,
) -> Result<(), String> {
    validate_id(id)?;
    let value = match state {
        SkillOverride::On => None,
        other => Some(serde_json::to_value(other).map_err(|e| e.to_string())?),
    };
    let patch = match value {
        Some(value) => atomic::Patch::SetPath {
            path: atomic::p!["skillOverrides", id],
            value,
        },
        None => atomic::Patch::RemovePath {
            path: atomic::p!["skillOverrides", id],
        },
    };
    let backup = ledger::backup_path(harness.config_root(), id, "override");
    let receipt = atomic::commit(&harness.settings_path(), vec![patch], Some(&backup))
        .map_err(|e| e.to_string())?;
    ledger::record(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "override",
        &receipt,
    )
    .map_err(|error| error.to_string())
}

pub fn read_skill(harness: &ClaudeCodeHarness, id: &str) -> Result<String, String> {
    validate_id(id)?;
    let skill_dir = harness.config_root().join("skills").join(id);
    ensure_plain_directory(&skill_dir)?;
    read_plain_file(&skill_dir.join("SKILL.md"))
}

pub fn edit_skill(
    harness: &ClaudeCodeHarness,
    id: &str,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    validate_id(id)?;
    let skill_dir = harness.config_root().join("skills").join(id);
    ensure_plain_directory(&skill_dir)?;
    validate_plain_tree(&skill_dir)?;
    let path = skill_dir.join("SKILL.md");
    ensure_plain_file_if_present(&path)?;
    let backup = ledger::backup_path(harness.config_root(), id, "edit");
    let expected = Sha256::digest(expected_content.as_bytes()).into();
    let receipt = atomic::restore(&path, expected, content.as_bytes(), Some(&backup)).map_err(
        |error| match error {
            atomic::CommitError::Drift { .. } => format!(
                "Skill {id} changed since this editor opened. Close the editor, refresh skills, and try again."
            ),
            other => other.to_string(),
        },
    )?;
    ledger::record(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "edit",
        &receipt,
    )
    .map_err(|error| error.to_string())
}

pub fn uninstall(harness: &ClaudeCodeHarness, id: &str) -> Result<(), String> {
    validate_id(id)?;
    let source = harness.config_root().join("skills").join(id);
    ensure_plain_directory(&source)?;
    let before = safe_directory_checksum(&source)?;
    let backup = ledger::backup_path(harness.config_root(), id, "uninstall");
    ensure_plain_parent(&backup)?;
    if path_exists_no_follow(&backup)? {
        return Err("skill uninstall backup already exists".into());
    }
    std::fs::create_dir_all(backup.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::rename(&source, &backup).map_err(|e| e.to_string())?;
    let after = safe_directory_checksum(&backup).map_err(|error| {
        format!("skill was uninstalled, but its backup could not be verified: {error}")
    })?;
    if after != before {
        return Err(
            "skill was uninstalled, but its backup checksum differs from the source".into(),
        );
    }
    ledger::record_moved(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "uninstall",
        Some(&backup),
        Some(&before),
        None,
    )
    .map_err(|error| error.to_string())
}

pub fn restore(harness: &ClaudeCodeHarness, id: &str, backup: &Path) -> Result<(), String> {
    validate_id(id)?;
    let uninstall = ledger::latest_row(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "uninstall",
        None,
    )?
    .ok_or_else(|| format!("no recorded uninstall for skill {id}"))?;
    let recorded_backup = uninstall
        .backup_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| format!("uninstall record for {id} has no backup path"))?;
    let expected_checksum = uninstall
        .before_checksum
        .as_deref()
        .ok_or_else(|| format!("uninstall record for {id} has no original checksum"))?;
    ensure_backup_path(harness.config_root(), backup)?;
    ensure_backup_path(harness.config_root(), &recorded_backup)?;
    if canonical_existing_dir(backup)? != canonical_existing_dir(&recorded_backup)? {
        return Err(format!(
            "backup is not the recorded uninstall for skill {id}"
        ));
    }
    let prior_rows = ledger::read(harness.config_root())?;
    if prior_rows.iter().any(|row| {
        row.resource_kind == "skill"
            && row.id == id
            && row.action == "restore"
            && row.backup_path.as_deref() == Some(recorded_backup.to_string_lossy().as_ref())
    }) {
        return Err(format!("backup for skill {id} has already been restored"));
    }
    let backup_checksum = safe_directory_checksum(backup)?;
    if backup_checksum != expected_checksum {
        return Err(format!(
            "backup checksum does not match the uninstall record for {id}"
        ));
    }
    let target = harness.config_root().join("skills").join(id);
    ensure_plain_parent(&target)?;
    if path_exists_no_follow(&target)? {
        return Err(format!("skill already exists: {id}"));
    }
    std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
    if let Err(error) = copy_dir(backup, &target) {
        return Err(with_cleanup_error(
            format!("skill restore copy failed: {error}"),
            &target,
        ));
    }
    let restored_checksum = safe_directory_checksum(&target).map_err(|error| {
        format!(
            "skill restore was copied, but the restored directory could not be verified: {error}"
        )
    })?;
    if restored_checksum != expected_checksum {
        return Err(with_cleanup_error(
            format!("restored skill checksum does not match the uninstall record for {id}"),
            &target,
        ));
    }
    ledger::record_moved(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        "restore",
        Some(&recorded_backup),
        Some(expected_checksum),
        Some(&restored_checksum),
    )
    .map_err(|error| error.to_string())
}

pub fn install(harness: &ClaudeCodeHarness, source: &Path) -> Result<String, String> {
    ensure_plain_directory(source)?;
    validate_plain_tree(source)?;
    let skill_file = source.join("SKILL.md");
    ensure_plain_file_if_present(&skill_file)?;
    if !skill_file.is_file() {
        return Err("source folder must contain SKILL.md".into());
    }
    let id = source
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("invalid skill folder name")?
        .to_owned();
    validate_id(&id)?;
    let destination = harness.config_root().join("skills").join(&id);
    ensure_plain_parent(&destination)?;
    if path_exists_no_follow(&destination)? {
        return Err(format!("skill already exists: {id}"));
    }
    let checksum = safe_directory_checksum(source)?;
    if let Err(error) = copy_dir(source, &destination) {
        return Err(with_cleanup_error(error, &destination));
    }
    let installed_checksum = match safe_directory_checksum(&destination) {
        Ok(checksum) => checksum,
        Err(error) => {
            return Err(with_cleanup_error(
                format!("installed skill could not be verified: {error}"),
                &destination,
            ))
        }
    };
    if installed_checksum != checksum {
        return Err(with_cleanup_error(
            "installed skill checksum differs from source".into(),
            &destination,
        ));
    }
    ledger::record_moved(
        harness.config_root(),
        ResourceKind::Skill,
        &id,
        "install",
        None,
        None,
        Some(&checksum),
    )
    .map_err(|error| error.to_string())?;
    Ok(id)
}

fn copy_dir(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "skill tree contains a link or reparse point: {}",
                from.display()
            ));
        }
        if metadata.is_dir() {
            copy_dir(&from, &to)?;
        } else if metadata.is_file() {
            std::fs::copy(from, to).map_err(|e| e.to_string())?;
        } else {
            return Err(format!(
                "skill tree contains a non-file entry: {}",
                from.display()
            ));
        }
    }
    Ok(())
}

fn read_plain_file(path: &Path) -> Result<String, String> {
    ensure_plain_file_if_present(path)?;
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn safe_directory_checksum(path: &Path) -> Result<String, String> {
    validate_plain_tree(path)?;
    ledger::directory_checksum(path)
}

fn validate_plain_tree(path: &Path) -> Result<(), String> {
    let metadata = plain_metadata(path)?;
    if !metadata.is_dir() {
        return Err(format!("expected a plain directory: {}", path.display()));
    }
    for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let child = entry.path();
        let metadata = plain_metadata(&child)?;
        if metadata.is_dir() {
            validate_plain_tree(&child)?;
        } else if !metadata.is_file() {
            return Err(format!(
                "unsupported entry in skill tree: {}",
                child.display()
            ));
        }
    }
    Ok(())
}

fn ensure_plain_directory(path: &Path) -> Result<(), String> {
    let metadata = plain_metadata(path)?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(format!("expected a plain directory: {}", path.display()))
    }
}

fn ensure_plain_file_if_present(path: &Path) -> Result<(), String> {
    match plain_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(format!("expected a plain file: {}", path.display())),
        Err(_error) if !path_exists_no_follow(path)? => Ok(()),
        Err(error) => Err(error),
    }
}

fn ensure_plain_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent")?;
    ensure_plain_components(parent)
}

fn plain_metadata(path: &Path) -> Result<std::fs::Metadata, String> {
    ensure_plain_components(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if is_link_or_reparse(&metadata) {
        return Err(format!(
            "refusing linked or reparse path: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

fn ensure_plain_components(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::Normal(name) => current.push(name),
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(format!(
                    "parent traversal is not allowed: {}",
                    path.display()
                ));
            }
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                return Err(format!(
                    "refusing linked or reparse path: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn path_exists_no_follow(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn canonical_existing_dir(path: &Path) -> Result<PathBuf, String> {
    ensure_plain_directory(path)?;
    std::fs::canonicalize(path).map_err(|error| error.to_string())
}

fn ensure_backup_path(config_root: &Path, backup: &Path) -> Result<(), String> {
    ensure_plain_directory(backup)?;
    let backup_root = ledger::backup_root(config_root);
    ensure_plain_directory(&backup_root)?;
    let actual_parent = canonical_existing_dir(backup.parent().ok_or("backup has no parent")?)?;
    let expected_parent = canonical_existing_dir(&backup_root)?;
    if actual_parent != expected_parent {
        return Err("backup is outside the harness backup directory".into());
    }
    Ok(())
}

fn with_cleanup_error(error: String, partial: &Path) -> String {
    match path_exists_no_follow(partial) {
        Ok(false) => error,
        Ok(true) => match std::fs::remove_dir_all(partial) {
            Ok(()) => error,
            Err(cleanup) => format!(
                "{error}; partial directory remains at {}: {cleanup}",
                partial.display()
            ),
        },
        Err(cleanup) => format!(
            "{error}; could not inspect partial install {}: {cleanup}",
            partial.display()
        ),
    }
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn validate_id(id: &str) -> Result<(), String> {
    let invalid = id.is_empty()
        || id == "."
        || id == ".."
        || id.ends_with(['.', ' '])
        || id
            .chars()
            .any(|character| character.is_control() || "<>:\"/\\|?*".contains(character));
    let device_name = id
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(['.', ' '])
        .to_ascii_uppercase();
    let reserved = matches!(device_name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (device_name.len() == 4
            && (device_name.starts_with("COM") || device_name.starts_with("LPT"))
            && matches!(device_name.as_bytes()[3], b'1'..=b'9'));
    if invalid || reserved {
        Err("invalid skill id".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::atomic;

    fn fixture() -> (tempfile::TempDir, ClaudeCodeHarness) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("claude");
        std::fs::create_dir_all(root.join("skills").join("alpha")).unwrap();
        std::fs::write(
            root.join("skills/alpha/SKILL.md"),
            "---\nname: Alpha Skill\ndescription: A fixture skill\n---\ncontent\n",
        )
        .unwrap();
        let harness = ClaudeCodeHarness::new(crate::harness::HarnessPaths {
            config_root: root,
            claude_json: dir.path().join(".claude.json"),
        });
        (dir, harness)
    }

    #[test]
    fn override_states_round_trip_through_settings() {
        let (_dir, harness) = fixture();
        for (state, expected) in [
            (SkillOverride::On, None),
            (SkillOverride::NameOnly, Some("name-only")),
            (
                SkillOverride::UserInvocableOnly,
                Some("user-invocable-only"),
            ),
            (SkillOverride::Off, Some("off")),
        ] {
            set_override(&harness, "alpha", state).unwrap();
            let settings = atomic::read_json(&harness.settings_path()).unwrap();
            assert_eq!(
                settings
                    .get("skillOverrides")
                    .and_then(|v| v.get("alpha"))
                    .and_then(Value::as_str),
                expected
            );
        }
    }

    #[test]
    fn inventory_reads_frontmatter_and_falls_back_on_malformed_yaml() {
        let (_dir, harness) = fixture();
        let (skills, memories) = inventory(&harness).unwrap();
        assert_eq!(skills[0].name, "Alpha Skill");
        assert_eq!(skills[0].description.as_deref(), Some("A fixture skill"));
        assert!(memories.is_empty());
        std::fs::write(
            harness.config_root().join("skills/alpha/SKILL.md"),
            "---\nname: [broken\n---\nbody\n",
        )
        .unwrap();
        let (skills, _) = inventory(&harness).unwrap();
        assert_eq!(skills[0].name, "alpha");
        assert_eq!(skills[0].description, None);
    }

    #[test]
    fn inventory_accepts_missing_and_unclosed_frontmatter() {
        assert_eq!(
            parse_frontmatter("body only", "folder").name.as_deref(),
            Some("folder")
        );
        assert_eq!(
            parse_frontmatter("---\nname: other", "folder")
                .name
                .as_deref(),
            Some("folder")
        );
        assert_eq!(
            parse_frontmatter("---\n---\nbody", "folder")
                .name
                .as_deref(),
            Some("folder")
        );
    }

    #[test]
    fn edit_skill_records_the_exact_original_backup_and_ledger_row() {
        let (_dir, harness) = fixture();
        let original = "---\nname: Alpha Skill\ndescription: A fixture skill\n---\ncontent\n";
        let updated = "updated skill content\n";

        edit_skill(&harness, "alpha", original, updated).unwrap();

        let path = harness.config_root().join("skills/alpha/SKILL.md");
        assert_eq!(std::fs::read_to_string(path).unwrap(), updated);
        let rows = ledger::read(harness.config_root()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "edit");
        assert_eq!(rows[0].resource_kind, "skill");
        assert_eq!(rows[0].id, "alpha");
        assert_eq!(
            rows[0].before_checksum.as_deref(),
            Some(atomic::hex(&Sha256::digest(original.as_bytes())).as_str())
        );
        assert_eq!(
            rows[0].after_checksum.as_deref(),
            Some(atomic::hex(&Sha256::digest(updated.as_bytes())).as_str())
        );
        let backup = PathBuf::from(rows[0].backup_path.as_deref().unwrap());
        assert_eq!(std::fs::read(backup).unwrap(), original.as_bytes());
    }

    #[test]
    fn edit_skill_refuses_stale_content_without_backup_or_ledger() {
        let (_dir, harness) = fixture();
        let stale = "---\nname: Alpha Skill\ndescription: A fixture skill\n---\ncontent\n";
        let newer = "newer skill content\n";
        let path = harness.config_root().join("skills/alpha/SKILL.md");
        std::fs::write(&path, newer).unwrap();

        let error = edit_skill(&harness, "alpha", stale, "overwritten").unwrap_err();

        assert_eq!(
            error,
            "Skill alpha changed since this editor opened. Close the editor, refresh skills, and try again."
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), newer);
        assert!(ledger::read(harness.config_root()).unwrap().is_empty());
        assert!(!ledger::backup_root(harness.config_root()).exists());
    }

    #[test]
    fn edit_skill_does_not_recreate_a_missing_file() {
        let (_dir, harness) = fixture();
        let expected = "---\nname: Alpha Skill\ndescription: A fixture skill\n---\ncontent\n";
        let path = harness.config_root().join("skills/alpha/SKILL.md");
        std::fs::remove_file(&path).unwrap();

        assert!(edit_skill(&harness, "alpha", expected, "recreated").is_err());

        assert!(!path.exists());
        assert!(ledger::read(harness.config_root()).unwrap().is_empty());
        assert!(!ledger::backup_root(harness.config_root()).exists());
    }

    #[test]
    fn only_first_edit_with_the_same_original_content_lands() {
        let (_dir, harness) = fixture();
        let original = "---\nname: Alpha Skill\ndescription: A fixture skill\n---\ncontent\n";
        let first = "first edit\n";
        let path = harness.config_root().join("skills/alpha/SKILL.md");

        edit_skill(&harness, "alpha", original, first).unwrap();
        assert!(edit_skill(&harness, "alpha", original, "second edit").is_err());

        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        let rows = ledger::read(harness.config_root()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "edit");
        let backup = PathBuf::from(rows[0].backup_path.as_deref().unwrap());
        assert_eq!(std::fs::read(backup).unwrap(), original.as_bytes());
    }

    #[test]
    fn uninstall_then_restore_preserves_the_skill_and_records_both_mutations() {
        let (_dir, harness) = fixture();
        let skill = harness.config_root().join("skills/alpha");
        let checksum = ledger::directory_checksum(&skill).unwrap();
        uninstall(&harness, "alpha").unwrap();
        assert!(!skill.exists());
        let rows = ledger::read(harness.config_root()).unwrap();
        assert_eq!(rows[0].action, "uninstall");
        assert_eq!(rows[0].before_checksum.as_deref(), Some(checksum.as_str()));
        let backup = PathBuf::from(rows[0].backup_path.as_deref().unwrap());
        assert!(backup.starts_with(ledger::backup_root(harness.config_root())));
        restore(&harness, "alpha", &backup).unwrap();
        assert_eq!(ledger::directory_checksum(&skill).unwrap(), checksum);
        assert!(backup.is_dir(), "restore preserves the original backup");
        assert_eq!(
            ledger::read(harness.config_root()).unwrap()[1].action,
            "restore"
        );
        assert!(restore(&harness, "alpha", &backup).is_err());
    }

    #[test]
    fn restore_rejects_wrong_skill_and_unrecorded_backup_paths() {
        let (_dir, harness) = fixture();
        uninstall(&harness, "alpha").unwrap();
        let recorded = PathBuf::from(
            ledger::read(harness.config_root()).unwrap()[0]
                .backup_path
                .as_deref()
                .unwrap(),
        );
        assert!(restore(&harness, "beta", &recorded).is_err());

        let arbitrary = ledger::backup_root(harness.config_root()).join("not-recorded");
        copy_dir(&recorded, &arbitrary).unwrap();
        assert!(restore(&harness, "alpha", &arbitrary).is_err());
        assert!(arbitrary.is_dir());
        assert!(!harness.config_root().join("skills/alpha").exists());
    }

    #[test]
    fn restore_rejects_backup_that_does_not_match_recorded_preimage() {
        let (_dir, harness) = fixture();
        uninstall(&harness, "alpha").unwrap();
        let backup = PathBuf::from(
            ledger::read(harness.config_root()).unwrap()[0]
                .backup_path
                .as_deref()
                .unwrap(),
        );
        std::fs::write(backup.join("SKILL.md"), "tampered").unwrap();
        assert!(restore(&harness, "alpha", &backup).is_err());
        assert!(backup.is_dir());
        assert!(!harness.config_root().join("skills/alpha").exists());
    }

    #[test]
    fn inventory_skips_an_unreadable_entry_without_losing_healthy_entries() {
        let (dir, harness) = fixture();
        let bad = harness.config_root().join("skills/bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("SKILL.md"), [0xff, 0xfe]).unwrap();
        let memories = harness.config_root().join("projects/test/memory");
        std::fs::create_dir_all(&memories).unwrap();
        std::fs::write(memories.join("broken.md"), [0xff, 0xfe]).unwrap();
        std::fs::write(memories.join("good.md"), "memory").unwrap();

        let (skills, memories) = inventory(&harness).unwrap();
        assert_eq!(
            skills
                .iter()
                .map(|skill| skill.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha"]
        );
        assert_eq!(
            memories
                .iter()
                .map(|memory| memory.id.as_str())
                .collect::<Vec<_>>(),
            ["test/good.md"]
        );
        assert!(dir.path().join("claude").exists());
    }

    #[test]
    fn skill_ids_reject_windows_paths_ads_and_device_names() {
        for id in [
            "",
            ".",
            "..",
            "a\\b",
            "a/b",
            "name:stream",
            "name.",
            "name ",
            "CON",
            "nul.txt",
            "COM1",
        ] {
            assert!(validate_id(id).is_err(), "accepted invalid id {id:?}");
        }
        assert!(validate_id("valid-skill_2").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn skill_file_links_are_not_read_or_edited() {
        use std::os::unix::fs::symlink;

        let (dir, harness) = fixture();
        let outside = dir.path().join("outside.md");
        let original = "outside content";
        std::fs::write(&outside, original).unwrap();
        let skill_file = harness.config_root().join("skills/alpha/SKILL.md");
        std::fs::remove_file(&skill_file).unwrap();
        symlink(&outside, &skill_file).unwrap();

        assert!(read_skill(&harness, "alpha").is_err());
        assert!(edit_skill(&harness, "alpha", original, "changed").is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn skill_read_edit_and_uninstall_refuse_linked_skill_paths() {
        use std::os::unix::fs::symlink;

        let (dir, harness) = fixture();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let original = "outside content";
        std::fs::write(outside.join("SKILL.md"), original).unwrap();
        let skill_dir = harness.config_root().join("skills/alpha");
        std::fs::remove_dir_all(&skill_dir).unwrap();
        symlink(&outside, &skill_dir).unwrap();

        assert!(read_skill(&harness, "alpha").is_err());
        assert!(edit_skill(&harness, "alpha", original, "changed").is_err());
        assert!(uninstall(&harness, "alpha").is_err());
        assert_eq!(
            std::fs::read_to_string(outside.join("SKILL.md")).unwrap(),
            original
        );
    }
}
