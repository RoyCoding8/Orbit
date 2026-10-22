use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde_json::{Map, Value};

use super::{CodexHarness, store};
use crate::{
    harness::{
        Harness, ResourceKind,
        claude::config::{ConfigLayer, ConfigSnapshot, EffectiveSetting, SettingsLayer},
    },
    ledger,
};

const PROJECT_IGNORED_KEYS: &[&str] = &[
    "openai_base_url",
    "chatgpt_base_url",
    "apps_mcp_product_sku",
    "model_provider",
    "model_providers",
    "notify",
    "profile",
    "profiles",
    "experimental_realtime_ws_base_url",
    "otel",
];

// Pinned to Codex CLI v0.163.0. Validate only recognized top-level keys so
// newer/third-party keys survive edits made by an older Orbit build.
const CODEX_CONFIG_SCHEMA: &str = include_str!("../../../schemas/codex-config.json");
static CONFIG_SCHEMA: OnceLock<Result<Value, String>> = OnceLock::new();

pub fn load_config(harness: &CodexHarness, project_root: &Path) -> Result<ConfigSnapshot, String> {
    let user_path = harness.settings_path();
    let user = object_at(&user_path)?;
    let mut layers = vec![ConfigLayer {
        name: "user".into(),
        path: user_path.to_string_lossy().into_owned(),
        values: mask_map(user)?,
    }];

    if project_root.is_dir() {
        let paths = project_config_paths(project_root);
        let trusted = project_trusted(harness, project_root)?;
        for (index, project_path) in paths.iter().enumerate() {
            let selected = index + 1 == paths.len();
            let project = if trusted && project_path.is_file() {
                object_at(project_path)?
            } else {
                Map::new()
            };
            layers.push(ConfigLayer {
                name: if selected {
                    "project"
                } else {
                    "project_parent"
                }
                .into(),
                path: project_path.to_string_lossy().into_owned(),
                values: mask_map(project)?,
            });
        }
    }

    let mut effective = BTreeMap::new();
    for layer in &layers {
        for (key, value) in &layer.values {
            if layer.name.starts_with("project") && PROJECT_IGNORED_KEYS.contains(&key.as_str()) {
                continue;
            }
            effective.insert(
                key.clone(),
                EffectiveSetting {
                    value: value.clone(),
                    source: if layer.name.starts_with("project") {
                        layer.path.clone()
                    } else {
                        layer.name.clone()
                    },
                },
            );
        }
    }
    Ok(ConfigSnapshot { layers, effective })
}

pub fn apply_value(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
    key: &str,
    value: Value,
) -> Result<(), String> {
    let path = settings_path(harness, project_root, layer)?;
    let key = key.trim();
    if key.is_empty() || key.contains('.') {
        return Err("setting key must be a non-empty top-level TOML key".into());
    }
    if layer == SettingsLayer::Project && PROJECT_IGNORED_KEYS.contains(&key) {
        return Err(format!(
            "Codex ignores `{key}` in project config; edit the user config instead"
        ));
    }
    let path_for_edit = path.clone();
    store::mutate(
        harness,
        ResourceKind::Settings,
        key,
        &config_action(layer),
        &path,
        move |document| {
            let mut value = normalize_http_headers(value)?;
            if document.contains_key(key) {
                let stored = store::value(document, &path_for_edit)?;
                if let Some(previous) = stored.get(key) {
                    let previous = normalize_http_headers(previous.clone())?;
                    crate::harness::credentials::restore_masked(&previous, &mut value)
                        .map_err(|_| format!("/{key}: a masked value inside an array cannot be restored; re-enter the credential"))?;
                }
            }
            value = restore_native_http_headers(value);
            validate_top_level(key, &value)?;
            if crate::harness::credentials::contains_mask(&value) {
                return Err(format!(
                    "/{key}: value still contains a masked placeholder; enter the real credential or remove the field"
                ));
            }
            let item = json_to_item(&value)?;
            document.insert(key, item);
            Ok(())
        },
    )
}

pub fn undo(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
    key: &str,
) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("setting key cannot be empty".into());
    }
    let path = settings_path(harness, project_root, layer)?;
    ledger::undo(
        harness.config_root(),
        ResourceKind::Settings,
        key,
        &config_action(layer),
        &path,
    )
}

pub(crate) fn settings_path(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
) -> Result<PathBuf, String> {
    match layer {
        SettingsLayer::User => Ok(harness.settings_path()),
        SettingsLayer::Project => {
            if !project_root.is_dir() {
                return Err("project directory is unavailable".into());
            }
            if !project_trusted(harness, project_root)? {
                return Err(
                    "Codex does not mark this project as trusted; project config is ignored".into(),
                );
            }
            Ok(project_path(project_root))
        }
        SettingsLayer::Local => Err("Codex config supports user and project layers only".into()),
    }
}

