#[cfg(not(windows))]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::Digest;

use super::{config::SettingsLayer, ClaudeCodeHarness};
use crate::harness::{atomic, Harness, HarnessError, ResourceKind, Result};

const HOOKS_KEY: &str = "hooks";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookMatcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub type HookEvents = BTreeMap<String, Vec<HookMatcher>>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookView {
    pub id: String,
    pub event: String,
    pub matcher: Option<String>,
    pub commands: Vec<String>,
    pub enabled: bool,
    pub layer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DisabledHooks {
    #[serde(default)]
    hooks: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookTestResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

impl ClaudeCodeHarness {
    pub fn hooks_list(&self, project_root: &Path, app_data: &Path) -> Result<Vec<HookView>> {
        let sidecar = app_data.join("hooks-disabled.json");
        let disabled = read_disabled(&sidecar)?;
        let mut live_ids = std::collections::BTreeSet::new();
        let mut views = Vec::new();
        for layer in [SettingsLayer::User, SettingsLayer::Project] {
            let path = layer.settings_path(self.config_root(), project_root);
            for (event, entries) in read_hooks(&path)? {
                for entry in entries {
                    let id = hook_id(layer, project_root, &event, &entry)?;
                    live_ids.insert(id.clone());
                    views.push(view(id, &event, entry, true, layer.label()));
                }
            }
        }
        for (id, raw) in disabled.hooks {
            if live_ids.contains(&id) {
                continue;
            }
            let item: DisabledHook =
                serde_json::from_str(&raw).map_err(|source| HarnessError::Parse {
                    path: sidecar.clone(),
                    source,
                })?;
            let layer = parse_layer(&item.layer)?;
            if hook_id(layer, project_root, &item.event, &item.entry)? != id {
                continue;
            }
            views.push(view(id, &item.event, item.entry, false, &item.layer));
        }
        views.sort_by(|a, b| {
            a.event
                .cmp(&b.event)
                .then(a.layer.cmp(&b.layer))
                .then(a.id.cmp(&b.id))
        });
        Ok(views)
    }

    pub fn hooks_save(
        &self,
        project_root: &Path,
        layer: SettingsLayer,
        event: &str,
        matcher: Option<String>,
        command: String,
        edit_id: Option<&str>,
    ) -> Result<()> {
        ensure_layer(layer)?;
        if event.trim().is_empty() || command.trim().is_empty() {
            return Err(invalid("event and command must not be empty"));
        }
        let path = layer.settings_path(self.config_root(), project_root);
        let mut events = read_hooks(&path)?;
        let before = events.clone();
        if let Some(id) = edit_id {
            let (old_event, index, entry) = unique_entry(layer, project_root, &events, id)?;
            let list = events.get_mut(&old_event).expect("resolved event exists");
            let Some(command_hook) = entry.hooks.iter().position(is_command_hook) else {
                return Err(invalid("hook has no command entry"));
            };
            let mut updated = entry;
            updated.matcher = matcher;
            updated.hooks[command_hook]["command"] = Value::String(command);
            list.remove(index);
            if list.is_empty() {
                events.remove(&old_event);
            }
            let target = events.entry(event.to_string()).or_default();
            if target.contains(&updated) {
                return Err(invalid(
                    "an identical hook already exists in this event and layer",
                ));
            }
            target.push(updated);
        } else {
            let new_entry = HookMatcher {
                matcher,
                hooks: vec![serde_json::json!({"type":"command", "command":command})],
                extra: BTreeMap::new(),
            };
            if before
                .get(event)
                .into_iter()
                .flatten()
                .any(|entry| *entry == new_entry)
            {
                return Err(invalid(
                    "an identical hook already exists in this event and layer",
                ));
            }
            events.entry(event.to_string()).or_default().push(new_entry);
        }
        write_hooks(self, project_root, layer, before, events)
    }

    pub fn hooks_disable(
        &self,
        project_root: &Path,
        app_data: &Path,
        layer: SettingsLayer,
        id: &str,
    ) -> Result<()> {
        ensure_layer(layer)?;
        let path = layer.settings_path(self.config_root(), project_root);
        let mut events = read_hooks(&path)?;
        let before = events.clone();
        let (event, index, entry) = unique_entry(layer, project_root, &events, id)?;
        let list = events.get_mut(&event).expect("resolved event exists");
        list.remove(index);
        if list.is_empty() {
            events.remove(&event);
        }
        let sidecar = app_data.join("hooks-disabled.json");
        let mut disabled = read_disabled(&sidecar)?;
        let raw = serde_json::to_string(&DisabledHook {
            layer: layer.label().into(),
            project_root: project_identity(layer, project_root),
            event: event.clone(),
            entry: entry.clone(),
        })
        .map_err(|source| HarnessError::WriteJson {
            path: sidecar.clone(),
            source,
        })?;
        disabled
            .hooks
            .insert(hook_id(layer, project_root, &event, &entry)?, raw);
        write_disabled(&sidecar, &disabled)?;
        write_hooks(self, project_root, layer, before, events)
    }

    pub fn hooks_enable(&self, project_root: &Path, app_data: &Path, id: &str) -> Result<()> {
        let sidecar = app_data.join("hooks-disabled.json");
        let mut disabled = read_disabled(&sidecar)?;
        let raw = disabled.hooks.get(id).ok_or_else(|| missing(&sidecar))?;
        let item: DisabledHook =
            serde_json::from_str(raw).map_err(|source| HarnessError::Parse {
                path: sidecar.clone(),
                source,
            })?;
        let layer = parse_layer(&item.layer)?;
        if item.project_root != project_identity(layer, project_root) {
            return Err(invalid("disabled hook belongs to a different project"));
        }
        let path = layer.settings_path(self.config_root(), project_root);
        let mut events = read_hooks(&path)?;
        let before = events.clone();
        let current = events.get(&item.event).cloned().unwrap_or_default();
        let already_live = current.iter().any(|entry| {
            hook_id(layer, project_root, &item.event, entry)
                .map(|candidate| candidate == id)
                .unwrap_or(false)
        });
        if !already_live
            && current
                .iter()
                .any(|entry| entry.matcher == item.entry.matcher)
        {
            return Err(invalid(
                "disabled hook is stale because its matcher has changed",
            ));
        }
        if !already_live {
            events
                .entry(item.event.clone())
                .or_default()
                .push(item.entry);
            write_hooks(self, project_root, layer, before, events)?;
        }
        disabled.hooks.remove(id);
        write_disabled(&sidecar, &disabled)
    }

    pub fn hooks_delete(
        &self,
        project_root: &Path,
        app_data: &Path,
        layer: SettingsLayer,
        id: &str,
    ) -> Result<()> {
        ensure_layer(layer)?;
        let path = layer.settings_path(self.config_root(), project_root);
        let mut events = read_hooks(&path)?;
        let before = events.clone();
        let sidecar = app_data.join("hooks-disabled.json");
        let mut disabled = read_disabled(&sidecar)?;
        match unique_entry(layer, project_root, &events, id) {
            Ok((event, index, _)) => {
                let list = events.get_mut(&event).expect("resolved event exists");
                list.remove(index);
                if list.is_empty() {
                    events.remove(&event);
                }
                write_hooks(self, project_root, layer, before, events)?;
                disabled.hooks.remove(id);
                write_disabled(&sidecar, &disabled)
            }
            Err(error) if disabled.hooks.contains_key(id) => {
                let item: DisabledHook =
                    serde_json::from_str(&disabled.hooks[id]).map_err(|source| {
                        HarnessError::Parse {
                            path: sidecar.clone(),
                            source,
                        }
                    })?;
                if parse_layer(&item.layer)? != layer
                    || item.project_root != project_identity(layer, project_root)
                    || hook_id(layer, project_root, &item.event, &item.entry)? != id
                {
                    return Err(error);
                }
                disabled.hooks.remove(id);
                write_disabled(&sidecar, &disabled)
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DisabledHook {
    layer: String,
    #[serde(default)]
    project_root: Option<String>,
    event: String,
    entry: HookMatcher,
}

pub fn test_command(
    command: &str,
    input: &Value,
    timeout: Duration,
) -> std::result::Result<HookTestResult, String> {
    if command.trim().is_empty() {
        return Err("Hook command must not be empty".into());
    }
    let sandbox = std::env::temp_dir().join(format!(
        "harness-agent-hook-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&sandbox)
        .map_err(|error| format!("Could not create hook test workspace: {error}"))?;
    let mut process = shell_command(command)?;
    process.current_dir(&sandbox).env_clear();
    #[cfg(not(windows))]
    process.process_group(0);
    if let Some(path) = std::env::var_os("PATH") {
        process.env("PATH", path);
    }
    #[cfg(windows)]
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            process.env(key, value);
        }
    }
    process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = process.spawn().map_err(|error| {
        let _ = std::fs::remove_dir_all(&sandbox);
        format!("Could not start hook command: {error}")
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Hook stdin unavailable".to_string())?;
    let bytes = serde_json::to_vec(input).map_err(|error| error.to_string())?;
    thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Hook stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Hook stderr unavailable".to_string())?;
    let (tx, rx) = mpsc::channel();
    for (is_err, mut stream) in [
        (false, Box::new(stdout) as Box<dyn Read + Send>),
        (true, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            let _ = tx.send((is_err, bytes));
        });
    }
    drop(tx);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            kill_process_tree(&mut child);
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut out = String::new();
    let mut err = String::new();
    for _ in 0..2 {
        if let Ok((is_err, bytes)) = rx.recv_timeout(Duration::from_millis(250)) {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if is_err {
                err = text;
            } else {
                out = text;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&sandbox);
    Ok(HookTestResult {
        stdout: out,
        stderr: err,
        exit_code: status.and_then(|s| s.code()),
        timed_out: status.is_none(),
    })
}

fn shell_command(command: &str) -> std::result::Result<Command, String> {
    #[cfg(windows)]
    {
        let mut process = Command::new("cmd");
        process
            .args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{command}\""));
        Ok(process)
    }
    #[cfg(not(windows))]
    {
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        Ok(process)
    }
}

fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
        let _ = child.wait();
    }
    #[cfg(not(windows))]
    {
        let group = format!("-{}", child.id());
        let _ = Command::new("kill")
            .arg("-9")
            .arg(&group)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn decode_hooks(value: Value, path: &Path) -> Result<HookEvents> {
    serde_json::from_value(value).map_err(|source| HarnessError::Parse {
        path: path.to_path_buf(),
        source,
    })
}
fn read_hooks(path: &Path) -> Result<HookEvents> {
    let document = atomic::read_json(path)?;
    decode_hooks(
        document
            .get(HOOKS_KEY)
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new())),
        path,
    )
}
fn write_hooks(
    harness: &ClaudeCodeHarness,
    project: &Path,
    layer: SettingsLayer,
    before: HookEvents,
    events: HookEvents,
) -> Result<()> {
    let path = layer.settings_path(harness.config_root(), project);
    let mut patches = Vec::new();
    for event in before
        .keys()
        .chain(events.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let old = before.get(event).cloned().unwrap_or_default();
        let new = events.get(event).cloned().unwrap_or_default();
        let event_path = atomic::JsonPath::new(vec![HOOKS_KEY.into(), (*event).clone()]);
        if old.is_empty() && !new.is_empty() {
            for entry in new {
                patches.push(atomic::Patch::Push {
                    path: event_path.clone(),
                    value: serde_json::to_value(entry).map_err(|source| {
                        HarnessError::WriteJson {
                            path: path.clone(),
                            source,
                        }
                    })?,
                    key: None,
                });
            }
            continue;
        }
        if new.is_empty() {
            for entry in old {
                patches.push(atomic::Patch::RemoveWhere {
                    path: event_path.clone(),
                    matches: serde_json::to_value(entry).map_err(|source| {
                        HarnessError::WriteJson {
                            path: path.clone(),
                            source,
                        }
                    })?,
                });
            }
            continue;
        }
        let mut replaced_old = std::collections::BTreeSet::new();
        let mut replaced_new = std::collections::BTreeSet::new();
        for (old_index, previous) in old
            .iter()
            .enumerate()
            .filter(|(_, entry)| !new.contains(entry))
        {
            if let Some((new_index, replacement)) = new.iter().enumerate().find(|(index, entry)| {
                !old.contains(entry)
                    && !replaced_new.contains(index)
                    && entry.matcher == previous.matcher
            }) {
                patches.push(atomic::Patch::ReplaceWhere {
                    path: atomic::JsonPath::new(vec![HOOKS_KEY.into(), (*event).clone()]),
                    matches: serde_json::to_value(previous).map_err(|source| {
                        HarnessError::WriteJson {
                            path: path.clone(),
                            source,
                        }
                    })?,
                    value: serde_json::to_value(replacement).map_err(|source| {
                        HarnessError::WriteJson {
                            path: path.clone(),
                            source,
                        }
                    })?,
                });
                replaced_old.insert(old_index);
                replaced_new.insert(new_index);
            }
        }
        for (index, entry) in old
            .iter()
            .enumerate()
            .filter(|(index, entry)| !new.contains(entry) && !replaced_old.contains(index))
        {
            let _ = index;
            patches.push(atomic::Patch::RemoveWhere {
                path: atomic::JsonPath::new(vec![HOOKS_KEY.into(), (*event).clone()]),
                matches: serde_json::to_value(entry).map_err(|source| HarnessError::WriteJson {
                    path: path.clone(),
                    source,
                })?,
            });
        }
        for (index, entry) in new
            .iter()
            .enumerate()
            .filter(|(index, entry)| !old.contains(entry) && !replaced_new.contains(index))
        {
            let _ = index;
            patches.push(atomic::Patch::Push {
                path: atomic::JsonPath::new(vec![HOOKS_KEY.into(), (*event).clone()]),
                value: serde_json::to_value(entry).map_err(|source| HarnessError::WriteJson {
                    path: path.clone(),
                    source,
                })?,
                key: None,
            });
        }
    }
    if patches.is_empty() {
        return Ok(());
    }
    commit_settings(harness, &path, layer, patches)
}
fn commit_settings(
    harness: &ClaudeCodeHarness,
    path: &Path,
    layer: SettingsLayer,
    patches: Vec<atomic::Patch>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| HarnessError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let action = format!("config-{}", layer.label());
    let backup = crate::ledger::backup_path(harness.config_root(), HOOKS_KEY, &action);
    let receipt = atomic::commit(path, patches, Some(&backup)).map_err(HarnessError::from)?;
    crate::ledger::record(
        harness.config_root(),
        ResourceKind::Settings,
        HOOKS_KEY,
        &action,
        &receipt,
    )
    .map_err(|error| HarnessError::Write {
        path: path.to_path_buf(),
        source: std::io::Error::other(error.to_string()),
    })
}
fn parse_layer(layer: &str) -> Result<SettingsLayer> {
    match layer {
        "user" => Ok(SettingsLayer::User),
        "project" => Ok(SettingsLayer::Project),
        _ => Err(invalid("unsupported hook settings layer")),
    }
}
fn ensure_layer(layer: SettingsLayer) -> Result<()> {
    if matches!(layer, SettingsLayer::User | SettingsLayer::Project) {
        Ok(())
    } else {
        Err(invalid(
            "hooks may be changed only in user or project settings",
        ))
    }
}
fn is_command_hook(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("command")
        && value.get("command").and_then(Value::as_str).is_some()
}
fn view(id: String, event: &str, entry: HookMatcher, enabled: bool, layer: &str) -> HookView {
    HookView {
        id,
        event: event.into(),
        matcher: entry.matcher,
        commands: entry
            .hooks
            .iter()
            .filter(|hook| is_command_hook(hook))
            .filter_map(|hook| hook["command"].as_str().map(str::to_owned))
            .collect(),
        enabled,
        layer: layer.into(),
    }
}
fn project_identity(layer: SettingsLayer, project_root: &Path) -> Option<String> {
    match layer {
        SettingsLayer::Project => Some(
            std::fs::canonicalize(project_root)
                .unwrap_or_else(|_| project_root.to_path_buf())
                .to_string_lossy()
                .into_owned(),
        ),
        SettingsLayer::User => None,
        SettingsLayer::Local => None,
    }
}
fn hook_id(
    layer: SettingsLayer,
    project_root: &Path,
    event: &str,
    entry: &HookMatcher,
) -> Result<String> {
    let identity = project_identity(layer, project_root);
    let bytes = serde_json::to_vec(&(identity.as_deref(), event, entry)).map_err(|source| {
        HarnessError::WriteJson {
            path: PathBuf::from("hooks"),
            source,
        }
    })?;
    Ok(format!(
        "{}:{event}:{}",
        layer.label(),
        crate::harness::atomic::hex(&sha2::Sha256::digest(&bytes))
    ))
}
fn unique_entry(
    layer: SettingsLayer,
    project_root: &Path,
    events: &HookEvents,
    id: &str,
) -> Result<(String, usize, HookMatcher)> {
    let mut found = Vec::new();
    for (event, entries) in events {
        for (index, entry) in entries.iter().enumerate() {
            if hook_id(layer, project_root, event, entry)? == id {
                found.push((event.clone(), index, entry.clone()));
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(invalid(
            "hook identity is stale or does not match this layer",
        )),
        _ => Err(invalid("hook identity is ambiguous")),
    }
}
fn invalid(message: &str) -> HarnessError {
    HarnessError::Write {
        path: PathBuf::from("hooks"),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message),
    }
}
fn missing(path: &Path) -> HarnessError {
    HarnessError::Read {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "hook not found"),
    }
}
fn read_disabled(path: &Path) -> Result<DisabledHooks> {
    atomic::read_struct(path)
}
fn write_disabled(path: &Path, disabled: &DisabledHooks) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| HarnessError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    atomic::commit(
        path,
        vec![atomic::Patch::SetPath {
            path: atomic::p!["hooks"],
            value: serde_json::to_value(&disabled.hooks).map_err(|source| {
                HarnessError::WriteJson {
                    path: path.to_path_buf(),
                    source,
                }
            })?,
        }],
        None,
    )
    .map(|_| ())
    .map_err(HarnessError::from)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::HarnessPaths;
    use serde_json::json;

    #[test]
    fn typed_hooks_round_trip_unknown_events_and_leave_other_settings_keys_untouched() {
        let raw = br#"{"theme":"dark","hooks":{"PreToolUse":[{"matcher":"Read","hooks":[{"type":"command","command":"echo ok"}]}],"FutureEvent":[{"hooks":[{"type":"command","command":"echo future"}]}]},"custom":{"v":1}}"#;
        let value: Value = serde_json::from_slice(raw).unwrap();
        let hooks = decode_hooks(value["hooks"].clone(), Path::new("fixture")).unwrap();
        let roundtrip = serde_json::to_value(hooks).unwrap();
        let mut result = value.clone();
        result["hooks"] = roundtrip;
        assert_eq!(result, value);
        assert_eq!(result["theme"], "dark");
        assert_eq!(result["custom"], json!({"v":1}));
        let directory = tempfile::tempdir().unwrap();
        let config_root = directory.path().join("user");
        let project = directory.path().join("project");
        let app_data = directory.path().join("app-data");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let settings = config_root.join("settings.json");
        std::fs::write(&settings, raw).unwrap();
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            config_root: config_root.clone(),
            claude_json: directory.path().join(".claude.json"),
        });
        assert_eq!(harness.hooks_list(&project, &app_data).unwrap().len(), 2);
        assert_eq!(std::fs::read(&settings).unwrap(), raw);
    }

    #[test]
    fn matcher_shape_accepts_each_documented_event_and_unknown_event() {
        let matcher =
            json!([{"matcher":"Read|Write","hooks":[{"type":"command","command":"echo"}]}]);
        for event in [
            "PreToolUse",
            "PostToolUse",
            "Notification",
            "UserPromptSubmit",
            "Stop",
            "SubagentStop",
            "PreCompact",
            "SessionStart",
            "SessionEnd",
            "FutureEvent",
        ] {
            let events = json!({event: matcher.clone()});
            assert!(
                decode_hooks(events, Path::new("fixture")).is_ok(),
                "{event}"
            );
        }
    }

    #[test]
    fn editing_command_preserves_sibling_hooks_unknown_fields_and_non_command_hooks() {
        let directory = tempfile::tempdir().unwrap();
        let config_root = directory.path().join("user");
        let project = directory.path().join("project");
        let app_data = directory.path().join("app-data");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let settings = config_root.join("settings.json");
        std::fs::write(&settings, br#"{"hooks":{"PreToolUse":[{"matcher":"Read","customMatcher":7,"hooks":[{"type":"command","command":"old","timeout":30,"future":"keep"},{"type":"command","command":"notify","timeout":9}]},{"matcher":"Read","hooks":[{"type":"prompt","prompt":"keep prompt"}]}]}}"#).unwrap();
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            config_root,
            claude_json: directory.path().join(".claude.json"),
        });
        let id = harness
            .hooks_list(&project, &app_data)
            .unwrap()
            .into_iter()
            .find(|item| item.commands.contains(&"old".to_string()))
            .unwrap()
            .id;
        harness
            .hooks_save(
                &project,
                SettingsLayer::User,
                "PreToolUse",
                Some("Read".into()),
                "new".into(),
                Some(&id),
            )
            .unwrap();
        let result = atomic::read_json(&settings).unwrap();
        assert_eq!(
            result["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "new"
        );
        assert_eq!(result["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 30);
        assert_eq!(
            result["hooks"]["PreToolUse"][0]["hooks"][0]["future"],
            "keep"
        );
        assert_eq!(
            result["hooks"]["PreToolUse"][0]["hooks"][1]["command"],
            "notify"
        );
        assert_eq!(result["hooks"]["PreToolUse"][0]["customMatcher"], 7);
        assert_eq!(
            result["hooks"]["PreToolUse"][1]["hooks"][0]["prompt"],
            "keep prompt"
        );
    }

    #[test]
    fn every_native_hook_change_records_exact_backup_and_receipt_hashes() {
        let directory = tempfile::tempdir().unwrap();
        let config_root = directory.path().join("user");
        let project = directory.path().join("project");
        let app_data = directory.path().join("app-data");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let settings = config_root.join("settings.json");
        std::fs::write(&settings, br#"{"theme":"dark"}"#).unwrap();
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            config_root: config_root.clone(),
            claude_json: directory.path().join(".claude.json"),
        });
        let mut expected_rows = 0;
        let mut check = |before: &[u8]| {
            let rows = crate::ledger::read(&config_root).unwrap();
            expected_rows += 1;
            assert_eq!(rows.len(), expected_rows);
            let row = rows.last().unwrap();
            assert_eq!(row.resource_kind, "settings");
            assert_eq!(row.id, "hooks");
            assert_eq!(row.action, "config-user");
            assert_eq!(
                row.before_checksum.as_deref(),
                Some(crate::harness::atomic::hex(&sha2::Sha256::digest(before)).as_str())
            );
            let backup = std::fs::read(row.backup_path.as_ref().unwrap()).unwrap();
            assert_eq!(backup, before);
            let landed = std::fs::read(&settings).unwrap();
            assert_eq!(
                row.after_checksum.as_deref(),
                Some(crate::harness::atomic::hex(&sha2::Sha256::digest(&landed)).as_str())
            );
            assert_eq!(
                row.target.as_deref(),
                Some(settings.to_string_lossy().as_ref())
            );
        };

        let before = std::fs::read(&settings).unwrap();
        harness
            .hooks_save(
                &project,
                SettingsLayer::User,
                "PreToolUse",
                Some("Read".into()),
                "first".into(),
                None,
            )
            .unwrap();
        check(&before);
        let id = harness
            .hooks_list(&project, &app_data)
            .unwrap()
            .into_iter()
            .find(|hook| hook.enabled)
            .unwrap()
            .id;

        let before = std::fs::read(&settings).unwrap();
        harness
            .hooks_save(
                &project,
                SettingsLayer::User,
                "PreToolUse",
                Some("Read".into()),
                "edited".into(),
                Some(&id),
            )
            .unwrap();
        check(&before);
        let id = harness
            .hooks_list(&project, &app_data)
            .unwrap()
            .into_iter()
            .find(|hook| hook.enabled)
            .unwrap()
            .id;

        let before = std::fs::read(&settings).unwrap();
        harness
            .hooks_disable(&project, &app_data, SettingsLayer::User, &id)
            .unwrap();
        check(&before);
        let id = harness
            .hooks_list(&project, &app_data)
            .unwrap()
            .into_iter()
            .find(|hook| !hook.enabled)
            .unwrap()
            .id;

        let before = std::fs::read(&settings).unwrap();
        harness.hooks_enable(&project, &app_data, &id).unwrap();
        check(&before);
        let id = harness
            .hooks_list(&project, &app_data)
            .unwrap()
            .into_iter()
            .find(|hook| hook.enabled)
            .unwrap()
            .id;

        let before = std::fs::read(&settings).unwrap();
        harness
            .hooks_delete(&project, &app_data, SettingsLayer::User, &id)
            .unwrap();
        check(&before);
    }

    #[test]
    fn project_hook_ids_and_disabled_records_are_scoped_to_the_project() {
        let directory = tempfile::tempdir().unwrap();
        let config_root = directory.path().join("user");
        let project_a = directory.path().join("project-a");
        let project_b = directory.path().join("project-b");
        let app_data = directory.path().join("app-data");
        std::fs::create_dir_all(&config_root).unwrap();
        for project in [&project_a, &project_b] {
            let settings = project.join(".claude/settings.json");
            std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
            std::fs::write(&settings, br#"{"hooks":{"PreToolUse":[{"matcher":"Read","hooks":[{"type":"command","command":"same"}]}]}}"#).unwrap();
        }
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            config_root,
            claude_json: directory.path().join(".claude.json"),
        });
        let id_a = harness.hooks_list(&project_a, &app_data).unwrap()[0]
            .id
            .clone();
        let id_b = harness.hooks_list(&project_b, &app_data).unwrap()[0]
            .id
            .clone();
        assert_ne!(id_a, id_b);
        harness
            .hooks_disable(&project_a, &app_data, SettingsLayer::Project, &id_a)
            .unwrap();
        let hooks_b = harness.hooks_list(&project_b, &app_data).unwrap();
        assert_eq!(hooks_b.len(), 1);
        assert!(hooks_b[0].enabled);
        assert_eq!(hooks_b[0].id, id_b);
    }

    #[test]
    fn content_id_refuses_ambiguous_and_stale_hooks() {
        let entry: HookMatcher =
            serde_json::from_value(json!({"hooks":[{"type":"command","command":"same"}]})).unwrap();
        let id = hook_id(
            SettingsLayer::User,
            Path::new("fixture"),
            "PreToolUse",
            &entry,
        )
        .unwrap();
        let duplicated =
            BTreeMap::from([("PreToolUse".into(), vec![entry.clone(), entry.clone()])]);
        assert!(
            unique_entry(SettingsLayer::User, Path::new("fixture"), &duplicated, &id)
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert!(unique_entry(
            SettingsLayer::User,
            Path::new("fixture"),
            &HookEvents::new(),
            &id
        )
        .unwrap_err()
        .to_string()
        .contains("stale"));
    }

    #[test]
    fn saving_an_identical_hook_refuses_and_leaves_the_file_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let config_root = directory.path().join("user/.claude");
        let project = directory.path().join("project");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            config_root: config_root.clone(),
            claude_json: directory.path().join(".claude.json"),
        });
        harness
            .hooks_save(
                &project,
                SettingsLayer::User,
                "PreToolUse",
                Some("Read".into()),
                "echo checked".into(),
                None,
            )
            .unwrap();
        let settings = config_root.join("settings.json");
        let after_first = atomic::read_json(&settings).unwrap();
        let duplicate = harness.hooks_save(
            &project,
            SettingsLayer::User,
            "PreToolUse",
            Some("Read".into()),
            "echo checked".into(),
            None,
        );
        assert!(
            duplicate
                .unwrap_err()
                .to_string()
                .contains("an identical hook already exists"),
            "duplicate save must refuse"
        );
        assert_eq!(
            atomic::read_json(&settings).unwrap(),
            after_first,
            "refused save must not change the file"
        );
        harness
            .hooks_save(
                &project,
                SettingsLayer::User,
                "PreToolUse",
                Some("Read".into()),
                "echo different".into(),
                None,
            )
            .unwrap();
        assert_eq!(
            atomic::read_json(&settings).unwrap()["hooks"]["PreToolUse"]
                .as_array()
                .unwrap()
                .len(),
            2,
            "a distinct command in the same event and layer must still save"
        );
    }

    #[test]
    fn editing_a_hook_onto_an_identical_sibling_refuses_and_leaves_the_file_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let config_root = directory.path().join("user/.claude");
        let project = directory.path().join("project");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            config_root: config_root.clone(),
            claude_json: directory.path().join(".claude.json"),
        });
        for command in ["echo alpha", "echo beta"] {
            harness
                .hooks_save(
                    &project,
                    SettingsLayer::User,
                    "PreToolUse",
                    Some("Read".into()),
                    command.into(),
                    None,
                )
                .unwrap();
        }
        let settings = config_root.join("settings.json");
        let before = atomic::read_json(&settings).unwrap();
        let app_data = directory.path().join("app-data");
        let beta_id = harness
            .hooks_list(&project, &app_data)
            .unwrap()
            .into_iter()
            .find(|hook| hook.commands.contains(&"echo beta".to_string()))
            .unwrap()
            .id;
        let edited = harness.hooks_save(
            &project,
            SettingsLayer::User,
            "PreToolUse",
            Some("Read".into()),
            "echo alpha".into(),
            Some(&beta_id),
        );
        assert!(
            edited
                .unwrap_err()
                .to_string()
                .contains("an identical hook already exists"),
            "edit onto an identical sibling must refuse"
        );
        assert_eq!(
            atomic::read_json(&settings).unwrap(),
            before,
            "refused edit must not change the file"
        );
    }

    #[test]
    fn command_runner_captures_success_and_failure_and_times_out() {
        let success = test_command(
            if cfg!(windows) { "more" } else { "cat" },
            &json!({"sample":true}),
            Duration::from_secs(1),
        )
        .unwrap();
        #[cfg(not(windows))]
        assert!(success.stdout.contains("sample"));
        #[cfg(windows)]
        assert_eq!(success.exit_code, Some(0));
        #[cfg(not(windows))]
        let failure =
            test_command("cat >/dev/null; exit 7", &json!({}), Duration::from_secs(1)).unwrap();
        #[cfg(windows)]
        let failure = test_command("exit /b 7", &json!({}), Duration::from_secs(1)).unwrap();
        assert_eq!(failure.exit_code, Some(7));
        #[cfg(not(windows))]
        let timeout = test_command("sleep 1", &json!({}), Duration::from_millis(30)).unwrap();
        #[cfg(windows)]
        let timeout = test_command(
            "ping -n 3 127.0.0.1 >NUL",
            &json!({}),
            Duration::from_millis(30),
        )
        .unwrap();
        assert!(timeout.timed_out);
    }

    #[test]
    fn timing_out_kills_descendants_the_child_handle_cannot_address() {
        let log = std::env::temp_dir().join(format!(
            "hook-tree-kill-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&log);
        let log_text = log.to_string_lossy().into_owned();
        let command = if cfg!(windows) {
            format!("ping -n 30 127.0.0.1 > \"{log_text}\"")
        } else {
            format!(
                "(i=0; while [ $i -lt 300 ]; do echo tick >> '{log_text}'; \
                     i=$((i + 1)); sleep 0.2; done) & sleep 30"
            )
        };
        let spawned = test_command(&command, &json!({}), Duration::from_millis(300)).unwrap();
        assert!(spawned.timed_out, "{spawned:?}");
        if cfg!(windows) {
            let deadline = Instant::now() + Duration::from_secs(8);
            let released = loop {
                if std::fs::remove_file(&log).is_ok() {
                    break true;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                thread::sleep(Duration::from_millis(100));
            };
            assert!(released, "a surviving descendant still holds the log open");
        } else {
            let frozen = std::fs::read(&log).unwrap_or_default();
            thread::sleep(Duration::from_millis(2500));
            assert_eq!(
                std::fs::read(&log).unwrap_or_default(),
                frozen,
                "a surviving descendant would keep appending to the log"
            );
        }
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn builder_uses_selected_layer_and_disable_enable_restores_the_hook() {
        let directory = tempfile::tempdir().unwrap();
        let config_root = directory.path().join("user/.claude");
        let project = directory.path().join("project");
        let app_data = directory.path().join("app-data");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            config_root: config_root.clone(),
            claude_json: directory.path().join(".claude.json"),
        });
        harness
            .hooks_save(
                &project,
                SettingsLayer::Project,
                "PreToolUse",
                Some("Read".into()),
                "echo checked".into(),
                None,
            )
            .unwrap();
        let project_settings = project.join(".claude/settings.json");
        let project_value = atomic::read_json(&project_settings).unwrap();
        assert_eq!(project_value["hooks"]["PreToolUse"][0]["matcher"], "Read");
        assert!(!config_root.join("settings.json").exists());

        let live = harness
            .hooks_list(&project, &app_data)
            .unwrap()
            .into_iter()
            .find(|hook| hook.enabled && hook.layer == "project")
            .unwrap();
        harness
            .hooks_disable(&project, &app_data, SettingsLayer::Project, &live.id)
            .unwrap();
        assert_eq!(
            atomic::read_json(&project_settings).unwrap()["hooks"]["PreToolUse"],
            json!([])
        );
        let disabled = harness.hooks_list(&project, &app_data).unwrap();
        assert_eq!(disabled.len(), 1);
        assert!(!disabled[0].enabled);
        harness
            .hooks_enable(&project, &app_data, &disabled[0].id)
            .unwrap();
        assert_eq!(
            atomic::read_json(&project_settings).unwrap()["hooks"]["PreToolUse"][0]["hooks"][0]
                ["command"],
            "echo checked"
        );
    }
}
