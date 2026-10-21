use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::credentials::{self, contains_mask};
use super::process_lifetime::{self, CaptureEnd, ChildTree};
use super::ClaudeCodeHarness;
use crate::harness::{atomic, Harness, HarnessError, ResourceKind, Result};

pub const MASK: &str = credentials::MASK;
const MCP_SERVERS_KEY: &str = "mcpServers";
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum bytes accepted from one MCP response stream before reporting a
/// bounded-buffer diagnosis.
const BENCH_BYTE_BUDGET: usize = 512 * 1024;
/// How much captured stderr an exit diagnostic includes.
const STDERR_TAIL: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StdioEntry {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpEntry {
    #[serde(alias = "serverUrl")]
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpEntry {
    Stdio(StdioEntry),
    Http(HttpEntry),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerView {
    pub name: String,
    pub kind: String,
    pub entry: Value,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchResult {
    pub tools: Vec<String>,
    pub latency_ms: u128,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DisabledServers {
    #[serde(default)]
    servers: BTreeMap<String, String>,
}

/// The two authoritative stores for one server identity. The disabled copy
/// lives only in the sidecar; the enabled entry lives only in `.claude.json`.
struct ServerState {
    active: BTreeMap<String, Value>,
    disabled: DisabledServers,
    sidecar: PathBuf,
    sidecar_dirty: bool,
    /// Commit-time equality requirement for the active entry. Set when this
    /// operation derived values from the stored entry (mask restoration), so
    /// an entry rotated by another writer between read and commit is refused
    /// instead of overwritten with the derived state.
    guard: Option<Value>,
}

impl ServerState {
    fn read(harness: &ClaudeCodeHarness, app_data: &Path) -> Result<Self> {
        let doc = atomic::read_json(&harness.claude_json_path())?;
        Ok(Self {
            active: doc
                .get(MCP_SERVERS_KEY)
                .and_then(Value::as_object)
                .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
            disabled: read_disabled(&app_data.join("mcp-disabled.json"))?,
            sidecar: app_data.join("mcp-disabled.json"),
            sidecar_dirty: false,
            guard: None,
        })
    }

    /// One entry per name. An enabled native entry is authoritative over any
    /// same-name stale disabled copy, so it replaces the shadow instead of
    /// joining it.
    fn views(&self) -> Vec<ServerView> {
        let mut by_name: BTreeMap<String, ServerView> = BTreeMap::new();
        for (name, raw) in &self.disabled.servers {
            if let Ok(entry) = serde_json::from_str::<Value>(raw) {
                by_name.insert(
                    name.clone(),
                    ServerView {
                        name: name.clone(),
                        kind: entry_kind(&entry).to_string(),
                        entry: mask_entry(&entry),
                        enabled: false,
                    },
                );
            }
        }
        for (name, entry) in &self.active {
            by_name.insert(
                name.clone(),
                ServerView {
                    name: name.clone(),
                    kind: entry_kind(entry).to_string(),
                    entry: mask_entry(entry),
                    enabled: true,
                },
            );
        }
        by_name.into_values().collect()
    }

    /// Serialize both stores. Config first (it is the state most writers
    /// rewrite), then the sidecar when this operation touched it. A failure
    /// from the second write reports what already landed; it does not claim
    /// the whole operation rolled back.
    fn commit(self, harness: &ClaudeCodeHarness, name: &str, action: &str) -> Result<()> {
        let slot = atomic::p![MCP_SERVERS_KEY, name];
        let patch = match (&self.guard, self.active.get(name)) {
            (Some(stored), Some(entry)) => atomic::Patch::ReplacePath {
                path: slot,
                matches: stored.clone(),
                value: entry.clone(),
            },
            (None, Some(entry)) => atomic::Patch::SetPath {
                path: slot,
                value: entry.clone(),
            },
            (_, None) => atomic::Patch::RemovePath { path: slot },
        };
        let path = harness.claude_json_path();
        let backup = crate::ledger::backup_path(harness.config_root(), name, action);
        crate::ledger::write(
            harness.config_root(),
            ResourceKind::McpServer,
            name,
            action,
            &path,
            atomic::RecordedWrite::Patches(vec![patch]),
            Some(&backup),
        )
        .map_err(|error| HarnessError::Write {
            path: path.clone(),
            source: std::io::Error::other(error.to_string()),
        })?;
        if self.sidecar_dirty {
            write_disabled(&self.sidecar, &self.disabled)
                .map_err(|error| partial(action, &path, error))?;
        }
        Ok(())
    }
}

/// A failed second-leg write after the config already landed. The message
/// names both facts so the caller stops instead of assuming a rollback.
fn partial(action: &str, landed: &Path, error: HarnessError) -> HarnessError {
    HarnessError::Mutation {
        path: landed.to_path_buf(),
        message: format!(
            "{action} landed in {}, but the sidecar update failed: {error}; \
             rerun the action to reconcile",
            landed.display()
        ),
    }
}

impl ClaudeCodeHarness {
    pub fn mcp_servers(&self, app_data: &Path) -> Result<Vec<ServerView>> {
        Ok(ServerState::read(self, app_data)?.views())
    }

    /// Saving a name retires its disabled copy in the same operation, so an
    /// enable afterwards replays the saved entry, never stale sidecar bytes.
    pub fn mcp_save(&self, name: &str, mut entry: Value, app_data: &Path) -> Result<()> {
        validate_name(name)?;
        let mut state = ServerState::read(self, app_data)?;
        let had_mask = contains_mask(&entry);
        let stored = state.active.get(name).cloned();
        if let Some(stored) = &stored {
            preserve_masked_credentials(stored, &mut entry)?;
        }
        if contains_mask(&entry) {
            return Err(HarnessError::Write {
                path: self.claude_json_path(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the edited entry still holds masked placeholders; \
                     re-enter the credential values",
                ),
            });
        }
        serde_json::from_value::<McpEntry>(entry.clone()).map_err(|source| {
            HarnessError::Parse {
                path: self.claude_json_path(),
                source,
            }
        })?;
        if had_mask {
            if let Some(active) = state.active.get(name) {
                state.guard = Some(active.clone());
            }
        }
        state.active.insert(name.to_string(), entry);
        if state.disabled.servers.remove(name).is_some() {
            state.sidecar_dirty = true;
        }
        state.commit(self, name, "save")
    }

    pub fn mcp_delete(&self, name: &str, app_data: &Path) -> Result<()> {
        let config_path = self.claude_json_path();
        let mut state = ServerState::read(self, app_data)?;
        let present = state.active.contains_key(name);
        let disabled = state.disabled.servers.contains_key(name);
        if !present && !disabled {
            return Err(missing_server(&config_path));
        }
        state.active.remove(name);
        if disabled {
            state.disabled.servers.remove(name);
            state.sidecar_dirty = true;
        }
        if present {
            state.commit(self, name, "delete")
        } else {
            write_disabled(&state.sidecar, &state.disabled)
        }
    }

    pub fn mcp_disable(&self, name: &str, app_data: &Path) -> Result<()> {
        let config_path = self.claude_json_path();
        let mut state = ServerState::read(self, app_data)?;
        let entry = state
            .active
            .remove(name)
            .ok_or_else(|| missing_server(&config_path))?;
        let raw = serde_json::to_string(&entry).map_err(|source| HarnessError::WriteJson {
            path: config_path.clone(),
            source,
        })?;
        state.disabled.servers.insert(name.to_string(), raw);
        write_disabled(&state.sidecar, &state.disabled)?;
        state.commit(self, name, "disable")
    }

    /// Enable replays the disabled copy only when the native name is absent
    /// or still holds exactly the entry that was disabled. A newer native
    /// save under the same name is never overwritten; the caller must delete
    /// the active entry or the disabled copy first.
    pub fn mcp_enable(&self, name: &str, app_data: &Path) -> Result<()> {
        let sidecar = app_data.join("mcp-disabled.json");
        let mut state = ServerState::read(self, app_data)?;
        let raw = state
            .disabled
            .servers
            .get(name)
            .ok_or_else(|| missing_server(&sidecar))?
            .clone();
        let disabled: Value = serde_json::from_str(&raw).map_err(|source| HarnessError::Parse {
            path: sidecar.clone(),
            source,
        })?;
        match state.active.get(name) {
            None => {
                state.active.insert(name.to_string(), disabled);
            }
            Some(active) if *active == disabled => {}
            Some(_) => {
                return Err(HarnessError::Write {
                    path: self.claude_json_path(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!(
                            "an enabled entry named {name} already exists and is newer than \
                             the disabled copy; delete one of them first"
                        ),
                    ),
                });
            }
        }
        state.disabled.servers.remove(name);
        state.sidecar_dirty = true;
        state.commit(self, name, "enable")
    }

    pub fn mcp_test(&self, name: &str) -> std::result::Result<BenchResult, String> {
        let doc = atomic::read_json(&self.claude_json_path()).map_err(|error| error.to_string())?;
        let entry = doc
            .get(MCP_SERVERS_KEY)
            .and_then(Value::as_object)
            .and_then(|map| map.get(name))
            .cloned()
            .ok_or_else(|| "MCP server is not enabled or no longer exists".to_string())?;
        match serde_json::from_value::<McpEntry>(entry)
            .map_err(|error| format!("Invalid MCP server entry for {name}: {error}"))?
        {
            McpEntry::Stdio(config) => test_stdio(&config),
            McpEntry::Http(config) => test_http(&config),
        }
    }
}

fn read_disabled(path: &Path) -> Result<DisabledServers> {
    atomic::read_struct(path)
}

fn write_disabled(path: &Path, state: &DisabledServers) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| HarnessError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    atomic::commit(
        path,
        vec![atomic::Patch::SetPath {
            path: atomic::p!["servers"],
            value: serde_json::to_value(&state.servers).map_err(|source| {
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

fn missing_server(path: &Path) -> HarnessError {
    HarnessError::Read {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "MCP server entry not found"),
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(HarnessError::Write {
            path: PathBuf::from("mcpServers"),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "server name must be non-empty and contain no control characters",
            ),
        });
    }
    Ok(())
}

fn entry_kind(entry: &Value) -> &'static str {
    if entry.get("command").is_some() {
        "stdio"
    } else {
        "http"
    }
}

/// One traversal decides both what the UI sees and, through
/// [`preserve_masked_credentials`], what gets saved. A string is masked when
/// its key looks secret, when it sits in `env`/`headers` and its value looks
/// like a credential, when it is an `args` element carrying an
/// `Authorization:`-style credential pair, or when it is a URL with
/// `user:pass@` userinfo or a secret-bearing query parameter.
fn mask_visit(name: &str, value: &Value, in_secret_container: bool) -> Value {
    match value {
        Value::String(text) => {
            if credentials::secret_name(name)
                || (in_secret_container && credentials::value_shape_secret(text))
                || carries_credential(text)
                || embeds_url_credential(text)
            {
                Value::String(MASK.to_string())
            } else {
                value.clone()
            }
        }
        Value::Array(items) => {
            let is_args = name == "args";
            let mut masked = Vec::with_capacity(items.len());
            let mut value_pending = false;
            for item in items {
                if value_pending && item.is_string() {
                    value_pending = false;
                    masked.push(Value::String(MASK.to_string()));
                    continue;
                }
                if is_args {
                    value_pending = item.as_str().is_some_and(credential_flag);
                }
                masked.push(mask_visit(name, item, in_secret_container));
            }
            Value::Array(masked)
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(field, child)| {
                    let container =
                        in_secret_container || matches!(field.as_str(), "env" | "headers");
                    (field.clone(), mask_visit(field, child, container))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `--header "Authorization: Bearer x"` style argument pairs, and a bare
/// header value inside args such as `-H Bearer x`.
fn carries_credential(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    for marker in ["authorization:", "proxy-authorization:", "-h "] {
        if let Some(start) = lowered.find(marker) {
            let rest = text[start + marker.len()..].trim();
            if !rest.is_empty() {
                return true;
            }
        }
    }
    false
}

/// `scheme://user:pass@host` userinfo and secret-named query parameters.
fn embeds_url_credential(text: &str) -> bool {
    let Ok(url) = text.parse::<reqwest::Url>() else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return true;
    }
    url.query_pairs()
        .any(|(name, _)| credentials::secret_name(name.as_ref()))
}

/// An argument naming its successor as a credential value, such as
/// `--api-key` or `--token`. The flag itself is not secret; the element
/// after it is.
fn credential_flag(text: &str) -> bool {
    let flag = text
        .strip_prefix("--")
        .or_else(|| text.strip_prefix('-'))
        .unwrap_or(text);
    flag.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && credentials::secret_name(flag)
}

fn mask_entry(entry: &Value) -> Value {
    mask_visit("", entry, false)
}

/// Swap masked placeholders back to the stored values at the same object
/// path. The stored document comes from a fresh disk read at the mutation
/// boundary. The walk mirrors [`mask_visit`] exactly, so every place the UI
/// can show a mask is a place this restore can fill.
///
/// Arrays are refused when they hold a mask: elements carry no name identity,
/// so a positional match could rebind one stored credential to a different
/// element after a reorder. The caller surfaces the refusal instead of saving.
fn preserve_masked_credentials(stored: &Value, edited: &mut Value) -> Result<()> {
    credentials::restore_masked(stored, edited).map_err(|_| HarnessError::Write {
        path: PathBuf::from("mcpServers"),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "masked values inside args cannot be restored positionally; \
             re-enter the credential",
        ),
    })
}

fn test_stdio(config: &StdioEntry) -> std::result::Result<BenchResult, String> {
    let started = Instant::now();
    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .envs(&config.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildTree::spawn(&mut command)
        .map_err(|error| format!("Could not start MCP server: {error}"))?;
    let stderr_log = child
        .child_mut()
        .stderr
        .take()
        .map(|stderr| process_lifetime::spawn_capture(stderr, STDERR_TAIL, CaptureEnd::End));
    let result = stdio_handshake(child.child_mut());
    let exit = child.cleanup();
    let diagnostic = stderr_log.and_then(|log| log.recv_timeout(STDERR_DRAIN_WAIT).ok());
    let tools = result.map_err(|error| finish_error(error, exit, diagnostic.as_deref()))?;
    Ok(BenchResult {
        tools,
        latency_ms: started.elapsed().as_millis(),
    })
}

/// Bound the wait for the concurrent stderr reader. Past it the bench reports
/// without the log rather than hanging on an inherited handle.
const STDERR_DRAIN_WAIT: Duration = Duration::from_secs(2);

fn finish_error(
    error: String,
    exit: Option<std::process::ExitStatus>,
    stderr: Option<&str>,
) -> String {
    match (exit, stderr) {
        (Some(status), Some(stderr)) if !stderr.is_empty() => {
            format!("{error}; server exited with {status}; stderr: {stderr}")
        }
        (Some(status), _) => format!("{error}; server exited with {status}"),
        (_, Some(stderr)) if !stderr.is_empty() => format!("{error}; stderr: {stderr}"),
        _ => error,
    }
}

/// Correlates replies by JSON-RPC id, skipping log lines and notifications.
struct RpcReader {
    receiver: mpsc::Receiver<read::Line>,
}

mod read {
    pub enum Line {
        Text(String),
        Failed(std::io::Error),
        Ended,
    }
}

impl RpcReader {
    fn spawn(stdout: ChildStdout) -> Self {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout.take((BENCH_BYTE_BUDGET + 1) as u64));
            let mut budget = BENCH_BYTE_BUDGET;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = sender.send(read::Line::Ended);
                        break;
                    }
                    Ok(_) => {
                        if line.len() > budget {
                            let _ = sender.send(read::Line::Failed(std::io::Error::other(
                                "output exceeded the bench buffering budget",
                            )));
                            break;
                        }
                        budget -= line.len();
                        if sender.send(read::Line::Text(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(read::Line::Failed(error));
                        break;
                    }
                }
            }
        });
        Self { receiver }
    }

    /// One JSON-RPC response with the expected id. Skips non-JSON lines,
    /// notifications (no `id`), and replies to other ids. A channel closure
    /// before the id arrives surfaces the reason: EOF, reader failure, or
    /// the timeout.
    fn next_response(&self, request: &str, id: u64) -> std::result::Result<Value, String> {
        let deadline = Instant::now() + RPC_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "MCP server did not respond to {request} within {} seconds",
                    RPC_TIMEOUT.as_secs()
                ));
            }
            match self.receiver.recv_timeout(remaining) {
                Ok(read::Line::Text(line)) => {
                    let Ok(value) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    match value.get("id") {
                        Some(found) if found == &json!(id) => return Ok(value),
                        Some(_) => continue,
                        None => continue,
                    }
                }
                Ok(read::Line::Failed(error)) => {
                    return Err(format!("Could not read MCP response: {error}"))
                }
                Ok(read::Line::Ended) => {
                    return Err(format!(
                        "MCP server closed stdout before answering {request}"
                    ))
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "MCP server did not respond to {request} within {} seconds",
                        RPC_TIMEOUT.as_secs()
                    ))
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("MCP server output reader stopped unexpectedly".into())
                }
            }
        }
    }
}

