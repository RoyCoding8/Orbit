//! Pure path-safety, containment, lexical normalization, and archive-path
//! validation logic shared between `src-tauri` and `formal/`.

use std::path::{Component, Path, PathBuf};

/// Abstract lexical path component mirroring `std::path::Component`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSeg {
    Root,
    Cur,
    Parent,
    Normal(u8),
}

/// Fixed-capacity stack path buffer for allocation-free lexical operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegPath<const N: usize> {
    pub data: [PathSeg; N],
    pub len: usize,
}

impl<const N: usize> SegPath<N> {
    pub fn new() -> Self {
        Self {
            data: [PathSeg::Cur; N],
            len: 0,
        }
    }

    pub fn push(&mut self, seg: PathSeg) {
        if self.len < N {
            self.data[self.len] = seg;
            self.len += 1;
        }
    }

    pub fn pop(&mut self) -> Option<PathSeg> {
        if self.len == 0 {
            None
        } else {
            self.len -= 1;
            Some(self.data[self.len])
        }
    }

    pub fn last(&self) -> Option<PathSeg> {
        if self.len == 0 {
            None
        } else {
            Some(self.data[self.len - 1])
        }
    }

    pub fn as_slice(&self) -> &[PathSeg] {
        &self.data[..self.len]
    }
}

impl<const N: usize> Default for SegPath<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Pure lexical normalizer over abstract path segments.
///
/// Resolves `.` and `..` without touching the filesystem. Preserves leading
/// `..` components on relative paths instead of silently dropping them, and
/// clamps `..` at `Root` on absolute paths.
pub fn normalize_lexical_segments<const N: usize>(path: &[PathSeg]) -> SegPath<N> {
    let mut normalized = SegPath::<N>::new();
    let mut i = 0;
    while i < path.len() {
        let component = path[i];
        match component {
            PathSeg::Root => normalized.push(PathSeg::Root),
            PathSeg::Normal(value) => normalized.push(PathSeg::Normal(value)),
            PathSeg::Cur => {}
            PathSeg::Parent => match normalized.last() {
                Some(PathSeg::Normal(_)) => {
                    normalized.pop();
                }
                Some(PathSeg::Root) => {}
                None | Some(PathSeg::Parent | PathSeg::Cur) => {
                    normalized.push(PathSeg::Parent);
                }
            },
        }
        i += 1;
    }
    normalized
}

/// Lexically normalize `.` and `..` components on a `std::path::Path` without
/// touching the filesystem. Preserves leading `..` components on relative paths
/// instead of silently dropping them.
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                None | Some(Component::ParentDir | Component::CurDir) => {
                    normalized.push(component.as_os_str());
                }
            },
        }
    }
    normalized
}

pub fn contains_seg(slice: &[PathSeg], target: PathSeg) -> bool {
    let mut i = 0;
    while i < slice.len() {
        if slice[i] == target {
            return true;
        }
        i += 1;
    }
    false
}

