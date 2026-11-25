//! Kani proof harnesses directly verifying `orbit_core::credentials`.

#[cfg(kani)]
mod verification {
    use orbit_core::credentials::{
        contains_mask, credential_flag_bytes, is_allowed_mcp_url_bytes,
        is_event_stream_content_type_bytes, is_https_scheme_bytes, restore_and_validate_entry,
        secret_name_bytes, value_shape_secret_bytes, ConfigNode, LeafVal, RestoreError, MASK_TOKEN,
    };

    #[kani::proof]
    #[kani::unwind(9)]
    fn proof_credential_flag_requires_dash_prefix_and_no_positional_false_positives() {
        let mode: u8 = kani::any();
        match mode & 3 {
            0 => {
                // Any non-dash-prefixed positional arg of length 1..=6 MUST return false.
                let len: usize = kani::any();
                kani::assume((1..=6).contains(&len));
                let mut buf = [0u8; 6];
                for i in 0..len {
                    buf[i] = kani::any();
                }
                kani::assume(buf[0] != b'-');
                assert!(!credential_flag_bytes(&buf[..len]));
            }
            1 => {
                // Valid vs invalid character flag check (`--key` vs `--key=`, `-key` vs `-key `).
                let prefix_double: bool = kani::any();
                let bad_char: bool = kani::any();
                let mut flag_buf = [0u8; 6];
                let flag_len = if prefix_double {
                    flag_buf[0] = b'-';
                    flag_buf[1] = b'-';
                    flag_buf[2] = b'k';
                    flag_buf[3] = b'e';
                    flag_buf[4] = b'y';
                    if bad_char {
                        flag_buf[5] = b'=';
                        6
                    } else {
                        5
                    }
                } else {
                    flag_buf[0] = b'-';
                    flag_buf[1] = b'k';
                    flag_buf[2] = b'e';
                    flag_buf[3] = b'y';
                    if bad_char {
                        flag_buf[4] = b' ';
                        5
                    } else {
                        4
                    }
                };
                assert_eq!(credential_flag_bytes(&flag_buf[..flag_len]), !bad_char);
            }
            2 => {
                // Bare dashes and non-secret flags rejected.
                assert!(!credential_flag_bytes(b"-"));
                assert!(!credential_flag_bytes(b"--"));
                assert!(!credential_flag_bytes(b"--port"));
            }
            _ => {
                // Valid secret flags accepted regardless of ASCII case.
                let upper: bool = kani::any();
                let mut flag = *b"--auth";
                if upper {
                    flag[2] = b'A';
                    flag[3] = b'U';
                    flag[4] = b'T';
                    flag[5] = b'H';
                }
                assert!(credential_flag_bytes(&flag));
            }
        }
    }

    #[kani::proof]
    #[kani::unwind(24)]
    fn proof_scheme_and_sse_media_type_case_insensitivity() {
        // 1. Symbolic case variations of `https://x.io`.
        let mask: u8 = kani::any();
        let mut url = *b"https://x.io";
        for i in 0..5 {
            if (mask & (1 << i)) != 0 {
                url[i] = url[i].to_ascii_uppercase();
            }
        }
        assert!(is_https_scheme_bytes(&url));
        assert!(is_allowed_mcp_url_bytes(&url));

        // 2. Plain HTTP is allowed ONLY for loopback hosts, never remote hosts.
        assert!(!is_allowed_mcp_url_bytes(b"http://x.io/m"));
        assert!(!is_allowed_mcp_url_bytes(b"HTTP://10.0.0.1"));
        assert!(!is_allowed_mcp_url_bytes(b"ftp://localhost"));
        assert!(is_allowed_mcp_url_bytes(b"http://localhost:80"));
        assert!(is_allowed_mcp_url_bytes(b"http://127.0.0.1"));
        assert!(is_allowed_mcp_url_bytes(b"http://[::1]:80"));

        // 3. Symbolic case variations of `text/event-stream` with optional `;a=b`.
        let sse_mask: u32 = kani::any();
        let with_param: bool = kani::any();
        let mut ct = [0u8; 21];
        let base = b"text/event-stream";
        for i in 0..17 {
            let b = base[i];
            ct[i] = if b.is_ascii_alphabetic() && ((sse_mask >> i) & 1) != 0 {
                b.to_ascii_uppercase()
            } else {
                b
            };
        }
        let ct_len = if with_param {
            ct[17..21].copy_from_slice(b";a=b");
            21
        } else {
            17
        };
        assert!(is_event_stream_content_type_bytes(&ct[..ct_len]));

        assert!(!is_event_stream_content_type_bytes(b"application/json"));
        assert!(!is_event_stream_content_type_bytes(b"text/event-stream-x"));
        assert!(!is_event_stream_content_type_bytes(
            b"text/plain;text/event-stream"
        ));
    }

