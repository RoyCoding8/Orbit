pub mod ingest;
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
    pub project: String,
    pub updated_at: Option<String>,
    pub message_count: usize,
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
