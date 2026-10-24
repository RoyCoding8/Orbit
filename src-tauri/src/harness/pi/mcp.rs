use serde_json::{Map, Value};

use super::PiHarness;
use crate::harness::{
    claude::mcp::{self, BenchResult, HttpEntry, ServerView, StdioEntry},
    json_store as store, Harness, ResourceKind,
};

const SERVERS: &str = "mcpServers";

pub fn list(harness: &PiHarness) -> Result<Vec<ServerView>, String> {
    let config = store::read_value(&harness.mcp_path())?;
    let servers = server_map(&config)?;
    let mut result = Vec::with_capacity(servers.len());
    for (name, entry) in servers {
        let kind = server_kind(entry)?;
        result.push(ServerView {
            name: name.clone(),
            kind: kind.into(),
            entry: mcp::mask_entry(entry),
            enabled: entry
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

pub fn save(harness: &PiHarness, name: &str, mut entry: Value) -> Result<(), String> {
    validate_name(name)?;
    let object = entry
        .as_object_mut()
        .ok_or("MCP server entry must be an object")?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if object.contains_key("command") {
                "stdio".into()
            } else {
                "http".into()
            }
        });
    if !matches!(kind.as_str(), "stdio" | "http" | "streamable-http") {
        return Err("Pi MCP transport must be `stdio` or `http`".into());
    }
    let enabled = object.get("enabled").and_then(Value::as_bool);
    if object.contains_key("enabled") && enabled.is_none() {
        return Err("`enabled` must be a boolean".into());
    }
    validate_entry(&entry, &kind)?;
    let id = name.to_owned();
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        "pi-mcp-save",
        &harness.mcp_path(),
        move |document| {
            let servers = servers_mut(document)?;
            if servers.keys().any(|existing| {
                existing != &id && normalized_name(existing) == normalized_name(&id)
            }) {
                return Err("MCP server name collides with an existing name after dash and underscore normalization".into());
            }
            if let Some(previous) = servers.get(&id) {
                mcp::preserve_masked_credentials(previous, &mut entry)
                    .map_err(|error| error.to_string())?;
            }
            if crate::harness::credentials::contains_mask(&entry) {
                return Err("the edited entry still holds masked placeholders; re-enter the credential values".into());
            }
            if let Some(enabled) = enabled {
                entry["enabled"] = Value::Bool(enabled);
            }
            validate_entry(&entry, &kind)?;
            servers.insert(id.clone(), entry);
            Ok(())
        },
    )
}

pub fn set_enabled(harness: &PiHarness, name: &str, enabled: bool) -> Result<(), String> {
    validate_name(name)?;
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        "pi-mcp-enabled",
        &harness.mcp_path(),
        |document| {
            let entry = servers_mut(document)?
                .get_mut(name)
                .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
            entry
                .as_object_mut()
                .ok_or("MCP server entry must be an object")?
                .insert("enabled".into(), Value::Bool(enabled));
            Ok(())
        },
    )
}

pub fn delete(harness: &PiHarness, name: &str) -> Result<(), String> {
    validate_name(name)?;
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        "pi-mcp-delete",
        &harness.mcp_path(),
        |document| {
            if servers_mut(document)?.remove(name).is_none() {
                return Err(format!("MCP server `{name}` was not found"));
            }
            Ok(())
        },
    )
}

pub fn test(harness: &PiHarness, name: &str) -> Result<BenchResult, String> {
    let entry = server_map(&store::read_value(&harness.mcp_path())?)?
        .get(name)
        .cloned()
        .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
    if entry.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Err("MCP server is disabled".into());
    }
    if entry.get("oauth").is_some() {
        return Err("Pi OAuth MCP servers must be tested in Pi because Orbit does not perform the OAuth flow".into());
    }
    if has_dynamic_reference(&entry) {
        return Err("MCP server uses Pi runtime environment or command references. Test it in Pi so Orbit does not expand or execute them.".into());
    }
    if entry.get("cwd").is_some_and(|cwd| {
        cwd.as_str()
            .is_none_or(|cwd| !std::path::Path::new(cwd).is_absolute())
    }) {
        return Err("Test this server in Pi, or set an absolute cwd. Pi resolves relative paths from the session directory and expands home-relative paths.".into());
    }
    match server_kind(&entry)? {
        "stdio" => mcp::test_stdio(
            &serde_json::from_value::<StdioEntry>(entry)
                .map_err(|error| format!("Invalid stdio MCP server: {error}"))?,
        ),
        _ => mcp::test_http(
            &serde_json::from_value::<HttpEntry>(entry)
                .map_err(|error| format!("Invalid HTTP MCP server: {error}"))?,
        ),
    }
}

fn server_map(value: &Value) -> Result<&Map<String, Value>, String> {
    match value.get(SERVERS) {
        None => Ok(empty_map()),
        Some(Value::Object(servers)) => Ok(servers),
        Some(_) => Err("`mcpServers` must be a JSON object".into()),
    }
}

