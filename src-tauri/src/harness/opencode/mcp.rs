use serde_json::{Map, Value};

use super::{config, OpenCodeHarness};
use crate::harness::{
    claude::mcp::{self, BenchResult, HttpEntry, ServerView, StdioEntry},
    credentials::contains_mask,
    Harness,
};

const FIELD: &str = "mcp";
const SERVERS: &str = "servers";

pub fn list(harness: &OpenCodeHarness) -> Result<Vec<ServerView>, String> {
    let config = config::read_config_value(&harness.settings_path())?;
    let Some(mcp) = config.get(FIELD) else {
        return Ok(Vec::new());
    };
    let Some(value) = mcp.get(SERVERS) else {
        return Ok(Vec::new());
    };
    let servers = value
        .as_object()
        .ok_or("OpenCode `mcp.servers` setting must be an object")?;
    let mut result = Vec::with_capacity(servers.len());
    for (name, native) in servers {
        let enabled = native
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let entry = to_editor_entry(native)?;
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .ok_or("invalid MCP transport")?
            .to_owned();
        result.push(ServerView {
            name: name.clone(),
            kind,
            entry: mcp::mask_entry(&entry),
            enabled,
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

pub fn save(harness: &OpenCodeHarness, name: &str, entry: Value) -> Result<(), String> {
    validate_name(name)?;
    let native = to_native_entry(entry)?;
    config::mutate_jsonc(
        harness,
        &harness.settings_path(),
        name,
        "opencode-mcp-save",
        |root| {
            let servers = root.object_value_or_set(FIELD).object_value_or_set(SERVERS);
            let mut native = native;
            if let Some(previous) = servers
                .get(name)
                .and_then(|property| property.to_serde_value())
            {
                let previous_editor = to_editor_entry(&previous)?;
                let mut edited = to_editor_entry(&native)?;
                mcp::preserve_masked_credentials(&previous_editor, &mut edited)
                    .map_err(|error| error.to_string())?;
                native = to_native_entry(edited)?;
                if let Some(enabled) = previous.get("enabled") {
                    native
                        .as_object_mut()
                        .expect("native MCP entry is an object")
                        .insert("enabled".into(), enabled.clone());
                }
            }
            if contains_mask(&native) {
                return Err("the edited entry still holds masked placeholders; re-enter the credential values".into());
            }
            validate_native(&native)?;
            servers.get(name).map_or_else(
                || {
                    servers.append(name, config::cst_input_value(native.clone()));
                },
                |property| property.set_value(config::cst_input_value(native.clone())),
            );
            Ok(())
        },
    )?;
    Ok(())
}

pub fn set_enabled(harness: &OpenCodeHarness, name: &str, enabled: bool) -> Result<(), String> {
    validate_name(name)?;
    let config = config::read_config_value(&harness.settings_path())?;
    if config
        .get(FIELD)
        .and_then(|mcp| mcp.get(SERVERS))
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(name))
        .is_none()
    {
        return Err(format!("MCP server `{name}` was not found"));
    }
    config::mutate_jsonc(
        harness,
        &harness.settings_path(),
        name,
        "opencode-mcp-enabled",
        |root| {
            let servers = root.object_value_or_set(FIELD).object_value_or_set(SERVERS);
            let entry = servers
                .get(name)
                .ok_or_else(|| format!("MCP server `{name}` was not found"))?
                .object_value_or_set();
            if let Some(property) = entry.get("enabled") {
                property.set_value(enabled.into());
            } else {
                entry.append("enabled", enabled.into());
            }
            Ok(())
        },
    )
}

pub fn delete(harness: &OpenCodeHarness, name: &str) -> Result<(), String> {
    validate_name(name)?;
    config::mutate_jsonc(
        harness,
        &harness.settings_path(),
        name,
        "opencode-mcp-delete",
        |root| {
            let servers = root.object_value_or_set(FIELD).object_value_or_set(SERVERS);
            let property = servers
                .get(name)
                .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
            property.remove();
            Ok(())
        },
    )?;
    Ok(())
}

pub fn test(harness: &OpenCodeHarness, name: &str) -> Result<BenchResult, String> {
    let config = config::read_config_value(&harness.settings_path())?;
    let native = config
        .get(FIELD)
        .and_then(|mcp| mcp.get(SERVERS))
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(name))
        .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
    if native.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Err("MCP server is disabled".into());
    }
    let entry = to_editor_entry(native)?;
    match entry.get("type").and_then(Value::as_str) {
        Some("stdio") => mcp::test_stdio(
            &serde_json::from_value::<StdioEntry>(entry)
                .map_err(|error| format!("Invalid local MCP server: {error}"))?,
        ),
        Some("http") => mcp::test_http(
            &serde_json::from_value::<HttpEntry>(entry)
                .map_err(|error| format!("Invalid remote MCP server: {error}"))?,
        ),
        _ => Err("unsupported OpenCode MCP transport".into()),
    }
}

fn to_editor_entry(native: &Value) -> Result<Value, String> {
    let object = native
        .as_object()
        .ok_or("MCP server entry must be an object")?;
    match object.get("type").and_then(Value::as_str) {
        Some("local") => {
            let command = object
                .get("command")
                .and_then(Value::as_array)
                .ok_or("OpenCode local MCP `command` must be an array")?;
            let executable = command
                .first()
                .and_then(Value::as_str)
                .ok_or("OpenCode local MCP command cannot be empty")?;
            let args = command.iter().skip(1).cloned().collect::<Vec<_>>();
            let mut view = Map::new();
            for (key, value) in object {
                if !["type", "command", "environment", "enabled"].contains(&key.as_str()) {
                    view.insert(key.clone(), value.clone());
                }
            }
            view.insert("type".into(), Value::String("stdio".into()));
            view.insert("command".into(), Value::String(executable.into()));
            view.insert("args".into(), Value::Array(args));
            if let Some(environment) = object.get("environment") {
                view.insert("env".into(), environment.clone());
            }
            if let Some(enabled) = object.get("enabled") {
                view.insert("enabled".into(), enabled.clone());
            }
            if let Some(timeout) = object.get("timeout") {
                view.insert("timeout".into(), timeout.clone());
            }
            Ok(Value::Object(view))
        }
        Some("remote") => {
            let mut view = object.clone();
            view.remove("type");
            view.insert("type".into(), Value::String("http".into()));
            if !view.contains_key("url") {
                return Err("OpenCode remote MCP requires `url`".into());
            }
            Ok(Value::Object(view))
        }
        _ => Err("OpenCode MCP entry requires `type` set to `local` or `remote`".into()),
    }
}

fn to_native_entry(mut entry: Value) -> Result<Value, String> {
    let object = entry
        .as_object_mut()
        .ok_or("MCP server entry must be an object")?;
    let kind = object
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            if object.contains_key("command") {
                "stdio".into()
            } else {
                "http".into()
            }
        });
    match kind.as_str() {
        "stdio" => {
            let command = object
                .remove("command")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or("local MCP command must be a string")?;
            if command.trim().is_empty() {
                return Err("local MCP command cannot be empty".into());
            }
            let args = object
                .remove("args")
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default();
            let mut command_parts = vec![Value::String(command)];
            command_parts.extend(args);
            let environment = object.remove("env");
            let enabled = object.remove("enabled");
            let timeout = object.remove("timeout");
            let mut native = Map::new();
            native.extend(object.clone());
            native.insert("type".into(), Value::String("local".into()));
            native.insert("command".into(), Value::Array(command_parts));
            if let Some(environment) = environment {
                native.insert("environment".into(), environment);
            }
            if let Some(enabled) = enabled {
                native.insert("enabled".into(), enabled);
            }
            if let Some(timeout) = timeout {
                native.insert("timeout".into(), timeout);
            }
            Ok(Value::Object(native))
        }
        "http" => {
            let url = object
                .remove("url")
                .ok_or("remote MCP URL cannot be empty")?;
            if url.as_str().is_none_or(str::is_empty) {
                return Err("remote MCP URL cannot be empty".into());
            }
            let headers = object.remove("headers");
            let oauth = object.remove("oauth");
            let enabled = object.remove("enabled");
            let mut native = object.clone();
            native.insert("type".into(), Value::String("remote".into()));
            native.insert("url".into(), url);
            if let Some(headers) = headers {
                native.insert("headers".into(), headers);
            }
            if let Some(oauth) = oauth {
                native.insert("oauth".into(), oauth);
            }
            if let Some(enabled) = enabled {
                native.insert("enabled".into(), enabled);
            }
            Ok(Value::Object(native))
        }
        _ => Err("OpenCode MCP transport must be `stdio` or `http`".into()),
    }
}

