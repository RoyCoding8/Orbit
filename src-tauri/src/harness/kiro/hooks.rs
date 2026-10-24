use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::KiroHarness;
use crate::harness::json_store as store;
use crate::harness::{
    atomic,
    claude::{config::SettingsLayer, hooks::HookView},
    Harness, ResourceKind,
};

const V2_EVENTS: &[&str] = &[
    "agentSpawn",
    "userPromptSubmit",
    "preToolUse",
    "postToolUse",
    "stop",
];

const V3_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostFileCreate",
    "PostFileSave",
    "PostFileDelete",
    "PreTaskExec",
    "PostTaskExec",
    "Stop",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HookSource {
    pub id: String,
    pub label: String,
    pub layer: SettingsLayer,
    pub events: Vec<String>,
    pub can_toggle: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Engine {
    V2,
    V3,
}

#[derive(Debug, Clone)]
struct Source {
    id: String,
    label: String,
    layer: SettingsLayer,
    path: PathBuf,
    format: Format,
    editable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    V2Agent,
    V3Agent,
    V3AgentMarkdown,
    V3Standalone,
}

#[derive(Debug, Clone)]
struct Located {
    source: Source,
    event: String,
    index: usize,
    entry: Value,
    id: String,
}

pub fn sources(harness: &KiroHarness, project: &Path) -> Result<Vec<HookSource>, String> {
    let engine = selected_engine(harness, project)?;
    let mut output = Vec::new();
    for layer in [SettingsLayer::User, SettingsLayer::Project] {
        for source in sources_for(harness, project, layer, engine)? {
            output.push(HookSource {
                id: source.id,
                label: source.label,
                layer: source.layer,
                events: available_events(source.format)
                    .iter()
                    .map(|event| (*event).to_owned())
                    .collect(),
                can_toggle: !matches!(source.format, Format::V2Agent | Format::V3AgentMarkdown),
            });
        }
    }
    output.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(output)
}

pub fn list(harness: &KiroHarness, project: &Path) -> Result<Vec<HookView>, String> {
    let mut output = Vec::new();
    let engine = selected_engine(harness, project)?;
    for layer in [SettingsLayer::User, SettingsLayer::Project] {
        for source in sources_for(harness, project, layer, engine)? {
            for (event, entries) in read_hooks(&source)? {
                for (index, entry) in entries.iter().enumerate() {
                    let id = entry_id(&source, &event, index, entry);
                    let (matcher, commands, enabled, editable) = match source.format {
                        Format::V2Agent => (
                            entry
                                .get("matcher")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            entry
                                .get("command")
                                .and_then(Value::as_str)
                                .map(|command| vec![command.to_owned()])
                                .unwrap_or_default(),
                            true,
                            entry.get("command").and_then(Value::as_str).is_some(),
                        ),
                        Format::V3Agent | Format::V3AgentMarkdown | Format::V3Standalone => {
                            let action = entry.get("action");
                            let command = action
                                .filter(|action| {
                                    action.get("type").and_then(Value::as_str) == Some("command")
                                })
                                .and_then(|action| action.get("command"))
                                .and_then(Value::as_str);
                            (
                                entry
                                    .get("matcher")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                command
                                    .map(|command| vec![command.to_owned()])
                                    .unwrap_or_default(),
                                entry
                                    .get("enabled")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(true),
                                command.is_some() && source.format != Format::V3AgentMarkdown,
                            )
                        }
                    };
                    output.push(HookView {
                        id,
                        event: event.clone(),
                        matcher,
                        commands,
                        enabled,
                        layer: source.layer.label().to_owned(),
                        source_path: source.path.to_string_lossy().into_owned(),
                        editable: source.editable && editable,
                    });
                }
            }
        }
    }
    output.sort_by(|left, right| {
        left.event
            .cmp(&right.event)
            .then(left.layer.cmp(&right.layer))
            .then(left.id.cmp(&right.id))
    });
    Ok(output)
}

pub fn save(
    harness: &KiroHarness,
    project: &Path,
    draft: crate::harness::HookDraft,
) -> Result<(), String> {
    let crate::harness::HookDraft {
        layer,
        event,
        matcher,
        command,
        edit_id,
        source_id,
    } = draft;
    let event = event.as_str();
    let engine = selected_engine(harness, project)?;
    validate_event(engine, event)?;
    if command.trim().is_empty() {
        return Err("hook command must not be empty".into());
    }
    let (source, located) = match edit_id.as_deref() {
        Some(id) => {
            let located = locate(harness, project, layer, engine, id)?;
            if !located.source.editable
                || !command_of(located.source.format, &located.entry).is_some()
            {
                return Err("this Kiro hook is inherited or uses a non-command action and cannot be edited here".into());
            }
            (located.source.clone(), Some(located))
        }
        None => {
            let source = match source_id.as_deref() {
                Some(id) => sources_for(harness, project, layer, engine)?
                    .into_iter()
                    .find(|source| source.id == id)
                    .ok_or("selected Kiro hook source is no longer available")?,
                None if engine == Engine::V3 => standalone_source(harness, project, layer)?,
                None => return Err("select a Kiro agent profile before adding a V2 hook".into()),
            };
            (source, None)
        }
    };
    if !source.editable {
        return Err("selected Kiro hook source is read-only".into());
    }
    match source.format {
        Format::V2Agent => edit_v2(harness, &source, event, matcher, command, located),
        Format::V3Agent | Format::V3Standalone => {
            edit_v3(harness, &source, event, matcher, command, located)
        }
        Format::V3AgentMarkdown => {
            Err("Kiro Markdown agent hooks are read-only here; edit the profile in Kiro".into())
        }
    }
}

pub fn set_enabled(
    harness: &KiroHarness,
    project: &Path,
    layer: SettingsLayer,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let located = locate(
        harness,
        project,
        layer,
        selected_engine(harness, project)?,
        id,
    )?;
    if located.source.format == Format::V2Agent {
        return Err("Kiro V2 agent-profile hooks have no native per-hook enable switch".into());
    }
    if !located.source.editable {
        return Err("selected Kiro hook source is read-only".into());
    }
    mutate_located(harness, &located, "enabled", |entry| {
        entry
            .as_object_mut()
            .ok_or("hook entry must be an object")?
            .insert("enabled".into(), Value::Bool(enabled));
        Ok(())
    })
}

pub fn delete(
    harness: &KiroHarness,
    project: &Path,
    layer: SettingsLayer,
    id: &str,
) -> Result<(), String> {
    let located = locate(
        harness,
        project,
        layer,
        selected_engine(harness, project)?,
        id,
    )?;
    if !located.source.editable {
        return Err("selected Kiro hook source is read-only".into());
    }
    let mut document = store::read_value(&located.source.path)?;
    let expected = document.clone();
    let entries = entries_mut(&mut document, &located.source, &located.event)?;
    let index = actual_entry_index(entries, &located.source, &located.event, &located)?;
    let current = entries
        .get(index)
        .ok_or("hook changed since it was loaded; refresh and retry")?;
    if current != &located.entry {
        return Err("hook changed since it was loaded; refresh and retry".into());
    }
    entries.remove(index);
    let empty_event = entries.is_empty();
    if empty_event {
        remove_event(&mut document, &located.source, &located.event)?;
    }
    if located.source.format == Format::V3Standalone
        && document["hooks"].as_array().is_some_and(Vec::is_empty)
    {
        return store::delete_file(
            harness,
            ResourceKind::Settings,
            id,
            "kiro-hook-delete",
            &located.source.path,
        );
    }
    write_document(
        harness,
        &located.source,
        id,
        "kiro-hook-delete",
        expected,
        document,
    )
}

fn selected_engine(harness: &KiroHarness, project: &Path) -> Result<Engine, String> {
    let mut engine = Engine::V2;
    for path in [
        harness.settings_path(),
        harness.project_settings_path(project),
    ] {
        let settings = store::read_value(&path)?;
        let settings = settings
            .as_object()
            .ok_or("Kiro CLI settings must contain a JSON object")?;
        if let Some(value) = settings.get("chat.agentEngine") {
            engine = match value.as_str() {
                Some("v2") => Engine::V2,
                Some("v3") => Engine::V3,
                _ => return Err("Unsupported Kiro chat.agentEngine; expected v2 or v3".into()),
            };
        }
    }
    Ok(engine)
}

fn sources_for(
    harness: &KiroHarness,
    project: &Path,
    layer: SettingsLayer,
    engine: Engine,
) -> Result<Vec<Source>, String> {
    match layer {
        SettingsLayer::User => source_roots(harness, project, layer, engine, harness.home()),
        SettingsLayer::Project if project.is_dir() => source_roots(
            harness,
            project,
            layer,
            engine,
            &harness.project_root(project),
        ),
        SettingsLayer::Project => Ok(Vec::new()),
        SettingsLayer::Local => Err("Kiro hooks support user and project sources only".into()),
    }
}

fn source_roots(
    harness: &KiroHarness,
    project: &Path,
    layer: SettingsLayer,
    engine: Engine,
    root: &Path,
) -> Result<Vec<Source>, String> {
    match engine {
        Engine::V2 => {
            let agents = root.join("agents");
            if !agents.is_dir() {
                return Ok(Vec::new());
            }
            let mut sources = Vec::new();
            for entry in std::fs::read_dir(&agents).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let path = entry.path();
                let metadata =
                    std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
                if atomic::is_link_or_reparse(&metadata)
                    || !metadata.is_file()
                    || path.extension().is_none_or(|extension| extension != "json")
                {
                    continue;
                }
                let agent = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .ok_or("invalid Kiro agent filename")?
                    .to_owned();
                sources.push(Source {
                    id: path.to_string_lossy().into_owned(),
                    label: format!("{} agent: {agent}", layer.label()),
                    layer,
                    path,
                    format: Format::V2Agent,
                    editable: true,
                });
            }
            sources.sort_by(|left, right| left.id.cmp(&right.id));
            let _ = (harness, project);
            Ok(sources)
        }
        Engine::V3 => {
            let mut sources = Vec::new();
            let agents_root = root.join("agents");
            if plain_directory(&agents_root)? {
                for entry in std::fs::read_dir(&agents_root).map_err(|error| error.to_string())? {
                    let entry = entry.map_err(|error| error.to_string())?;
                    let path = entry.path();
                    let metadata =
                        std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
                    if atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
                        continue;
                    }
                    let format = match path.extension().and_then(|extension| extension.to_str()) {
                        Some("json") => Format::V3Agent,
                        Some("md") => Format::V3AgentMarkdown,
                        _ => continue,
                    };
                    let agent = path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("agent");
                    sources.push(Source {
                        id: path.to_string_lossy().into_owned(),
                        label: format!("{} agent: {agent}", layer.label()),
                        layer,
                        path,
                        format,
                        editable: format == Format::V3Agent,
                    });
                }
            }
            let hooks_root = root.join("hooks");
            if plain_directory(&hooks_root)? {
                for entry in std::fs::read_dir(&hooks_root).map_err(|error| error.to_string())? {
                    let entry = entry.map_err(|error| error.to_string())?;
                    let path = entry.path();
                    let metadata =
                        std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
                    if atomic::is_link_or_reparse(&metadata)
                        || !metadata.is_file()
                        || path.extension().is_none_or(|extension| extension != "json")
                    {
                        continue;
                    }
                    sources.push(Source {
                        id: path.to_string_lossy().into_owned(),
                        label: format!(
                            "{} hooks: {}",
                            layer.label(),
                            path.file_stem().unwrap_or_default().to_string_lossy()
                        ),
                        layer,
                        path,
                        format: Format::V3Standalone,
                        editable: true,
                    });
                }
            }
            let default = standalone_source(harness, project, layer)?;
            if !sources.iter().any(|source| source.id == default.id) {
                sources.push(default);
            }
            sources.sort_by(|left, right| left.id.cmp(&right.id));
            Ok(sources)
        }
    }
}

