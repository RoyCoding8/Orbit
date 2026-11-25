//! Pure credential classification, CLI flag detection, HTTP/SSE scheme validation,
//! and bounded mask-restoration logic shared between `src-tauri` and `formal/`.

/// Placeholder string used to mask secret values in UI-facing configuration views.
pub const MASK: &str = "••••••••";

fn contains_icase_const<const M: usize>(haystack: &[u8], needle: &[u8; M]) -> bool {
    if haystack.len() < M {
        return false;
    }
    let mut i = 0;
    while i + M <= haystack.len() {
        let mut matched = true;
        for j in 0..M {
            if haystack[i + j].to_ascii_lowercase() != needle[j] {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
        i += 1;
    }
    false
}

fn eq_icase_const<const M: usize>(a: &[u8], b: &[u8; M]) -> bool {
    if a.len() != M {
        return false;
    }
    let mut i = 0;
    while i < M {
        if a[i].to_ascii_lowercase() != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Return true if a field/header/flag name indicates a credential (byte-slice form).
pub fn secret_name_bytes(bytes: &[u8]) -> bool {
    contains_icase_const(bytes, b"key")
        || contains_icase_const(bytes, b"token")
        || contains_icase_const(bytes, b"secret")
        || contains_icase_const(bytes, b"auth")
        || contains_icase_const(bytes, b"password")
        || contains_icase_const(bytes, b"credential")
}

/// Return true if a field/header/flag name indicates a credential (`&str` wrapper).
pub fn secret_name(name: &str) -> bool {
    secret_name_bytes(name.as_bytes())
}

/// Return true if a value's prefix/shape looks like a bearer token or API key (byte-slice form).
pub fn value_shape_secret_bytes(bytes: &[u8]) -> bool {
    contains_icase_const(bytes, b"bearer ")
        || (bytes.len() >= 3 && bytes[0] == b's' && bytes[1] == b'k' && bytes[2] == b'-')
        || (bytes.len() >= 4
            && bytes[0] == b'g'
            && bytes[1] == b'h'
            && bytes[2] == b'p'
            && bytes[3] == b'_')
        || (bytes.len() >= 11 && eq_icase_const(&bytes[..11], b"github_pat_"))
}

/// Return true if a value's prefix/shape looks like a bearer token or API key (`&str` wrapper).
pub fn value_shape_secret(text: &str) -> bool {
    value_shape_secret_bytes(text.as_bytes())
}

/// Return true if a CLI argument names its successor as a credential value
/// (e.g. `--api-key` or `-token`), requiring a leading `-` or `--` (byte-slice form).
pub fn credential_flag_bytes(bytes: &[u8]) -> bool {
    let flag = if bytes.len() >= 2 && bytes[0] == b'-' && bytes[1] == b'-' {
        &bytes[2..]
    } else if !bytes.is_empty() && bytes[0] == b'-' {
        &bytes[1..]
    } else {
        return false;
    };
    if flag.is_empty() {
        return false;
    }
    let mut i = 0;
    while i < flag.len() {
        let b = flag[i];
        if !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return false;
        }
        i += 1;
    }
    secret_name_bytes(flag)
}

/// Return true if a CLI argument names its successor as a credential value (`&str` wrapper).
pub fn credential_flag(text: &str) -> bool {
    credential_flag_bytes(text.as_bytes())
}

fn split_scheme_bytes(url: &[u8]) -> Option<(&[u8], &[u8])> {
    if url.len() < 4 {
        return None;
    }
    let mut i = 0;
    while i + 3 <= url.len() {
        if url[i] == b':' && url[i + 1] == b'/' && url[i + 2] == b'/' {
            return Some((&url[..i], &url[i + 3..]));
        }
        i += 1;
    }
    None
}

/// Return true if `url` uses the `https://` scheme (ASCII case-insensitive) with a non-empty rest.
pub fn is_https_scheme_bytes(url: &[u8]) -> bool {
    let Some((scheme, rest)) = split_scheme_bytes(url) else {
        return false;
    };
    !rest.is_empty() && eq_icase_const(scheme, b"https")
}

/// Return true if `url` uses the `https://` scheme (`&str` wrapper).
pub fn is_https_scheme(url: &str) -> bool {
    is_https_scheme_bytes(url.as_bytes())
}

/// Return true if `url` uses `http://` targeting a loopback host (`localhost`, `127.0.0.1`, `[::1]`).
pub fn is_loopback_http_url_bytes(url: &[u8]) -> bool {
    let Some((scheme, rest)) = split_scheme_bytes(url) else {
        return false;
    };
    if !eq_icase_const(scheme, b"http") {
        return false;
    }
    let mut auth_end = rest.len();
    let mut i = 0;
    while i < rest.len() {
        if matches!(rest[i], b'/' | b'?' | b'#') {
            auth_end = i;
            break;
        }
        i += 1;
    }
    let authority = &rest[..auth_end];
    let mut host_start = 0;
    let mut j = 0;
    while j < authority.len() {
        if authority[j] == b'@' {
            host_start = j + 1;
        }
        j += 1;
    }
    let host_port = &authority[host_start..];
    let host = if !host_port.is_empty() && host_port[0] == b'[' {
        let after_open = &host_port[1..];
        let mut close_pos = None;
        let mut k = 0;
        while k < after_open.len() {
            if after_open[k] == b']' {
                close_pos = Some(k);
                break;
            }
            k += 1;
        }
        let Some(pos) = close_pos else {
            return false;
        };
        &after_open[..pos]
    } else {
        let mut colon = host_port.len();
        let mut k = 0;
        while k < host_port.len() {
            if host_port[k] == b':' {
                colon = k;
                break;
            }
            k += 1;
        }
        &host_port[..colon]
    };
    eq_icase_const(host, b"localhost")
        || eq_icase_const(host, b"127.0.0.1")
        || eq_icase_const(host, b"::1")
}

/// Return true if `url` uses `http://` targeting a loopback host (`&str` wrapper).
pub fn is_loopback_http_url(url: &str) -> bool {
    is_loopback_http_url_bytes(url.as_bytes())
}

/// Remote MCP transport policy: requires HTTPS or a loopback HTTP address (byte-slice form).
pub fn is_allowed_mcp_url_bytes(url: &[u8]) -> bool {
    is_https_scheme_bytes(url) || is_loopback_http_url_bytes(url)
}

/// Remote MCP transport policy (`&str` wrapper).
pub fn is_allowed_mcp_url(url: &str) -> bool {
    is_allowed_mcp_url_bytes(url.as_bytes())
}

/// Return true if an HTTP `Content-Type` header specifies `text/event-stream`
/// (ASCII case-insensitive, ignoring optional `;` parameters) (byte-slice form).
pub fn is_event_stream_content_type_bytes(content_type: &[u8]) -> bool {
    let mut semi = content_type.len();
    let mut i = 0;
    while i < content_type.len() {
        if content_type[i] == b';' {
            semi = i;
            break;
        }
        i += 1;
    }
    let media = trim_ascii_ws(&content_type[..semi]);
    eq_icase_const(media, b"text/event-stream")
}

/// Return true if an HTTP `Content-Type` header specifies `text/event-stream` (`&str` wrapper).
pub fn is_event_stream_content_type(content_type: &str) -> bool {
    is_event_stream_content_type_bytes(content_type.as_bytes())
}

fn trim_ascii_ws(mut s: &[u8]) -> &[u8] {
    while !s.is_empty() && s[0].is_ascii_whitespace() {
        s = &s[1..];
    }
    while !s.is_empty() && s[s.len() - 1].is_ascii_whitespace() {
        s = &s[..s.len() - 1];
    }
    s
}

/// Pure bounded JSON-like value model for credential mask restoration.
pub const MASK_TOKEN: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafVal {
    Str(u8),
    NonStr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigNode {
    Leaf(LeafVal),
    Array([LeafVal; 2]),
    Object([Option<(u8, LeafVal)>; 2]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreError {
    MaskInArray,
    FreshPlaceholderRemaining,
}

pub fn leaf_is_mask(v: LeafVal) -> bool {
    matches!(v, LeafVal::Str(MASK_TOKEN))
}

pub fn contains_mask(node: &ConfigNode) -> bool {
    match *node {
        ConfigNode::Leaf(v) => leaf_is_mask(v),
        ConfigNode::Array(items) => leaf_is_mask(items[0]) || leaf_is_mask(items[1]),
        ConfigNode::Object(slots) => {
            slots[0].is_some_and(|(_, v)| leaf_is_mask(v))
                || slots[1].is_some_and(|(_, v)| leaf_is_mask(v))
        }
    }
}

/// Restore masked placeholders from `stored` into `edited`, rejecting masks in
/// arrays and any fresh/unrestored mask placeholder remaining afterwards.
pub fn restore_and_validate_entry(
    stored: Option<&ConfigNode>,
    edited: &mut ConfigNode,
) -> Result<(), RestoreError> {
    if let Some(stored) = stored {
        restore_masked_node(stored, edited)?;
    }
    if contains_mask(edited) {
        return Err(RestoreError::FreshPlaceholderRemaining);
    }
    Ok(())
}

fn restore_masked_node(stored: &ConfigNode, edited: &mut ConfigNode) -> Result<(), RestoreError> {
    match edited {
        ConfigNode::Leaf(v) if leaf_is_mask(*v) => {
            if let ConfigNode::Leaf(LeafVal::Str(secret)) = *stored {
                *v = LeafVal::Str(secret);
            }
            Ok(())
        }
        ConfigNode::Array(items) => {
            if leaf_is_mask(items[0]) || leaf_is_mask(items[1]) {
                return Err(RestoreError::MaskInArray);
            }
            Ok(())
        }
        ConfigNode::Object(updated_slots) => {
            if let ConfigNode::Object(stored_slots) = stored {
                let mut i = 0;
                while i < 2 {
                    if let Some((key, child)) = &mut updated_slots[i] {
                        if leaf_is_mask(*child) {
                            let mut j = 0;
                            while j < 2 {
                                if let Some((s_key, s_val)) = stored_slots[j] {
                                    if s_key == *key {
                                        if let LeafVal::Str(secret) = s_val {
                                            *child = LeafVal::Str(secret);
                                        }
                                        break;
                                    }
                                }
                                j += 1;
                            }
                        }
                    }
                    i += 1;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
