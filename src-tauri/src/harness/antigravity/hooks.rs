use super::AntigravityHarness;
use crate::harness::{
    atomic,
    claude::{config::SettingsLayer, hooks::HookView},
    json_store, Harness, HookDraft, ResourceKind,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const EVENTS: [&str; 6] = [
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "PreInvocation",
    "PostInvocation",
    "Stop",
];

struct Row {
    view: HookView,
    group: String,
    pointer: String,
}

pub fn path(
    harness: &AntigravityHarness,
    project: &Path,
    layer: SettingsLayer,
) -> Result<PathBuf, String> {
    match layer {
        SettingsLayer::User => Ok(harness.config_root().join("hooks.json")),
        SettingsLayer::Project => Ok(harness.project_root(project).join("hooks.json")),
        SettingsLayer::Local => Err("Antigravity hooks have user and project scopes only".into()),
    }
}

fn escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

fn rows(document: &Value, layer: SettingsLayer, source: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    for (name, group) in document.as_object().into_iter().flatten() {
        for event in EVENTS {
            for (i, entry) in group
                .get(event)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let base = format!("/{}/{event}/{i}", escape(name));
                let commands = if let Some(hooks) = entry.get("hooks").and_then(Value::as_array) {
                    hooks
                        .iter()
                        .enumerate()
                        .map(|(j, hook)| (hook, format!("{base}/hooks/{j}")))
                        .collect::<Vec<_>>()
                } else {
                    vec![(entry, base)]
                };
                for (command, pointer) in commands {
                    let Some(text) = command.get("command").and_then(Value::as_str) else {
                        continue;
                    };
                    if command
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t != "command")
                    {
                        continue;
                    }
                    let id = atomic::hex(&Sha256::digest(
                        format!("{}\0{pointer}\0{command}", source.display()).as_bytes(),
                    ));
                    rows.push(Row {
                        group: name.clone(),
                        pointer,
                        view: HookView {
                            id,
                            event: event.into(),
                            matcher: entry
                                .get("matcher")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            commands: vec![text.into()],
                            enabled: group.get("enabled").and_then(Value::as_bool) != Some(false),
                            layer: layer.label().into(),
                            source_path: source.to_string_lossy().into_owned(),
                            editable: true,
                        },
                    });
                }
            }
        }
    }
    rows
}

pub fn list(harness: &AntigravityHarness, project: &Path) -> Result<Vec<HookView>, String> {
    let mut result = Vec::new();
    for layer in [SettingsLayer::User, SettingsLayer::Project] {
        let path = path(harness, project, layer)?;
        result.extend(
            rows(&json_store::read_value(&path)?, layer, &path)
                .into_iter()
                .map(|r| r.view),
        );
    }
    Ok(result)
}

pub fn save(harness: &AntigravityHarness, project: &Path, draft: HookDraft) -> Result<(), String> {
    if !EVENTS.contains(&draft.event.as_str()) {
        return Err("Unsupported Antigravity hook event".into());
    }
    if draft.command.trim().is_empty() {
        return Err("Hook command cannot be empty".into());
    }
    let path = path(harness, project, draft.layer)?;
    json_store::mutate_json(
        harness,
        ResourceKind::Settings,
        "hooks",
        "antigravity-hooks",
        &path,
        |doc| {
            if let Some(id) = &draft.edit_id {
                let row = rows(doc, draft.layer, &path)
                    .into_iter()
                    .find(|r| &r.view.id == id)
                    .ok_or("Hook changed. Refresh hooks before editing.")?;
                if row.view.event != draft.event {
                    return Err("Remove and recreate a hook to change its event".into());
                }
                doc.pointer_mut(&format!("{}/command", row.pointer))
                    .ok_or("Hook command is missing")?
                    .clone_from(&Value::String(draft.command));
                let base = row.pointer.split("/hooks/").next().unwrap_or(&row.pointer);
                let entry = doc
                    .pointer_mut(base)
                    .and_then(Value::as_object_mut)
                    .ok_or("Hook entry is missing")?;
                if let Some(matcher) = draft.matcher.filter(|m| !m.is_empty()) {
                    entry.insert("matcher".into(), Value::String(matcher));
                } else {
                    entry.remove("matcher");
                }
            } else {
                let command = json!({"type":"command","command":draft.command});
                let entry = if matches!(draft.event.as_str(), "PreToolUse" | "PostToolUse") {
                    json!({"matcher":draft.matcher.unwrap_or_default(),"hooks":[command]})
                } else {
                    command
                };
                doc.as_object_mut()
                    .ok_or("hooks.json must contain an object")?
                    .insert(
                        format!("orbit-{}", uuid::Uuid::new_v4()),
                        json!({(draft.event):[entry]}),
                    );
            }
            Ok(())
        },
    )
}

pub fn set_enabled(
    harness: &AntigravityHarness,
    project: &Path,
    layer: SettingsLayer,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let path = path(harness, project, layer)?;
    json_store::mutate_json(
        harness,
        ResourceKind::Settings,
        "hooks",
        "antigravity-hooks",
        &path,
        |doc| {
            let row = rows(doc, layer, &path)
                .into_iter()
                .find(|r| r.view.id == id)
                .ok_or("Hook changed. Refresh hooks.")?;
            doc[&row.group]["enabled"] = Value::Bool(enabled);
            Ok(())
        },
    )
}

pub fn delete(
    harness: &AntigravityHarness,
    project: &Path,
    layer: SettingsLayer,
    id: &str,
) -> Result<(), String> {
    let path = path(harness, project, layer)?;
    json_store::mutate_json(
        harness,
        ResourceKind::Settings,
        "hooks",
        "antigravity-hooks",
        &path,
        |doc| {
            let row = rows(doc, layer, &path)
                .into_iter()
                .find(|r| r.view.id == id)
                .ok_or("Hook changed. Refresh hooks.")?;
            let (array, index) = row
                .pointer
                .rsplit_once('/')
                .ok_or("Invalid hook location")?;
            doc.pointer_mut(array)
                .and_then(Value::as_array_mut)
                .ok_or("Hook list is missing")?
                .remove(index.parse::<usize>().map_err(|e| e.to_string())?);
            Ok(())
        },
    )
}