fn servers_mut(value: &mut Value) -> Result<&mut Map<String, Value>, String> {
    let object = value
        .as_object_mut()
        .ok_or("MCP configuration must contain a JSON object")?;
    object
        .entry(SERVERS)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "`mcpServers` must be a JSON object".into())
}

fn empty_map() -> &'static Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn server_kind(entry: &Value) -> Result<&'static str, String> {
    let object = entry
        .as_object()
        .ok_or("MCP server entry must be an object")?;
    let kind = object.get("type").and_then(Value::as_str);
    match kind {
        Some("stdio") => {
            if object.contains_key("command") {
                Ok("stdio")
            } else {
                Err("stdio MCP server must have a `command`".into())
            }
        }
        Some("http" | "streamable-http") => {
            if object.contains_key("url") {
                Ok("http")
            } else {
                Err("HTTP MCP server must have a `url`".into())
            }
        }
        Some("sse") => {
            Err("Pi SSE MCP servers are not supported by the current Orbit adapter".into())
        }
        Some(_) => Err("unsupported Pi MCP transport".into()),
        None => match (object.contains_key("command"), object.contains_key("url")) {
            (true, false) => Ok("stdio"),
            (false, true) => Ok("http"),
            (true, true) => Err("MCP server cannot contain both `command` and `url`".into()),
            (false, false) => Err("MCP server must have a `command` or `url`".into()),
        },
    }
}

fn validate_entry(entry: &Value, expected: &str) -> Result<(), String> {
    let kind = server_kind(entry)?;
    if kind != expected && !(expected == "streamable-http" && kind == "http") {
        return Err("MCP transport does not match the entry".into());
    }
    match kind {
        "stdio" => {
            let config: StdioEntry = serde_json::from_value(entry.clone())
                .map_err(|error| format!("Invalid stdio MCP server: {error}"))?;
            if config.command.trim().is_empty() {
                return Err("stdio MCP command cannot be empty".into());
            }
        }
        _ => {
            let config: HttpEntry = serde_json::from_value(entry.clone())
                .map_err(|error| format!("Invalid HTTP MCP server: {error}"))?;
            if config.url.trim().is_empty() {
                return Err("HTTP MCP URL cannot be empty".into());
            }
        }
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        Err(
            "Pi MCP server names may contain only ASCII letters, digits, underscores, and hyphens"
                .into(),
        )
    } else {
        Ok(())
    }
}

fn normalized_name(name: &str) -> String {
    name.replace('-', "_").to_ascii_lowercase()
}

fn has_dynamic_reference(value: &Value) -> bool {
    match value {
        Value::String(value) => value.contains("${") || value.trim_start().starts_with('!'),
        Value::Array(values) => values.iter().any(has_dynamic_reference),
        Value::Object(values) => values.values().any(has_dynamic_reference),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_mcp_crud_preserves_fields_masks_secrets_and_refuses_runtime_expansion() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = PiHarness::new(root.path().join(".pi/agent"), root.path().join("home"));
        save(
            &harness,
            "local-server",
            json!({
                "type":"stdio", "command":"!server", "args":["--safe"],
                "enabled":false, "env":{"API_TOKEN":"fixture-secret"}, "custom":{"kept":true}
            }),
        )
        .unwrap();

        let view = list(&harness).unwrap();
        assert_eq!(view[0].kind, "stdio");
        assert!(!view[0].enabled);
        assert_eq!(view[0].entry["env"]["API_TOKEN"], "••••••••");
        assert_eq!(view[0].entry["custom"]["kept"], true);
        assert!(save(&harness, "local_server", json!({"command":"other"}))
            .unwrap_err()
            .contains("collides"));

        set_enabled(&harness, "local-server", true).unwrap();
        let stored = store::read_value(&harness.mcp_path()).unwrap();
        assert_eq!(stored["mcpServers"]["local-server"]["enabled"], true);
        assert_eq!(
            stored["mcpServers"]["local-server"]["env"]["API_TOKEN"],
            "fixture-secret"
        );
        assert!(test(&harness, "local-server")
            .unwrap_err()
            .contains("runtime environment or command references"));
        delete(&harness, "local-server").unwrap();
        assert!(list(&harness).unwrap().is_empty());
        save(
            &harness,
            "relative",
            json!({"command":"does-not-exist", "cwd":"./tools"}),
        )
        .unwrap();
        assert!(test(&harness, "relative")
            .unwrap_err()
            .contains("set an absolute cwd"));
    }

    #[test]
    fn streamable_http_is_supported_but_sse_is_rejected() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = PiHarness::new(root.path().join(".pi/agent"), root.path().join("home"));
        save(&harness, "remote", json!({"type":"streamable-http","url":"https://example.test/mcp","headers":{"Accept":"application/json"}})).unwrap();
        assert_eq!(list(&harness).unwrap()[0].kind, "http");
        assert_eq!(
            store::read_value(&harness.mcp_path()).unwrap()["mcpServers"]["remote"]["type"],
            "streamable-http"
        );
        assert!(save(
            &harness,
            "events",
            json!({"type":"sse","url":"https://example.test/events"})
        )
        .unwrap_err()
        .contains("must be `stdio` or `http`"));
    }
}
