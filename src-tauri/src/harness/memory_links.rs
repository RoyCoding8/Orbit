use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::claude::memories::{
    MemoryFile, MemoryReference, MemoryRole, ReferenceKind, ReferenceStatus,
};
pub(crate) use orbit_core::path_safety::resolve_memory_reference as resolve_reference_with_home;

fn env_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn without_references(source: &MemoryFile, target: &Path, imports: bool) -> String {
    let content = &source.content;
    let source_path = Path::new(&source.path);
    let home = env_home();
    let points_to_target = |destination: &str| {
        resolve_reference_with_home(source_path, destination, home.as_deref()).as_deref()
            == Some(target)
    };
    let line_start_of = |offset: usize| content[..offset].rfind('\n').map_or(0, |pos| pos + 1);

    let references = reference_spans(content, imports);
    let mut non_target_lines = HashSet::new();
    let mut target_ranges = Vec::new();
    for (destination, _, range) in references {
        if points_to_target(&destination) {
            target_ranges.push(range);
        } else {
            non_target_lines.insert(line_start_of(range.start));
        }
    }

    let mut removed = Vec::with_capacity(target_ranges.len());
    for range in target_ranges {
        let line_start = line_start_of(range.start);
        let line_end = content[range.start..]
            .find('\n')
            .map_or(content.len(), |pos| range.start + pos + 1);
        let prefix = content[line_start..range.start].trim();
        let index_entry = source.role == MemoryRole::Index
            && (prefix.is_empty()
                || matches!(prefix, "-" | "*" | "+")
                || prefix.strip_suffix('.').is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                }))
            && range.end <= line_end
            && !non_target_lines.contains(&line_start);
        removed.push(if index_entry {
            line_start..line_end
        } else {
            range
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
    let mut result = String::with_capacity(content.len());
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
    let home = env_home();
    resolve_reference_with_home(source, target, home.as_deref())
}

pub(crate) fn populate_references<'a>(
    files: impl IntoIterator<Item = &'a mut MemoryFile>,
    known: &BTreeMap<PathBuf, String>,
    imports: bool,
    home: Option<&Path>,
    mut is_internal: impl FnMut(&str, Option<&Path>) -> bool,
) {
    for file in files {
        let source_path = Path::new(&file.path);
        file.references = parse_references(&file.content, imports)
            .into_iter()
            .map(|(target, kind)| {
                let resolved = resolve_reference_with_home(source_path, &target, home);
                let file_id = resolved.as_ref().and_then(|path| known.get(path)).cloned();
                let status = if file_id.is_some() {
                    ReferenceStatus::Known
                } else if is_internal(&target, resolved.as_deref()) {
                    ReferenceStatus::Unresolved
                } else {
                    ReferenceStatus::External
                };
                MemoryReference {
                    target,
                    kind,
                    status,
                    file_id,
                }
            })
            .collect();
    }
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