    #[kani::proof]
    #[kani::unwind(4)]
    fn proof_masked_placeholder_restoration_and_fresh_rejection() {
        let stored_secret: u8 = kani::any();
        kani::assume(stored_secret != MASK_TOKEN);
        let stored_is_str: bool = kani::any();
        let stored_val = if stored_is_str {
            LeafVal::Str(stored_secret)
        } else {
            LeafVal::NonStr
        };
        let stored = ConfigNode::Object([Some((1, stored_val)), None]);

        let edit_k1_masked: bool = kani::any();
        let edit_k1_new: u8 = kani::any();
        kani::assume(edit_k1_new != MASK_TOKEN);
        let edit_k2_present: bool = kani::any();
        let edit_k2_masked: bool = kani::any();
        let edit_k2_val: u8 = kani::any();
        kani::assume(edit_k2_val != MASK_TOKEN);

        let mut edited = ConfigNode::Object([
            Some((
                1,
                LeafVal::Str(if edit_k1_masked {
                    MASK_TOKEN
                } else {
                    edit_k1_new
                }),
            )),
            if edit_k2_present {
                Some((
                    2,
                    LeafVal::Str(if edit_k2_masked {
                        MASK_TOKEN
                    } else {
                        edit_k2_val
                    }),
                ))
            } else {
                None
            },
        ]);

        let res = restore_and_validate_entry(Some(&stored), &mut edited);

        let fresh_mask = (edit_k2_present && edit_k2_masked) || (edit_k1_masked && !stored_is_str);
        if fresh_mask {
            assert_eq!(res, Err(RestoreError::FreshPlaceholderRemaining));
        } else {
            assert!(res.is_ok());
            assert!(!contains_mask(&edited));
            if let ConfigNode::Object(slots) = edited {
                let (_, v1) = slots[0].unwrap();
                if edit_k1_masked {
                    assert_eq!(v1, LeafVal::Str(stored_secret));
                } else {
                    assert_eq!(v1, LeafVal::Str(edit_k1_new));
                }
            }
        }

        let arr_mask_pos: u8 = kani::any();
        kani::assume(arr_mask_pos < 2);
        let mut arr = ConfigNode::Array([
            LeafVal::Str(if arr_mask_pos == 0 { MASK_TOKEN } else { 7 }),
            LeafVal::Str(if arr_mask_pos == 1 { MASK_TOKEN } else { 8 }),
        ]);
        let stored_arr = ConfigNode::Array([LeafVal::Str(42), LeafVal::Str(43)]);
        assert_eq!(
            restore_and_validate_entry(Some(&stored_arr), &mut arr),
            Err(RestoreError::MaskInArray)
        );
    }

    #[kani::proof]
    #[kani::unwind(15)]
    fn proof_secret_key_and_value_classification() {
        let choice: u8 = kani::any();
        let mask: u16 = kani::any();
        let mut buf = [0u8; 10];
        let len = match choice % 6 {
            0 => {
                buf[..3].copy_from_slice(b"key");
                3
            }
            1 => {
                buf[..5].copy_from_slice(b"token");
                5
            }
            2 => {
                buf[..6].copy_from_slice(b"secret");
                6
            }
            3 => {
                buf[..4].copy_from_slice(b"auth");
                4
            }
            4 => {
                buf[..8].copy_from_slice(b"password");
                8
            }
            _ => {
                buf[..10].copy_from_slice(b"credential");
                10
            }
        };
        for i in 0..len {
            if ((mask >> i) & 1) != 0 {
                buf[i] = buf[i].to_ascii_uppercase();
            }
        }
        assert!(secret_name_bytes(&buf[..len]));
        assert!(!secret_name_bytes(b"timeout"));
        assert!(!secret_name_bytes(b"hostname"));

        let bmask: u8 = kani::any();
        let mut bearer = *b"bearer xyz";
        for i in 0..6 {
            if (bmask & (1 << i)) != 0 {
                bearer[i] = bearer[i].to_ascii_uppercase();
            }
        }
        assert!(value_shape_secret_bytes(&bearer));
        assert!(value_shape_secret_bytes(b"sk-1234"));
        assert!(value_shape_secret_bytes(b"ghp_abcd"));
        assert!(value_shape_secret_bytes(b"github_pat_123"));
        assert!(!value_shape_secret_bytes(b"plain-val"));
    }
}
