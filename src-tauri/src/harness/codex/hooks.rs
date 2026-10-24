use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{config, store, CodexHarness};
use crate::harness::{
    claude::{
        config::SettingsLayer,
        hooks::{self, HookTestResult, HookView},
    },
    Harness, ResourceKind,
};

const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "Stop",
    "SubagentStart",
    "SubagentStop",
    "Interrupt",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Json,
    Toml,
}

#[derive(Debug, Clone)]
struct Source {
    layer: SettingsLayer,
    format: Format,
    path: PathBuf,
    editable: bool,
}

#[derive(Debug, Clone)]
struct Located {
    source: Source,
    event: String,
    index: usize,
    entry: Value,
    id: String,
}

pub fn list(harness: &CodexHarness, project_root: &Path) -> Result<Vec<HookView>, String> {
    let mut output = Vec::new();
    for layer in [SettingsLayer::User, SettingsLayer::Project] {
        if layer == SettingsLayer::Project && !config::project_trusted(harness, project_root)? {
            continue;
        }
        for source in sources(harness, project_root, layer)? {
            let hooks = read_source(&source)?;
            for (event, groups) in hooks {
                let Some(groups) = groups.as_array() else {
                    continue;
                };
                for (index, entry) in groups.iter().enumerate() {
                    let id = entry_id(&source, &event, index, entry);
                    let matcher = entry
                        .get("matcher")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let commands = command_handlers(entry);
                    output.push(HookView {
                        id,
                        event: event.clone(),
                        matcher,
                        editable: source.editable && !commands.is_empty(),
                        commands,
                        enabled: true,
                        layer: source_layer_name(&source).into(),
                        source_path: source.path.to_string_lossy().into_owned(),
                    });
                }
            }
        }
    }
    output.sort_by(|a, b| {
        a.event
            .cmp(&b.event)
            .then(a.layer.cmp(&b.layer))
            .then(a.id.cmp(&b.id))
    });
    Ok(output)
}

pub fn save(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
    event: &str,
    matcher: Option<String>,
    command: String,
    edit_id: Option<&str>,
) -> Result<(), String> {
    ensure_layer(harness, project_root, layer)?;
    validate_event(event)?;
    if command.trim().is_empty() {
        return Err("hook command must not be empty".into());
    }
    let (source, old) = if let Some(id) = edit_id {
        let located = locate(harness, project_root, layer, id)?;
        if located.source.editable && !command_handlers(&located.entry).is_empty() {
            (located.source.clone(), Some(located))
        } else if !located.source.editable {
            return Err(
                "inherited Codex project hooks are read-only; edit the source layer directly"
                    .into(),
            );
        } else {
            return Err("this Codex hook uses a non-command handler and is read-only here".into());
        }
    } else {
        (write_source(harness, project_root, layer)?, None)
    };
    let action = format!(
        "codex-hooks-save-{}-{}",
        layer_name(layer),
        format_name(source.format)
    );
    let source_for_check = source.clone();
    edit_source(harness, &source, &action, move |hooks| {
        if let Some(old) = old {
            let old_groups = hooks
                .get_mut(&old.event)
                .and_then(Value::as_array_mut)
                .ok_or("hook changed since it was loaded; refresh and retry")?;
            let current = old_groups
                .get(old.index)
                .ok_or("hook changed since it was loaded; refresh and retry")?;
            if entry_id(&source_for_check, &old.event, old.index, current) != old.id {
                return Err("hook changed since it was loaded; refresh and retry".into());
            }
            let mut updated = current.clone();
            let object = updated
                .as_object_mut()
                .ok_or("hook matcher group must be an object")?;
            set_optional_string(object, "matcher", matcher);
            let handlers = object
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .ok_or("hook handlers must be an array")?;
            let handler = handlers
                .iter_mut()
                .find(|item| item.get("type").and_then(Value::as_str) == Some("command"))
                .ok_or("hook has no command handler")?;
            handler
                .as_object_mut()
                .ok_or("hook handler must be an object")?
                .insert("command".into(), Value::String(command));
            old_groups.remove(old.index);
            let old_empty = old_groups.is_empty();
            if old_empty {
                hooks.remove(&old.event);
            }
            event_groups_mut(hooks, event)?.push(updated);
        } else {
            let groups = event_groups_mut(hooks, event)?;
            let mut new_entry =
                serde_json::json!({"hooks": [{"type":"command", "command":command}]});
            if let Some(matcher) = matcher {
                new_entry["matcher"] = Value::String(matcher);
            }
            if groups.iter().any(|existing| existing == &new_entry) {
                return Err("an identical hook already exists in this event and layer".into());
            }
            groups.push(new_entry);
        }
        Ok(())
    })
}