fn standalone_source(
    harness: &KiroHarness,
    project: &Path,
    layer: SettingsLayer,
) -> Result<Source, String> {
    let root = match layer {
        SettingsLayer::User => harness.home().to_path_buf(),
        SettingsLayer::Project if project.is_dir() => harness.project_root(project),
        SettingsLayer::Project => return Err("project directory is unavailable".into()),
        SettingsLayer::Local => {
            return Err("Kiro hooks support user and project sources only".into())
        }
    };
    let path = root.join("hooks").join("orbit-hooks.json");
    Ok(Source {
        id: path.to_string_lossy().into_owned(),
        label: format!("{} standalone hooks", layer.label()),
        layer,
        path,
        format: Format::V3Standalone,
        editable: true,
    })
}

fn plain_directory(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(!atomic::is_link_or_reparse(&metadata) && metadata.is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn available_events(format: Format) -> &'static [&'static str] {
    match format {
        Format::V2Agent => V2_EVENTS,
        Format::V3Agent | Format::V3AgentMarkdown | Format::V3Standalone => V3_EVENTS,
    }
}

fn read_hooks(source: &Source) -> Result<HashMap<String, Vec<Value>>, String> {
    let mut output = HashMap::new();
    let document = if source.format == Format::V3AgentMarkdown {
        Value::Null
    } else {
        store::read_value(&source.path)?
    };
    match source.format {
        Format::V2Agent => {
            let Some(events) = document.get("hooks").and_then(Value::as_object) else {
                return Ok(output);
            };
            for (event, entries) in events {
                if let Some(entries) = entries.as_array() {
                    output.insert(event.clone(), entries.clone());
                }
            }
        }
        Format::V3Agent | Format::V3Standalone => {
            let Some(hooks) = document.get("hooks").and_then(Value::as_array) else {
                return Ok(output);
            };
            for entry in hooks {
                if let Some(event) = entry.get("trigger").and_then(Value::as_str) {
                    output
                        .entry(event.to_owned())
                        .or_insert_with(Vec::new)
                        .push(entry.clone());
                }
            }
        }
        Format::V3AgentMarkdown => {
            let markdown =
                std::fs::read_to_string(&source.path).map_err(|error| error.to_string())?;
            let Some(frontmatter) = markdown_frontmatter(&markdown) else {
                return Ok(output);
            };
            let profile: serde_yaml::Value = serde_yaml::from_str(frontmatter)
                .map_err(|error| format!("invalid Kiro Markdown profile front matter: {error}"))?;
            let Some(hooks) = profile
                .get("hooks")
                .and_then(serde_yaml::Value::as_sequence)
            else {
                return Ok(output);
            };
            for hook in hooks {
                let entry = serde_json::to_value(hook).map_err(|error| error.to_string())?;
                if let Some(event) = entry.get("trigger").and_then(Value::as_str) {
                    output
                        .entry(event.to_owned())
                        .or_insert_with(Vec::new)
                        .push(entry);
                }
            }
        }
    }
    Ok(output)
}

fn locate(
    harness: &KiroHarness,
    project: &Path,
    layer: SettingsLayer,
    engine: Engine,
    id: &str,
) -> Result<Located, String> {
    for source in sources_for(harness, project, layer, engine)? {
        for (event, entries) in read_hooks(&source)? {
            for (index, entry) in entries.iter().enumerate() {
                if entry_id(&source, &event, index, entry) == id {
                    return Ok(Located {
                        source,
                        event,
                        index,
                        entry: entry.clone(),
                        id: id.to_owned(),
                    });
                }
            }
        }
    }
    Err("Kiro hook is no longer available".into())
}

fn entry_id(source: &Source, event: &str, index: usize, entry: &Value) -> String {
    let mut identity = source.id.as_bytes().to_vec();
    identity.extend_from_slice(event.as_bytes());
    identity.extend_from_slice(&index.to_le_bytes());
    if let Some(name) = entry.get("name").and_then(Value::as_str) {
        identity.extend_from_slice(name.as_bytes());
    }
    format!("kiro-hook-{}", atomic::hex(&Sha256::digest(identity)))
}

fn edit_v2(
    harness: &KiroHarness,
    source: &Source,
    event: &str,
    matcher: Option<String>,
    command: String,
    located: Option<Located>,
) -> Result<(), String> {
    let mut document = store::read_value(&source.path)?;
    let expected = document.clone();
    let events = document
        .as_object_mut()
        .ok_or("Kiro agent profile must contain a JSON object")?
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("V2 agent hooks must be an event object")?;
    if let Some(located) = located {
        let entries = events
            .get_mut(&located.event)
            .and_then(Value::as_array_mut)
            .ok_or("hook changed since it was loaded; refresh and retry")?;
        let index = actual_entry_index(entries, &located.source, &located.event, &located)?;
        let current = entries
            .get(index)
            .ok_or("hook changed since it was loaded; refresh and retry")?;
        if current != &located.entry {
            return Err("hook changed since it was loaded; refresh and retry".into());
        }
        let mut updated = current.clone();
        let object = updated
            .as_object_mut()
            .ok_or("V2 hook entry must be an object")?;
        object.insert("command".into(), Value::String(command));
        set_optional_string(object, "matcher", matcher);
        entries.remove(index);
        if events
            .get(&located.event)
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            events.remove(&located.event);
        }
        events
            .entry(event.to_owned())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or("V2 hook event must contain an array")?
            .push(updated);
    } else {
        let mut hook = Map::new();
        hook.insert("command".into(), Value::String(command));
        set_optional_string(&mut hook, "matcher", matcher);
        let entries = events
            .entry(event.to_owned())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or("V2 hook event must contain an array")?;
        let value = Value::Object(hook);
        if entries.contains(&value) {
            return Err("an identical hook already exists in this Kiro agent".into());
        }
        entries.push(value);
    }
    write_document(
        harness,
        source,
        &source.id,
        "kiro-hook-save",
        expected,
        document,
    )
}