pub fn segs_start_with(path: &[PathSeg], prefix: &[PathSeg]) -> bool {
    if path.len() < prefix.len() {
        return false;
    }
    let mut i = 0;
    while i < prefix.len() {
        if path[i] != prefix[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Pure containment check over lexical path segments.
pub fn path_is_within_segments(
    path: &[PathSeg],
    path_has_link: bool,
    root: &[PathSeg],
    root_has_link: bool,
) -> bool {
    if contains_seg(path, PathSeg::Parent) || contains_seg(root, PathSeg::Parent) {
        return false;
    }
    if path_has_link || root_has_link {
        return false;
    }
    let resolved_path = normalize_lexical_segments::<4>(path);
    let resolved_root = normalize_lexical_segments::<4>(root);
    if resolved_root.len == 0 {
        return false;
    }
    segs_start_with(resolved_path.as_slice(), resolved_root.as_slice())
}

/// Sentinel segment identifiers for `<config_root>/.orbit/backups`.
pub const SEG_ORBIT: u8 = 250;
pub const SEG_BACKUPS: u8 = 251;

/// Pure backup-root containment check (`<config_root>/.orbit/backups/<entry>`).
pub fn backup_path_is_contained(
    config_root: &[PathSeg],
    backup_path: &[PathSeg],
    backup_has_link: bool,
    root_has_link: bool,
) -> bool {
    if backup_has_link
        || root_has_link
        || contains_seg(config_root, PathSeg::Parent)
        || contains_seg(backup_path, PathSeg::Parent)
    {
        return false;
    }
    let resolved_backup = normalize_lexical_segments::<6>(backup_path);
    if resolved_backup.len == 0 {
        return false;
    }
    let parent = &resolved_backup.as_slice()[..resolved_backup.len - 1];
    let mut expected_root = normalize_lexical_segments::<6>(config_root);
    expected_root.push(PathSeg::Normal(SEG_ORBIT));
    expected_root.push(PathSeg::Normal(SEG_BACKUPS));
    parent == expected_root.as_slice()
}

/// Return true if `value` is a safe single filename / path component
/// (non-empty, not `.` or `..`, and no `/`, `\`, `:`, or NUL).
pub fn is_safe_path_component_bytes(value: &[u8]) -> bool {
    if value.is_empty()
        || (value.len() == 1 && value[0] == b'.')
        || (value.len() == 2 && value[0] == b'.' && value[1] == b'.')
    {
        return false;
    }
    let mut i = 0;
    while i < value.len() {
        let b = value[i];
        if b == b'/' || b == b'\\' || b == b':' || b == 0 {
            return false;
        }
        i += 1;
    }
    true
}

/// Return true if `value` is a safe single filename / path component.
pub fn is_safe_path_component(value: &str) -> bool {
    is_safe_path_component_bytes(value.as_bytes())
}

fn eq_icase_3(a: &[u8], b: &[u8; 3]) -> bool {
    a.len() == 3
        && a[0].to_ascii_lowercase() == b[0]
        && a[1].to_ascii_lowercase() == b[1]
        && a[2].to_ascii_lowercase() == b[2]
}

fn eq_icase_4(a: &[u8], b: &[u8; 4]) -> bool {
    a.len() == 4
        && a[0].to_ascii_lowercase() == b[0]
        && a[1].to_ascii_lowercase() == b[1]
        && a[2].to_ascii_lowercase() == b[2]
        && a[3].to_ascii_lowercase() == b[3]
}

fn eq_icase_6(a: &[u8], b: &[u8; 6]) -> bool {
    a.len() == 6
        && a[0].to_ascii_lowercase() == b[0]
        && a[1].to_ascii_lowercase() == b[1]
        && a[2].to_ascii_lowercase() == b[2]
        && a[3].to_ascii_lowercase() == b[3]
        && a[4].to_ascii_lowercase() == b[4]
        && a[5].to_ascii_lowercase() == b[5]
}

/// Pure validator for relative archive paths (byte-slice form).
/// Rejects empty/oversize paths, backslashes, drive prefixes/ADS (`:`), NUL,
/// traversal, control characters, Windows reserved device names, `.git`/`.orbit`,
/// and paths nested deeper than 24 components.
pub fn validate_relative_path_bytes(text: &[u8]) -> Result<(), &'static str> {
    if text.is_empty() || text.len() > 500 {
        return Err("Archive contains an unsafe relative path");
    }
    let mut start = 0;
    let mut count = 0usize;
    let mut pos = 0;
    while pos <= text.len() {
        if pos < text.len() && matches!(text[pos], b'\\' | b':' | 0) {
            return Err("Archive contains an unsafe relative path");
        }
        if pos == text.len() || text[pos] == b'/' {
            let component = &text[start..pos];
            count += 1;
            if !is_valid_relative_component(component) {
                return Err("Archive contains an unsafe relative path");
            }
            start = pos + 1;
        }
        pos += 1;
    }
    if count > 24 {
        return Err("Archive path is too deeply nested");
    }
    Ok(())
}

/// Pure validator for relative archive paths (`&str` wrapper).
pub fn validate_relative_path(text: &str) -> Result<(), &'static str> {
    validate_relative_path_bytes(text.as_bytes())
}

/// Validate a single relative path component against traversal, trailing dot/space,
/// C0/DEL/C1 control characters, Windows forbidden characters, `.git`/`.orbit`,
/// and Windows reserved device names.
pub fn is_valid_relative_component(component: &[u8]) -> bool {
    if component.is_empty()
        || (component.len() == 1 && component[0] == b'.')
        || (component.len() == 2 && component[0] == b'.' && component[1] == b'.')
    {
        return false;
    }
    let last = component[component.len() - 1];
    if last == b'.' || last == b' ' {
        return false;
    }
    let mut i = 0;
    let mut dot_pos = component.len();
    while i < component.len() {
        let b = component[i];
        if b < 0x20 || b == 0x7f || matches!(b, b'<' | b'>' | b'"' | b'|' | b'?' | b'*') {
            return false;
        }
        // UTF-8 C1 control characters U+0080..=U+009F (0xC2 0x80..=0x9F).
        if b == 0xC2 && i + 1 < component.len() && (0x80..=0x9F).contains(&component[i + 1]) {
            return false;
        }
        if b == b'.' && dot_pos == component.len() {
            dot_pos = i;
        }
        i += 1;
    }
    if eq_icase_4(component, b".git") || eq_icase_6(component, b".orbit") {
        return false;
    }
    let stem = &component[..dot_pos];
    !is_windows_reserved_device_stem(stem)
}

fn is_windows_reserved_device_stem(stem: &[u8]) -> bool {
    if eq_icase_3(stem, b"con")
        || eq_icase_3(stem, b"prn")
        || eq_icase_3(stem, b"aux")
        || eq_icase_3(stem, b"nul")
    {
        return true;
    }
    if stem.len() == 4 {
        let prefix_ok = eq_icase_3(&stem[..3], b"com") || eq_icase_3(&stem[..3], b"lpt");
        let digit_ok = matches!(stem[3], b'1'..=b'9');
        return prefix_ok && digit_ok;
    }
    false
}

/// Map an allowed image file extension (ASCII case-insensitive) to its MIME type.
pub fn image_mime_for_extension_bytes(ext: &[u8]) -> Option<&'static str> {
    if eq_icase_3(ext, b"png") {
        Some("image/png")
    } else if eq_icase_3(ext, b"jpg") || eq_icase_4(ext, b"jpeg") {
        Some("image/jpeg")
    } else if eq_icase_4(ext, b"webp") {
        Some("image/webp")
    } else if eq_icase_3(ext, b"gif") {
        Some("image/gif")
    } else {
        None
    }
}

