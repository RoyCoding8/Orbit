use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};

use super::PiHarness;
use crate::{
    harness::{
        claude::config::{ConfigSnapshot, EffectiveSetting, SettingsLayer},
        json_store as store, Harness, ResourceKind,
    },
    ledger,
};

pub fn load_config(harness: &PiHarness, project: &Path) -> Result<ConfigSnapshot, String> {
    let mut layers = vec![store::settings_layer("user", &harness.settings_path())?];
    if project.is_dir() {
        layers.push(store::settings_layer(
            "project",
            &harness.project_settings_path(project),
        )?);
    }

    let mut merged = Map::new();
    let mut sources = BTreeMap::new();
    for layer in &layers {
        for (key, value) in &layer.values {
            if is_resource_array(key) && value.is_array() {
                if let Some(dst) = merged.get_mut(key).and_then(Value::as_array_mut) {
                    dst.extend(value.as_array().into_iter().flatten().cloned());
                } else {
                    merged.insert(key.clone(), value.clone());
                }
            } else {
                merge_value(
                    merged.entry(key.clone()).or_insert(Value::Null),
                    value.clone(),
                );
            }
            sources.insert(key.clone(), layer.name.clone());
        }
    }
    let effective = merged
        .into_iter()
        .map(|(key, value)| {
            let source = sources.remove(&key).unwrap_or_else(|| "user".into());
            (key, EffectiveSetting { value, source })
        })
        .collect();
    Ok(ConfigSnapshot { layers, effective })
}

pub fn apply_value(
    harness: &PiHarness,
    project: &Path,
    layer: SettingsLayer,
    key: &str,
    value: Value,
) -> Result<(), String> {
    store::apply_setting(
        harness,
        &format!("pi-config-{}", layer.label()),
        &settings_path(harness, project, layer)?,
        key,
        value,
    )
}

pub fn undo(
    harness: &PiHarness,
    project: &Path,
    layer: SettingsLayer,
    key: &str,
) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("setting key must not be empty".into());
    }
    ledger::undo(
        harness.config_root(),
        ResourceKind::Settings,
        key,
        &format!("pi-config-{}", layer.label()),
        &settings_path(harness, project, layer)?,
    )
    .map_err(|error| error.to_string())
}

fn settings_path(
    harness: &PiHarness,
    project: &Path,
    layer: SettingsLayer,
) -> Result<PathBuf, String> {
    match layer {
        SettingsLayer::User => Ok(harness.settings_path()),
        SettingsLayer::Project if project.is_dir() => Ok(harness.project_settings_path(project)),
        SettingsLayer::Project => Err("project directory is unavailable".into()),
        SettingsLayer::Local => Err("Pi settings support user and project layers only".into()),
    }
}

fn merge_value(target: &mut Value, incoming: Value) {
    if let (Some(target), Some(incoming)) = (target.as_object_mut(), incoming.as_object()) {
        for (key, value) in incoming {
            merge_value(
                target.entry(key.clone()).or_insert(Value::Null),
                value.clone(),
            );
        }
    } else {
        *target = incoming;
    }
}

fn is_resource_array(key: &str) -> bool {
    matches!(
        key,
        "skills" | "packages" | "extensions" | "prompts" | "themes"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_strip_bom_merge_objects_and_track_the_project_source() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = PiHarness::new(root.path().join(".pi/agent"), root.path().to_path_buf());
        let project = root.path().join("project");
        std::fs::create_dir_all(harness.config_root()).unwrap();
        std::fs::create_dir_all(project.join(".pi")).unwrap();
        std::fs::write(
            harness.settings_path(),
            b"\xef\xbb\xbf{\"theme\":\"dark\",\"ui\":{\"a\":1},\"skills\":[\"global\"]}",
        )
        .unwrap();
        std::fs::write(
            harness.project_settings_path(&project),
            br#"{"ui":{"b":2},"skills":["project"]}"#,
        )
        .unwrap();

        let snapshot = load_config(&harness, &project).unwrap();
        assert_eq!(snapshot.effective["theme"].value, "dark");
        assert_eq!(snapshot.effective["ui"].value, json!({"a":1,"b":2}));
        assert_eq!(snapshot.effective["ui"].source, "project");
        assert_eq!(
            snapshot.effective["skills"].value,
            json!(["global", "project"])
        );
    }
}