pub fn set_enabled(
    _harness: &CodexHarness,
    _project_root: &Path,
    _layer: SettingsLayer,
    _id: &str,
    _enabled: bool,
) -> Result<(), String> {
    Err("Codex stores hook review and disable state against its native hook hash; manage trust in Codex with `/hooks`. Orbit leaves hook definitions intact.".into())
}

pub fn delete(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
    id: &str,
) -> Result<(), String> {
    ensure_layer(harness, project_root, layer)?;
    let located = locate(harness, project_root, layer, id)?;
    if !located.source.editable {
        return Err(
            "inherited Codex project hooks are read-only; edit the source layer directly".into(),
        );
    }
    if command_handlers(&located.entry).is_empty() {
        return Err("this Codex hook uses a non-command handler and is read-only here".into());
    }
    let source = located.source.clone();
    let event = located.event.clone();
    let id = located.id.clone();
    let index = located.index;
    let source_for_check = source.clone();
    let action = format!(
        "codex-hooks-delete-{}-{}",
        layer_name(layer),
        format_name(source.format)
    );
    edit_source(harness, &source, &action, move |hooks| {
        let groups = event_groups_mut(hooks, &event)?;
        let current = groups
            .get(index)
            .ok_or("hook changed since it was loaded; refresh and retry")?;
        if entry_id(&source_for_check, &event, index, current) != id {
            return Err("hook changed since it was loaded; refresh and retry".into());
        }
        groups.remove(index);
        let empty = groups.is_empty();
        if empty {
            hooks.remove(&event);
        }
        Ok(())
    })
}

pub fn test(command: &str, input: &Value) -> Result<HookTestResult, String> {
    hooks::test_command(command, input, Duration::from_secs(30))
}

fn sources(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
) -> Result<Vec<Source>, String> {
    let roots = match layer {
        SettingsLayer::User => vec![(harness.config_root().to_path_buf(), true)],
        SettingsLayer::Project => {
            if !project_root.is_dir() {
                return Ok(Vec::new());
            }
            let project_dirs = config::project_config_dirs(project_root);
            let count = project_dirs.len();
            project_dirs
                .into_iter()
                .enumerate()
                .map(|(index, folder)| {
                    let selected = index + 1 == count;
                    (folder.join(".codex"), selected)
                })
                .collect()
        }
        SettingsLayer::Local => {
            return Err("Codex config supports user and project layers only".into());
        }
    };
    let mut sources = Vec::new();
    for (root, editable) in roots {
        for (format, path) in [
            (Format::Json, root.join("hooks.json")),
            (Format::Toml, root.join("config.toml")),
        ] {
            if path.is_file() && (format == Format::Json || has_inline_hooks(&path)?) {
                sources.push(Source {
                    layer,
                    format,
                    path,
                    editable,
                });
            }
        }
    }
    Ok(sources)
}

fn write_source(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
) -> Result<Source, String> {
    let mut candidates = sources(harness, project_root, layer)?
        .into_iter()
        .filter(|source| source.editable)
        .collect::<Vec<_>>();
    let configured = candidates
        .iter()
        .filter_map(|source| read_source(source).ok())
        .filter(|hooks| !hooks.is_empty())
        .count();
    if configured > 1 {
        return Err("both hooks.json and inline hooks are configured in this layer; edit the existing source directly to avoid adding a duplicate".into());
    }
    if configured == 1 {
        return candidates
            .into_iter()
            .find(|source| read_source(source).is_ok_and(|hooks| !hooks.is_empty()))
            .ok_or_else(|| "configured Codex hook source disappeared".into());
    }
    if configured == 0 && candidates.len() == 1 {
        return Ok(candidates.remove(0));
    }
    if configured == 0 && candidates.len() > 1 {
        return Err("both hooks.json and inline hooks are present in this layer; edit the existing source directly to avoid adding a duplicate".into());
    }
    candidates.sort_by_key(|source| if source.format == Format::Json { 0 } else { 1 });
    Ok(candidates.into_iter().next().unwrap_or_else(|| {
        let root = if layer == SettingsLayer::User {
            harness.config_root().to_path_buf()
        } else {
            project_root.join(".codex")
        };
        Source {
            layer,
            format: Format::Json,
            path: root.join("hooks.json"),
            editable: true,
        }
    }))
}

fn has_inline_hooks(path: &Path) -> Result<bool, String> {
    Ok(store::read_value(path)?.get("hooks").is_some())
}

