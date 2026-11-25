//! Shared filesystem containment, symlink, and path-safety guards across harnesses.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::ledger;

pub(crate) fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(crate) fn path_exists_no_follow(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn ensure_plain_components(path: &Path) -> Result<(), String> {
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
            Component::Prefix(prefix) => {
                current.push(prefix.as_os_str());
                continue;
            }
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
        match fs::symlink_metadata(&current) {
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

pub(crate) fn ensure_plain_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent")?;
    ensure_plain_components(parent)
}

pub(crate) fn plain_metadata(path: &Path) -> Result<fs::Metadata, String> {
    ensure_plain_components(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if is_link_or_reparse(&metadata) {
        return Err(format!(
            "refusing linked or reparse path: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

pub(crate) fn ensure_plain_directory(path: &Path) -> Result<(), String> {
    let metadata = plain_metadata(path)?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(format!("expected a plain directory: {}", path.display()))
    }
}

pub(crate) fn ensure_plain_file_if_present(path: &Path) -> Result<(), String> {
    match plain_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(format!("expected a plain file: {}", path.display())),
        Err(_error) if !path_exists_no_follow(path)? => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn ensure_plain_path(path: &Path) -> Result<(), String> {
    ensure_plain_components(path)?;
    ensure_plain_file_if_present(path)
}

pub(crate) fn prepare_plain_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent")?;
    ensure_plain_parent(path)?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed creating {}: {error}", parent.display()))?;
    ensure_plain_path(path)
}

pub(crate) fn read_plain_file(path: &Path) -> Result<String, String> {
    ensure_plain_file_if_present(path)?;
    fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn canonical_existing_dir(path: &Path) -> Result<PathBuf, String> {
    ensure_plain_directory(path)?;
    fs::canonicalize(path).map_err(|error| error.to_string())
}

/// Canonicalize `path`, or if its leaf/tail does not yet exist (e.g. a deleted
/// file during restore), canonicalize the nearest existing plain ancestor and
/// re-append the non-existent normal tail components. Rejects `..` and links.
pub(crate) fn canonical_or_missing_tail(path: &Path) -> Option<PathBuf> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }
    ensure_plain_components(path).ok()?;
    let mut nearest = path.to_path_buf();
    let mut missing_tail = Vec::new();
    let metadata = loop {
        match fs::symlink_metadata(&nearest) {
            Ok(metadata) => break metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = nearest.file_name()?.to_os_string();
                missing_tail.push(name);
                nearest = nearest.parent()?.to_path_buf();
            }
            Err(_) => return None,
        }
    };
    if is_link_or_reparse(&metadata) || (!missing_tail.is_empty() && !metadata.is_dir()) {
        return None;
    }
    let mut resolved = fs::canonicalize(&nearest).ok()?;
    for component in missing_tail.into_iter().rev() {
        resolved.push(component);
    }
    Some(resolved)
}

/// Check whether `path` is contained within `root`, rejecting `..` traversal
/// components and symlinks along the path. Works even when `path` or `root`
/// does not yet exist on disk.
pub(crate) fn path_is_within(path: &Path, root: &Path) -> bool {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
        || root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }
    let Some(resolved_path) = canonical_or_missing_tail(path) else {
        return false;
    };
    let Some(resolved_root) = canonical_or_missing_tail(root) else {
        return false;
    };
    resolved_path.starts_with(&resolved_root)
}

pub(crate) use orbit_core::path_safety::normalize_lexical;

pub(crate) fn validate_plain_tree(path: &Path) -> Result<(), String> {
    let metadata = plain_metadata(path)?;
    if !metadata.is_dir() {
        return Err(format!("expected a plain directory: {}", path.display()));
    }
    for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
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

pub(crate) fn copy_dir(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "skill tree contains a link or reparse point: {}",
                from.display()
            ));
        }
        if metadata.is_dir() {
            copy_dir(&from, &to)?;
        } else if metadata.is_file() {
            let mut input =
                fs::File::open(&from).map_err(|error| format!("{}: {error}", from.display()))?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&to)
                .map_err(|error| format!("{}: {error}", to.display()))?;
            std::io::copy(&mut input, &mut output)
                .map_err(|error| format!("{}: {error}", to.display()))?;
            drop(output);
            fs::set_permissions(&to, metadata.permissions())
                .map_err(|error| format!("{}: {error}", to.display()))?;
        } else {
            return Err(format!(
                "skill tree contains a non-file entry: {}",
                from.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn safe_directory_checksum(path: &Path) -> Result<String, String> {
    validate_plain_tree(path)?;
    ledger::directory_checksum(path)
}

pub(crate) fn ensure_backup_path(config_root: &Path, backup: &Path) -> Result<(), String> {
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

pub(crate) fn ensure_backup_file_path(config_root: &Path, backup: &Path) -> Result<(), String> {
    ensure_plain_components(backup)?;
    let metadata = fs::symlink_metadata(backup).map_err(|error| error.to_string())?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err("skill backup is not a plain file".into());
    }
    let backup_root = ledger::backup_root(config_root);
    ensure_plain_directory(&backup_root)?;
    let actual_parent = canonical_existing_dir(backup.parent().ok_or("backup has no parent")?)?;
    let expected_parent = canonical_existing_dir(&backup_root)?;
    if actual_parent != expected_parent {
        return Err("backup is outside the harness backup directory".into());
    }
    Ok(())
}
