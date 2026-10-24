use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use jsonc_parser::{
    cst::{CstInputValue, CstRootNode},
    ParseOptions,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::OpenCodeHarness;
use crate::{
    harness::{
        atomic::RecordedWrite,
        claude::config::{
            masked_value, ConfigLayer, ConfigSnapshot, EffectiveSetting, SettingsLayer,
        },
        claude::skills::{ensure_plain_components, ensure_plain_file_if_present},
        credentials::{contains_mask, restore_masked},
        Harness, ResourceKind,
    },
    ledger,
};

pub fn load_config(
    harness: &OpenCodeHarness,
    project_root: &Path,
) -> Result<ConfigSnapshot, String> {
    let mut layers = vec![read_layer("user", config_path(harness.config_root()))?];
    if project_root.is_dir() {
        let active = project_config_path(project_root);
        for path in project_config_paths(project_root) {
            layers.push(read_layer(
                if path == active {
                    "project"
                } else {
                    "project_parent"
                },
                path,
            )?);
        }
    }

    let mut effective = BTreeMap::new();
    for layer in &layers {
        for (key, value) in &layer.values {
            let masked = masked_value(key, value, 0, false);
            effective
                .entry(key.clone())
                .and_modify(|setting: &mut EffectiveSetting| {
                    merge_effective_value(&mut setting.value, masked.clone());
                    setting.source = layer.name.clone();
                })
                .or_insert_with(|| EffectiveSetting {
                    value: masked,
                    source: layer.name.clone(),
                });
        }
    }
    Ok(ConfigSnapshot { layers, effective })
}

pub fn apply_value(
    harness: &OpenCodeHarness,
    project_root: &Path,
    layer: SettingsLayer,
    key: &str,
    value: Value,
) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.contains('.') {
        return Err("setting key must be a non-empty top-level OpenCode config key".into());
    }
    let path = match layer {
        SettingsLayer::User => config_path(harness.config_root()),
        SettingsLayer::Project => {
            if !project_root.is_dir() {
                return Err("project directory is unavailable".into());
            }
            project_config_path(project_root)
        }
        SettingsLayer::Local => {
            return Err("OpenCode V2 has user and project config layers only".into())
        }
    };
    mutate_jsonc(
        harness,
        &path,
        key,
        &format!("opencode-config-{}", layer.label()),
        |object| {
            let mut value = value;
            if let Some(previous) = object
                .get(key)
                .and_then(|property| property.to_serde_value())
            {
                restore_masked(&previous, &mut value)
                    .map_err(|_| format!("/{key}: a masked value inside an array cannot be restored; re-enter the credential"))?;
            }
            if contains_mask(&value) {
                return Err(format!(
                    "/{key}: value still contains a masked placeholder; enter the real credential or remove the field"
                ));
            }
            if let Some(property) = object.get(key) {
                property.set_value(cst_input_value(value));
            } else {
                object.append(key, cst_input_value(value));
            }
            Ok(())
        },
    )
}

pub fn undo(
    harness: &OpenCodeHarness,
    project_root: &Path,
    layer: SettingsLayer,
    key: &str,
) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("setting key cannot be empty".into());
    }
    let path = match layer {
        SettingsLayer::User => config_path(harness.config_root()),
        SettingsLayer::Project => project_config_path(project_root),
        SettingsLayer::Local => {
            return Err("OpenCode V2 has user and project config layers only".into())
        }
    };
    ledger::undo(
        harness.config_root(),
        ResourceKind::Settings,
        key,
        &format!("opencode-config-{}", layer.label()),
        &path,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn config_path(root: &Path) -> PathBuf {
    select_config_file(root)
}

pub(crate) fn project_config_path(project_root: &Path) -> PathBuf {
    let nested = select_config_file(&project_root.join(".opencode"));
    if nested.is_file() {
        nested
    } else {
        select_config_file(project_root)
    }
}

pub(crate) fn project_config_paths(project_root: &Path) -> Vec<PathBuf> {
    let active = project_config_path(project_root);
    let ancestors = project_root.ancestors().collect::<Vec<_>>();
    let mut paths = Vec::new();
    for nested in [false, true] {
        for ancestor in ancestors.iter().rev() {
            let root = if nested {
                ancestor.join(".opencode")
            } else {
                ancestor.to_path_buf()
            };
            let path = select_config_file(&root);
            if path.is_file() || path == active {
                paths.push(path);
            }
        }
    }
    paths
}

fn merge_effective_value(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    merge_effective_value(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn select_config_file(directory: &Path) -> PathBuf {
    let jsonc = directory.join("opencode.jsonc");
    let json = directory.join("opencode.json");
    if jsonc.is_file() {
        jsonc
    } else {
        json
    }
}

fn read_layer(name: &str, path: PathBuf) -> Result<ConfigLayer, String> {
    let values = read_object(&path)?
        .into_iter()
        .map(|(key, value)| {
            let masked = crate::harness::claude::config::masked_value(&key, &value, 0, false);
            (key, masked)
        })
        .collect();
    Ok(ConfigLayer {
        name: name.into(),
        path: path.to_string_lossy().into_owned(),
        values,
    })
}

fn read_object(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("failed reading {}: {error}", path.display())),
    };
    let root = CstRootNode::parse(&text, &ParseOptions::default())
        .map_err(|error| format!("failed parsing {}: {error}", path.display()))?;
    let value = root
        .to_serde_value()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))
}

