use serde_json::{Map, Value};

use super::{CodexHarness, store};
use crate::harness::{
    Harness, ResourceKind,
    claude::mcp::{self, BenchResult, HttpEntry, ServerView, StdioEntry},
};

const SERVERS: &str = "mcp_servers";

pub fn list(harness: &CodexHarness) -> Result<Vec<ServerView>, String> {
    let config = store::read_value(&harness.settings_path())?;
    let servers = server_map(&config)?;
    let mut result = Vec::with_capacity(servers.len());
    for (name, raw) in servers {
        let kind = kind(raw)?;
        let enabled = raw.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let mut entry = normalized_headers(raw.clone());
        entry
            .as_object_mut()
            .expect("MCP entry is an object")
            .insert("type".into(), Value::String(kind.into()));
        let entry = mcp::mask_entry(&entry);
        result.push(ServerView {
            name: name.clone(),
            kind: kind.into(),
            entry,
            enabled,
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

pub fn save(harness: &CodexHarness, name: &str, mut entry: Value) -> Result<(), String> {
    validate_name(name)?;
    let mut edited = entry
        .as_object()
        .cloned()
        .ok_or("MCP server entry must be an object")?;
    let kind = edited
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            if edited.contains_key("command") {
                "stdio".into()
            } else {
                "http".into()
            }
        });
    if !matches!(kind.as_str(), "stdio" | "http") {
        return Err("Codex MCP transport must be `stdio` or `http`".into());
    }
    let enabled = edited
        .remove("enabled")
        .map(|value| value.as_bool().ok_or("`enabled` must be a boolean"))
        .transpose()?;
    if let Some(headers) = edited.remove("headers") {
        if edited.contains_key("http_headers") {
            return Err("entry cannot contain both `headers` and Codex `http_headers`".into());
        }
        edited.insert("http_headers".into(), headers);
    }
    entry = Value::Object(edited);
    let normalized = normalized_headers(entry);
    let id = name.to_owned();
    let path = harness.settings_path();
    let path_for_edit = path.clone();
    store::mutate(
        harness,
        ResourceKind::McpServer,
        name,
        "codex-mcp-save",
        &path,
        move |document| {
            let current = store::value(document, &path_for_edit)?;
            let previous = server_map(&current)?.get(&id).cloned();
            let mut candidate = normalized;
            if let Some(previous) = &previous {
                mcp::preserve_masked_credentials(
                    &normalized_headers(previous.clone()),
                    &mut candidate,
                )
                .map_err(|error| error.to_string())?;
            }
            if crate::harness::credentials::contains_mask(&candidate) {
                return Err(
                    "the edited entry still holds masked placeholders; re-enter the credential values"
                        .into(),
                );
            }
            let mut raw = restore_native_headers(candidate);
            let object = raw
                .as_object_mut()
                .ok_or("MCP server entry must be an object")?;
            if let Some(enabled) = enabled {
                object.insert("enabled".into(), Value::Bool(enabled));
            } else if let Some(old_enabled) = previous.as_ref().and_then(|old| old.get("enabled")) {
                object.insert("enabled".into(), old_enabled.clone());
            }
            validate_entry(&raw, &kind)?;
            let item = json_item(&raw)?;
            document[SERVERS][id.as_str()] = item;
            Ok(())
        },
    )?;
    Ok(())
}

pub fn set_enabled(harness: &CodexHarness, name: &str, enabled: bool) -> Result<(), String> {
    validate_name(name)?;
    store::mutate(
        harness,
        ResourceKind::McpServer,
        name,
        "codex-mcp-enabled",
        &harness.settings_path(),
        |document| {
            let entry = document
                .get_mut(SERVERS)
                .and_then(|servers| servers.get_mut(name))
                .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
            if !entry.is_table() && !entry.is_inline_table() {
                return Err(format!("MCP server `{name}` must be a TOML table"));
            }
            entry["enabled"] = toml_edit::value(enabled);
            Ok(())
        },
    )
}

pub fn delete(harness: &CodexHarness, name: &str) -> Result<(), String> {
    validate_name(name)?;
    store::mutate(
        harness,
        ResourceKind::McpServer,
        name,
        "codex-mcp-delete",
        &harness.settings_path(),
        |document| {
            let Some(servers) = document.get_mut(SERVERS) else {
                return Err(format!("MCP server `{name}` was not found"));
            };
            let removed = if let Some(table) = servers.as_table_mut() {
                table.remove(name).is_some()
            } else if let Some(table) = servers.as_inline_table_mut() {
                table.remove(name).is_some()
            } else {
                false
            };
            if !removed {
                return Err(format!("MCP server `{name}` was not found"));
            }
            Ok(())
        },
    )
}

pub fn test(harness: &CodexHarness, name: &str) -> Result<BenchResult, String> {
    let entry = server_map(&store::read_value(&harness.settings_path())?)?
        .get(name)
        .cloned()
        .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
    if entry.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Err("MCP server is disabled".into());
    }
    let kind = kind(&entry)?;
    let normalized = normalized_headers(entry.clone());
    match kind {
        "stdio" => mcp::test_stdio(
            &serde_json::from_value::<StdioEntry>(normalized)
                .map_err(|error| format!("Invalid stdio MCP server: {error}"))?,
        ),
        _ => test_http_native(&entry, normalized),
    }
}

fn server_map(value: &Value) -> Result<&Map<String, Value>, String> {
    match value.get(SERVERS) {
        None => Ok(empty_map()),
        Some(Value::Object(servers)) => Ok(servers),
        Some(_) => Err("`mcp_servers` must be a TOML table".into()),
    }
}

fn empty_map() -> &'static Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn kind(entry: &Value) -> Result<&'static str, String> {
    let object = entry
        .as_object()
        .ok_or("MCP server entry must be an object")?;
    if object.contains_key("command") {
        Ok("stdio")
    } else if object.contains_key("url") {
        Ok("http")
    } else {
        Err("MCP server must have a `command` or `url`".into())
    }
}

fn validate_entry(entry: &Value, expected: &str) -> Result<(), String> {
    match expected {
        "stdio" => {
            let config: StdioEntry = serde_json::from_value(entry.clone())
                .map_err(|error| format!("Invalid stdio MCP server: {error}"))?;
            if config.command.trim().is_empty() {
                return Err("stdio MCP command cannot be empty".into());
            }
        }
        "http" => {
            let config: HttpEntry = serde_json::from_value(normalized_headers(entry.clone()))
                .map_err(|error| format!("Invalid HTTP MCP server: {error}"))?;
            if config.url.trim().is_empty() {
                return Err("HTTP MCP URL cannot be empty".into());
            }
            validate_env_header_map(entry.get("env_http_headers"))?;
        }
        _ => return Err("unsupported MCP transport".into()),
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        Err("MCP server name must be non-empty and contain no control characters".into())
    } else {
        Ok(())
    }
}

fn validate_env_header_map(value: Option<&Value>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let object = value
        .as_object()
        .ok_or("`env_http_headers` must map header names to environment variable names")?;
    if object.iter().any(|(header, variable)| {
        header.trim().is_empty() || variable.as_str().is_none_or(str::is_empty)
    }) {
        return Err(
            "`env_http_headers` must contain non-empty header and environment variable names"
                .into(),
        );
    }
    Ok(())
}

fn test_http_native(entry: &Value, mut normalized: Value) -> Result<BenchResult, String> {
    if entry.get("http_headers_helper").is_some() {
        return Err("Codex `http_headers_helper` is not run by Orbit's MCP test; test this connection with Codex.".into());
    }
    if entry
        .get("auth")
        .and_then(Value::as_str)
        .is_some_and(|auth| matches!(auth, "oauth" | "chatgpt"))
    {
        return Err("Codex OAuth or ChatGPT-session authentication is not available to Orbit's MCP test; configure explicit headers or a bearer-token environment variable.".into());
    }
    let object = normalized
        .as_object_mut()
        .ok_or("HTTP MCP entry must be an object")?;
    let headers = object
        .entry("headers")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("Codex `http_headers` must be a string map")?;
    if let Some(token_var) = entry.get("bearer_token_env_var").and_then(Value::as_str) {
        let token = std::env::var(token_var).map_err(|_| {
            format!("MCP bearer token environment variable `{token_var}` is not set")
        })?;
        headers.retain(|name, _| !name.eq_ignore_ascii_case("authorization"));
        headers.insert(
            "Authorization".into(),
            Value::String(format!("Bearer {token}")),
        );
    }
    if let Some(env_headers) = entry.get("env_http_headers") {
        validate_env_header_map(Some(env_headers))?;
        for (header, variable) in env_headers.as_object().unwrap() {
            let Some(value) = variable.as_str().and_then(|name| std::env::var(name).ok()) else {
                continue;
            };
            if entry.get("bearer_token_env_var").is_some()
                && header.eq_ignore_ascii_case("authorization")
            {
                continue;
            }
            headers.insert(header.clone(), Value::String(value));
        }
    }
    mcp::test_http(
        &serde_json::from_value::<HttpEntry>(normalized)
            .map_err(|error| format!("Invalid HTTP MCP server: {error}"))?,
    )
}

fn normalized_headers(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        if let Some(headers) = object.remove("http_headers") {
            object.insert("headers".into(), headers);
        }
    }
    value
}

