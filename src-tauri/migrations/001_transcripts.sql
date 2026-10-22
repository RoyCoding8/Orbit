PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    source_path TEXT NOT NULL,
    updated_at TEXT,
    title TEXT,
    message_count INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
    session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    uuid TEXT NOT NULL,
    parent_uuid TEXT,
    message_type TEXT NOT NULL,
    text TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    position INTEGER NOT NULL,
    attachment TEXT,
    tool_result TEXT,
    tool_result_note TEXT,
    PRIMARY KEY (session_id, uuid)
);
CREATE INDEX IF NOT EXISTS messages_session_position ON messages(session_id, position);
CREATE INDEX IF NOT EXISTS messages_parent ON messages(session_id, parent_uuid);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    session_id UNINDEXED,
    uuid UNINDEXED,
    text,
    tokenize = 'unicode61'
);
CREATE TABLE IF NOT EXISTS transcript_ingest_errors (
    session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    error TEXT NOT NULL,
    PRIMARY KEY (session_id, position)
);