fn edit_v3(
    harness: &KiroHarness,
    source: &Source,
    event: &str,
    matcher: Option<String>,
    command: String,
    located: Option<Located>,
) -> Result<(), String> {
    let mut document = store::read_value(&source.path)?;
    let expected = document.clone();
    if source.format == Format::V3Standalone {
        if document.is_null()
            || (document.get("version").is_none() && document.get("hooks").is_none())
        {
            document = serde_json::json!({"version":"v1","hooks":[]});
        }
        if document.get("version").and_then(Value::as_str) != Some("v1") {
            return Err("Kiro standalone hooks must use version `v1`".into());
        }
    } else if !document.is_object() {
        return Err("Kiro V3 agent profile must contain a JSON object".into());
    }
    let entries = document
        .as_object_mut()
        .ok_or("Kiro hook source must contain an object")?
        .entry("hooks")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or(
            "Kiro V3 agent hooks must be an array; run /upgrade-agent for legacy hook formats",
        )?;
    if let Some(located) = located {
        let index = actual_entry_index(entries, source, &located.event, &located)?;
        let current = entries
            .get(index)
            .ok_or("hook changed since it was loaded; refresh and retry")?;
        if current != &located.entry {
            return Err("hook changed since it was loaded; refresh and retry".into());
        }
        let mut updated = current.clone();
        let object = updated
            .as_object_mut()
            .ok_or("hook entry must be an object")?;
        object.insert("trigger".into(), Value::String(event.to_owned()));
        set_optional_string(object, "matcher", matcher);
        object.insert(
            "action".into(),
            serde_json::json!({"type":"command","command":command}),
        );
        entries.remove(index);
        entries.push(updated);
    } else {
        let index = NEXT_HOOK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("orbit-hook-{index}");
        let mut hook = serde_json::json!({
            "name":name,
            "trigger":event,
            "action":{"type":"command","command":command},
            "timeout":30,
            "enabled":true
        });
        if let Some(matcher) = matcher {
            hook["matcher"] = Value::String(matcher);
        }
        if entries.contains(&hook) {
            return Err("an identical hook already exists in this Kiro source".into());
        }
        entries.push(hook);
    }
    write_document(
        harness,
        source,
        &source.id,
        "kiro-hook-save",
        expected,
        document,
    )
}

