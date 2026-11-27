//! Kani proof harnesses directly verifying `orbit_core::credentials`.

#[cfg(kani)]
mod verification {
    use orbit_core::credentials::{
        credential_flag_bytes, is_event_stream_content_type_bytes, is_https_scheme_bytes,
        secret_name_bytes, value_shape_secret_bytes,
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
        // Symbolic case variations of `text/event-stream` with optional `;a=b`.
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
