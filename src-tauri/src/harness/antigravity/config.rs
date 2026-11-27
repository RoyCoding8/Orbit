use super::AntigravityHarness;
use crate::{
    harness::{
        claude::config::{masked_value, ConfigSnapshot, EffectiveSetting, SettingsLayer},
        credentials, json_store, Harness, ResourceKind,
    },
    ledger,
};
use serde_json::Value;
use std::path::Path;

fn mask_setting(key: &str, value: &Value) -> Value {
    match value {
        Value::String(text)
            if !text.is_empty()
                && (credentials::secret_name(key) || credentials::value_shape_secret(text)) =>
        {
            Value::String(credentials::MASK.into())
        }
        _ => masked_value(key, value, 0, false),
    }
}

pub fn load_config(
    harness: &AntigravityHarness,
    _project: &Path,
) -> Result<ConfigSnapshot, String> {
    let mut layer = json_store::settings_layer("user", &harness.settings_path())?;
    for (key, value) in &mut layer.values {
        *value = mask_setting(key, value);
    }
    let effective = layer
        .values
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                EffectiveSetting {
                    value: value.clone(),
                    source: "user".into(),
                },
            )
        })
        .collect();
    Ok(ConfigSnapshot {
        layers: vec![layer],
        effective,
    })
}

pub fn apply_value(
    harness: &AntigravityHarness,
    _project: &Path,
    layer: SettingsLayer,
    key: &str,
    value: Value,
) -> Result<(), String> {
    check_layer(layer)?;
    json_store::apply_setting(
        harness,
        "antigravity-cli-settings",
        &harness.settings_path(),
        key,
        value,
    )
}

pub fn undo(
    harness: &AntigravityHarness,
    _project: &Path,
    layer: SettingsLayer,
    key: &str,
) -> Result<(), String> {
    check_layer(layer)?;
    ledger::undo(
        harness.config_root(),
        ResourceKind::Settings,
        key,
        "antigravity-cli-settings",
        &harness.settings_path(),
    )
}

fn check_layer(layer: SettingsLayer) -> Result<(), String> {
    if layer != SettingsLayer::User {
        return Err("Antigravity CLI preferences are user-wide. App and project preferences are managed in Antigravity.".into());
    }
    Ok(())
}