static NEXT_HOOK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn mutate_located(
    harness: &KiroHarness,
    located: &Located,
    action: &str,
    update: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    let mut document = store::read_value(&located.source.path)?;
    let expected = document.clone();
    let entries = entries_mut(&mut document, &located.source, &located.event)?;
    let index = actual_entry_index(entries, &located.source, &located.event, located)?;
    let current = entries
        .get_mut(index)
        .ok_or("hook changed since it was loaded; refresh and retry")?;
    if current != &located.entry {
        return Err("hook changed since it was loaded; refresh and retry".into());
    }
    update(current)?;
    write_document(
        harness,
        &located.source,
        &located.id,
        &format!("kiro-hook-{action}"),
        expected,
        document,
    )
}

fn entries_mut<'a>(
    document: &'a mut Value,
    source: &Source,
    event: &str,
) -> Result<&'a mut Vec<Value>, String> {
    match source.format {
        Format::V2Agent => document
            .get_mut("hooks")
            .and_then(Value::as_object_mut)
            .ok_or("V2 agent hooks must be an event object")?
            .get_mut(event)
            .and_then(Value::as_array_mut)
            .ok_or("hook event is no longer available".into()),
        Format::V3Agent | Format::V3Standalone => document
            .get_mut("hooks")
            .and_then(Value::as_array_mut)
            .ok_or("V3 hooks must contain an array".into()),
        Format::V3AgentMarkdown => {
            Err("Kiro Markdown agent hooks are read-only here; edit the profile in Kiro".into())
        }
    }
}

