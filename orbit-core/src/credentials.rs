//! Credential classification, CLI flag detection, HTTPS and SSE media-type checks.

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