fn validate_native(entry: &Value) -> Result<(), String> {
    match entry.get("type").and_then(Value::as_str) {
        Some("local") => {
            let config = to_editor_entry(entry)?;
            let _: StdioEntry = serde_json::from_value(config)
                .map_err(|error| format!("Invalid local MCP server: {error}"))?;
            Ok(())
        }
        Some("remote") => {
            let config = to_editor_entry(entry)?;
            let _: HttpEntry = serde_json::from_value(config)
                .map_err(|error| format!("Invalid remote MCP server: {error}"))?;
            Ok(())
        }
        _ => Err("OpenCode MCP type must be `local` or `remote`".into()),
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        Err("MCP server name cannot be empty or contain control characters".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tempdir;

    #[test]
    fn native_local_server_roundtrips_through_editor_contract_and_jsonc_comments_survive() {
        let temp = tempdir().unwrap();
        let harness = OpenCodeHarness::new(temp.path().join("cfg"), temp.path().to_path_buf());
        std::fs::create_dir_all(harness.config_root()).unwrap();
        let path = harness.settings_path();
        std::fs::write(
            &path,
            "{\n  // useful note\n  \"mcp\": {\"servers\": {}},\n}\n",
        )
        .unwrap();
        save(&harness, "fixture", serde_json::json!({"type":"stdio","command":"node","args":["server.js"],"env":{"MODE":"test"}})).unwrap();
        let found = list(&harness).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].entry["command"], "node");
        assert_eq!(found[0].entry["args"][0], "server.js");
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .contains("// useful note"));
        delete(&harness, "fixture").unwrap();
        assert!(list(&harness).unwrap().is_empty());
    }
}
