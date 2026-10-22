use std::{fs, path::Path};

use serde_json::Value;
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

use super::CodexHarness;
use crate::{
    harness::{atomic, Harness, ResourceKind},
    ledger,
};

pub(crate) fn read(path: &Path) -> Result<DocumentMut, String> {
    ensure_plain(path)?;
    match fs::read(path) {
        Ok(bytes) => {
            let source = String::from_utf8(bytes)
                .map_err(|_| format!("{} is not valid UTF-8 TOML", path.display()))?;
            source.parse::<DocumentMut>().map_err(|error| {
                format!(
                    "failed parsing TOML in {}: {}",
                    path.display(),
                    error.message()
                )
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(error) => Err(format!("failed reading {}: {error}", path.display())),
    }
}

pub(crate) fn read_value(path: &Path) -> Result<Value, String> {
    let document = read(path)?;
    toml_edit::de::from_str(&document.to_string()).map_err(|error| {
        format!(
            "failed converting {} to JSON values: {}",
            path.display(),
            error.message()
        )
    })
}

pub(crate) fn mutate(
    harness: &CodexHarness,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
    edit: impl FnOnce(&mut DocumentMut) -> Result<(), String>,
) -> Result<(), String> {
    ensure_plain(path)?;
    let original = read_original(path)?;
    let mut document = match &original {
        Some(bytes) => {
            let source = String::from_utf8(bytes.clone())
                .map_err(|_| format!("{} is not valid UTF-8 TOML", path.display()))?;
            source.parse::<DocumentMut>().map_err(|error| {
                format!(
                    "failed parsing TOML in {}: {}",
                    path.display(),
                    error.message()
                )
            })?
        }
        None => DocumentMut::new(),
    };
    edit(&mut document)?;
    let updated = document.to_string();
    if original.as_deref() == Some(updated.as_bytes()) {
        return Ok(());
    }
    let change = match original {
        Some(original) => {
            atomic::RecordedWrite::Restore(Sha256::digest(&original).into(), updated.as_bytes())
        }
        None => atomic::RecordedWrite::Create(updated.as_bytes()),
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

pub(crate) fn mutate_json(
    harness: &CodexHarness,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
    edit: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    ensure_plain(path)?;
    let original = read_original(path)?;
    let mut document = match &original {
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|error| format!("failed parsing {}: {error}", path.display()))?,
        None => Value::Object(serde_json::Map::new()),
    };
    if !document.is_object() {
        return Err(format!("{} must contain a JSON object", path.display()));
    }
    edit(&mut document)?;
    let updated = serde_json::to_vec_pretty(&document)
        .map_err(|error| format!("failed serializing {}: {error}", path.display()))?;
    if original.as_deref() == Some(updated.as_slice()) {
        return Ok(());
    }
    let change = match original {
        Some(original) => {
            atomic::RecordedWrite::Restore(Sha256::digest(&original).into(), &updated)
        }
        None => atomic::RecordedWrite::Create(&updated),
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

fn read_original(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed reading {}: {error}", path.display())),
    }
}

fn ensure_plain(path: &Path) -> Result<(), String> {
    crate::harness::claude::skills::ensure_plain_components(path)?;
    crate::harness::claude::skills::ensure_plain_file_if_present(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "orbit-codex-store-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn toml_mutation_preserves_comments_and_unknown_keys() {
        let root = temp_root();
        let home = root.join("home");
        let config = home.join("config.toml");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            &config,
            "# keep me\nunknown_key = 'keep too'\nmodel = 'old'\n",
        )
        .unwrap();
        let harness = CodexHarness::new(home, root.clone());
        mutate(
            &harness,
            ResourceKind::Settings,
            "model",
            "apply",
            &config,
            |doc| {
                let item = toml_edit::ser::to_document(&serde_json::json!({"v": "new"}))
                    .map_err(|error| error.to_string())?;
                doc["model"] = item["v"].clone();
                Ok(())
            },
        )
        .unwrap();
        let saved = fs::read_to_string(&config).unwrap();
        assert!(saved.contains("# keep me"));
        assert!(saved.contains("unknown_key = 'keep too'"));
        assert!(saved.contains("model = \"new\""));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn toml_mutation_refuses_to_overwrite_a_concurrent_change() {
        let root = temp_root();
        let home = root.join("home");
        let config = home.join("config.toml");
        fs::create_dir_all(&home).unwrap();
        fs::write(&config, "model = 'before'\n").unwrap();
        let harness = CodexHarness::new(home, root.clone());
        let result = mutate(
            &harness,
            ResourceKind::Settings,
            "model",
            "apply",
            &config,
            |doc| {
                doc["model"] = toml_edit::value("our edit");
                fs::write(&config, "model = 'concurrent edit'\n").unwrap();
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            "model = 'concurrent edit'\n"
        );
        let _ = fs::remove_dir_all(root);
    }
}
