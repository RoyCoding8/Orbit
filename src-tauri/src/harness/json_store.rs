use std::{fs, path::Path};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{
    harness::{atomic, claude::config::ConfigLayer, Harness, ResourceKind},
    ledger,
};

pub(crate) fn settings_layer(name: &str, path: &Path) -> Result<ConfigLayer, String> {
    let mut values = read_value(path)?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    for (key, value) in &mut values {
        *value = crate::harness::claude::config::masked_value(key, value, 0, false);
    }
    Ok(ConfigLayer {
        name: name.into(),
        path: path.to_string_lossy().into_owned(),
        values,
    })
}

pub(crate) fn apply_setting(
    harness: &impl Harness,
    action: &str,
    path: &Path,
    key: &str,
    mut value: Value,
) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("setting key must not be empty".into());
    }
    mutate_json(
        harness,
        ResourceKind::Settings,
        key,
        action,
        path,
        |document| {
            let values = document
                .as_object_mut()
                .ok_or("settings file must contain a JSON object")?;
            if let Some(previous) = values.get(key) {
                crate::harness::credentials::restore_masked(previous, &mut value).map_err(
                    |_| "masked settings arrays cannot be edited; re-enter the credential values",
                )?;
            }
            if crate::harness::credentials::contains_mask(&value) {
                return Err("setting still contains a masked placeholder; enter the real credential or remove the field".into());
            }
            values.insert(key.to_owned(), value);
            Ok(())
        },
    )
}

pub(crate) fn read_value(path: &Path) -> Result<Value, String> {
    ensure_plain(path)?;
    match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
                .map_err(|error| format!("failed parsing {}: {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(error) => Err(format!("failed reading {}: {error}", path.display())),
    }
}

pub(crate) fn mutate_json(
    harness: &impl Harness,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
    edit: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent")?;
    crate::harness::claude::skills::ensure_plain_parent(path)?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed creating {}: {error}", parent.display()))?;
    ensure_plain(path)?;
    let original = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("failed reading {}: {error}", path.display())),
    };
    let mut value = match &original {
        Some(bytes) => {
            serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes))
                .map_err(|error| format!("failed parsing {}: {error}", path.display()))?
        }
        None => Value::Object(Map::new()),
    };
    if !value.is_object() {
        return Err(format!("{} must contain a JSON object", path.display()));
    }
    edit(&mut value)?;
    let updated = serde_json::to_vec_pretty(&value)
        .map_err(|error| format!("failed serializing {}: {error}", path.display()))?;
    if original.as_deref() == Some(updated.as_slice()) {
        return Ok(());
    }
    let change = match original {
        Some(original) => {
            atomic::RecordedWrite::Restore(Sha256::digest(&original).into(), updated.as_slice())
        }
        None => atomic::RecordedWrite::Create(updated.as_slice()),
    };
    ensure_plain(path)?;
    let backup = ledger::backup_path(harness.config_root(), id, action);
    ledger::write(
        harness.config_root(),
        kind,
        id,
        action,
        path,
        change,
        Some(&backup),
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

pub(crate) fn delete_file(
    harness: &impl Harness,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
) -> Result<(), String> {
    ensure_plain(path)?;
    let bytes =
        fs::read(path).map_err(|error| format!("failed reading {}: {error}", path.display()))?;
    let backup = ledger::backup_path(harness.config_root(), id, action);
    ledger::remove_file(
        harness.config_root(),
        kind,
        id,
        action,
        path,
        Sha256::digest(bytes).into(),
        &backup,
    )
    .map_err(|error| error.to_string())
}

fn ensure_plain(path: &Path) -> Result<(), String> {
    crate::harness::claude::skills::ensure_plain_components(path)?;
    crate::harness::claude::skills::ensure_plain_file_if_present(path)
}