fn actual_entry_index(
    entries: &[Value],
    source: &Source,
    event: &str,
    located: &Located,
) -> Result<usize, String> {
    if source.format == Format::V2Agent {
        return Ok(located.index);
    }
    let mut event_index = 0;
    for (index, entry) in entries.iter().enumerate() {
        if entry.get("trigger").and_then(Value::as_str) != Some(event) {
            continue;
        }
        if event_index == located.index {
            if entry_id(source, event, event_index, entry) == located.id {
                return Ok(index);
            }
            return Err("hook changed since it was loaded; refresh and retry".into());
        }
        event_index += 1;
    }
    Err("hook changed since it was loaded; refresh and retry".into())
}

fn remove_event(document: &mut Value, source: &Source, event: &str) -> Result<(), String> {
    match source.format {
        Format::V2Agent => {
            document
                .get_mut("hooks")
                .and_then(Value::as_object_mut)
                .ok_or("V2 agent hooks must be an event object")?
                .remove(event);
        }
        Format::V3Agent | Format::V3Standalone => {
            let hooks = document
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .ok_or("standalone hooks must contain an array")?;
            hooks.retain(|entry| entry.get("trigger").and_then(Value::as_str) != Some(event));
        }
        Format::V3AgentMarkdown => {
            return Err(
                "Kiro Markdown agent hooks are read-only here; edit the profile in Kiro".into(),
            );
        }
    }
    Ok(())
}

