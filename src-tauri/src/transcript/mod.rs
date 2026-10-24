pub mod archive;
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
