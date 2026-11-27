//! Pure checksum parsing, YAML frontmatter extraction, iterative spine ordering,
//! title deduplication, and bounded stream helpers shared between
//! `src-tauri` and `formal/`.

use std::collections::{HashMap, HashSet};
use std::io::Read;

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decode a 64-byte ASCII hex SHA-256 checksum into 32 raw bytes.
/// Completely panic-free on arbitrary byte slices.
pub fn parse_checksum_bytes(raw: &[u8]) -> Result<[u8; 32], &'static str> {
    if raw.len() != 64 {
        return Err("file checksum is malformed");
    }
    let mut bytes = [0_u8; 32];
    for index in 0..32 {
        let hi = hex_nibble(raw[index * 2]).ok_or("file checksum is malformed")?;
        let lo = hex_nibble(raw[index * 2 + 1]).ok_or("file checksum is malformed")?;
        bytes[index] = (hi << 4) | lo;
    }
    Ok(bytes)
}

/// Decode a 64-character ASCII hex SHA-256 checksum into 32 raw bytes (`&str` wrapper).
pub fn parse_checksum(value: &str) -> Result<[u8; 32], &'static str> {
    parse_checksum_bytes(value.as_bytes())
}

fn find_newline_from(content: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < content.len() {
        if content[i] == b'\n' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn line_is_fence(content: &[u8], mut start: usize, mut end: usize) -> bool {
    if end > start && content[end - 1] == b'\r' {
        end -= 1;
    }
    while start < end && content[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && content[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end - start == 3
        && content[start] == b'-'
        && content[start + 1] == b'-'
        && content[start + 2] == b'-'
}

/// Extract the YAML body between an opening `---` line and a closing `---` line
/// on UTF-8 byte slices. Strips an optional leading UTF-8 BOM (`0xEF 0xBB 0xBF`),
/// supports both LF and CRLF, and trims ASCII whitespace around fence lines.
pub fn extract_frontmatter_body_bytes(content: &[u8]) -> Option<&[u8]> {
    let base =
        if content.len() >= 3 && content[0] == 0xEF && content[1] == 0xBB && content[2] == 0xBF {
            3
        } else {
            0
        };
    let first_end = find_newline_from(content, base)?;
    if !line_is_fence(content, base, first_end) {
        return None;
    }
    let mut offset = first_end + 1;
    let body_start = offset;
    while offset <= content.len() {
        let (line_end, next_offset) = match find_newline_from(content, offset) {
            Some(idx) => (idx, idx + 1),
            None => (content.len(), content.len() + 1),
        };
        if line_is_fence(content, offset, line_end) {
            let mut body_end = offset;
            if body_end >= body_start + 2
                && content[body_end - 2] == b'\r'
                && content[body_end - 1] == b'\n'
            {
                body_end -= 2;
            } else if body_end > body_start && content[body_end - 1] == b'\n' {
                body_end -= 1;
            }
            return Some(&content[body_start..body_end]);
        }
        offset = next_offset;
    }
    None
}

/// Extract the YAML body between an opening `---` line and a closing `---` line (`&str` wrapper).
pub fn extract_frontmatter_body(content: &str) -> Option<&str> {
    extract_frontmatter_body_bytes(content.as_bytes())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
}

/// Pure iterative DFS pre-order traversal over `(uuid, parent_uuid, position)` items.
/// Returns `(original_index, depth, branch_index)` tuples without recursion.
pub fn order_spine_indices<'a>(
    items: impl ExactSizeIterator<Item = (&'a str, Option<&'a str>, usize)>,
) -> Vec<(usize, usize, usize)> {
    let len = items.len();
    let mut by_uuid: HashMap<&'a str, (usize, usize)> = HashMap::with_capacity(len);
    let mut children: HashMap<Option<&'a str>, Vec<&'a str>> = HashMap::new();
    for (idx, (uuid, parent_uuid, position)) in items.enumerate() {
        by_uuid.insert(uuid, (idx, position));
        children.entry(parent_uuid).or_default().push(uuid);
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|uuid| by_uuid.get(uuid).map_or(usize::MAX, |&(_, pos)| pos));
    }
    let mut ordered = Vec::with_capacity(by_uuid.len());
    let mut seen = HashSet::with_capacity(by_uuid.len());
    let mut stack: Vec<(&'a str, usize, usize)> = Vec::new();

    if let Some(roots) = children.get(&None) {
        for (branch_index, uuid) in roots.iter().enumerate().rev() {
            stack.push((*uuid, 0, branch_index));
        }
    }
    drain_spine_stack(&mut stack, &children, &by_uuid, &mut seen, &mut ordered);

    let mut remaining: Vec<(&'a str, usize)> = by_uuid
        .iter()
        .filter(|(uuid, _)| !seen.contains(*uuid))
        .map(|(&uuid, &(_, pos))| (uuid, pos))
        .collect();
    remaining.sort_by_key(|&(_, pos)| pos);
    for (uuid, _) in remaining {
        if !seen.contains(uuid) {
            stack.push((uuid, 0, 0));
            drain_spine_stack(&mut stack, &children, &by_uuid, &mut seen, &mut ordered);
        }
    }
    ordered
}

fn drain_spine_stack<'a>(
    stack: &mut Vec<(&'a str, usize, usize)>,
    children: &HashMap<Option<&'a str>, Vec<&'a str>>,
    by_uuid: &HashMap<&'a str, (usize, usize)>,
    seen: &mut HashSet<&'a str>,
    ordered: &mut Vec<(usize, usize, usize)>,
) {
    while let Some((uuid, depth, branch_index)) = stack.pop() {
        if !seen.insert(uuid) {
            continue;
        }
        if let Some(&(idx, _)) = by_uuid.get(uuid) {
            ordered.push((idx, depth, branch_index));
        }
        if let Some(kids) = children.get(&Some(uuid)) {
            for (child_branch, child_uuid) in kids.iter().enumerate().rev() {
                stack.push((*child_uuid, depth + 1, child_branch));
            }
        }
    }
}

/// Return a collision-free title by appending `-1`, `-2`, ... if `source` is already in `existing`.
pub fn unique_title(source: &str, existing: &HashSet<String>) -> String {
    if !existing.contains(source) {
        return source.to_owned();
    }
    let mut index = 1u64;
    loop {
        let candidate = format!("{source}-{index}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

/// Probe for one byte past the read budget without overflowing.
pub fn read_probe_limit(max_bytes: u64) -> u64 {
    max_bytes.saturating_add(1)
}

/// Error returned by [`read_bounded_bytes`].
#[derive(Debug)]
pub enum ReadBoundedError {
    Io(std::io::Error),
    TooLarge,
}

/// Read from any `std::io::Read` up to `max_bytes`, consuming at most `max_bytes + 1` bytes.
pub fn read_bounded_bytes<R: Read>(reader: R, max_bytes: u64) -> Result<Vec<u8>, ReadBoundedError> {
    let mut buf = Vec::new();
    reader
        .take(read_probe_limit(max_bytes))
        .read_to_end(&mut buf)
        .map_err(ReadBoundedError::Io)?;
    if buf.len() as u64 > max_bytes {
        Err(ReadBoundedError::TooLarge)
    } else {
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn checksum_string_parser_handles_mixed_hex_and_rejects_non_ascii() {
        let hex = "0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789ABCDEF";
        assert_eq!(parse_checksum(hex), parse_checksum_bytes(hex.as_bytes()));
        assert!(parse_checksum(&format!("é{}", "0".repeat(62))).is_err());
        assert!(parse_checksum(&"é".repeat(32)).is_err());
        for len in 66..=68 {
            assert!(parse_checksum(&"0".repeat(len)).is_err());
        }
    }

    #[test]
    fn frontmatter_preserves_multiline_unicode() {
        assert_eq!(
            extract_frontmatter_body("\u{feff}---\r\nname: 雪\r\ndescription: café\r\n---\r\nbody"),
            Some("name: 雪\r\ndescription: café")
        );
    }

    #[test]
    fn frontmatter_distinguishes_an_empty_body_from_a_malformed_fence() {
        assert_eq!(extract_frontmatter_body("---\n---"), Some(""));
        assert_eq!(extract_frontmatter_body("---x\nk: v\n---\n"), None);
    }

    #[test]
    fn every_small_spine_graph_visits_each_node_once() {
        let ids = ["a", "b", "c", "d"];
        let parents = [
            None,
            Some("a"),
            Some("b"),
            Some("c"),
            Some("d"),
            Some("missing"),
        ];
        for mut graph in 0..parents.len().pow(4) {
            let nodes = std::array::from_fn::<_, 4, _>(|index| {
                let parent = parents[graph % parents.len()];
                graph /= parents.len();
                (ids[index], parent, index)
            });
            let mut indices: Vec<_> = order_spine_indices(nodes.into_iter())
                .into_iter()
                .map(|(index, _, _)| index)
                .collect();
            indices.sort_unstable();
            assert_eq!(indices, [0, 1, 2, 3], "{nodes:?}");
        }
    }

    #[test]
    fn spine_orders_parents_before_children_without_recursing() {
        let ids: Vec<_> = (0..20_000).map(|i| i.to_string()).collect();
        let count = ids.len();
        let ordered = order_spine_indices((0..count).rev().map(|i| {
            (
                ids[i].as_str(),
                i.checked_sub(1).map(|parent| ids[parent].as_str()),
                count - i,
            )
        }));
        assert_eq!(ordered.len(), count);
        for (depth, entry) in ordered.into_iter().enumerate() {
            assert_eq!(entry, (count - depth - 1, depth, 0));
        }
    }

    #[test]
    fn titles_choose_the_first_free_suffix() {
        let title = "Chat-1 雪".repeat(20);
        let mut existing: HashSet<_> = (1..=15).map(|suffix| format!("{title}-{suffix}")).collect();
        assert_eq!(unique_title(&title, &existing), title);
        existing.insert(title.clone());
        assert_eq!(unique_title(&title, &existing), format!("{title}-16"));
        existing.remove(&format!("{title}-2"));
        assert_eq!(unique_title(&title, &existing), format!("{title}-2"));
        assert_eq!(unique_title("", &HashSet::from([String::new()])), "-1");
    }

    #[test]
    fn bounded_read_limits_consumption_and_preserves_errors() {
        let data = [0x42; 100];
        for (limit, consumed) in [(64, 65), (0, 1)] {
            let mut reader = Cursor::new(data);
            assert!(matches!(
                read_bounded_bytes(&mut reader, limit),
                Err(ReadBoundedError::TooLarge)
            ));
            assert_eq!(reader.position(), consumed);
        }
        assert_eq!(read_bounded_bytes(&data[..64], 64).unwrap(), &data[..64]);
        assert_eq!(read_bounded_bytes(&data[..], u64::MAX).unwrap(), data);
        assert!(read_bounded_bytes(&[][..], 0).unwrap().is_empty());
        assert!(matches!(
            read_bounded_bytes(BrokenReader, 64),
            Err(ReadBoundedError::Io(error)) if error.to_string() == "test reader failure"
        ));
    }

    struct BrokenReader;

    impl Read for BrokenReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("test reader failure"))
        }
    }
}
