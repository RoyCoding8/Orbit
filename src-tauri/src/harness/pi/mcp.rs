use serde_json::Value;

use super::PiHarness;
use crate::harness::{
    claude::mcp::{BenchResult, ServerView},
    json_mcp::{self, EnabledField, JsonMcpDialect},
};

const PI_DIALECT: JsonMcpDialect = JsonMcpDialect {
    enabled_field: EnabledField::Enabled,
    keep_type_on_save: true,
    allow_streamable_http: true,
    allow_server_url_alias: false,
    strict_ascii_names: true,
    reject_bang_dynamic_ref: true,
    transport_error: "Pi MCP transport must be `stdio` or `http`",
    oauth_error: "Pi OAuth MCP servers must be tested in Pi because Orbit does not perform the OAuth flow",
    dynamic_ref_error: "MCP server uses Pi runtime environment or command references. Test it in Pi so Orbit does not expand or execute them.",
    cwd_error: "Test this server in Pi, or set an absolute cwd. Pi resolves relative paths from the session directory and expands home-relative paths.",
};

pub fn list(harness: &PiHarness) -> Result<Vec<ServerView>, String> {
    json_mcp::list_with(harness, &PI_DIALECT)
}

pub fn save(harness: &PiHarness, name: &str, entry: Value) -> Result<(), String> {
    json_mcp::save_with(harness, &PI_DIALECT, name, entry)
}

pub fn set_enabled(harness: &PiHarness, name: &str, enabled: bool) -> Result<(), String> {
    json_mcp::set_enabled_with(harness, &PI_DIALECT, name, enabled)
}

pub fn delete(harness: &PiHarness, name: &str) -> Result<(), String> {
    json_mcp::delete_with(harness, &PI_DIALECT, name)
}

pub fn test(harness: &PiHarness, name: &str) -> Result<BenchResult, String> {
    json_mcp::test_with(harness, &PI_DIALECT, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{json_store as store, Harness};
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

    #[test]
    fn editing_disabled_server_without_enabled_field_preserves_disabled_state() {
        let root = crate::test_support::tempdir().unwrap();
        let harness = PiHarness::new(root.path().join(".pi/agent"), root.path().join("home"));
        save(
            &harness,
            "svc",
            json!({"type":"stdio","command":"cmd","enabled":false}),
        )
        .unwrap();
        save(&harness, "svc", json!({"type":"stdio","command":"cmd2"})).unwrap();
        let view = list(&harness).unwrap();
        assert!(!view[0].enabled);
        assert_eq!(view[0].entry["command"], "cmd2");
    }
}