fn write_document(
    harness: &KiroHarness,
    source: &Source,
    id: &str,
    action: &str,
    expected: Value,
    document: Value,
) -> Result<(), String> {
    let updated = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    store::mutate_json(
        harness,
        ResourceKind::Settings,
        id,
        action,
        &source.path,
        move |current| {
            if *current != expected {
                return Err(
                    "hook source changed while it was being edited; refresh and retry".into(),
                );
            }
            *current = serde_json::from_slice(&updated).map_err(|error| error.to_string())?;
            Ok(())
        },
    )
}

fn command_of(format: Format, entry: &Value) -> Option<&str> {
    match format {
        Format::V2Agent => entry.get("command").and_then(Value::as_str),
        Format::V3Agent | Format::V3Standalone => entry
            .get("action")
            .filter(|action| action.get("type").and_then(Value::as_str) == Some("command"))
            .and_then(|action| action.get("command"))
            .and_then(Value::as_str),
        Format::V3AgentMarkdown => entry
            .get("action")
            .filter(|action| action.get("type").and_then(Value::as_str) == Some("command"))
            .and_then(|action| action.get("command"))
            .and_then(Value::as_str),
    }
}

fn markdown_frontmatter(markdown: &str) -> Option<&str> {
    let rest = markdown.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    Some(rest[..end].strip_suffix('\r').unwrap_or(&rest[..end]))
}

fn set_optional_string(object: &mut Map<String, Value>, key: &str, value: Option<String>) {
    match value {
        Some(value) => {
            object.insert(key.to_owned(), Value::String(value));
        }
        None => {
            object.remove(key);
        }
    }
}

