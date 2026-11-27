//! Kani proof harnesses directly verifying `orbit_core::transcript_and_parsing`.

#[cfg(kani)]
mod verification {
    use orbit_core::transcript_and_parsing::{
        extract_frontmatter_body_bytes, parse_checksum_bytes, read_probe_limit,
    };

    #[kani::proof]
    #[kani::unwind(66)]
    fn proof_parse_checksum_panic_free_and_roundtrip() {
        if kani::any() {
            let raw: [u8; 64] = kani::any();
            let decoded = parse_checksum_bytes(&raw);
            assert_eq!(decoded.is_ok(), raw.iter().all(u8::is_ascii_hexdigit));
            if let Ok(digest) = decoded {
                let hex = b"0123456789abcdef";
                for i in 0..digest.len() {
                    assert_eq!(
                        hex[usize::from(digest[i] >> 4)],
                        raw[i * 2].to_ascii_lowercase()
                    );
                    assert_eq!(
                        hex[usize::from(digest[i] & 15)],
                        raw[i * 2 + 1].to_ascii_lowercase()
                    );
                }
            }
        } else {
            let raw: [u8; 65] = kani::any();
            let len: usize = kani::any();
            kani::assume(len <= raw.len() && len != 64);
            assert!(parse_checksum_bytes(&raw[..len]).is_err());
        }
    }

    #[kani::proof]
    #[kani::unwind(9)]
    fn proof_extract_frontmatter_body_invariants() {
        let with_bom: bool = kani::any();
        let with_crlf: bool = kani::any();
        let with_trail_ws: bool = kani::any();
        let valid_open: bool = kani::any();
        let has_close: bool = kani::any();

        let body_byte: u8 = kani::any();
        kani::assume(body_byte != b'\r' && body_byte != b'\n');
        let mut buf = [0u8; 16];
        let mut pos = 0usize;

        if with_bom {
            buf[pos] = 0xEF;
            buf[pos + 1] = 0xBB;
            buf[pos + 2] = 0xBF;
            pos += 3;
        }
        if valid_open {
            buf[pos] = b'-';
            buf[pos + 1] = b'-';
            buf[pos + 2] = b'-';
        } else {
            buf[pos] = b'a';
            buf[pos + 1] = b'b';
            buf[pos + 2] = b'c';
        }
        pos += 3;
        if with_trail_ws {
            buf[pos] = b' ';
            pos += 1;
        }
        if with_crlf {
            buf[pos] = b'\r';
            pos += 1;
        }
        buf[pos] = b'\n';
        pos += 1;

        buf[pos] = body_byte;
        pos += 1;
        if with_crlf {
            buf[pos] = b'\r';
            pos += 1;
        }
        buf[pos] = b'\n';
        pos += 1;

        if has_close {
            buf[pos] = b'-';
            buf[pos + 1] = b'-';
            buf[pos + 2] = b'-';
            pos += 3;
            if with_trail_ws {
                buf[pos] = b' ';
                pos += 1;
            }
        }

        let extracted = extract_frontmatter_body_bytes(&buf[..pos]);
        if valid_open && has_close {
            let body = extracted.unwrap();
            assert_eq!(body.len(), 1);
            assert_eq!(body[0], body_byte);
        } else {
            assert!(extracted.is_none());
        }
    }

    #[kani::proof]
    fn proof_read_probe_limit() {
        let maximum: u64 = kani::any();
        assert_eq!(
            read_probe_limit(maximum),
            maximum.checked_add(1).unwrap_or(u64::MAX)
        );
    }
}