pub(crate) fn read_config_value(path: &Path) -> Result<Value, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Value::Object(Map::new()))
        }
        Err(error) => return Err(format!("failed reading {}: {error}", path.display())),
    };
    CstRootNode::parse(&text, &ParseOptions::default())
        .map_err(|error| format!("failed parsing {}: {error}", path.display()))?
        .to_serde_value()
        .ok_or_else(|| format!("{} does not contain a JSON value", path.display()))
}

pub(crate) fn cst_input_value(value: Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(value) => CstInputValue::Bool(value),
        Value::Number(value) => CstInputValue::Number(value.to_string()),
        Value::String(value) => CstInputValue::String(value),
        Value::Array(values) => {
            CstInputValue::Array(values.into_iter().map(cst_input_value).collect())
        }
        Value::Object(values) => CstInputValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, cst_input_value(value)))
                .collect(),
        ),
    }
}

pub(crate) fn mutate_jsonc(
    harness: &OpenCodeHarness,
    path: &Path,
    id: &str,
    action: &str,
    edit: impl FnOnce(&jsonc_parser::cst::CstObject) -> Result<(), String>,
) -> Result<(), String> {
    ensure_plain_components(path)?;
    ensure_plain_file_if_present(path)?;
    let original = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("failed reading {}: {error}", path.display())),
    };
    let text = original
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .map_err(|_| format!("{} is not valid UTF-8 JSONC", path.display()))?
        .unwrap_or("{}");
    let root = CstRootNode::parse(text, &ParseOptions::default())
        .map_err(|error| format!("failed parsing {}: {error}", path.display()))?;
    if !root.to_serde_value().is_some_and(|value| value.is_object()) {
        return Err(format!("{} must contain a JSON object", path.display()));
    }
    edit(&root.object_value_or_set())?;
    let updated = root.to_string();
    if original.as_deref() == Some(updated.as_bytes()) {
        return Ok(());
    }
    let change = match original {
        Some(bytes) => RecordedWrite::Restore(Sha256::digest(&bytes).into(), updated.as_bytes()),
        None => RecordedWrite::Create(updated.as_bytes()),
    };
    ensure_plain_components(path)?;
    ensure_plain_file_if_present(path)?;
    let backup = ledger::backup_path(harness.config_root(), id, action);
    ledger::write(
        harness.config_root(),
        ResourceKind::Settings,
        id,
        action,
        path,
        change,
        Some(&backup),
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tempdir;

    #[test]
    fn config_edit_preserves_jsonc_comments_and_undo_restores_bytes() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        let harness = OpenCodeHarness::new(home.clone(), temp.path().to_owned());
        let path = home.join("opencode.jsonc");
        let original = b"{\n  // keep this note\n  \"model\": \"anthropic/claude-sonnet-4\",\n}\n";
        fs::write(&path, original).unwrap();
        apply_value(
            &harness,
            &project,
            SettingsLayer::User,
            "model",
            Value::String("openai/gpt-5".into()),
        )
        .unwrap();
        let edited = fs::read_to_string(&path).unwrap();
        assert!(edited.contains("// keep this note"));
        assert!(edited.contains("openai/gpt-5"));
        undo(&harness, &project, SettingsLayer::User, "model").unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn project_config_is_separate_from_user_config() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        let harness = OpenCodeHarness::new(home.clone(), temp.path().to_owned());
        fs::write(home.join("opencode.json"), r#"{"model":"user"}"#).unwrap();
        fs::write(project.join("opencode.json"), r#"{"model":"project"}"#).unwrap();
        let snapshot = load_config(&harness, &project).unwrap();
        assert_eq!(snapshot.layers.len(), 2);
        assert_eq!(snapshot.effective["model"].value, "project");
        assert_eq!(snapshot.effective["model"].source, "project");
        fs::create_dir(project.join(".opencode")).unwrap();
        let nested = project.join(".opencode/opencode.jsonc");
        let original = b"{\n// nested wins\n\"model\":\"nested\"\n}";
        fs::write(&nested, original).unwrap();
        let snapshot = load_config(&harness, &project).unwrap();
        let active = snapshot
            .layers
            .iter()
            .find(|layer| layer.name == "project")
            .unwrap();
        assert_eq!(Path::new(&active.path), nested);
        assert_eq!(snapshot.effective["model"].value, "nested");
        apply_value(
            &harness,
            &project,
            SettingsLayer::Project,
            "model",
            Value::String("edited".into()),
        )
        .unwrap();
        assert_eq!(read_config_value(&nested).unwrap()["model"], "edited");
        assert_eq!(
            read_config_value(&project.join("opencode.json")).unwrap()["model"],
            "project"
        );
        undo(&harness, &project, SettingsLayer::Project, "model").unwrap();
        assert_eq!(fs::read(&nested).unwrap(), original);
        let child = project.join("child");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("opencode.json"), r#"{"model":"child"}"#).unwrap();
        let snapshot = load_config(&harness, &child).unwrap();
        assert_eq!(snapshot.effective["model"].value, "nested");
        assert_eq!(snapshot.effective["model"].source, "project_parent");
        assert_eq!(
            Path::new(
                &snapshot
                    .layers
                    .iter()
                    .find(|layer| layer.name == "project")
                    .unwrap()
                    .path
            ),
            child.join("opencode.json")
        );
    }
}