fn read_source(source: &Source) -> Result<Map<String, Value>, String> {
    crate::harness::claude::skills::ensure_plain_components(&source.path)?;
    crate::harness::claude::skills::ensure_plain_file_if_present(&source.path)?;
    let root = match source.format {
        Format::Json => match std::fs::read(&source.path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .map_err(|error| format!("failed parsing {}: {error}", source.path.display()))?
                .as_object()
                .cloned()
                .ok_or_else(|| format!("{} must contain a JSON object", source.path.display()))?,
            Err(error) => return Err(format!("failed reading {}: {error}", source.path.display())),
        },
        Format::Toml => match store::read_value(&source.path)? {
            Value::Object(object) => object,
            _ => {
                return Err(format!(
                    "{} must contain a TOML table",
                    source.path.display()
                ));
            }
        },
    };
    match root.get("hooks") {
        None => Ok(Map::new()),
        Some(Value::Object(hooks)) => Ok(hooks.clone()),
        Some(_) => Err(format!(
            "{} `hooks` field must be an object",
            source.path.display()
        )),
    }
}

fn locate(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
    id: &str,
) -> Result<Located, String> {
    for source in sources(harness, project_root, layer)? {
        for (event, groups) in read_source(&source)? {
            if let Some(groups) = groups.as_array() {
                for (index, entry) in groups.iter().enumerate() {
                    let candidate = entry_id(&source, &event, index, entry);
                    if candidate == id {
                        return Ok(Located {
                            source,
                            event,
                            index,
                            entry: entry.clone(),
                            id: candidate,
                        });
                    }
                }
            }
        }
    }
    Err("Codex hook no longer exists in this layer; refresh and retry".into())
}

fn edit_source(
    harness: &CodexHarness,
    source: &Source,
    action: &str,
    edit: impl FnOnce(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), String> {
    match source.format {
        Format::Json => store::mutate_json(
            harness,
            ResourceKind::Settings,
            &source.path.to_string_lossy(),
            action,
            &source.path,
            move |root| {
                let root = root
                    .as_object_mut()
                    .ok_or("hooks.json must contain a JSON object")?;
                let hooks = root
                    .entry("hooks")
                    .or_insert_with(|| Value::Object(Map::new()));
                let hooks = hooks
                    .as_object_mut()
                    .ok_or("hooks.json `hooks` field must be an object")?;
                edit(hooks)
            },
        ),
        Format::Toml => store::mutate(
            harness,
            ResourceKind::Settings,
            &source.path.to_string_lossy(),
            action,
            &source.path,
            move |document| {
                let mut wrapper = toml_edit::DocumentMut::new();
                if let Some(item) = document.get("hooks") {
                    wrapper["hooks"] = item.clone();
                }
                let raw: Value = toml_edit::de::from_str(&wrapper.to_string())
                    .map_err(|error| format!("failed reading inline hooks: {}", error.message()))?;
                let mut hooks = raw
                    .get("hooks")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                edit(&mut hooks)?;
                let item = json_item(&Value::Object(hooks))?;
                document["hooks"] = item;
                Ok(())
            },
        ),
    }
}

fn event_groups_mut<'a>(
    hooks: &'a mut Map<String, Value>,
    event: &str,
) -> Result<&'a mut Vec<Value>, String> {
    let groups = hooks
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    groups
        .as_array_mut()
        .ok_or_else(|| format!("Codex `{event}` hooks must be an array of matcher groups"))
}

fn command_handlers(entry: &Value) -> Vec<String> {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|hook| hook.get("type").and_then(Value::as_str) == Some("command"))
        .filter_map(|hook| {
            hook.get("command")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn entry_id(source: &Source, event: &str, index: usize, entry: &Value) -> String {
    let raw = serde_json::to_vec(entry).unwrap_or_default();
    let hash = Sha256::digest(raw)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let source_path = source
        .path
        .to_string_lossy()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "codex:{}:{}:{}:{}:{}:{hash}",
        layer_name(source.layer),
        format_name(source.format),
        event,
        index,
        source_path
    )
}

fn source_layer_name(source: &Source) -> &'static str {
    if source.layer == SettingsLayer::Project && !source.editable {
        "project_parent"
    } else {
        layer_name(source.layer)
    }
}

fn ensure_layer(
    harness: &CodexHarness,
    project_root: &Path,
    layer: SettingsLayer,
) -> Result<(), String> {
    match layer {
        SettingsLayer::User => Ok(()),
        SettingsLayer::Project
            if project_root.is_dir() && config::project_trusted(harness, project_root)? =>
        {
            Ok(())
        }
        SettingsLayer::Project => {
            Err("Codex does not mark this project as trusted; project hooks are ignored".into())
        }
        SettingsLayer::Local => Err("Codex config supports user and project layers only".into()),
    }
}