/// Map an allowed image file extension (`&str` wrapper) to its MIME type.
pub fn image_mime_for_extension(ext: &str) -> Option<&'static str> {
    image_mime_for_extension_bytes(ext.as_bytes())
}

/// Return true if a byte-encoded path ends with a non-dotfile filename having
/// an allowed image extension.
pub fn is_allowed_image_path_bytes(path: &[u8]) -> bool {
    let mut name_start = 0;
    let mut i = 0;
    while i < path.len() {
        if path[i] == b'/' || path[i] == b'\\' {
            name_start = i + 1;
        }
        i += 1;
    }
    let file_name = &path[name_start..];
    if file_name.is_empty()
        || (file_name.len() == 1 && file_name[0] == b'.')
        || (file_name.len() == 2 && file_name[0] == b'.' && file_name[1] == b'.')
    {
        return false;
    }
    let mut last_dot = None;
    let mut j = 0;
    while j < file_name.len() {
        if file_name[j] == b'.' {
            last_dot = Some(j);
        }
        j += 1;
    }
    let Some(dot) = last_dot else {
        return false;
    };
    if dot == 0 {
        return false;
    }
    image_mime_for_extension_bytes(&file_name[dot + 1..]).is_some()
}

/// Return true if `path` has an allowed image file extension.
pub fn is_allowed_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(image_mime_for_extension)
        .is_some()
}

fn is_rollout_filename(path: &[u8]) -> bool {
    if path.len() == 13 {
        let target = b"rollout.jsonl";
        for i in 0..13 {
            if path[i] != target[i] {
                return false;
            }
        }
        return true;
    }
    if path.len() == 17 {
        let target = b"rollout.jsonl.zst";
        for i in 0..17 {
            if path[i] != target[i] {
                return false;
            }
        }
        return true;
    }
    false
}