fn stdio_handshake(child: &mut Child) -> std::result::Result<Vec<String>, String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "MCP server stdout unavailable".to_string())?;
    let reader = RpcReader::spawn(stdout);
    let input = child
        .stdin
        .as_mut()
        .ok_or_else(|| "MCP server stdin unavailable".to_string())?;
    write_rpc(
        input,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"harness-agent","version":"0.1.0"}}}),
    )?;
    let response = reader.next_response("initialize", 1)?;
    if response.get("error").is_some() {
        return Err("MCP server rejected initialize".into());
    }
    write_rpc(
        input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )?;
    write_rpc(
        input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    )?;
    let response = reader.next_response("tools/list", 2)?;
    if let Some(error) = response.get("error") {
        return Err(format!("MCP server rejected tools/list: {error}"));
    }
    Ok(response
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect())
}

fn write_rpc(input: &mut impl Write, value: Value) -> std::result::Result<(), String> {
    serde_json::to_writer(&mut *input, &value)
        .map_err(|_| "Could not encode MCP request".to_string())?;
    input
        .write_all(b"\n")
        .and_then(|_| input.flush())
        .map_err(|_| "Could not send MCP request".to_string())
}

fn test_http(config: &HttpEntry) -> std::result::Result<BenchResult, String> {
    if !config.url.starts_with("https://") && !is_loopback_http(&config.url) {
        return Err("Remote MCP servers require HTTPS; use https:// or a loopback address".into());
    }
    let started = Instant::now();
    let client = reqwest::blocking::Client::builder()
        .timeout(RPC_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("Could not create HTTP client: {error}"))?;
    let mut request = client
        .post(&config.url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    for (key, value) in &config.headers {
        let key = reqwest::header::HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| format!("Invalid MCP header name: {key}"))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| format!("Invalid MCP header value for {key}"))?;
        request = request.header(key, value);
    }
    let initialized = request.json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"harness-agent","version":"0.1.0"}}})).send().map_err(|error| safe_http_error(&error))?;
    if initialized.status().is_redirection() {
        return Err(format!(
            "MCP server redirected (HTTP {}); redirects are not followed, point the entry at the final URL",
            initialized.status().as_u16()
        ));
    }
    if !initialized.status().is_success() {
        return Err(format!(
            "MCP initialize returned HTTP {}",
            initialized.status().as_u16()
        ));
    }
    let session = initialized
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let endpoint = config.url.clone();
    let initialization = decode_http_response(initialized, &json!(1))?;
    if initialization.get("error").is_some() {
        return Err("MCP server rejected initialize".into());
    }
    let mut notify = client
        .post(&endpoint)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    let mut tools = client
        .post(&endpoint)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    for (key, value) in &config.headers {
        notify = notify.header(key, value);
        tools = tools.header(key, value);
    }
    if let Some(id) = session {
        notify = notify.header("mcp-session-id", &id);
        tools = tools.header("mcp-session-id", id);
    }
    let _ = notify
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .map_err(|error| safe_http_error(&error))?;
    let response = tools
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .send()
        .map_err(|error| safe_http_error(&error))?;
    if !response.status().is_success() {
        return Err(format!(
            "MCP tools/list returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let payload = decode_http_response(response, &json!(2))?;
    if let Some(error) = payload.get("error") {
        return Err(format!("MCP server rejected tools/list: {error}"));
    }
    let tools = payload
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    Ok(BenchResult {
        tools,
        latency_ms: started.elapsed().as_millis(),
    })
}

/// A plain-HTTP URL is legitimate only for this machine's own addresses.
fn is_loopback_http(url: &str) -> bool {
    let Ok(parsed) = url.parse::<reqwest::Url>() else {
        return false;
    };
    if parsed.scheme() != "http" {
        return false;
    }
    match parsed.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

fn decode_http_response(
    response: reqwest::blocking::Response,
    expected_id: &Value,
) -> std::result::Result<Value, String> {
    let is_event_stream = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let mut body = BufReader::new(response.take((BENCH_BYTE_BUDGET + 1) as u64));
    if !is_event_stream {
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes)
            .map_err(|_| "Could not read MCP server response".to_string())?;
        if bytes.len() > BENCH_BYTE_BUDGET {
            return Err("MCP server response exceeded buffering budget".into());
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "MCP server returned invalid JSON response".to_string())?;
        return response_with_id(value, expected_id)
            .ok_or_else(|| "MCP server returned no response matching request id".into());
    }

    let mut consumed = 0usize;
    let mut line = String::new();
    let mut data = String::new();
    loop {
        line.clear();
        let bytes = body
            .read_line(&mut line)
            .map_err(|_| "Could not read MCP server response".to_string())?;
        if bytes == 0 {
            if let Some(value) = event_response(&data, expected_id) {
                return Ok(value);
            }
            break;
        }
        consumed += bytes;
        if consumed > BENCH_BYTE_BUDGET {
            return Err("MCP server response exceeded buffering budget".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if let Some(value) = event_response(&data, expected_id) {
                return Ok(value);
            }
            data.clear();
        } else if !line.starts_with(':') {
            if let Some(value) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
    }
    Err("MCP server returned no response matching request id".into())
}

fn event_response(data: &str, expected_id: &Value) -> Option<Value> {
    let value = serde_json::from_str(data).ok()?;
    response_with_id(value, expected_id)
}

fn response_with_id(value: Value, expected_id: &Value) -> Option<Value> {
    if value.get("id") == Some(expected_id) {
        Some(value)
    } else {
        None
    }
}

fn safe_http_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "MCP server request timed out".into()
    } else if error.is_connect() {
        "Could not connect to MCP server".into()
    } else {
        "MCP server request failed".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::mpsc,
    };

    fn fixture() -> (tempfile::TempDir, ClaudeCodeHarness) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("claude");
        fs::create_dir_all(&root).unwrap();
        let harness = ClaudeCodeHarness::new(crate::harness::HarnessPaths {
            config_root: root,
            claude_json: dir.path().join(".claude.json"),
        });
        (dir, harness)
    }

    fn config_file(harness: &ClaudeCodeHarness, value: Value) {
        fs::write(
            harness.claude_json_path(),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }

    fn local_http_response(
        body: Vec<u8>,
        content_type: &str,
    ) -> (reqwest::blocking::Response, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let content_type = content_type.to_string();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
            .post(format!("http://{address}/mcp"))
            .body("{}")
            .send()
            .unwrap();
        (response, server)
    }

    fn open_sse_response(
        body: String,
    ) -> (
        reqwest::blocking::Response,
        mpsc::Sender<()>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, wait_for_release) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
                )
                .unwrap();
            write!(stream, "{:X}\r\n", body.len()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
            stream.write_all(b"\r\n").unwrap();
            stream.flush().unwrap();
            let _ = wait_for_release.recv_timeout(Duration::from_secs(5));
            let _ = stream.write_all(b"0\r\n\r\n");
        });
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .post(format!("http://{address}/mcp"))
            .body("{}")
            .send()
            .unwrap();
        (response, release, server)
    }

    #[test]
    fn stdio_shape_round_trips_unknown_fields() {
        let value =
            json!({"command":"node","args":["server.js"],"env":{"MODE":"safe"},"cwd":"/fixture"});
        let entry: StdioEntry = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(entry).unwrap(), value);
    }

    #[test]
    fn http_shape_round_trips_server_url_alias_and_unknown_fields() {
        let value = json!({"serverUrl":"https://example.test/mcp","headers":{"x-api-key":"not-real"},"timeout":5});
        let entry: HttpEntry = serde_json::from_value(value.clone()).unwrap();
        let mut roundtrip = serde_json::to_value(entry).unwrap();
        let url = roundtrip["url"].clone();
        roundtrip
            .as_object_mut()
            .unwrap()
            .insert("serverUrl".into(), url);
        roundtrip.as_object_mut().unwrap().remove("url");
        assert_eq!(roundtrip, value);
    }

    #[test]
    fn disabling_and_enabling_restores_entry_bytes_and_leaves_other_keys() {
        let (dir, harness) = fixture();
        let app_data = dir.path().join("app-data");
        let entry =
            json!({"command":"node","args":["server.js"],"env":{"MODE":"safe"},"custom":{"x":1}});
        config_file(
            &harness,
            json!({"mcpServers":{"alpha":entry.clone(),"beta":{"command":"other"}},"theme":"dark"}),
        );
        let expected = serde_json::to_string(&entry).unwrap();
        harness.mcp_disable("alpha", &app_data).unwrap();
        let sidecar: Value =
            serde_json::from_slice(&fs::read(app_data.join("mcp-disabled.json")).unwrap()).unwrap();
        assert_eq!(sidecar["servers"]["alpha"], expected);
        assert!(sidecar["servers"]["alpha"]
            .as_str()
            .unwrap()
            .as_bytes()
            .eq(expected.as_bytes()));
        assert!(harness
            .mcp_servers(&app_data)
            .unwrap()
            .iter()
            .find(|server| server.name == "alpha")
            .is_some_and(|server| !server.enabled));
        harness.mcp_enable("alpha", &app_data).unwrap();
        let restored = atomic::read_json(&harness.claude_json_path()).unwrap();
        assert_eq!(restored["mcpServers"]["alpha"], entry);
        assert_eq!(restored["theme"], "dark");
        let sidecar: Value =
            serde_json::from_slice(&fs::read(app_data.join("mcp-disabled.json")).unwrap()).unwrap();
        assert!(sidecar["servers"].get("alpha").is_none());
    }

    #[test]
    fn failed_disable_reservation_preserves_the_active_entry_and_can_be_retried() {
        let (dir, harness) = fixture();
        let app_data = dir.path().join("app-data");
        let entry = json!({"command":"fixture-server","env":{"SERVICE_TOKEN":"fixture-secret"}});
        config_file(&harness, json!({"mcpServers":{"alpha":entry.clone()}}));
        let ledger = harness.config_root().join(".harness-agent/ledger.json");
        fs::create_dir_all(&ledger).unwrap();

        let error = harness.mcp_disable("alpha", &app_data).unwrap_err();
        assert!(error.to_string().contains("reservation failed"));
        assert_eq!(
            atomic::read_json(&harness.claude_json_path()).unwrap()["mcpServers"]["alpha"],
            entry
        );
        let sidecar = atomic::read_json(&app_data.join("mcp-disabled.json")).unwrap();
        assert_eq!(
            sidecar["servers"]["alpha"],
            serde_json::to_string(&entry).unwrap()
        );

        fs::remove_dir(&ledger).unwrap();
        harness.mcp_disable("alpha", &app_data).unwrap();
        assert!(
            atomic::read_json(&harness.claude_json_path()).unwrap()["mcpServers"]
                .get("alpha")
                .is_none()
        );
        harness.mcp_enable("alpha", &app_data).unwrap();
        assert_eq!(
            atomic::read_json(&harness.claude_json_path()).unwrap()["mcpServers"]["alpha"],
            entry
        );
    }

    #[test]
    #[cfg(windows)]
    fn stdout_without_a_newline_is_capped_before_buffering_the_whole_line() {
        let mut child = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::Out.Write('x' * 524290)",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let reader = RpcReader::spawn(child.stdout.take().unwrap());
        let result = reader.next_response("initialize", 1);
        let _ = child.kill();
        let _ = child.wait();
        assert!(result.unwrap_err().contains("buffering budget"));
    }

    #[test]
    fn deleting_a_disabled_server_removes_its_sidecar_entry_without_rewriting_native_state() {
        let (dir, harness) = fixture();
        let app_data = dir.path().join("app-data");
        config_file(
            &harness,
            json!({"mcpServers":{"alpha":{"command":"run"},"beta":{"command":"other"}},"theme":"dark"}),
        );
        harness.mcp_disable("alpha", &app_data).unwrap();
        let before = fs::read(harness.claude_json_path()).unwrap();

        harness.mcp_delete("alpha", &app_data).unwrap();

        assert_eq!(fs::read(harness.claude_json_path()).unwrap(), before);
        let servers = harness.mcp_servers(&app_data).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "beta");
        assert!(servers[0].enabled);
    }

    #[test]
    fn list_never_shows_duplicate_names_and_native_entry_wins_over_stale_sidecar() {
        let (dir, harness) = fixture();
        let app_data = dir.path().join("app-data");
        let native = json!({"command":"native-current"});
        config_file(&harness, json!({"mcpServers":{"alpha":native.clone()}}));
        let stale = serde_json::to_string(&json!({"command":"stale-disabled-copy"})).unwrap();
        let sidecar = app_data.join("mcp-disabled.json");
        fs::create_dir_all(&app_data).unwrap();
        fs::write(
            &sidecar,
            serde_json::to_vec(&json!({"servers":{"alpha":stale}})).unwrap(),
        )
        .unwrap();

        let servers = harness.mcp_servers(&app_data).unwrap();
        let alphas: Vec<_> = servers.iter().filter(|s| s.name == "alpha").collect();
        assert_eq!(alphas.len(), 1, "no duplicate names");
        assert!(
            alphas[0].enabled,
            "the enabled native entry is authoritative"
        );
        assert_eq!(alphas[0].entry, native);
    }

    #[test]
    fn saving_a_disabled_name_clears_the_sidecar_copy_so_enable_keeps_the_save() {
        let (dir, harness) = fixture();
        let app_data = dir.path().join("app-data");
        let stale = json!({"command":"stale","args":["old"]});
        config_file(&harness, json!({"mcpServers":{}}));
        fs::create_dir_all(&app_data).unwrap();
        fs::write(
            app_data.join("mcp-disabled.json"),
            serde_json::to_vec(
                &json!({"servers":{"alpha":serde_json::to_string(&stale).unwrap()}}),
            )
            .unwrap(),
        )
        .unwrap();

        let fresh = json!({"command":"fresh","args":["new"]});
        harness.mcp_save("alpha", fresh.clone(), &app_data).unwrap();

        let servers = harness.mcp_servers(&app_data).unwrap();
        let alpha = servers.iter().find(|s| s.name == "alpha").unwrap();
        assert!(
            alpha.enabled,
            "saving a disabled name enables the fresh entry"
        );
        assert_eq!(alpha.entry, fresh);
        let sidecar: Value =
            serde_json::from_slice(&fs::read(app_data.join("mcp-disabled.json")).unwrap()).unwrap();
        assert!(sidecar["servers"].get("alpha").is_none());

        harness.mcp_enable("alpha", &app_data).unwrap_err();
        let after_enable = atomic::read_json(&harness.claude_json_path()).unwrap();
        assert_eq!(
            after_enable["mcpServers"]["alpha"], fresh,
            "enable must not replay stale sidecar bytes over the newer save"
        );
    }

    #[test]
    fn masked_edits_of_disabled_only_entries_refuse_without_replaying_credentials() {
        let (dir, harness) = fixture();
        let app_data = dir.path().join("app-data");
        let original = json!({"command":"fixture-server","env":{"SERVICE_TOKEN":"fixture-secret"}});
        config_file(&harness, json!({"mcpServers":{"alpha":original}}));
        harness.mcp_disable("alpha", &app_data).unwrap();
        let config_before = fs::read(harness.claude_json_path()).unwrap();
        let sidecar_before = fs::read(app_data.join("mcp-disabled.json")).unwrap();

        let error = harness
            .mcp_save(
                "alpha",
                json!({
                    "command":"edited-server", "env":{"SERVICE_TOKEN":MASK}
                }),
                &app_data,
            )
            .unwrap_err();
        assert!(error.to_string().contains("re-enter the credential values"));
        assert_eq!(fs::read(harness.claude_json_path()).unwrap(), config_before);
        assert_eq!(
            fs::read(app_data.join("mcp-disabled.json")).unwrap(),
            sidecar_before
        );
    }

    #[test]
    fn enable_refuses_to_overwrite_a_newer_active_entry() {
        let (dir, harness) = fixture();
        let app_data = dir.path().join("app-data");
        let newer = json!({"command":"newer-native"});
        config_file(&harness, json!({"mcpServers":{"alpha":newer.clone()}}));
        fs::create_dir_all(&app_data).unwrap();
        fs::write(
            app_data.join("mcp-disabled.json"),
            serde_json::to_vec(&json!({"servers":{"alpha":"{\"command\":\"old\"}"}})).unwrap(),
        )
        .unwrap();

        let error = harness.mcp_enable("alpha", &app_data).unwrap_err();
        assert!(error.to_string().contains("newer than the disabled copy"));
        let active = atomic::read_json(&harness.claude_json_path()).unwrap();
        assert_eq!(
            active["mcpServers"]["alpha"], newer,
            "the newer entry survived"
        );
        let sidecar: Value =
            serde_json::from_slice(&fs::read(app_data.join("mcp-disabled.json")).unwrap()).unwrap();
        assert!(
            sidecar["servers"]["alpha"].is_string(),
            "the disabled copy survived"
        );
    }

    #[test]
    fn masking_covers_bearer_api_key_args_url_and_nested_values_in_one_traversal() {
        let value = json!({
            "command":"run",
            "args":["--header","Authorization: Bearer args-secret","--api-key","sk-args-token"],
            "url":"https://user:pass@host/mcp?api_key=query-secret",
            "env":{"SERVICE_TOKEN":"plain-secret","MODE":"safe","NESTED":{"token":"deep-secret"}},
            "headers":{"Authorization":"Bearer header-secret","x-api-key":"key-value","Accept":"application/json"}
        });
        let masked = mask_entry(&value);
        let serialized = serde_json::to_string(&masked).unwrap();
        for secret in [
            "args-secret",
            "sk-args-token",
            "user:pass",
            "query-secret",
            "plain-secret",
            "deep-secret",
            "header-secret",
            "key-value",
            "Bearer",
        ] {
            assert!(!serialized.contains(secret), "leaked {secret}");
        }
        assert_eq!(masked["args"][2], "--api-key");
        assert_eq!(masked["env"]["MODE"], "safe");
        assert_eq!(masked["headers"]["Accept"], "application/json");
        assert_eq!(masked["command"], "run");
    }

    #[test]
    fn saving_a_masked_edit_over_the_active_entry_lands_restored_values() {
        let (dir, harness) = fixture();
        let app_data = dir.path().join("app-data");
        config_file(
            &harness,
            json!({"mcpServers":{"alpha":{"command":"run","args":["--api-key","real-arg-token"],"env":{"SERVICE_TOKEN":"real-token","MODE":"old"},"headers":{"Authorization":"real-header-token"}}}}),
        );
        let edited = json!({
            "command":"run2",
            "args":["--api-key","real-arg-token"],
            "env":{"SERVICE_TOKEN":MASK,"MODE":"new"},
            "headers":{"Authorization":MASK}
        });
        harness
            .mcp_save("alpha", edited.clone(), &app_data)
            .unwrap();
        let saved = atomic::read_json(&harness.claude_json_path()).unwrap();
        assert_eq!(
            saved["mcpServers"]["alpha"]["env"]["SERVICE_TOKEN"],
            "real-token"
        );
        assert_eq!(saved["mcpServers"]["alpha"]["env"]["MODE"], "new");
        assert_eq!(saved["mcpServers"]["alpha"]["command"], "run2");
        assert_eq!(
            saved["mcpServers"]["alpha"]["headers"]["Authorization"],
            "real-header-token"
        );
        assert_eq!(
            saved["mcpServers"]["alpha"]["args"],
            json!(["--api-key", "real-arg-token"])
        );

        let before_invalid_edit = fs::read(harness.claude_json_path()).unwrap();
        let error = harness
            .mcp_save(
                "alpha",
                json!({"command":"run3","args":["--api-key",MASK]}),
                &app_data,
            )
            .unwrap_err();
        assert!(error.to_string().contains("re-enter the credential"));
        assert_eq!(
            fs::read(harness.claude_json_path()).unwrap(),
            before_invalid_edit,
            "an unidentifiable array credential must not replace the active entry"
        );
    }

    #[test]
    fn dead_stdio_command_surfaces_spawn_failure_without_panicking() {
        let error = test_stdio(&StdioEntry {
            command: "missing-harness-agent-mcp-command".into(),
            args: vec![],
            env: BTreeMap::new(),
            extra: BTreeMap::new(),
        })
        .unwrap_err();
        assert!(error.starts_with("Could not start MCP server:"));
        assert!(!error.contains("token"));
    }

    #[test]
    fn plain_http_rejection_does_not_echo_query_credentials() {
        let error = test_http(&HttpEntry {
            url: "http://example.test/mcp?access_token=fixture-secret".into(),
            headers: BTreeMap::new(),
            extra: BTreeMap::new(),
        })
        .unwrap_err();
        assert!(error.contains("https://"), "names the fix: {error}");
        assert!(
            !error.contains("example.test"),
            "must not echo the URL: {error}"
        );
        assert!(
            !error.contains("fixture-secret"),
            "must not leak query tokens: {error}"
        );
    }

    #[test]
    fn event_stream_waits_for_the_matching_id_and_returns_before_eof() {
        let body = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/log\"}\n\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{}}\n\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\n",
            "data: \"id\":1,\n",
            "data: \"result\":{\"accepted\":true}}\n\n",
        )
        .to_string();
        let (response, release, server) = open_sse_response(body);
        let (decoded_sender, decoded_receiver) = mpsc::channel();
        let decoder = thread::spawn(move || {
            let _ = decoded_sender.send(decode_http_response(response, &json!(1)));
        });
        let decoded = decoded_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("decoder must return on the matching event before stream EOF");
        release.send(()).unwrap();
        server.join().unwrap();
        decoder.join().unwrap();
        let value = decoded.unwrap();
        assert_eq!(value["id"], 1);
        assert_eq!(value["result"]["accepted"], true);
    }

    #[test]
    fn http_response_body_is_capped_before_json_decode() {
        let (response, server) =
            local_http_response(vec![b'x'; BENCH_BYTE_BUDGET + 1], "application/json");
        assert_eq!(
            decode_http_response(response, &json!(1)).unwrap_err(),
            "MCP server response exceeded buffering budget"
        );
        server.join().unwrap();
    }

    #[test]
    fn json_http_response_must_match_its_request_id() {
        let body = serde_json::to_vec(&json!({
            "jsonrpc":"2.0",
            "id":2,
            "result":{"tools":[{"name":"fixture-tool"}]}
        }))
        .unwrap();
        let (response, server) = local_http_response(body, "application/json");
        let value = decode_http_response(response, &json!(2)).unwrap();
        server.join().unwrap();
        assert_eq!(value["id"], 2);
        assert_eq!(value["result"]["tools"][0]["name"], "fixture-tool");

        let body = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":2,"result":{}})).unwrap();
        let (response, server) = local_http_response(body, "application/json");
        assert_eq!(
            decode_http_response(response, &json!(1)).unwrap_err(),
            "MCP server returned no response matching request id"
        );
        server.join().unwrap();
    }

    #[test]
    fn loopback_http_and_https_are_accepted_for_the_bench() {
        assert!(is_loopback_http("http://localhost:3000/mcp"));
        assert!(is_loopback_http("http://127.0.0.1:9/mcp"));
        assert!(is_loopback_http("http://[::1]:9/mcp"));
        assert!(!is_loopback_http("https://example.test/mcp"));
        assert!(!is_loopback_http("http://192.168.1.10/mcp"));
    }
}
