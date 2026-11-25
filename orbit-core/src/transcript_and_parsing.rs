//! Pure checksum parsing, YAML frontmatter extraction, iterative spine ordering,
//! title deduplication, and bounded stream/UTF-8 helpers shared between
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

fn nibble_to_hex(n: u8, uppercase: bool) -> u8 {
    if n < 10 {
        b'0' + n
    } else if uppercase {
        b'A' + (n - 10)
    } else {
        b'a' + (n - 10)
    }
}

/// Encode a 32-byte digest into a 64-byte ASCII hex array.
pub fn encode_hex_32(bytes: &[u8; 32], uppercase: bool) -> [u8; 64] {
    let mut out = [0u8; 64];
    for i in 0..32 {
        out[i * 2] = nibble_to_hex(bytes[i] >> 4, uppercase);
        out[i * 2 + 1] = nibble_to_hex(bytes[i] & 0x0f, uppercase);
    }
    out
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

/// Pure fixed-index node representation for stack-allocated spine ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpineNodeIdx {
    pub id: usize,
    pub parent: Option<usize>,
    pub position: usize,
}

/// Loop-free sorting network for up to 3 node IDs by `position`.
fn sort_up_to_3(ids: &mut [usize; 3], len: usize, nodes: &[SpineNodeIdx; 3]) {
    if len >= 2 && nodes[ids[1]].position < nodes[ids[0]].position {
        ids.swap(0, 1);
    }
    if len == 3 {
        if nodes[ids[2]].position < nodes[ids[1]].position {
            ids.swap(1, 2);
        }
        if nodes[ids[1]].position < nodes[ids[0]].position {
            ids.swap(0, 1);
        }
    }
}

/// Fixed-capacity stack-allocated iterative DFS pre-order traversal for up to 3 nodes.
#[allow(clippy::needless_range_loop)]
pub fn order_spine_nodes_bounded(
    nodes: &[SpineNodeIdx; 3],
    n: usize,
) -> ([(usize, usize, usize); 3], usize) {
    let mut ordered = [(0usize, 0usize, 0usize); 3];
    let mut ord_len = 0usize;
    let mut seen = [false; 3];
    let mut stack = [(0usize, 0usize, 0usize); 8];
    let mut stack_len = 0usize;

    let mut roots = [0usize; 3];
    let mut roots_len = 0usize;
    for i in 0..3 {
        if i < n && nodes[i].parent.is_none() {
            roots[roots_len] = nodes[i].id;
            roots_len += 1;
        }
    }
    sort_up_to_3(&mut roots, roots_len, nodes);
    for rev in 0..3 {
        if rev < roots_len {
            let r = roots_len - 1 - rev;
            stack[stack_len] = (roots[r], 0, r);
            stack_len += 1;
        }
    }

    let mut remaining = [0usize; 3];
    for k in 0..3 {
        if k < n {
            remaining[k] = k;
        }
    }
    sort_up_to_3(&mut remaining, n, nodes);
    let mut rem_cursor = 0usize;

    // Unified iterative loop: drains stack, then seeds next unseen remaining node (cycle/orphan).
    // At most 6 total pops/seeds can occur in a 3-node graph (each node has <= 1 parent).
    for _step in 0..6 {
        if stack_len == 0 {
            while rem_cursor < n && seen[remaining[rem_cursor]] {
                rem_cursor += 1;
            }
            if rem_cursor >= n {
                break;
            }
            stack[0] = (remaining[rem_cursor], 0, 0);
            stack_len = 1;
            rem_cursor += 1;
        }
        stack_len -= 1;
        let (id, depth, branch_index) = stack[stack_len];
        if seen[id] {
            continue;
        }
        seen[id] = true;
        ordered[ord_len] = (id, depth, branch_index);
        ord_len += 1;

        let mut kids = [0usize; 3];
        let mut kids_len = 0usize;
        for i in 0..3 {
            if i < n && nodes[i].parent == Some(id) {
                kids[kids_len] = nodes[i].id;
                kids_len += 1;
            }
        }
        sort_up_to_3(&mut kids, kids_len, nodes);
        for rev in 0..3 {
            if rev < kids_len {
                let c = kids_len - 1 - rev;
                if stack_len < stack.len() {
                    stack[stack_len] = (kids[c], depth + 1, c);
                    stack_len += 1;
                }
            }
        }
    }
    (ordered, ord_len)
}

/// Minimal node representation for pure iterative spine ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpineNode {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub position: usize,
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

/// Pure iterative DFS pre-order traversal of a parent-linked message graph.
/// Returns `(uuid, depth, branch_index)` tuples without recursion, preventing
/// stack overflow on deeply nested or adversarial chains and cycles.
pub fn order_spine_nodes(nodes: &[SpineNode]) -> Vec<(String, usize, usize)> {
    order_spine_indices(
        nodes
            .iter()
            .map(|n| (n.uuid.as_str(), n.parent_uuid.as_deref(), n.position)),
    )
    .into_iter()
    .map(|(idx, depth, branch)| (nodes[idx].uuid.clone(), depth, branch))
    .collect()
}

/// Pure title collision resolver over a bitmask of occupied slots (`bit 0` = base title).
pub fn unique_title_slot(existing_mask: u8) -> u8 {
    if (existing_mask & 1) == 0 {
        return 0;
    }
    let mut index = 1u8;
    while index < 8 {
        if (existing_mask & (1u8 << index)) == 0 {
            return index;
        }
        index += 1;
    }
    8
}

/// Format `unique_title` into a fixed stack buffer (allocation-free).
pub fn format_unique_title_buf(source: &[u8], slot: u8) -> ([u8; 16], usize) {
    let mut out = [0u8; 16];
    let mut len = source.len().min(14);
    let mut i = 0;
    while i < len {
        out[i] = source[i];
        i += 1;
    }
    if slot > 0 {
        out[len] = b'-';
        out[len + 1] = b'0' + slot;
        len += 2;
    }
    (out, len)
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

/// Pure bounded-stream decision rule: caps read to `max_bytes + 1` and accepts iff `stream_len <= max_bytes`.
pub fn read_bounded_stream(stream_len: u64, max_bytes: u64) -> Result<(u64, u64), u64> {
    let take_cap = max_bytes.saturating_add(1);
    let read_bytes = stream_len.min(take_cap);
    if read_bytes > max_bytes {
        Err(read_bytes)
    } else {
        Ok((read_bytes, read_bytes))
    }
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
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(ReadBoundedError::Io)?;
    read_bounded_stream(buf.len() as u64, max_bytes)
        .map(|_| buf)
        .map_err(|_| ReadBoundedError::TooLarge)
}

/// UTF-8-safe bounded prefix truncation (never splits a multi-byte code point).
pub fn truncate_utf8_bounded(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