/// Pure validator for relative file paths inside a Codex transcript archive (byte-slice form).
pub fn validate_codex_archive_path_bytes(path: &[u8]) -> Result<(), &'static str> {
    if is_rollout_filename(path) {
        return Ok(());
    }
    if path.len() > 7
        && path[0] == b'a'
        && path[1] == b's'
        && path[2] == b's'
        && path[3] == b'e'
        && path[4] == b't'
        && path[5] == b's'
        && path[6] == b'/'
    {
        let name = &path[7..];
        if is_safe_path_component_bytes(name) && is_allowed_image_path_bytes(name) {
            return Ok(());
        }
    }
    Err("Codex archive contains an unsafe file path.")
}

/// Pure validator for relative file paths inside a Codex transcript archive (`&str` wrapper).
pub fn validate_codex_archive_path(path: &str) -> Result<(), &'static str> {
    validate_codex_archive_path_bytes(path.as_bytes())
}

/// Allocation-free ASCII case-folding collision normalizer over fixed byte buffers.
pub fn normalize_collision_bytes<const N: usize>(
    input: &[u8],
    case_insensitive: bool,
) -> ([u8; N], usize) {
    let mut out = [0u8; N];
    let len = input.len().min(N);
    let mut i = 0;
    while i < len {
        out[i] = if case_insensitive {
            input[i].to_ascii_lowercase()
        } else {
            input[i]
        };
        i += 1;
    }
    (out, len)
}

/// Pure path normalizer for filesystem collision checks. On case-insensitive
/// filesystems (Windows and macOS default APFS/HFS+), folds case so paths
/// differing only by letter case are detected as collisions.
pub fn normalize_collision_path(path: &str, case_insensitive: bool) -> String {
    if case_insensitive {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

/// Parsed target kind for memory link resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTargetKind {
    Empty,
    ExternalScheme,
    TildeOnly,
    TildeSlash,
    Absolute,
    Relative,
}

fn starts_with_ignore_ascii_case(text: &str, prefix: &str) -> bool {
    text.as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// Classify a cleaned memory-link target string (after trimming `<`/`>` and
/// stripping `#fragment`/`?query`).
pub fn classify_memory_target(raw: &str) -> MemoryTargetKind {
    if raw.is_empty() {
        MemoryTargetKind::Empty
    } else if starts_with_ignore_ascii_case(raw, "http:")
        || starts_with_ignore_ascii_case(raw, "https:")
        || starts_with_ignore_ascii_case(raw, "mailto:")
    {
        MemoryTargetKind::ExternalScheme
    } else if raw == "~" {
        MemoryTargetKind::TildeOnly
    } else if raw.starts_with("~/") || raw.starts_with("~\\") {
        MemoryTargetKind::TildeSlash
    } else if Path::new(raw).is_absolute() {
        MemoryTargetKind::Absolute
    } else {
        MemoryTargetKind::Relative
    }
}

/// Resolve a memory link over abstract lexical path segments.
pub fn resolve_memory_link_segments(
    source_parent: Option<&[PathSeg]>,
    kind: MemoryTargetKind,
    tail: &[PathSeg],
    home: Option<&[PathSeg]>,
) -> Option<SegPath<5>> {
    match kind {
        MemoryTargetKind::Empty | MemoryTargetKind::ExternalScheme => None,
        MemoryTargetKind::TildeOnly => Some(normalize_lexical_segments::<5>(home?)),
        MemoryTargetKind::TildeSlash => {
            let home_segs = home?;
            let mut combined = SegPath::<5>::new();
            let mut i = 0;
            while i < home_segs.len() {
                combined.push(home_segs[i]);
                i += 1;
            }
            let mut j = 0;
            while j < tail.len() {
                combined.push(tail[j]);
                j += 1;
            }
            Some(normalize_lexical_segments::<5>(combined.as_slice()))
        }
        MemoryTargetKind::Absolute => {
            let mut combined = SegPath::<5>::new();
            combined.push(PathSeg::Root);
            let mut j = 0;
            while j < tail.len() {
                combined.push(tail[j]);
                j += 1;
            }
            Some(normalize_lexical_segments::<5>(combined.as_slice()))
        }
        MemoryTargetKind::Relative => {
            let parent = source_parent?;
            let mut combined = SegPath::<5>::new();
            let mut i = 0;
            while i < parent.len() {
                combined.push(parent[i]);
                i += 1;
            }
            let mut j = 0;
            while j < tail.len() {
                combined.push(tail[j]);
                j += 1;
            }
            Some(normalize_lexical_segments::<5>(combined.as_slice()))
        }
    }
}
