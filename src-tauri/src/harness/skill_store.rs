use std::path::Path;

use super::{atomic, claude::skills, Harness, ResourceKind};
use crate::ledger;

pub(crate) fn install_tree(
    harness: &impl Harness,
    id: &str,
    action: &str,
    source: &Path,
    target: &Path,
) -> Result<(), String> {
    skills::ensure_plain_parent(target)?;
    std::fs::create_dir_all(target.parent().ok_or("skill path has no parent")?)
        .map_err(|error| error.to_string())?;
    let guard = atomic::lock_mutations(target).map_err(|error| error.to_string())?;
    if skills::path_exists_no_follow(target)? {
        return Err("skill already exists at its destination".into());
    }
    let checksum = skills::safe_directory_checksum(source)?;
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
    skills::copy_dir(source, target).map_err(|error| {
        format!("reserved install {operation} failed; inspect the partial destination: {error}")
    })?;
    ledger::verify_directory_checksum(&skills::safe_directory_checksum(target)?, &checksum)
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
    skills::ensure_plain_components(source)?;
    let backup = ledger::backup_path(harness.config_root(), id, action);
    skills::ensure_plain_parent(&backup)?;
    if skills::path_exists_no_follow(&backup)? {
        return Err("skill uninstall backup already exists".into());
    }
    std::fs::create_dir_all(backup.parent().ok_or("backup path has no parent")?)
        .map_err(|error| error.to_string())?;
    let guard = atomic::lock_mutations(source).map_err(|error| error.to_string())?;
    let checksum = skills::safe_directory_checksum(source)?;
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
    ledger::verify_directory_checksum(&skills::safe_directory_checksum(&backup)?, &checksum)
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
    skills::ensure_backup_path(harness.config_root(), backup)?;
    ledger::verify_directory_checksum(&skills::safe_directory_checksum(backup)?, expected)
        .map_err(|error| format!("cannot restore skill {id}: {error}"))?;
    skills::ensure_plain_parent(target)?;
    let guard = atomic::lock_mutations(target).map_err(|error| error.to_string())?;
    if skills::path_exists_no_follow(target)? {
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
    skills::copy_dir(backup, target).map_err(|error| {
        format!("reserved restore {operation} failed; inspect the partial destination: {error}")
    })?;
    ledger::verify_directory_checksum(&skills::safe_directory_checksum(target)?, expected)
        .map_err(|error| format!("reserved restore {operation} failed verification: {error}"))?;
    ledger::complete_mutation(harness.config_root(), &operation, &guard)
        .map_err(|error| error.to_string())
}
