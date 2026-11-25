use serde_json::{Map, Value};

#[cfg(test)]
use super::kiro::KiroHarness;
use crate::harness::json_store as store;
use crate::harness::{
    claude::mcp::{self, BenchResult, HttpEntry, ServerView, StdioEntry},
    Harness, ResourceKind,
};

const SERVERS: &str = "mcpServers";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnabledField {
    /// Entry stores `"disabled": true` when disabled and omits it when enabled (Kiro, Antigravity).
    Disabled,
    /// Entry stores `"enabled": bool` (defaulting to `true` when absent) (Pi).
    Enabled,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct JsonMcpDialect {
    pub(crate) enabled_field: EnabledField,
    pub(crate) keep_type_on_save: bool,
    pub(crate) allow_streamable_http: bool,
    pub(crate) allow_server_url_alias: bool,
    pub(crate) strict_ascii_names: bool,
    pub(crate) reject_bang_dynamic_ref: bool,
    pub(crate) transport_error: &'static str,
    pub(crate) oauth_error: &'static str,
    pub(crate) dynamic_ref_error: &'static str,
    pub(crate) cwd_error: &'static str,
}

pub(crate) const STANDARD_DIALECT: JsonMcpDialect = JsonMcpDialect {
    enabled_field: EnabledField::Disabled,
    keep_type_on_save: false,
    allow_streamable_http: false,
    allow_server_url_alias: true,
    strict_ascii_names: false,
    reject_bang_dynamic_ref: false,
    transport_error: "MCP transport must be `stdio` or `http`",
    oauth_error: "OAuth and account-authenticated MCP servers must be tested in the native agent",
    dynamic_ref_error:
        "Test this server in its native agent so runtime references can be resolved.",
    cwd_error: "Test this server in its native agent, or set an absolute working directory.",
};

pub fn list(harness: &impl Harness) -> Result<Vec<ServerView>, String> {
    list_with(harness, &STANDARD_DIALECT)
}

pub(crate) fn list_global(harness: &impl Harness) -> Result<Vec<ServerView>, String> {
    list(harness)
}

pub fn save(harness: &impl Harness, name: &str, entry: Value) -> Result<(), String> {
    save_with(harness, &STANDARD_DIALECT, name, entry)
}

pub fn set_enabled(harness: &impl Harness, name: &str, enabled: bool) -> Result<(), String> {
    set_enabled_with(harness, &STANDARD_DIALECT, name, enabled)
}

pub fn delete(harness: &impl Harness, name: &str) -> Result<(), String> {
    delete_with(harness, &STANDARD_DIALECT, name)
}

pub fn test(harness: &impl Harness, name: &str) -> Result<BenchResult, String> {
    test_with(harness, &STANDARD_DIALECT, name)
}

pub(crate) fn list_with(
    harness: &impl Harness,
    dialect: &JsonMcpDialect,
) -> Result<Vec<ServerView>, String> {
    let config = store::read_value(&harness.mcp_path())?;
    let servers = server_map(&config)?;
    let mut result = Vec::with_capacity(servers.len());
    for (name, raw) in servers {
        let kind = server_kind(raw, dialect)?;
        let enabled = is_enabled(raw, dialect.enabled_field);
        let mut entry = raw.clone();
        if !dialect.keep_type_on_save {
            entry
                .as_object_mut()
                .expect("MCP server entry is an object")
                .insert("type".into(), Value::String(kind.into()));
        }
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

pub(crate) fn save_with(
    harness: &impl Harness,
    dialect: &JsonMcpDialect,
    name: &str,
    entry: Value,
) -> Result<(), String> {
    validate_name(name, dialect)?;
    let mut candidate = entry
        .as_object()
        .cloned()
        .ok_or("MCP server entry must be an object")?;
    let kind = if dialect.keep_type_on_save {
        candidate
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
    } else {
        candidate
            .remove("type")
            .and_then(|value| value.as_str().map(str::to_owned))
    }
    .unwrap_or_else(|| {
        if candidate.contains_key("command") {
            "stdio".into()
        } else {
            "http".into()
        }
    });
    let valid_kind = matches!(kind.as_str(), "stdio" | "http")
        || (dialect.allow_streamable_http && kind == "streamable-http");
    if !valid_kind {
        return Err(dialect.transport_error.into());
    }
    let enabled = match dialect.enabled_field {
        EnabledField::Disabled => candidate
            .remove("enabled")
            .map(|value| value.as_bool().ok_or("`enabled` must be a boolean"))
            .transpose()?,
        EnabledField::Enabled => {
            if let Some(value) = candidate.get("enabled") {
                Some(value.as_bool().ok_or("`enabled` must be a boolean")?)
            } else {
                None
            }
        }
    };
    let mut candidate = Value::Object(candidate);
    validate_entry(&candidate, &kind, dialect)?;
    let id = name.to_owned();
    let dialect = *dialect;
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        &format!("{}-mcp-save", harness.id().0),
        &harness.mcp_path(),
        move |document| {
            let servers = servers_mut(document)?;
            if dialect.strict_ascii_names
                && servers.keys().any(|existing| {
                    existing != &id && normalized_name(existing) == normalized_name(&id)
                })
            {
                return Err(
                    "MCP server name collides with an existing name after dash and underscore normalization"
                        .into(),
                );
            }
            let mut previous = servers.get(&id).cloned();
            if dialect.allow_server_url_alias {
                if let Some(previous) = previous.as_mut().and_then(Value::as_object_mut) {
                    if candidate.get("url").is_some() && previous.contains_key("serverUrl") {
                        let url = previous.remove("serverUrl").unwrap();
                        previous.insert("url".into(), url);
                    }
                }
            }
            if let Some(previous) = &previous {
                mcp::preserve_masked_credentials(previous, &mut candidate)
                    .map_err(|error| error.to_string())?;
            }
            if crate::harness::credentials::contains_mask(&candidate) {
                return Err(
                    "the edited entry still holds masked placeholders; re-enter the credential values"
                        .into(),
                );
            }
            let object = candidate
                .as_object_mut()
                .ok_or("MCP server entry must be an object")?;
            apply_enabled_on_save(object, previous.as_ref(), enabled, dialect.enabled_field);
            validate_entry(&candidate, &kind, &dialect)?;
            servers.insert(id.clone(), candidate);
            Ok(())
        },
    )
}

pub(crate) fn set_enabled_with(
    harness: &impl Harness,
    dialect: &JsonMcpDialect,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    validate_name(name, dialect)?;
    let field = dialect.enabled_field;
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        &format!("{}-mcp-enabled", harness.id().0),
        &harness.mcp_path(),
        |document| {
            let entry = servers_mut(document)?
                .get_mut(name)
                .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
            let object = entry
                .as_object_mut()
                .ok_or("MCP server entry must be an object")?;
            match field {
                EnabledField::Disabled => {
                    if enabled {
                        object.remove("disabled");
                    } else {
                        object.insert("disabled".into(), Value::Bool(true));
                    }
                }
                EnabledField::Enabled => {
                    object.insert("enabled".into(), Value::Bool(enabled));
                }
            }
            Ok(())
        },
    )
}

pub(crate) fn delete_with(
    harness: &impl Harness,
    dialect: &JsonMcpDialect,
    name: &str,
) -> Result<(), String> {
    validate_name(name, dialect)?;
    store::mutate_json(
        harness,
        ResourceKind::McpServer,
        name,
        &format!("{}-mcp-delete", harness.id().0),
        &harness.mcp_path(),
        |document| {
            if servers_mut(document)?.remove(name).is_none() {
                return Err(format!("MCP server `{name}` was not found"));
            }
            Ok(())
        },
    )
}

pub(crate) fn test_with(
    harness: &impl Harness,
    dialect: &JsonMcpDialect,
    name: &str,
) -> Result<BenchResult, String> {
    let entry = server_map(&store::read_value(&harness.mcp_path())?)?
        .get(name)
        .cloned()
        .ok_or_else(|| format!("MCP server `{name}` was not found"))?;
    if !is_enabled(&entry, dialect.enabled_field) {
        return Err("MCP server is disabled".into());
    }
    if entry.get("oauth").is_some() || entry.get("authProviderType").is_some() {
        return Err(dialect.oauth_error.into());
    }
    if has_dynamic_reference(&entry, dialect.reject_bang_dynamic_ref) {
        return Err(dialect.dynamic_ref_error.into());
    }
    if entry.get("cwd").is_some_and(|v| {
        v.as_str()
            .is_none_or(|s| !std::path::Path::new(s).is_absolute())
    }) {
        return Err(dialect.cwd_error.into());
    }
    match server_kind(&entry, dialect)? {
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

fn is_enabled(entry: &Value, field: EnabledField) -> bool {
    match field {
        EnabledField::Disabled => entry.get("disabled").and_then(Value::as_bool) != Some(true),
        EnabledField::Enabled => entry
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    }
}

fn apply_enabled_on_save(
    object: &mut Map<String, Value>,
    previous: Option<&Value>,
    enabled: Option<bool>,
    field: EnabledField,
) {
    match field {
        EnabledField::Disabled => match enabled {
            Some(true) => {
                object.remove("disabled");
            }
            Some(false) => {
                object.insert("disabled".into(), Value::Bool(true));
            }
            None => {
                if let Some(disabled) = previous.and_then(|entry| entry.get("disabled").cloned()) {
                    object.insert("disabled".into(), disabled);
                }
            }
        },
        EnabledField::Enabled => match enabled {
            Some(value) => {
                object.insert("enabled".into(), Value::Bool(value));
            }
            None => {
                if let Some(prev_enabled) = previous.and_then(|entry| entry.get("enabled").cloned())
                {
                    object.insert("enabled".into(), prev_enabled);
                }
            }
        },
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

fn server_kind(entry: &Value, dialect: &JsonMcpDialect) -> Result<&'static str, String> {
    let object = entry
        .as_object()
        .ok_or("MCP server entry must be an object")?;
    if dialect.allow_server_url_alias
        && object.contains_key("url")
        && object.contains_key("serverUrl")
    {
        return Err("MCP server must use only one of `url` or `serverUrl`".into());
    }
    if dialect.keep_type_on_save {
        if let Some(kind) = object.get("type").and_then(Value::as_str) {
            return match kind {
                "stdio" => {
                    if object.contains_key("command") {
                        Ok("stdio")
                    } else {
                        Err("stdio MCP server must have a `command`".into())
                    }
                }
                "http" => {
                    if object.contains_key("url") {
                        Ok("http")
                    } else {
                        Err("HTTP MCP server must have a `url`".into())
                    }
                }
                "streamable-http" if dialect.allow_streamable_http => {
                    if object.contains_key("url") {
                        Ok("http")
                    } else {
                        Err("HTTP MCP server must have a `url`".into())
                    }
                }
                "sse" => {
                    Err("Pi SSE MCP servers are not supported by the current Orbit adapter".into())
                }
                _ => Err("unsupported Pi MCP transport".into()),
            };
        }
    }
    let has_url = object.contains_key("url")
        || (dialect.allow_server_url_alias && object.contains_key("serverUrl"));
    match (object.contains_key("command"), has_url) {
        (true, false) => Ok("stdio"),
        (false, true) => Ok("http"),
        (true, true) => Err("MCP server cannot contain both `command` and `url`".into()),
        (false, false) => Err("MCP server must have a `command` or `url`".into()),
    }
}

fn validate_entry(entry: &Value, expected: &str, dialect: &JsonMcpDialect) -> Result<(), String> {
    let kind = server_kind(entry, dialect)?;
    if kind != expected
        && !(dialect.allow_streamable_http && expected == "streamable-http" && kind == "http")
    {
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

fn validate_name(name: &str, dialect: &JsonMcpDialect) -> Result<(), String> {
    if dialect.strict_ascii_names {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(
                "Pi MCP server names may contain only ASCII letters, digits, underscores, and hyphens"
                    .into(),
            );
        }
    } else if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err("MCP server name must be non-empty and contain no control characters".into());
    }
    Ok(())
}

fn normalized_name(name: &str) -> String {
    name.replace('-', "_").to_ascii_lowercase()
}

fn has_dynamic_reference(value: &Value, reject_bang: bool) -> bool {
    match value {
        Value::String(text) => {
            text.contains("${") || (reject_bang && text.trim_start().starts_with('!'))
        }
        Value::Array(values) => values
            .iter()
            .any(|item| has_dynamic_reference(item, reject_bang)),
        Value::Object(values) => values
            .values()
            .any(|item| has_dynamic_reference(item, reject_bang)),
        _ => false,
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