fn validate_event(event: &str) -> Result<(), String> {
    if EVENTS.contains(&event) {
        Ok(())
    } else {
        Err(format!("`{event}` is not a documented Codex hook event"))
    }
}

fn set_optional_string(object: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        object.insert(key.into(), Value::String(value));
    } else {
        object.remove(key);
    }
}

fn json_item(value: &Value) -> Result<toml_edit::Item, String> {
    let document = toml_edit::ser::to_document(&serde_json::json!({ "hooks": value }))
        .map_err(|error| format!("hooks cannot be represented in TOML: {error}"))?;
    document
        .get("hooks")
        .cloned()
        .ok_or_else(|| "hooks cannot be represented in TOML".into())
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Json => "json",
        Format::Toml => "toml",
    }
}
fn layer_name(layer: SettingsLayer) -> &'static str {
    match layer {
        SettingsLayer::User => "user",
        SettingsLayer::Project => "project",
        SettingsLayer::Local => "local",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn harness() -> (tempfile::TempDir, CodexHarness, PathBuf) {
        let root = crate::test_support::tempdir().unwrap();
        let config = root.path().join(".codex");
        let project = root.path().join("project");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&project).unwrap();
        let harness = CodexHarness::new(config, root.path().to_path_buf());
        (root, harness, project)
    }

    #[test]
    fn command_hook_json_crud_preserves_unrelated_metadata() {
        let (_root, h, project) = harness();
        fs::write(
            h.config_root().join("hooks.json"),
            r#"{"description":"keep","other":true}"#,
        )
        .unwrap();
        let id_before = list(&h, &project).unwrap();
        assert!(id_before.is_empty());
        save(
            &h,
            &project,
            SettingsLayer::User,
            "PreToolUse",
            Some("Bash".into()),
            "echo ok".into(),
            None,
        )
        .unwrap();
        let current = list(&h, &project).unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].commands, vec!["echo ok"]);
        assert!(
            set_enabled(&h, &project, SettingsLayer::User, &current[0].id, false)
                .unwrap_err()
                .contains("/hooks")
        );
        delete(&h, &project, SettingsLayer::User, &current[0].id).unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(h.config_root().join("hooks.json")).unwrap()).unwrap();
        assert_eq!(saved["description"], "keep");
        assert_eq!(saved["other"], true);
        assert!(list(&h, &project).unwrap().is_empty());
    }

    #[test]
    fn untrusted_project_hooks_are_not_loaded_or_mutable() {
        let (_root, h, project) = harness();
        fs::create_dir_all(project.join(".codex")).unwrap();
        fs::write(
            project.join(".codex/hooks.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        )
        .unwrap();
        assert!(list(&h, &project).unwrap().is_empty());
        assert!(save(
            &h,
            &project,
            SettingsLayer::Project,
            "Stop",
            None,
            "echo no".into(),
            None
        )
        .is_err());
    }

    #[test]
    fn inherited_project_hooks_are_listed_read_only_with_unique_source_ids() {
        let (_root, h, project) = harness();
        let repository = project.join("repo");
        let nested = repository.join("nested");
        fs::create_dir_all(repository.join(".git")).unwrap();
        fs::create_dir_all(repository.join(".codex")).unwrap();
        fs::create_dir_all(nested.join(".codex")).unwrap();
        let repository_key = repository.to_string_lossy().replace('\\', "/");
        fs::write(
            h.settings_path(),
            format!("[projects.\"{repository_key}\"]\ntrust_level = 'trusted'\n"),
        )
        .unwrap();
        let same_hook =
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo inherited"}]}]}}"#;
        fs::write(repository.join(".codex/hooks.json"), same_hook).unwrap();
        fs::write(nested.join(".codex/hooks.json"), same_hook).unwrap();

        let listed = list(&h, &nested).unwrap();
        assert_eq!(listed.len(), 2);
        assert_ne!(listed[0].id, listed[1].id);
        let inherited = listed
            .iter()
            .find(|hook| hook.layer == "project_parent")
            .unwrap();
        assert!(!inherited.editable);
        assert_eq!(
            inherited.source_path,
            repository
                .join(".codex/hooks.json")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
        assert!(delete(&h, &nested, SettingsLayer::Project, &inherited.id)
            .unwrap_err()
            .contains("read-only"));
        assert_eq!(list(&h, &nested).unwrap().len(), 2);
    }
}
