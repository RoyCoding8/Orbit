use super::{ingest::open_database, SearchHit, TranscriptError, TranscriptMessage};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
    path::Path,
};

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

struct LightMessage {
    uuid: String,
    parent_uuid: Option<String>,
    position: usize,
    text: String,
}

pub fn thread_page(
    app_data_dir: &Path,
    session_id: &str,
    offset: usize,
    limit: usize,
) -> Result<Vec<TranscriptMessage>, TranscriptError> {
    let connection = open_database(app_data_dir)?;
    let ordered = order_spine(load_session(&connection, session_id)?);
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
    let mut statement = connection.prepare_cached(
        "SELECT uuid, parent_uuid, message_type, text, raw_json, position, attachment, tool_result, tool_result_note FROM messages WHERE session_id = ?1 ORDER BY position",
    )?;
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

fn load_session_light(
    connection: &Connection,
    session_id: &str,
    with_text: bool,
) -> Result<Vec<LightMessage>, TranscriptError> {
    let sql = if with_text {
        "SELECT uuid, parent_uuid, position, text FROM messages WHERE session_id = ?1 ORDER BY position"
    } else {
        "SELECT uuid, parent_uuid, position, '' FROM messages WHERE session_id = ?1 ORDER BY position"
    };
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement.query_map([session_id], |row| {
        Ok(LightMessage {
            uuid: row.get(0)?,
            parent_uuid: row.get(1)?,
            position: row.get::<_, i64>(2)? as usize,
            text: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

use orbit_core::transcript_and_parsing::order_spine_indices;
pub use orbit_core::transcript_and_parsing::{order_spine_nodes, SpineNode};

fn order_spine(messages: Vec<StoredMessage>) -> Vec<TranscriptMessage> {
    let order = order_spine_indices(
        messages
            .iter()
            .map(|m| (m.uuid.as_str(), m.parent_uuid.as_deref(), m.position)),
    );
    let mut slots: Vec<Option<StoredMessage>> = messages.into_iter().map(Some).collect();
    let mut ordered = Vec::with_capacity(order.len());
    for (idx, depth, branch_index) in order {
        if let Some(message) = slots[idx].take() {
            ordered.push(TranscriptMessage {
                uuid: message.uuid,
                parent_uuid: message.parent_uuid,
                message_type: message.message_type,
                text: message.text,
                raw_json: message.raw_json,
                position: message.position,
                depth,
                branch_index,
                attachment: message.attachment,
                tool_result: message.tool_result,
                tool_result_note: message.tool_result_note,
            });
        }
    }
    ordered
}

pub fn message_page_offset(
    app_data_dir: &Path,
    session_id: &str,
    uuid: &str,
    page_size: usize,
) -> Result<Option<usize>, TranscriptError> {
    let connection = open_database(app_data_dir)?;
    let messages = load_session_light(&connection, session_id, false)?;
    let order = order_spine_indices(
        messages
            .iter()
            .map(|m| (m.uuid.as_str(), m.parent_uuid.as_deref(), m.position)),
    );
    let size = page_size.max(1);
    Ok(order
        .iter()
        .position(|&(idx, _, _)| messages[idx].uuid == uuid)
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
    let mut statement = connection.prepare_cached(
        "SELECT m.session_id, s.project, m.uuid, m.position, m.message_type, m.text \
         FROM messages_fts f JOIN messages m ON m.session_id=f.session_id AND m.uuid=f.uuid \
         JOIN sessions s ON s.session_id=m.session_id \
         WHERE s.indexed = 1 AND messages_fts MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows = statement
        .query_map(params![match_query, limit.clamp(1, 200) as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? as usize,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut wanted_by_session: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (session_id, _, uuid, _, _, _) in &rows {
        wanted_by_session
            .entry(session_id.as_str())
            .or_default()
            .insert(uuid.as_str());
    }
    let mut context: HashMap<(String, String), (Option<String>, Option<String>)> =
        HashMap::with_capacity(rows.len());
    for (session_id, wanted) in wanted_by_session {
        let messages = load_session_light(connection, session_id, true)?;
        let order = order_spine_indices(
            messages
                .iter()
                .map(|m| (m.uuid.as_str(), m.parent_uuid.as_deref(), m.position)),
        );
        for (pos, &(idx, _, _)) in order.iter().enumerate() {
            let uuid = messages[idx].uuid.as_str();
            if wanted.contains(uuid) {
                let prev = pos
                    .checked_sub(1)
                    .map(|p| messages[order[p].0].text.clone());
                let next = order
                    .get(pos + 1)
                    .map(|&(n, _, _)| messages[n].text.clone());
                context.insert((session_id.to_owned(), uuid.to_owned()), (prev, next));
            }
        }
    }
    Ok(rows
        .into_iter()
        .map(
            |(session_id, project, uuid, position, message_type, text)| {
                let (before, after) = context
                    .get(&(session_id.clone(), uuid.clone()))
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
pub fn build_match_query(query: &str) -> Option<String> {
    let mut result = String::new();
    for token in query
        .split_whitespace()
        .filter(|token| token.chars().any(char::is_alphanumeric))
    {
        if !result.is_empty() {
            result.push(' ');
        }
        result.push('"');
        for ch in token.chars() {
            if ch == '"' {
                result.push_str("\"\"");
            } else {
                result.push(ch);
            }
        }
        result.push('"');
    }
    (!result.is_empty()).then_some(result)
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
        let _ = write!(markdown, "## {role} · {}\n\n", message.position);
        if message.depth > 0 {
            let _ = write!(
                markdown,
                "Branch depth: {} · branch {}\n\n",
                message.depth,
                message.branch_index + 1
            );
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
            let _ = write!(markdown, "_Note: {note}_\n\n");
        }
        append_unknown_content_note(&message.raw_json, &mut markdown);
        markdown.push_str("---\n\n");
    }
    Ok(markdown)
}

fn append_unknown_content_note(raw_json: &str, output: &mut String) {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(raw_json) else {
        return;
    };
    let mut first = true;
    for key in object.keys().filter(|key| {
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
    }) {
        if first {
            output.push_str("_Additional source fields preserved in database: ");
            first = false;
        } else {
            output.push_str(", ");
        }
        output.push_str(key);
    }
    if !first {
        output.push_str("._\n\n");
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
        let temp = crate::test_support::tempdir().unwrap();
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
        let temp = crate::test_support::tempdir().unwrap();
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
        let temp = crate::test_support::tempdir().unwrap();
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
        let temp = crate::test_support::tempdir().unwrap();
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
            if matches!(query, "***" | "   ") {
                assert!(
                    outcome.unwrap().is_empty(),
                    "query {query:?} should have no hits"
                );
            }
        }
        let hits = search_connection(&connection, "error: failed", 10).unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.uuid.as_str()).collect::<Vec<_>>(),
            ["three"]
        );
    }

    #[test]
    fn branch_context_uses_spine_order_not_file_position() {
        let temp = crate::test_support::tempdir().unwrap();
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
        let temp = crate::test_support::tempdir().unwrap();
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
}
