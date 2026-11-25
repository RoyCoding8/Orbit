pub mod antigravity;
pub mod archive;
pub mod catalog;
pub mod codex;
pub mod conversion;
pub mod deletion;
pub mod ingest;
mod jsonl;
pub mod kiro;
pub mod opencode;
pub mod pi;
pub mod spine;

use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TranscriptError {
    #[error("transcript database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("transcript filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transcript path is outside the configured projects directory")]
    InvalidRoot,
    #[error(
        "transcript cache schema version {found} is newer than this build supports ({supported}); delete the cache file to rebuild it"
    )]
    UnsupportedSchema { found: i64, supported: i64 },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub title: Option<String>,
    pub project: String,
    pub updated_at: Option<String>,
    pub message_count: usize,
    pub indexed: bool,
}

#[derive(Default)]
pub(super) struct SessionTitle {
    custom: Option<String>,
    generated: Option<String>,
}

impl SessionTitle {
    pub fn observe(&mut self, record: &serde_json::Value) {
        let (slot, key) = match record.get("type").and_then(serde_json::Value::as_str) {
            Some("custom-title") => (&mut self.custom, "customTitle"),
            Some("ai-title") => (&mut self.generated, "aiTitle"),
            _ => return,
        };
        if let Some(title) = record
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            *slot = Some(title.to_owned());
        }
    }

    pub fn finish(self) -> Option<String> {
        self.custom.or(self.generated)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMessage {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub message_type: String,
    pub text: String,
    #[serde(skip_serializing)]
    pub raw_json: String,
    pub position: usize,
    pub depth: usize,
    pub branch_index: usize,
    pub attachment: Option<String>,
    pub tool_result: Option<String>,
    pub tool_result_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub session_id: String,
    pub project: String,
    pub uuid: String,
    pub position: usize,
    pub message_type: String,
    pub text: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

pub fn database_path(app_data_dir: &Path) -> std::path::PathBuf {
    app_data_dir.join("transcripts.sqlite3")
}

/// Strip Windows extended-length path prefixes (`\\?\UNC\` and `\\?\`).
pub fn strip_windows_extended_prefix(value: &str) -> String {
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(drive) = value.strip_prefix(r"\\?\") {
        drive.to_owned()
    } else {
        value.to_owned()
    }
}

/// Format a filesystem path as a native path string without Windows extended prefixes.
pub fn native_path(path: &Path) -> String {
    strip_windows_extended_prefix(&path.to_string_lossy())
}

pub use orbit_core::transcript_and_parsing::unique_title;

/// Recursively collect non-empty text segments from a JSON message/content value.
pub fn collect_text(value: &serde_json::Value, parts: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if !text.is_empty() => parts.push(text.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_text(item, parts);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(serde_json::Value::as_str) {
                if !text.is_empty() {
                    parts.push(text.to_owned());
                }
            } else if let Some(content) = map.get("content") {
                collect_text(content, parts);
            }
        }
        _ => {}
    }
}

/// Extract normalized newline-joined text from a JSON message or content block value.
pub fn extract_text(value: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    collect_text(value, &mut parts);
    parts.join("\n")
}

/// Convert a `SystemTime` to an epoch-millisecond decimal string.
pub fn modified_epoch_ms(time: SystemTime) -> Option<String> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().to_string())
}

pub use orbit_core::path_safety::{
    image_mime_for_extension, is_allowed_image_path, is_safe_path_component,
};
