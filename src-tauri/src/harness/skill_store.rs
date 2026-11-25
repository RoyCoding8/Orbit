use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::{atomic, fs_guard, Harness, ResourceKind};
use crate::ledger;

#[derive(Debug, Clone, Default)]
pub(crate) struct Frontmatter {
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
}

pub(crate) use orbit_core::transcript_and_parsing::extract_frontmatter_body;

pub(crate) fn parse_frontmatter(content: &str, fallback_name: &str) -> Frontmatter {
    let fallback = || Frontmatter {
        name: Some(fallback_name.to_owned()),
        description: None,
    };
    let Some(yaml) = extract_frontmatter_body(content) else {
        return fallback();
    };
    let fields: BTreeMap<String, serde_yaml::Value> = match serde_yaml::from_str(yaml) {
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

pub(crate) fn hashed_skill_id(path: &Path) -> String {
    format!(
        "skill-{}",
        atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
    )
}

pub(crate) fn install_tree(
    harness: &impl Harness,
    id: &str,
    action: &str,
    source: &Path,
    target: &Path,
) -> Result<(), String> {
    fs_guard::ensure_plain_parent(target)?;
    std::fs::create_dir_all(target.parent().ok_or("skill path has no parent")?)
        .map_err(|error| error.to_string())?;
    let guard = atomic::lock_mutations(target).map_err(|error| error.to_string())?;
    if fs_guard::path_exists_no_follow(target)? {
        return Err("skill already exists at its destination".into());
    }
    let checksum = fs_guard::safe_directory_checksum(source)?;
    let plan = atomic::MutationPlan {
        target: target.into(),
        backup: None,
        before_checksum: None,
        after_checksum: Some(checksum.clone()),
    };
    let operation = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        action,
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    fs_guard::copy_dir(source, target).map_err(|error| {
        format!("reserved install {operation} failed; inspect the partial destination: {error}")
    })?;
    ledger::verify_directory_checksum(&fs_guard::safe_directory_checksum(target)?, &checksum)
        .map_err(|error| error.to_string())?;
    ledger::complete_mutation(harness.config_root(), &operation, &guard)
        .map_err(|error| error.to_string())
}

pub(crate) fn uninstall_tree(
    harness: &impl Harness,
    id: &str,
    action: &str,
    source: &Path,
) -> Result<(), String> {
    fs_guard::ensure_plain_components(source)?;
    let backup = ledger::backup_path(harness.config_root(), id, action);
    fs_guard::ensure_plain_parent(&backup)?;
    std::fs::create_dir_all(backup.parent().ok_or("backup path has no parent")?)
        .map_err(|error| error.to_string())?;
    let guard = atomic::lock_mutations(source).map_err(|error| error.to_string())?;
    if fs_guard::path_exists_no_follow(&backup)? {
        return Err("skill uninstall backup already exists".into());
    }
    let checksum = fs_guard::safe_directory_checksum(source)?;
    let plan = atomic::MutationPlan {
        target: source.into(),
        backup: Some(backup.clone()),
        before_checksum: Some(checksum.clone()),
        after_checksum: None,
    };
    let operation = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        action,
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(source, &backup)
        .map_err(|error| format!("reserved uninstall {operation} did not complete: {error}"))?;
    ledger::verify_directory_checksum(&fs_guard::safe_directory_checksum(&backup)?, &checksum)
        .map_err(|error| error.to_string())?;
    ledger::complete_mutation(harness.config_root(), &operation, &guard)
        .map_err(|error| error.to_string())
}

pub(crate) fn restore_tree(
    harness: &impl Harness,
    id: &str,
    action: &str,
    backup: &Path,
    target: &Path,
    expected: &str,
) -> Result<(), String> {
    fs_guard::ensure_backup_path(harness.config_root(), backup)?;
    ledger::verify_directory_checksum(&fs_guard::safe_directory_checksum(backup)?, expected)
        .map_err(|error| format!("cannot restore skill {id}: {error}"))?;
    fs_guard::ensure_plain_parent(target)?;
    let guard = atomic::lock_mutations(target).map_err(|error| error.to_string())?;
    if fs_guard::path_exists_no_follow(target)? {
        return Err("skill already exists at its original path".into());
    }
    let plan = atomic::MutationPlan {
        target: target.into(),
        backup: Some(backup.into()),
        before_checksum: None,
        after_checksum: Some(expected.into()),
    };
    let operation = ledger::reserve_mutation(
        harness.config_root(),
        ResourceKind::Skill,
        id,
        action,
        &plan,
        &guard,
    )
    .map_err(|error| error.to_string())?;
    fs_guard::copy_dir(backup, target).map_err(|error| {
        format!("reserved restore {operation} failed; inspect the partial destination: {error}")
    })?;
    ledger::verify_directory_checksum(&fs_guard::safe_directory_checksum(target)?, expected)
        .map_err(|error| format!("reserved restore {operation} failed verification: {error}"))?;
    ledger::complete_mutation(harness.config_root(), &operation, &guard)
        .map_err(|error| error.to_string())
}
