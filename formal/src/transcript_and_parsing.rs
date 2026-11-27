//! Kani proof harnesses directly verifying `orbit_core::transcript_and_parsing`.

#[cfg(kani)]
mod verification {
    use orbit_core::transcript_and_parsing::{
        extract_frontmatter_body_bytes, parse_checksum, parse_checksum_bytes, read_probe_limit,
    };

    fn nibble_to_hex(n: u8, uppercase: bool) -> u8 {
        if n < 10 {
            b'0' + n
        } else if uppercase {
            b'A' + (n - 10)
        } else {
            b'a' + (n - 10)
        }
    }

    fn encode_hex_32(bytes: &[u8; 32], uppercase: bool) -> [u8; 64] {
        let mut out = [0u8; 64];
        for i in 0..32 {
            out[i * 2] = nibble_to_hex(bytes[i] >> 4, uppercase);
            out[i * 2 + 1] = nibble_to_hex(bytes[i] & 0x0f, uppercase);
        }
        out
    }

    #[kani::proof]
    fn proof_parse_checksum_panic_free_and_roundtrip() {
        let mode: u8 = kani::any();
        match mode % 5 {
            0 => {
                let mut digest = [0x5Au8; 32];
                let idx: usize = kani::any();
                kani::assume(idx < 32);
                let sym_byte: u8 = kani::any();
                digest[idx] = sym_byte;
                let uppercase: bool = kani::any();
                let hex = encode_hex_32(&digest, uppercase);
                let decoded = parse_checksum_bytes(&hex).unwrap();
                assert_eq!(decoded[idx], sym_byte);
            }
            1 => {
                let mut corrupted = [b'a'; 64];
                let pos: usize = kani::any();
                kani::assume(pos < 64);
                let bad_byte: u8 = kani::any();
                kani::assume(!bad_byte.is_ascii_hexdigit());
                corrupted[pos] = bad_byte;
                assert!(parse_checksum_bytes(&corrupted).is_err());
            }
            2 => {
                let buf = [b'0'; 64];
                let short_len: usize = kani::any();
                kani::assume(short_len < 64);
                assert!(parse_checksum_bytes(&buf[..short_len]).is_err());
            }
            3 => {
                let buf = [b'0'; 68];
                let long_len: usize = kani::any();
                kani::assume((65..=68).contains(&long_len));
                assert!(parse_checksum_bytes(&buf[..long_len]).is_err());
            }
            _ => {
                let mut multibyte = [0xC3u8; 64];
                multibyte[1] = 0xA9;
                assert!(parse_checksum_bytes(&multibyte).is_err());
                // Valid UTF-8 `&str` boundary cases: 64 bytes (63 chars) and 64 bytes (32 chars).
                assert!(parse_checksum(
                    "é00000000000000000000000000000000000000000000000000000000000000"
                )
                .is_err());
                assert!(parse_checksum("éééééééééééééééééééééééééééééééé").is_err());
                assert!(parse_checksum(
                    "0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789ABCDEF"
                )
                .is_ok());
            }
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
