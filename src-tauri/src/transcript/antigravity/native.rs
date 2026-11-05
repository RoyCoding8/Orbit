use super::*;
use crate::harness::process_lifetime::ChildTree;
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Instant,
};

pub(super) struct Native {
    child: ChildTree,
    responses: Receiver<Value>,
    client: reqwest::blocking::Client,
    port: u16,
    log_directory: tempfile::TempDir,
}

impl Native {
    pub(super) fn start(harness: &AntigravityHarness, store: &str) -> Result<Self, String> {
        let root = store_root(harness, store)?;
        ensure_plain_components(&root)?;
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let logs = tempfile::Builder::new()
            .prefix(".orbit-native-")
            .tempdir_in(&root)
            .map_err(|e| e.to_string())?;
        let mut command = Command::new(executable()?);
        command
            .args([
                "acp",
                "--gemini-dir",
                ".gemini",
                "--app-data-dir",
                if store == "app" {
                    "antigravity"
                } else {
                    "antigravity-cli"
                },
                "--cli_dir_name",
            ])
            .arg(
                logs.path()
                    .file_name()
                    .ok_or("Invalid native log directory")?,
            )
            .args([
                "--disable-telemetry",
                "--include-default-customizations=false",
            ])
            .env(
                "HOME",
                super::super::conversion::native_path(harness.home()),
            )
            .env(
                "USERPROFILE",
                super::super::conversion::native_path(harness.home()),
            )
            .current_dir(harness.home())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = ChildTree::spawn(&mut command)
            .map_err(|e| format!("Could not start agy: {e}. Install Antigravity CLI 1.5.0."))?;
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .ok_or("Antigravity stdout is unavailable")?;
        let (sender, responses) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout).take(8 * 1024 * 1024);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    break;
                };
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let mut native = Self {
            child,
            responses,
            port: 0,
            client: reqwest::blocking::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| e.to_string())?,
            log_directory: logs,
        };
        let init = native.acp(1, "initialize", json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"Orbit","version":env!("CARGO_PKG_VERSION")}}))?;
        if init["agentInfo"]["version"] != "1.5.0" {
            return Err("Native Antigravity chat changes require agy 1.5.0. This installed version has not been verified; browsing and export remain available.".into());
        }
        native.acp(2, "session/list", json!({}))?;
        let log_root = native.log_directory.path().join("log");
        for entry in fs::read_dir(&log_root).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            ensure_plain_components(&path)?;
            let log = fs::read_to_string(path).map_err(|e| e.to_string())?;
            for line in log.lines().filter(|line| line.ends_with(" for HTTP")) {
                if let Some(port) = line
                    .split("port at ")
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|s| s.parse::<u16>().ok())
                {
                    native.port = port;
                }
            }
        }
        if native.port == 0 {
            return Err("Antigravity did not expose its native local chat service.".into());
        }
        Ok(native)
    }

    pub(super) fn call(&self, method: &str, body: Value) -> Result<Value, String> {
        let response = self
            .client
            .post(format!(
                "http://127.0.0.1:{}/exa.language_server_pb.LanguageServerService/{method}",
                self.port
            ))
            .json(&body)
            .send()
            .map_err(|e| format!("Antigravity {method} failed: {e}"))?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Antigravity response exceeded the size limit".into());
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid Antigravity native response")?;
        if !status.is_success() {
            return Err(format!(
                "Antigravity {method}: {}",
                value["message"].as_str().unwrap_or("native request failed")
            ));
        }
        Ok(value)
    }

    fn acp(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let input = self
            .child
            .child_mut()
            .stdin
            .as_mut()
            .ok_or("Antigravity stdin is unavailable")?;
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let value = self
                .responses
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| "Antigravity native request timed out or exited")?;
            if value["id"] == id {
                if value.get("error").is_some() {
                    return Err(format!("Antigravity {method} failed: {}", value["error"]));
                }
                return Ok(value["result"].clone());
            }
        }
    }

    #[cfg(test)]
    pub(super) fn sessions(&mut self) -> Result<Value, String> {
        self.acp(4, "session/list", json!({}))
    }

    #[cfg(test)]
    pub(super) fn replay(&mut self, id: &str, cwd: &Path) -> Result<Vec<Value>, String> {
        let input = self.child.child_mut().stdin.as_mut().ok_or("No stdin")?;
        writeln!(input,"{}",json!({"jsonrpc":"2.0","id":3,"method":"session/load","params":{"sessionId":id,"cwd":cwd,"mcpServers":[]}})).map_err(|e| e.to_string())?;
        let mut updates = Vec::new();
        loop {
            let value = self
                .responses
                .recv_timeout(Duration::from_secs(30))
                .map_err(|e| e.to_string())?;
            if value["id"] == 3 {
                if value.get("error").is_some() {
                    return Err(value["error"].to_string());
                }
                return Ok(updates);
            }
            if value["method"] == "session/update" {
                updates.push(value["params"]["update"].clone());
            }
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        self.child.cleanup();
    }
}

fn executable() -> Result<PathBuf, String> {
    let name = if cfg!(windows) { "agy.exe" } else { "agy" };
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let path = PathBuf::from(home).join(if cfg!(windows) {
            "AppData/Local/agy/bin/agy.exe"
        } else {
            ".local/bin/agy"
        });
        if path.is_file() {
            return Ok(path);
        }
    }
    Err("Install Antigravity CLI 1.5.0 and make agy available to Orbit.".into())
}