fn validate_event(engine: Engine, event: &str) -> Result<(), String> {
    let allowed = match engine {
        Engine::V2 => V2_EVENTS,
        Engine::V3 => V3_EVENTS,
    };
    if allowed.contains(&event) {
        Ok(())
    } else {
        Err(format!(
            "unsupported Kiro {} hook trigger `{event}`",
            if engine == Engine::V2 { "V2" } else { "V3" }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn v2_hooks_edit_the_selected_agent_profile_and_have_no_toggle() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));
        let project = root.path().join("project");
        std::fs::create_dir_all(harness.home().join("agents")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let agent = harness.home().join("agents/reviewer.json");
        std::fs::write(
            &agent,
            br#"{"name":"reviewer","hooks":{"preToolUse":[{"command":"old","matcher":"read"}]}}"#,
        )
        .unwrap();

        let source_list = sources(&harness, &project).unwrap();
        assert_eq!(source_list.len(), 1);
        assert!(Path::new(&source_list[0].id).ends_with("agents/reviewer.json"));
        let hooks = list(&harness, &project).unwrap();
        assert_eq!(hooks[0].commands, ["old"]);
        assert!(
            set_enabled(&harness, &project, SettingsLayer::User, &hooks[0].id, false)
                .unwrap_err()
                .contains("no native per-hook enable switch")
        );
        save(
            &harness,
            &project,
            crate::harness::HookDraft {
                layer: SettingsLayer::User,
                event: "postToolUse".into(),
                matcher: None,
                command: "new".into(),
                edit_id: Some(hooks[0].id.clone()),
                source_id: Some(source_list[0].id.clone()),
            },
        )
        .unwrap();
        let saved = store::read_value(&agent).unwrap();
        assert_eq!(saved["hooks"]["postToolUse"][0]["command"], "new");
        assert!(saved["hooks"].get("preToolUse").is_none());
    }

    #[test]
    fn v3_standalone_hooks_preserve_native_enable_and_delete_behavior() {
        let root = crate::test_support::tempdir().unwrap();
        let home = root.path().join("home/.kiro");
        let harness = KiroHarness::new(home.clone());
        std::fs::create_dir_all(home.join("settings")).unwrap();
        std::fs::create_dir_all(home.join("hooks")).unwrap();
        std::fs::write(harness.settings_path(), br#"{"chat.agentEngine":"v3"}"#).unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        save(
            &harness,
            &project,
            crate::harness::HookDraft {
                layer: SettingsLayer::User,
                event: "PreToolUse".into(),
                matcher: Some("read".into()),
                command: "fixture-command".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .unwrap();
        let hook = list(&harness, &project).unwrap().pop().unwrap();
        assert_eq!(hook.commands, ["fixture-command"]);
        let layer: SettingsLayer = serde_json::from_value(json!(hook.layer)).unwrap();
        assert_eq!(layer, SettingsLayer::User);
        set_enabled(&harness, &project, layer, &hook.id, false).unwrap();
        assert!(!list(&harness, &project).unwrap()[0].enabled);
        delete(&harness, &project, SettingsLayer::User, &hook.id).unwrap();
        assert!(list(&harness, &project).unwrap().is_empty());
    }

    #[test]
    fn v3_interleaved_triggers_mutate_the_matching_native_array_entry() {
        let root = crate::test_support::tempdir().unwrap();
        let home = root.path().join("home/.kiro");
        let harness = KiroHarness::new(home.clone());
        std::fs::create_dir_all(home.join("settings")).unwrap();
        std::fs::create_dir_all(home.join("hooks")).unwrap();
        std::fs::write(harness.settings_path(), br#"{"chat.agentEngine":"v3"}"#).unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let path = home.join("hooks/native.json");
        std::fs::write(
            &path,
            br#"{"version":"v1","hooks":[{"name":"first","trigger":"PreToolUse","action":{"type":"command","command":"first"}},{"name":"middle","trigger":"PostTaskExec","action":{"type":"command","command":"middle"}},{"name":"last","trigger":"PreToolUse","action":{"type":"command","command":"last"}}]}"#,
        )
        .unwrap();

        let target = list(&harness, &project)
            .unwrap()
            .into_iter()
            .find(|hook| hook.commands == ["last"])
            .unwrap();
        set_enabled(&harness, &project, SettingsLayer::User, &target.id, false).unwrap();
        assert!(!store::read_value(&path).unwrap()["hooks"][2]["enabled"]
            .as_bool()
            .unwrap());
        delete(&harness, &project, SettingsLayer::User, &target.id).unwrap();
        let remaining = store::read_value(&path).unwrap();
        assert_eq!(remaining["hooks"].as_array().unwrap().len(), 2);
        assert_eq!(remaining["hooks"][1]["name"], "middle");
    }

    #[test]
    fn v2_default_source_discovery_does_not_require_login_or_inference() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));
        std::fs::create_dir_all(harness.home().join("agents")).unwrap();
        std::fs::write(
            harness.home().join("agents/worker.json"),
            json!({"name":"worker","hooks":{"agentSpawn":[{"command":"echo start"}]}}).to_string(),
        )
        .unwrap();
        assert!(!sources(&harness, root.path()).unwrap()[0].can_toggle);
        assert_eq!(sources(&harness, root.path()).unwrap()[0].events, V2_EVENTS);
    }

    #[test]
    fn v3_empty_source_is_creatable_with_all_native_triggers() {
        let root = crate::test_support::tempdir().unwrap();
        let home = root.path().join("home/.kiro");
        let harness = KiroHarness::new(home.clone());
        std::fs::create_dir_all(home.join("settings")).unwrap();
        std::fs::write(harness.settings_path(), br#"{"chat.agentEngine":"v3"}"#).unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let choices = sources(&harness, &project).unwrap();
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().all(|source| source.events == V3_EVENTS));
        let source = choices
            .iter()
            .find(|source| source.layer == SettingsLayer::Project)
            .unwrap();
        save(
            &harness,
            &project,
            crate::harness::HookDraft {
                layer: SettingsLayer::Project,
                event: "PostFileSave".into(),
                matcher: None,
                command: "fixture-command".into(),
                edit_id: None,
                source_id: Some(source.id.clone()),
            },
        )
        .unwrap();
        let saved = store::read_value(&PathBuf::from(&source.id)).unwrap();
        assert_eq!(saved["version"], "v1");
        assert_eq!(saved["hooks"][0]["trigger"], "PostFileSave");
    }

    #[test]
    fn v3_json_agent_inline_hooks_use_native_profile_array() {
        let root = crate::test_support::tempdir().unwrap();
        let home = root.path().join("home/.kiro");
        let harness = KiroHarness::new(home.clone());
        std::fs::create_dir_all(home.join("settings")).unwrap();
        std::fs::create_dir_all(home.join("agents")).unwrap();
        std::fs::write(harness.settings_path(), br#"{"chat.agentEngine":"v3"}"#).unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let profile = home.join("agents/reviewer.json");
        std::fs::write(&profile, br#"{"name":"reviewer","description":"fixture"}"#).unwrap();

        let choices = sources(&harness, &project).unwrap();
        let source = choices
            .iter()
            .find(|source| source.id.ends_with("reviewer.json"))
            .unwrap();
        assert_eq!(source.events, V3_EVENTS);
        save(
            &harness,
            &project,
            crate::harness::HookDraft {
                layer: SettingsLayer::User,
                event: "PostFileSave".into(),
                matcher: Some("*.rs".into()),
                command: "fixture-command".into(),
                edit_id: None,
                source_id: Some(source.id.clone()),
            },
        )
        .unwrap();
        let saved = store::read_value(&profile).unwrap();
        assert_eq!(saved["name"], "reviewer");
        assert_eq!(saved["hooks"][0]["trigger"], "PostFileSave");
        assert_eq!(saved["hooks"][0]["matcher"], "*.rs");
        let listed = list(&harness, &project).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].commands, ["fixture-command"]);
        assert!(Path::new(&listed[0].source_path).ends_with("agents/reviewer.json"));
        set_enabled(
            &harness,
            &project,
            SettingsLayer::User,
            &listed[0].id,
            false,
        )
        .unwrap();
        assert!(!list(&harness, &project).unwrap()[0].enabled);
    }

    #[test]
    fn v3_markdown_agent_hooks_are_listed_read_only() {
        let root = crate::test_support::tempdir().unwrap();
        let home = root.path().join("home/.kiro");
        let harness = KiroHarness::new(home.clone());
        std::fs::create_dir_all(home.join("settings")).unwrap();
        std::fs::create_dir_all(home.join("agents")).unwrap();
        std::fs::write(harness.settings_path(), br#"{"chat.agentEngine":"v3"}"#).unwrap();
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            home.join("agents/reviewer.md"),
            "---\nname: reviewer\nhooks:\n  - name: save\n    trigger: PostFileSave\n    action:\n      type: command\n      command: echo saved\n---\nReview changes carefully.\n",
        )
        .unwrap();

        let sources = sources(&harness, &project).unwrap();
        let markdown_source = sources
            .iter()
            .find(|source| source.id.ends_with("reviewer.md"))
            .unwrap();
        assert!(!markdown_source.can_toggle);
        let listed = list(&harness, &project).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].commands, ["echo saved"]);
        assert!(!listed[0].editable);
        assert!(
            delete(&harness, &project, SettingsLayer::User, &listed[0].id)
                .unwrap_err()
                .contains("read-only")
        );
    }
}