pub(crate) fn project_trusted(harness: &CodexHarness, project_root: &Path) -> Result<bool, String> {
    let config = object_at(&harness.settings_path())?;
    let Some(projects) = config.get("projects").and_then(Value::as_object) else {
        return Ok(false);
    };
    let target = path_identity(&codex_project_root(project_root));
    Ok(projects.iter().any(|(path, config)| {
        path_identity(&codex_project_root(Path::new(path))) == target
            && config.get("trust_level").and_then(Value::as_str) == Some("trusted")
    }))
}

fn project_path(project_root: &Path) -> PathBuf {
    project_root.join(".codex").join("config.toml")
}

pub(crate) fn project_config_paths(project_root: &Path) -> Vec<PathBuf> {
    let selected_config = project_path(project_root);
    project_config_dirs(project_root)
        .into_iter()
        .map(|directory| directory.join(".codex").join("config.toml"))
        .filter(|path| path.is_file() || path_identity(path) == path_identity(&selected_config))
        .collect()
}

pub(crate) fn project_config_dirs(project_root: &Path) -> Vec<PathBuf> {
    let selected = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let repository = codex_project_root(&selected);
    let git_root = has_git_marker(&repository);
    let mut directories = vec![selected.clone()];
    let mut current = selected.as_path();
    while git_root && path_identity(current) != path_identity(&repository) {
        let Some(parent) = current.parent() else {
            break;
        };
        directories.push(parent.to_path_buf());
        current = parent;
    }
    directories.reverse();
    directories
}

fn has_git_marker(path: &Path) -> bool {
    let marker = path.join(".git");
    marker.is_dir() || marker.is_file()
}

fn config_action(layer: SettingsLayer) -> String {
    format!("codex-config-{}", layer_name(layer))
}

fn layer_name(layer: SettingsLayer) -> &'static str {
    match layer {
        SettingsLayer::User => "user",
        SettingsLayer::Project => "project",
        SettingsLayer::Local => "local",
    }
}

fn object_at(path: &Path) -> Result<Map<String, Value>, String> {
    let value = store::read_value(path)?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must contain a TOML table", path.display()))
}

fn mask_map(values: Map<String, Value>) -> Result<Map<String, Value>, String> {
    values
        .into_iter()
        .map(|(key, value)| {
            let value = normalize_http_headers(value)?;
            let masked = crate::harness::claude::config::masked_value(&key, &value, 0, false);
            let masked = restore_native_http_headers(masked);
            Ok((key, masked))
        })
        .collect()
}

fn normalize_http_headers(mut value: Value) -> Result<Value, String> {
    normalize_headers_in_place(&mut value, false)?;
    Ok(value)
}

fn normalize_headers_in_place(value: &mut Value, restore: bool) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            let from = if restore { "headers" } else { "http_headers" };
            let to = if restore { "http_headers" } else { "headers" };
            if object.contains_key(from) && object.contains_key(to) {
                return Err(format!(
                    "configuration object cannot contain both `{from}` and `{to}`"
                ));
            }
            if let Some(headers) = object.remove(from) {
                object.insert(to.into(), headers);
            }
            for child in object.values_mut() {
                normalize_headers_in_place(child, restore)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                normalize_headers_in_place(child, restore)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn restore_native_http_headers(mut value: Value) -> Value {
    // A conflict is only possible for malformed config. Retain its original
    // shape rather than replacing the user's value during a read.
    let _ = normalize_headers_in_place(&mut value, true);
    value
}

fn json_to_item(value: &Value) -> Result<toml_edit::Item, String> {
    let wrapped = serde_json::json!({ "value": value });
    let document = toml_edit::ser::to_document(&wrapped)
        .map_err(|error| format!("value cannot be represented in TOML: {error}"))?;
    document
        .get("value")
        .cloned()
        .ok_or_else(|| "value cannot be represented in TOML".into())
}

fn validate_top_level(key: &str, value: &Value) -> Result<(), String> {
    let schema = CONFIG_SCHEMA.get_or_init(|| {
        serde_json::from_str(CODEX_CONFIG_SCHEMA)
            .map_err(|error| format!("invalid bundled Codex config schema: {error}"))
    });
    let schema = schema.as_ref().map_err(Clone::clone)?;
    let Some(property) = schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(key))
    else {
        return Ok(());
    };
    let mut scoped = schema.clone();
    scoped["properties"] = serde_json::json!({ key: property });
    scoped["additionalProperties"] = Value::Bool(true);
    let validator = jsonschema::validator_for(&scoped)
        .map_err(|_| "bundled Codex config schema could not be initialized".to_string())?;
    let candidate = serde_json::json!({ key: value });
    if let Some(error) = validator.iter_errors(&candidate).next() {
        return Err(format!(
            "Codex setting `{key}` has an invalid value ({})",
            error.instance_path
        ));
    }
    Ok(())
}

fn codex_project_root(path: &Path) -> PathBuf {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut current = path.as_path();
    while current.parent().is_some() {
        if has_git_marker(current) {
            return current.to_path_buf();
        }
        current = current.parent().expect("checked above");
    }
    path
}

