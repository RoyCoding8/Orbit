use serde_json::{Map, Value};

use super::KiroHarness;
use crate::harness::json_store as store;
use crate::harness::{
    claude::mcp::{self, BenchResult, HttpEntry, ServerView, StdioEntry},
    Harness, ResourceKind,
};

const SERVERS: &str = "mcpServers";

pub fn list(harness: &KiroHarness) -> Result<Vec<ServerView>, String> {
    list_at(&harness.mcp_path())
}

pub(crate) fn list_global(harness: &KiroHarness) -> Result<Vec<ServerView>, String> {
    list(harness)
}

fn list_at(path: &std::path::Path) -> Result<Vec<ServerView>, String> {
    let config = store::read_value(path)?;
    let servers = server_map(&config)?;
    let mut result = Vec::with_capacity(servers.len());
    for (name, raw) in servers {
        let kind = kind(raw)?;
        let enabled = raw.get("disabled").and_then(Value::as_bool) != Some(true);
        let mut entry = raw.clone();
        entry
            .as_object_mut()
            .expect("MCP server entry is an object")
            .insert("type".into(), Value::String(kind.into()));
        result.push(ServerView {
            name: name.clone(),
            kind: kind.into(),
            entry: mcp::mask_entry(&entry),
            enabled,
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

pub fn save(harness: &KiroHarness, name: &str, entry: Value) -> Result<(), String> {
    validate_name(name)?;
    let mut candidate = entry
        .as_object()
        .cloned()
        .ok_or("MCP server entry must be an object")?;
    let kind = candidate
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            if candidate.contains_key("command") {
                "stdio".into()
            } else {
                "http".into()
            }
        });
    if !matches!(kind.as_str(), "stdio" | "http") {
        return Err("Kiro MCP transport must be `stdio` or `http`".into());
    }
    let enabled = candidate
        .remove("enabled")
        .map(|value| value.as_bool().ok_or("`enabled` must be a boolean"))
        .transpose()?;
    let mut candidate = Value::Object(candidate);
    validate_entry(&candidate, &kind)?;
    let id = name.to_owned();
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        "kiro-mcp-save",
        &harness.mcp_path(),
        move |document| {
            let servers = servers_mut(document)?;
            let previous = servers.get(&id).cloned();
            if let Some(previous) = &previous {
                mcp::preserve_masked_credentials(previous, &mut candidate)
                    .map_err(|error| error.to_string())?;
            }
            if crate::harness::credentials::contains_mask(&candidate) {
                return Err("the edited entry still holds masked placeholders; re-enter the credential values".into());
            }
            let object = candidate
                .as_object_mut()
                .ok_or("MCP server entry must be an object")?;
            match enabled {
                Some(true) => {
                    object.remove("disabled");
                }
                Some(false) => {
                    object.insert("disabled".into(), Value::Bool(true));
                }
                None => {
                    if let Some(disabled) =
                        previous.and_then(|entry| entry.get("disabled").cloned())
                    {
                        object.insert("disabled".into(), disabled);
                    }
                }
            }
            validate_entry(&candidate, &kind)?;
            servers.insert(id.clone(), candidate);
            Ok(())
        },
    )
}

pub fn set_enabled(harness: &KiroHarness, name: &str, enabled: bool) -> Result<(), String> {
    validate_name(name)?;
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        "kiro-mcp-enabled",
        &harness.mcp_path(),
        |document| {
            let entry = servers_mut(document)?
                .get_mut(name)
                .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
            let object = entry
                .as_object_mut()
                .ok_or("MCP server entry must be an object")?;
            if enabled {
                object.remove("disabled");
            } else {
                object.insert("disabled".into(), Value::Bool(true));
            }
            Ok(())
        },
    )
}

pub fn delete(harness: &KiroHarness, name: &str) -> Result<(), String> {
    validate_name(name)?;
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        "kiro-mcp-delete",
        &harness.mcp_path(),
        |document| {
            if servers_mut(document)?.remove(name).is_none() {
                return Err(format!("MCP server `{name}` was not found"));
            }
            Ok(())
        },
    )
}

pub fn test(harness: &KiroHarness, name: &str) -> Result<BenchResult, String> {
    let entry = server_map(&store::read_value(&harness.mcp_path())?)?
        .get(name)
        .cloned()
        .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
    if entry.get("disabled").and_then(Value::as_bool) == Some(true) {
        return Err("MCP server is disabled".into());
    }
    if entry.get("oauth").is_some() {
        return Err("Kiro OAuth MCP servers must be tested in Kiro because Orbit does not perform the OAuth flow".into());
    }
    match kind(&entry)? {
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
    let servers = object
        .entry(SERVERS)
        .or_insert_with(|| Value::Object(Map::new()));
    servers
        .as_object_mut()
        .ok_or_else(|| "`mcpServers` must be a JSON object".into())
}

fn empty_map() -> &'static Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn kind(entry: &Value) -> Result<&'static str, String> {
    let object = entry
        .as_object()
        .ok_or("MCP server entry must be an object")?;
    match (object.contains_key("command"), object.contains_key("url")) {
        (true, false) => Ok("stdio"),
        (false, true) => Ok("http"),
        (true, true) => Err("MCP server cannot contain both `command` and `url`".into()),
        (false, false) => Err("MCP server must have a `command` or `url`".into()),
    }
}

fn validate_entry(entry: &Value, expected: &str) -> Result<(), String> {
    if kind(entry)? != expected {
        return Err("MCP transport does not match the entry".into());
    }
    match expected {
        "stdio" => {
            let config: StdioEntry = serde_json::from_value(entry.clone())
                .map_err(|error| format!("Invalid stdio MCP server: {error}"))?;
            if config.command.trim().is_empty() {
                return Err("stdio MCP command cannot be empty".into());
            }
        }
        "http" => {
            let config: HttpEntry = serde_json::from_value(entry.clone())
                .map_err(|error| format!("Invalid HTTP MCP server: {error}"))?;
            if config.url.trim().is_empty() {
                return Err("HTTP MCP URL cannot be empty".into());
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn server_crud_uses_kiro_mcp_shape_and_masks_credentials() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));

        save(
            &harness,
            "fixture",
            json!({
                "type":"stdio",
                "command":"fixture-server",
                "args":["--local"],
                "env":{"API_TOKEN":"secret-fixture"}
            }),
        )
        .unwrap();
        let listed = list(&harness).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, "stdio");
        assert_eq!(listed[0].entry["env"]["API_TOKEN"], "••••••••");

        set_enabled(&harness, "fixture", false).unwrap();
        let saved = store::read_value(&harness.mcp_path()).unwrap();
        assert_eq!(saved["mcpServers"]["fixture"]["disabled"], true);
        assert_eq!(
            saved["mcpServers"]["fixture"]["env"]["API_TOKEN"],
            "secret-fixture"
        );

        delete(&harness, "fixture").unwrap();
        assert!(list(&harness).unwrap().is_empty());
    }
}
