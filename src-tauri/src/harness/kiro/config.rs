use std::path::{Path, PathBuf};

use serde_json::Value;

use super::KiroHarness;
use crate::harness::json_store as store;
use crate::{
    harness::{
        claude::config::{ConfigSnapshot, EffectiveSetting, SettingsLayer},
        Harness, ResourceKind,
    },
    ledger,
};

pub fn load_config(harness: &KiroHarness, project_root: &Path) -> Result<ConfigSnapshot, String> {
    let mut layers = vec![store::settings_layer("user", &harness.settings_path())?];
    if project_root.is_dir() {
        layers.push(store::settings_layer(
            "project",
            &harness.project_settings_path(project_root),
        )?);
    }

    let mut effective = std::collections::BTreeMap::new();
    for layer in &layers {
        for (key, value) in &layer.values {
            effective.insert(
                key.clone(),
                EffectiveSetting {
                    value: value.clone(),
                    source: layer.name.clone(),
                },
            );
        }
    }
    Ok(ConfigSnapshot { layers, effective })
}

pub fn apply_value(
    harness: &KiroHarness,
    project: &Path,
    layer: SettingsLayer,
    key: &str,
    value: Value,
) -> Result<(), String> {
    store::apply_setting(
        harness,
        &format!("kiro-config-{}", layer.label()),
        &settings_path(harness, project, layer)?,
        key,
        value,
    )
}

pub fn undo(
    harness: &KiroHarness,
    project_root: &Path,
    layer: SettingsLayer,
    key: &str,
) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("setting key must not be empty".into());
    }
    let path = settings_path(harness, project_root, layer)?;
    ledger::undo(
        harness.config_root(),
        ResourceKind::Settings,
        key,
        &format!("kiro-config-{}", layer.label()),
        &path,
    )
}

pub(crate) fn settings_path(
    harness: &KiroHarness,
    project_root: &Path,
    layer: SettingsLayer,
) -> Result<PathBuf, String> {
    match layer {
        SettingsLayer::User => Ok(harness.settings_path()),
        SettingsLayer::Project if project_root.is_dir() => {
            Ok(harness.project_settings_path(project_root))
        }
        SettingsLayer::Project => Err("project directory is unavailable".into()),
        SettingsLayer::Local => {
            Err("Kiro CLI settings support user and project layers only".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn layers_overlay_and_keep_native_dotted_setting_names() {
        let root = tempfile::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));
        let project = root.path().join("project");
        std::fs::create_dir_all(project.join(".kiro/settings")).unwrap();
        std::fs::create_dir_all(harness.home().join("settings")).unwrap();
        std::fs::write(
            harness.settings_path(),
            br#"{"chat.model":"global","chat.agentEngine":"v2"}"#,
        )
        .unwrap();
        std::fs::write(
            harness.project_settings_path(&project),
            br#"{"chat.model":"project"}"#,
        )
        .unwrap();

        let snapshot = load_config(&harness, &project).unwrap();

        assert_eq!(snapshot.layers.len(), 2);
        assert_eq!(snapshot.effective["chat.model"].value, "project");
        assert_eq!(snapshot.effective["chat.model"].source, "project");
        assert_eq!(snapshot.effective["chat.agentEngine"].value, "v2");
    }

    #[test]
    fn applying_masked_setting_preserves_existing_credential() {
        let root = tempfile::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));
        std::fs::create_dir_all(harness.home().join("settings")).unwrap();
        std::fs::write(
            harness.settings_path(),
            br#"{"provider":{"apiKey":"secret-fixture","model":"old"}}"#,
        )
        .unwrap();

        apply_value(
            &harness,
            root.path(),
            SettingsLayer::User,
            "provider",
            json!({"apiKey":"••••••••","model":"new"}),
        )
        .unwrap();

        let saved: Value =
            serde_json::from_slice(&std::fs::read(harness.settings_path()).unwrap()).unwrap();
        assert_eq!(saved["provider"]["apiKey"], "secret-fixture");
        assert_eq!(saved["provider"]["model"], "new");
    }
}
