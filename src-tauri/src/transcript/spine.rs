use super::{ingest::open_database, SearchHit, TranscriptError, TranscriptMessage};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{collections::HashMap, path::Path};

#[derive(Debug)]
struct StoredMessage {
    uuid: String,
    parent_uuid: Option<String>,
    message_type: String,
    text: String,
    raw_json: String,
    position: usize,
    attachment: Option<String>,
    tool_result: Option<String>,
    tool_result_note: Option<String>,
}

pub fn thread_page(
    app_data_dir: &Path,
    session_id: &str,
    offset: usize,
    limit: usize,
) -> Result<Vec<TranscriptMessage>, TranscriptError> {
    let connection = open_database(app_data_dir)?;
    let stored = load_session(&connection, session_id)?;
    let ordered = order_spine(stored);
    Ok(ordered
        .into_iter()
        .skip(offset)
        .take(limit.clamp(1, 500))
        .collect())
}

fn load_session(
    connection: &Connection,
    session_id: &str,
) -> Result<Vec<StoredMessage>, TranscriptError> {
    let mut statement = connection.prepare("SELECT uuid, parent_uuid, message_type, text, raw_json, position, attachment, tool_result, tool_result_note FROM messages WHERE session_id = ?1 ORDER BY position")?;
    let rows = statement.query_map([session_id], |row| {
        Ok(StoredMessage {
            uuid: row.get(0)?,
            parent_uuid: row.get(1)?,
            message_type: row.get(2)?,
            text: row.get(3)?,
            raw_json: row.get(4)?,
            position: row.get::<_, i64>(5)? as usize,
            attachment: row.get(6)?,
            tool_result: row.get(7)?,
            tool_result_note: row.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn order_spine(messages: Vec<StoredMessage>) -> Vec<TranscriptMessage> {
    let mut records = std::collections::HashMap::new();
    let mut children: std::collections::HashMap<Option<String>, Vec<String>> =
        std::collections::HashMap::new();
    for message in messages {
        let uuid = message.uuid.clone();
        let parent = message.parent_uuid.clone();
        children.entry(parent).or_default().push(uuid.clone());
        records.insert(uuid, message);
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|uuid| {
            records
                .get(uuid)
                .map_or(usize::MAX, |message| message.position)
        });
    }
    let mut ordered = Vec::with_capacity(records.len());
    let mut seen = std::collections::HashSet::with_capacity(records.len());
    visit_children(None, 0, &children, &records, &mut seen, &mut ordered);
    let mut remaining = records.keys().cloned().collect::<Vec<_>>();
    remaining.sort_by_key(|uuid| {
        records
            .get(uuid)
            .map_or(usize::MAX, |message| message.position)
    });
    for uuid in remaining {
        if !seen.contains(&uuid) {
            visit_by_id(&uuid, 0, 0, &children, &records, &mut seen, &mut ordered);
        }
    }
    ordered
}

fn visit_children(
    parent: Option<String>,
    depth: usize,
    children: &std::collections::HashMap<Option<String>, Vec<String>>,
    records: &std::collections::HashMap<String, StoredMessage>,
    seen: &mut std::collections::HashSet<String>,
    ordered: &mut Vec<TranscriptMessage>,
) {
    if let Some(siblings) = children.get(&parent) {
        for (branch_index, uuid) in siblings.iter().enumerate() {
            visit_by_id(uuid, depth, branch_index, children, records, seen, ordered);
        }
    }
}

fn visit_by_id(
    uuid: &str,
    depth: usize,
    branch_index: usize,
    children: &std::collections::HashMap<Option<String>, Vec<String>>,
    records: &std::collections::HashMap<String, StoredMessage>,
    seen: &mut std::collections::HashSet<String>,
    ordered: &mut Vec<TranscriptMessage>,
) {
    if !seen.insert(uuid.to_owned()) {
        return;
    }
    let Some(message) = records.get(uuid) else {
        return;
    };
    ordered.push(TranscriptMessage {
        uuid: message.uuid.clone(),
        parent_uuid: message.parent_uuid.clone(),
        message_type: message.message_type.clone(),
        text: message.text.clone(),
        raw_json: message.raw_json.clone(),
        position: message.position,
        depth,
        branch_index,
        attachment: message.attachment.clone(),
        tool_result: message.tool_result.clone(),
        tool_result_note: message.tool_result_note.clone(),
    });
    visit_children(
        Some(uuid.to_owned()),
        depth + 1,
        children,
        records,
        seen,
        ordered,
    );
}

pub fn message_page_offset(
    app_data_dir: &Path,
    session_id: &str,
    uuid: &str,
    page_size: usize,
) -> Result<Option<usize>, TranscriptError> {
    let connection = open_database(app_data_dir)?;
    let messages = order_spine(load_session(&connection, session_id)?);
    let size = page_size.max(1);
    Ok(messages
        .iter()
        .position(|message| message.uuid == uuid)
        .map(|index| index / size * size))
}

pub fn search(
    app_data_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, TranscriptError> {
    let connection = open_database(app_data_dir)?;
    search_connection(&connection, query, limit)
}

pub(crate) fn search_connection(
    connection: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, TranscriptError> {
    let Some(match_query) = build_match_query(query) else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare(
        "SELECT m.session_id, s.project, m.uuid, m.position, m.message_type, m.text \
         FROM messages_fts f JOIN messages m ON m.session_id=f.session_id AND m.uuid=f.uuid \
         JOIN sessions s ON s.session_id=m.session_id \
         WHERE messages_fts MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows = statement.query_map(params![match_query, limit.clamp(1, 200) as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)? as usize,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    let sessions = rows
        .iter()
        .map(|(session_id, _, _, _, _, _)| session_id.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut context = HashMap::with_capacity(sessions.len());
    for session_id in sessions {
        let ordered = order_spine(load_session(connection, &session_id)?);
        let entries = ordered
            .iter()
            .map(|message| (message.uuid.clone(), message.text.clone()))
            .collect::<Vec<_>>();
        let mut neighbors = HashMap::with_capacity(entries.len());
        for (index, (uuid, _)) in entries.iter().enumerate() {
            let prev = if index > 0 {
                Some(entries[index - 1].1.clone())
            } else {
                None
            };
            let next = entries.get(index + 1).map(|(_, text)| text.clone());
            neighbors.insert(uuid.clone(), (prev, next));
        }
        context.insert(session_id, neighbors);
    }
    Ok(rows
        .into_iter()
        .map(
            |(session_id, project, uuid, position, message_type, text)| {
                let (before, after) = context
                    .get(&session_id)
                    .and_then(|neighbors| neighbors.get(&uuid))
                    .cloned()
                    .unwrap_or((None, None));
                SearchHit {
                    session_id,
                    project,
                    uuid,
                    position,
                    message_type,
                    text,
                    before,
                    after,
                }
            },
        )
        .collect())
}

/// Builds a literal-safe FTS5 MATCH expression. Each whitespace token is
/// quoted as a phrase with embedded quotes doubled, so punctuation like
/// `C++` or `error: failed` searches as text instead of FTS syntax.
/// Tokens with no alphanumeric characters cannot match under any quoting,
/// so they are dropped; a query that is all punctuation returns no hits.
fn build_match_query(query: &str) -> Option<String> {
    let phrases = query
        .split_whitespace()
        .filter(|token| token.chars().any(char::is_alphanumeric))
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    (!phrases.is_empty()).then(|| phrases.join(" "))
}

pub fn export_markdown(app_data_dir: &Path, session_id: &str) -> Result<String, TranscriptError> {
    let connection = open_database(app_data_dir)?;
    let project: String = connection.query_row(
        "SELECT project FROM sessions WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )?;
    let messages = order_spine(load_session(&connection, session_id)?);
    let mut markdown = format!(
        "# Transcript {session_id}\n\nProject: `{}`\n\n",
        project.replace('`', "\\`")
    );
    for message in messages {
        let role = match message.message_type.as_str() {
            "assistant" => "Assistant",
            "user" => "User",
            "system" => "System",
            other => other,
        };
        markdown.push_str(&format!("## {role} · {}\n\n", message.position));
        if message.depth > 0 {
            markdown.push_str(&format!(
                "Branch depth: {} · branch {}\n\n",
                message.depth,
                message.branch_index + 1
            ));
        }
        if !message.text.is_empty() {
            markdown.push_str(&message.text);
            markdown.push_str("\n\n");
        }
        if let Some(attachment) = message.attachment {
            markdown.push_str("### Attachment\n\n");
            markdown.push_str(&attachment);
            markdown.push_str("\n\n");
        }
        if let Some(result) = message.tool_result {
            markdown.push_str("### Tool result\n\n```text\n");
            markdown.push_str(&result.replace("```", "``\\`"));
            markdown.push_str("\n```\n\n");
        }
        if let Some(note) = message.tool_result_note {
            markdown.push_str(&format!("_Note: {note}_\n\n"));
        }
        append_unknown_content_note(&message.raw_json, &mut markdown);
        markdown.push_str("---\n\n");
    }
    Ok(markdown)
}

fn append_unknown_content_note(raw_json: &str, output: &mut String) {
    let Ok(value) = serde_json::from_str::<Value>(raw_json) else {
        return;
    };
    let Some(object) = value.as_object() else {
        return;
    };
    let unknown = object
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "uuid"
                    | "parentUuid"
                    | "type"
                    | "message"
                    | "content"
                    | "attachment"
                    | "rendered"
                    | "toolUseResult"
                    | "timestamp"
                    | "sessionId"
                    | "session_id"
                    | "cwd"
                    | "version"
                    | "gitBranch"
                    | "isSidechain"
                    | "promptId"
                    | "origin"
                    | "promptSource"
                    | "turnOrigin"
                    | "toolUseID"
                    | "toolUseId"
            )
        })
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        output.push_str(&format!(
            "_Additional source fields preserved in database: {}._\n\n",
            unknown
                .iter()
                .map(|key| key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn insert_message(
        connection: &Connection,
        uuid: &str,
        parent: Option<&str>,
        position: usize,
        text: &str,
    ) {
        connection.execute("INSERT INTO sessions(session_id, project, source_path, message_count) VALUES ('s', 'p', '', 0) ON CONFLICT DO NOTHING", []).unwrap();
        connection.execute("INSERT INTO messages(session_id, uuid, parent_uuid, message_type, text, raw_json, position) VALUES ('s', ?1, ?2, 'user', ?3, '{}', ?4)", params![uuid, parent, text, position as i64]).unwrap();
        connection
            .execute(
                "INSERT INTO messages_fts(session_id, uuid, text) VALUES ('s', ?1, ?2)",
                params![uuid, text],
            )
            .unwrap();
    }

    #[test]
    fn branch_point_renders_each_child_and_descendant() {
        let temp = tempfile::tempdir().unwrap();
        let connection = open_database(temp.path()).unwrap();
        insert_message(&connection, "root", None, 1, "root");
        insert_message(&connection, "branch-a", Some("root"), 2, "a");
        insert_message(&connection, "branch-b", Some("root"), 3, "b");
        insert_message(&connection, "leaf", Some("branch-a"), 4, "leaf");
        let ordered = order_spine(load_session(&connection, "s").unwrap());
        assert_eq!(
            ordered
                .iter()
                .map(|message| message.uuid.as_str())
                .collect::<Vec<_>>(),
            ["root", "branch-a", "leaf", "branch-b"]
        );
        assert_eq!(ordered[1].branch_index, 0);
        assert_eq!(ordered[3].branch_index, 1);
    }

    #[test]
    fn markdown_export_includes_every_message_and_formats_tool_result() {
        let temp = tempfile::tempdir().unwrap();
        let connection = open_database(temp.path()).unwrap();
        insert_message(&connection, "one", None, 1, "first message");
        insert_message(&connection, "two", Some("one"), 2, "second message");
        connection
            .execute(
                "UPDATE messages SET attachment = 'inline attachment' WHERE uuid = 'one'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE messages SET tool_result = 'tool output' WHERE uuid = 'two'",
                [],
            )
            .unwrap();
        let output = export_markdown(temp.path(), "s").unwrap();
        assert!(output.starts_with("# Transcript s"));
        assert!(output.contains("first message") && output.contains("second message"));
        assert!(output.contains("### Attachment\n\ninline attachment"));
        assert!(output.contains("```text\ntool output\n```"));
    }

    #[test]
    fn search_returns_hit_with_adjacent_context() {
        let temp = tempfile::tempdir().unwrap();
        let connection = open_database(temp.path()).unwrap();
        insert_message(&connection, "before", None, 1, "previous context");
        insert_message(
            &connection,
            "hit",
            Some("before"),
            2,
            "distinctive transcript phrase",
        );
        insert_message(&connection, "after", Some("hit"), 3, "following context");
        let hits = search_connection(&connection, "distinctive", 10).unwrap();
        assert_eq!(hits[0].uuid, "hit");
        assert_eq!(hits[0].before.as_deref(), Some("previous context"));
        assert_eq!(hits[0].after.as_deref(), Some("following context"));
    }

    #[test]
    fn punctuation_queries_search_as_literals_instead_of_erroring() {
        let temp = tempfile::tempdir().unwrap();
        let connection = open_database(temp.path()).unwrap();
        insert_message(&connection, "one", None, 1, "refactor the C++ parser");
        insert_message(
            &connection,
            "two",
            Some("one"),
            2,
            "color scheme for C# notes",
        );
        insert_message(
            &connection,
            "three",
            Some("two"),
            3,
            "handle error: failed gracefully",
        );
        for query in [
            "C++",
            "C#",
            "\"unterminated",
            "error: failed",
            "error AND",
            ")))",
            "***",
            "   ",
        ] {
            let outcome = search_connection(&connection, query, 10);
            assert!(outcome.is_ok(), "query {query:?} errored: {outcome:?}");
        }
        let hits = search_connection(&connection, "error: failed", 10).unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.uuid.as_str()).collect::<Vec<_>>(),
            ["three"]
        );
    }

    #[test]
    fn branch_context_uses_spine_order_not_file_position() {
        let temp = tempfile::tempdir().unwrap();
        let connection = open_database(temp.path()).unwrap();
        insert_message(&connection, "root", None, 1, "root text");
        insert_message(
            &connection,
            "branch-a",
            Some("root"),
            2,
            "first branch leaf",
        );
        insert_message(
            &connection,
            "branch-b",
            Some("root"),
            3,
            "needle in sibling branch",
        );
        insert_message(
            &connection,
            "child-a",
            Some("branch-a"),
            4,
            "last message in first branch",
        );
        let hits = search_connection(&connection, "needle", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].before.as_deref(),
            Some("last message in first branch")
        );
    }

    #[test]
    fn thread_page_serializes_without_raw_json_but_export_keeps_the_note() {
        let temp = tempfile::tempdir().unwrap();
        let connection = open_database(temp.path()).unwrap();
        connection
            .execute(
                "INSERT INTO sessions(session_id, project, source_path, message_count) VALUES ('s', 'p', '', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages(session_id, uuid, message_type, text, raw_json, position) VALUES ('s', 'm1', 'user', 'body', '{\"future-kind\":{\"x\":1}}', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages_fts(session_id, uuid, text) VALUES ('s', 'm1', 'body')",
                [],
            )
            .unwrap();
        let page = thread_page(temp.path(), "s", 0, 10).unwrap();
        assert_eq!(page.len(), 1);
        let serialized = serde_json::to_string(&page[0]).unwrap();
        assert!(!serialized.contains("rawJson"));
        let markdown = export_markdown(temp.path(), "s").unwrap();
        assert!(markdown.contains("Additional source fields preserved in database: future-kind."));
    }

    #[test]
    fn all_punctuation_query_returns_no_hits_without_error() {
        let temp = tempfile::tempdir().unwrap();
        let connection = open_database(temp.path()).unwrap();
        assert!(search_connection(&connection, "***", 10)
            .unwrap()
            .is_empty());
        assert!(search_connection(&connection, "   ", 10)
            .unwrap()
            .is_empty());
    }
}