fn restore_native_headers(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        if let Some(headers) = object.remove("headers") {
            object.insert("http_headers".into(), headers);
        }
    }
    value
}

fn json_item(value: &Value) -> Result<toml_edit::Item, String> {
    let document = toml_edit::ser::to_document(&serde_json::json!({ "entry": value }))
        .map_err(|error| format!("MCP server cannot be represented in TOML: {error}"))?;
    document
        .get("entry")
        .cloned()
        .ok_or_else(|| "MCP server cannot be represented in TOML".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn harness() -> (std::path::PathBuf, CodexHarness) {
        let root = std::env::temp_dir().join(format!(
            "orbit-codex-mcp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let h = CodexHarness::new(root.join(".codex"), root.clone());
        (root, h)
    }

    #[test]
    fn native_server_crud_normalizes_ui_shape_and_preserves_other_settings() {
        let (root, h) = harness();
        fs::create_dir_all(h.config_root()).unwrap();
        fs::write(h.settings_path(), "# retained\nother = 42\n").unwrap();
        save(&h, "sample", serde_json::json!({"type":"http","url":"https://example.test/mcp","headers":{"Authorization":"Bearer private-token"}})).unwrap();
        let views = list(&h).unwrap();
        assert_eq!(views[0].kind, "http");
        assert_eq!(
            views[0].entry["headers"]["Authorization"],
            crate::harness::credentials::MASK
        );
        assert_eq!(
            store::read_value(&h.settings_path()).unwrap()[SERVERS]["sample"]["http_headers"]["Authorization"],
            "Bearer private-token"
        );
        assert!(
            !serde_json::to_string(&views)
                .unwrap()
                .contains("private-token")
        );
        store::mutate(
            &h,
            ResourceKind::McpServer,
            "sample",
            "test-rotate-credential",
            &h.settings_path(),
            |document| {
                document[SERVERS]["sample"]["http_headers"]["Authorization"] =
                    toml_edit::value("Bearer rotated-token");
                Ok(())
            },
        )
        .unwrap();
        let mut edited = views[0].entry.clone();
        edited["url"] = Value::String("https://example.test/updated".into());
        save(&h, "sample", edited).unwrap();
        assert_eq!(
            store::read_value(&h.settings_path()).unwrap()[SERVERS]["sample"]["http_headers"]["Authorization"],
            "Bearer rotated-token"
        );
        assert!(
            fs::read_to_string(h.settings_path())
                .unwrap()
                .contains("# retained")
        );
        set_enabled(&h, "sample", false).unwrap();
        assert!(!list(&h).unwrap()[0].enabled);
        delete(&h, "sample").unwrap();
        assert!(list(&h).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
