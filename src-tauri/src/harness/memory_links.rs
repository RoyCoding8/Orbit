use super::claude::memories::{MemoryFile, MemoryRole, ReferenceKind};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub(crate) fn without_references(source: &MemoryFile, target: &Path, imports: bool) -> String {
    let content = &source.content;
    let references = reference_spans(content, imports);
    let points_to_target = |destination: &str| {
        resolve_reference(Path::new(&source.path), destination).as_deref() == Some(target)
    };
    let mut removed = Vec::new();
    for (destination, _, range) in &references {
        if !points_to_target(destination) {
            continue;
        }
        let line_start = content[..range.start]
            .rfind('\n')
            .map_or(0, |position| position + 1);
        let line_end = content[range.start..]
            .find('\n')
            .map_or(content.len(), |position| range.start + position + 1);
        let prefix = content[line_start..range.start].trim();
        let index_entry = source.role == MemoryRole::Index
            && (prefix.is_empty()
                || matches!(prefix, "-" | "*" | "+")
                || prefix.strip_suffix('.').is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                }))
            && range.end <= line_end
            && !references.iter().any(|(other, _, span)| {
                span.start >= line_start && span.start < line_end && !points_to_target(other)
            });
        removed.push(if index_entry {
            line_start..line_end
        } else {
            range.clone()
        });
    }
    for (_, definition) in pulldown_cmark::Parser::new(content)
        .reference_definitions()
        .iter()
    {
        if points_to_target(&definition.dest) {
            removed.push(definition.span.clone());
        }
    }
    removed.sort_by_key(|range| range.start);
    let mut result = String::new();
    let mut cursor = 0;
    for range in removed {
        if range.start > cursor {
            result.push_str(&content[cursor..range.start]);
        }
        cursor = cursor.max(range.end);
    }
    result.push_str(&content[cursor..]);
    result
}

pub(crate) fn resolve_reference(source: &Path, target: &str) -> Option<PathBuf> {
    let raw = target
        .trim()
        .trim_matches('<')
        .trim_matches('>')
        .split('#')
        .next()?
        .split('?')
        .next()?;
    if raw.is_empty()
        || raw.starts_with("http:")
        || raw.starts_with("https:")
        || raw.starts_with("mailto:")
    {
        return None;
    }
    let candidate = if raw.starts_with("~/") {
        return Some(PathBuf::from(raw));
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        source.parent()?.join(raw)
    };
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
        }
    }
    Some(normalized)
}

pub(crate) fn parse_references(content: &str, imports: bool) -> Vec<(String, ReferenceKind)> {
    reference_spans(content, imports)
        .into_iter()
        .map(|(target, kind, _)| (target, kind))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn reference_spans(
    content: &str,
    imports: bool,
) -> Vec<(String, ReferenceKind, std::ops::Range<usize>)> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut result = Vec::new();
    let mut code_block = false;
    for (event, range) in Parser::new(content).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_block = true,
            Event::End(TagEnd::CodeBlock) => code_block = false,
            Event::Start(Tag::Link { dest_url, .. }) => {
                result.push((dest_url.into_string(), ReferenceKind::MarkdownLink, range));
            }
            Event::Text(_) if imports && !code_block => {
                for (target, span) in context_imports(&content[range.clone()]) {
                    result.push((
                        target,
                        ReferenceKind::Import,
                        range.start + span.start..range.start + span.end,
                    ));
                }
            }
            _ => {}
        }
    }
    result
}

fn context_imports(text: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let mut imports = Vec::new();
    for (offset, character) in text.char_indices() {
        if character != '@'
            || text[..offset]
                .chars()
                .next_back()
                .is_some_and(|previous| !previous.is_whitespace() && !matches!(previous, '(' | '['))
        {
            continue;
        }
        let mut remaining = text[offset + 1..].char_indices().peekable();
        if remaining
            .peek()
            .is_some_and(|(_, character)| matches!(character, '\'' | '"'))
        {
            continue;
        }
        let mut target = String::new();
        let mut end = offset + 1;
        while let Some((position, character)) = remaining.next() {
            if character == '\\'
                && remaining
                    .peek()
                    .is_some_and(|(_, next)| next.is_whitespace())
            {
                let (position, next) = remaining.next().unwrap();
                target.push(next);
                end = offset + 1 + position + next.len_utf8();
            } else if character.is_whitespace()
                || matches!(character, ')' | ']' | ',' | ';' | '\'' | '"' | '<' | '>')
            {
                break;
            } else {
                target.push(character);
                end = offset + 1 + position + character.len_utf8();
            }
        }
        let trimmed = target.trim_end_matches('.');
        if !trimmed.is_empty() {
            imports.push((
                trimmed.to_owned(),
                offset..end - (target.len() - trimmed.len()),
            ));
        }
    }
    imports
}