fn path_identity(path: &Path) -> String {
    let value = path
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/");
    #[cfg(windows)]
    {
        value.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn fixture() -> (PathBuf, CodexHarness, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "orbit-codex-config-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join(".codex");
        let project = root.join("project");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        let harness = CodexHarness::new(home.clone(), root.clone());
        (root, harness, project)
    }

    #[test]
    fn load_masks_nested_secrets_and_uses_trusted_project_override() {
        let (root, harness, project) = fixture();
        let project_key = project.to_string_lossy().replace('\\', "/");
        fs::write(harness.settings_path(), format!("model = 'base'\n\n[env]\nAPI_TOKEN = 'private'\n\n[projects.\"{project_key}\"]\ntrust_level = 'trusted'\n")).unwrap();
        fs::create_dir_all(project.join(".codex")).unwrap();
        fs::write(
            project.join(".codex/config.toml"),
            "model = 'project'\ncustom = true\n",
        )
        .unwrap();
        let snapshot = load_config(&harness, &project).unwrap();
        assert_eq!(
            snapshot.layers[0].values["env"]["API_TOKEN"],
            crate::harness::credentials::MASK
        );
        assert_eq!(snapshot.effective["model"].value, "project");
        assert_eq!(
            snapshot.effective["model"].source,
            project
                .join(".codex/config.toml")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_writes_require_native_trust_and_local_layer_is_rejected() {
        let (root, harness, project) = fixture();
        assert!(settings_path(&harness, &project, SettingsLayer::Project).is_err());
        assert!(settings_path(&harness, &project, SettingsLayer::Local).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn null_config_values_are_rejected_at_the_toml_boundary() {
        assert!(json_to_item(&Value::Null).is_err());
    }

    #[test]
    fn validates_known_schema_keys_but_preserves_unknown_keys() {
        assert!(validate_top_level("approval_policy", &Value::String("never".into())).is_ok());
        assert!(validate_top_level("approval_policy", &Value::Number(12.into())).is_err());
        assert!(validate_top_level("future_setting", &Value::Bool(true)).is_ok());
    }

    #[test]
    fn project_ignored_keys_do_not_appear_in_effective_settings() {
        let (root, harness, project) = fixture();
        let key = project.to_string_lossy().replace('\\', "/");
        fs::write(
            harness.settings_path(),
            format!("model = 'base'\n\n[projects.\"{key}\"]\ntrust_level = 'trusted'\n"),
        )
        .unwrap();
        fs::create_dir_all(project.join(".codex")).unwrap();
        fs::write(
            project.join(".codex/config.toml"),
            "model_provider = 'ignored'\nmodel = 'project'\n",
        )
        .unwrap();
        let snapshot = load_config(&harness, &project).unwrap();
        assert_eq!(snapshot.layers[1].values["model_provider"], "ignored");
        assert!(!snapshot.effective.contains_key("model_provider"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nested_trusted_project_configs_merge_from_git_root_to_selected_directory() {
        let (root, harness, project) = fixture();
        let repository = project.join("repo");
        let nested = repository.join("packages").join("app");
        fs::create_dir_all(repository.join(".git")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        let repository_key = repository.to_string_lossy().replace('\\', "/");
        fs::write(
            harness.settings_path(),
            format!("model = 'user'\n\n[projects.\"{repository_key}\"]\ntrust_level = 'trusted'\n"),
        )
        .unwrap();
        fs::create_dir_all(repository.join(".codex")).unwrap();
        fs::write(
            repository.join(".codex/config.toml"),
            "model = 'root'\nroot_only = true\n",
        )
        .unwrap();
        fs::create_dir_all(nested.join(".codex")).unwrap();
        fs::write(
            nested.join(".codex/config.toml"),
            "model = 'nested'\nnested_only = true\n",
        )
        .unwrap();

        let snapshot = load_config(&harness, &nested).unwrap();
        assert_eq!(snapshot.layers.len(), 3);
        assert_eq!(snapshot.layers[1].name, "project_parent");
        assert_eq!(snapshot.layers[2].name, "project");
        assert_eq!(snapshot.effective["model"].value, "nested");
        assert_eq!(
            snapshot.effective["model"].source,
            nested
                .join(".codex/config.toml")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
        assert_eq!(
            snapshot.effective["root_only"].source,
            repository
                .join(".codex/config.toml")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn masked_settings_restore_credentials_from_the_document_being_mutated() {
        let (root, harness, project) = fixture();
        fs::write(
            harness.settings_path(),
            "[env]\nAPI_TOKEN = 'current-token'\nMODE = 'old'\n",
        )
        .unwrap();
        apply_value(
            &harness,
            &project,
            SettingsLayer::User,
            "env",
            serde_json::json!({
                "API_TOKEN": crate::harness::credentials::MASK,
                "MODE": "new"
            }),
        )
        .unwrap();
        let saved = store::read_value(&harness.settings_path()).unwrap();
        assert_eq!(saved["env"]["API_TOKEN"], "current-token");
        assert_eq!(saved["env"]["MODE"], "new");
        let _ = fs::remove_dir_all(root);
    }
}
