//! Kani proof harnesses directly verifying `orbit_core::transcript_and_parsing`.

#[cfg(kani)]
mod verification {
    use orbit_core::transcript_and_parsing::{
        encode_hex_32, extract_frontmatter_body_bytes, format_unique_title_buf,
        order_spine_nodes_bounded, parse_checksum_bytes, read_bounded_stream,
        truncate_utf8_bounded, unique_title_slot, SpineNodeIdx,
    };

    #[kani::proof]
    fn proof_parse_checksum_panic_free_and_roundtrip() {
        let mode: u8 = kani::any();
        match mode % 4 {
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
            _ => {
                let mut multibyte = [0xC3u8; 64];
                multibyte[1] = 0xA9;
                assert!(parse_checksum_bytes(&multibyte).is_err());
            }
        }
    }

    #[kani::proof]
    #[kani::unwind(17)]
    fn proof_extract_frontmatter_body_invariants() {
        let with_bom: bool = kani::any();
        let with_crlf: bool = kani::any();
        let with_trail_ws: bool = kani::any();
        let valid_open: bool = kani::any();
        let has_close: bool = kani::any();

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

        buf[pos] = b'k';
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
            assert_eq!(body[0], b'k');
        } else {
            assert!(extracted.is_none());
        }
    }

    #[kani::proof]
    #[kani::unwind(7)]
    fn proof_order_spine_nodes_cycles_uniqueness_and_parent_order() {
        let n: usize = kani::any();
        kani::assume(n <= 3);

        let mut nodes = [SpineNodeIdx {
            id: 0,
            parent: None,
            position: 0,
        }; 3];
        for i in 0..n {
            let has_parent: bool = kani::any();
            let parent = if has_parent && n > 0 {
                let p: usize = kani::any();
                kani::assume(p < n);
                Some(p)
            } else {
                None
            };
            let pos: usize = kani::any();
            kani::assume(pos < 3);
            nodes[i] = SpineNodeIdx {
                id: i,
                parent,
                position: pos,
            };
        }

        let (ordered, ord_len) = order_spine_nodes_bounded(&nodes, n);

        assert_eq!(ord_len, n);
        let mut counts = [0u8; 3];
        let mut rank = [0usize; 3];
        let mut depth_of = [0usize; 3];
        for idx in 0..ord_len {
            let (id, depth, _branch) = ordered[idx];
            assert!(id < n);
            counts[id] += 1;
            rank[id] = idx;
            depth_of[id] = depth;
        }
        for i in 0..n {
            assert_eq!(counts[i], 1);
        }

        for i in 0..n {
            if let Some(p) = nodes[i].parent {
                let mut cur = p;
                let mut reaches_root = false;
                for _ in 0..3 {
                    match nodes[cur].parent {
                        None => {
                            reaches_root = true;
                            break;
                        }
                        Some(next) => cur = next,
                    }
                }
                if reaches_root {
                    assert!(rank[p] < rank[i]);
                    assert_eq!(depth_of[i], depth_of[p] + 1);
                }
            }
        }
    }

    #[kani::proof]
    #[kani::unwind(10)]
    fn proof_unique_title_collision_suffixing() {
        let mask: u8 = kani::any();
        kani::assume(mask < 16);

        let slot = unique_title_slot(mask);
        assert!(slot <= 4);
        assert_eq!((mask >> slot) & 1, 0);
        for s in 0..slot {
            assert_eq!((mask >> s) & 1, 1);
        }

        let (title, title_len) = format_unique_title_buf(b"chat", slot);
        if (mask & 1) == 0 {
            assert_eq!(&title[..title_len], b"chat");
        } else {
            assert_eq!(title_len, 6);
            assert_eq!(&title[..5], b"chat-");
            assert_eq!(title[5], b'0' + slot);
        }
    }

    #[kani::proof]
    #[kani::unwind(10)]
    fn proof_bounded_decompression_and_truncation() {
        let stream_len: u64 = kani::any();
        let max_bytes: u64 = kani::any();
        kani::assume(max_bytes < u64::MAX);

        match read_bounded_stream(stream_len, max_bytes) {
            Ok((read_bytes, out_len)) => {
                assert!(stream_len <= max_bytes);
                assert_eq!(out_len, stream_len);
                assert!(read_bytes <= max_bytes);
            }
            Err(read_bytes) => {
                assert!(stream_len > max_bytes);
                assert_eq!(read_bytes, max_bytes + 1);
            }
        }

        let sample = "aé🦀z";
        let limit: usize = kani::any();
        kani::assume(limit <= 10);
        let truncated = truncate_utf8_bounded(sample, limit);
        assert!(truncated.len() <= limit);
        assert!(sample.starts_with(truncated));
        assert!(sample.is_char_boundary(truncated.len()));
    }
}
