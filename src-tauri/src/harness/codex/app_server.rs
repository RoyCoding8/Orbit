//! Short-lived requests to the installed Codex CLI. No agent turns are started.

use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use super::CodexHarness;
use crate::harness::{process_lifetime::ChildTree, Harness};

const RESPONSE_BUDGET: u64 = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn delete_session(harness: &CodexHarness, session_id: &str) -> Result<(), String> {
    let mut command = Command::new("codex");
    command
        .args(["delete", session_id, "--force"])
        .env("CODEX_HOME", harness.config_root())
        .env("HOME", harness.home())
        .env("USERPROFILE", harness.home())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(sqlite_home) = &harness.sqlite_home {
        command.env("CODEX_SQLITE_HOME", sqlite_home);
    } else {
        command.env_remove("CODEX_SQLITE_HOME");
    }
    let mut child = ChildTree::spawn(&mut command)
        .map_err(|error| format!("Could not start Codex session deletion: {error}"))?;
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    loop {
        match child.child_mut().try_wait() {
            Ok(Some(status)) => {
                child.cleanup();
                return status
                    .success()
                    .then_some(())
                    .ok_or_else(|| "Codex CLI could not delete the session.".to_owned());
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                child.cleanup();
                return Err("Codex session deletion did not finish within 30 seconds.".into());
            }
            Err(error) => {
                child.cleanup();
                return Err(format!("Could not check Codex deletion process: {error}"));
            }
        }
    }
}

pub(crate) fn request(
    harness: &CodexHarness,
    cwd: &Path,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let mut command = Command::new("codex");
    command
        .args(["app-server", "--stdio"])
        .current_dir(cwd)
        .env("CODEX_HOME", harness.config_root())
        .env("HOME", harness.home())
        .env("USERPROFILE", harness.home());
    if let Some(sqlite_home) = &harness.sqlite_home {
        command.env("CODEX_SQLITE_HOME", sqlite_home);
    } else {
        command.env_remove("CODEX_SQLITE_HOME");
    }
    exchange(&mut command, method, params)
}

fn exchange(command: &mut Command, method: &str, params: Value) -> Result<Value, String> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = ChildTree::spawn(command)
        .map_err(|error| format!("Could not start Codex app-server: {error}. Install Codex CLI and make it available on PATH."))?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or("Codex stdout is unavailable")?;
    let responses = responses(stdout);
    let input = child
        .child_mut()
        .stdin
        .as_mut()
        .ok_or("Codex stdin is unavailable")?;
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    send(
        input,
        json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params": {
            "clientInfo":{"name":"Orbit", "version":env!("CARGO_PKG_VERSION")},
            "capabilities":{"experimentalApi":true}
        }}),
    )?;
    receive(&responses, 1, "initialize", deadline)?;
    send(input, json!({"jsonrpc":"2.0", "method":"initialized"}))?;
    send(
        input,
        json!({"jsonrpc":"2.0", "id":2, "method":method, "params":params}),
    )?;
    receive(&responses, 2, method, deadline)
}

fn send(input: &mut impl Write, message: Value) -> Result<(), String> {
    serde_json::to_writer(&mut *input, &message)
        .map_err(|error| format!("Could not write Codex request: {error}"))?;
    input
        .write_all(b"\n")
        .and_then(|_| input.flush())
        .map_err(|error| format!("Could not write Codex request: {error}"))
}

fn responses(stdout: impl Read + Send + 'static) -> Receiver<Result<Value, String>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout).take(RESPONSE_BUDGET + 1);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) if reader.limit() == 0 => {
                    let _ = sender.send(Err("Codex response exceeded the 8 MB limit".into()));
                    return;
                }
                Ok(_) => {
                    if let Ok(value) = serde_json::from_str(&line) {
                        if sender.send(Ok(value)).is_err() {
                            return;
                        }
                    }
                }
                Err(_) => {
                    let _ = sender.send(Err("Could not read Codex response".into()));
                    return;
                }
            }
        }
    });
    receiver
}

fn receive(
    responses: &Receiver<Result<Value, String>>,
    id: u64,
    method: &str,
    deadline: Instant,
) -> Result<Value, String> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Codex did not respond within 30 seconds".to_owned());
        }
        let response = responses.recv_timeout(remaining)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => "Codex did not respond within 30 seconds".to_owned(),
                mpsc::RecvTimeoutError::Disconnected => "Codex closed its output before replying; check its configuration with the Codex CLI".to_owned(),
            })??;
        if response.get("id") != Some(&json!(id)) || response.get("method").is_some() {
            continue;
        }
        if let Some(error) = response.get("error") {
            // Native config errors can contain source lines with credentials.
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(-1);
            return Err(format!(
                "Codex rejected {method} (code {code}); check its configuration with the Codex CLI"
            ));
        }
        return response
            .get("result")
            .cloned()
            .ok_or_else(|| format!("Codex returned no result for {method}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlates_responses_and_keeps_native_errors_concealed() {
        let responses = responses(std::io::Cursor::new(concat!(
            "startup log\n",
            "{\"method\":\"notification\"}\n",
            "{\"id\":1,\"result\":{}}\n",
            "{\"id\":2,\"result\":{\"data\":[\"native skill\"]}}\n",
            "{\"id\":3,\"error\":{\"code\":-32600,\"message\":\"secret config line\"}}\n"
        )));
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            receive(&responses, 2, "skills/list", deadline).unwrap(),
            json!({"data":["native skill"]})
        );
        let error = receive(&responses, 3, "config/read", deadline).unwrap_err();
        assert!(error.contains("-32600"));
        assert!(!error.contains("secret config line"));
    }
}
