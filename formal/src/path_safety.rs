//! Kani safety checks for production path predicates.

#[cfg(kani)]
mod verification {
    use orbit_core::path_safety::{
        is_safe_path_component_bytes, is_valid_relative_component,
        validate_codex_archive_path_bytes, validate_relative_path_bytes,
    };

    #[kani::proof]
    #[kani::unwind(8)]
    fn proof_is_safe_path_component_and_windows_device_rejection() {
        // 1. Symbolic bounded byte string for `is_safe_path_component_bytes`.
        let len: usize = kani::any();
        kani::assume(len <= 4);
        let mut buf = [0u8; 4];
        for i in 0..len {
            buf[i] = kani::any();
        }
        let slice = &buf[..len];
        let safe = is_safe_path_component_bytes(slice);
        if safe {
            assert!(!slice.is_empty());
            assert!(!(slice.len() == 1 && slice[0] == b'.'));
            assert!(!(slice.len() == 2 && slice[0] == b'.' && slice[1] == b'.'));
            for &b in slice {
                assert!(b != b'/' && b != b'\\' && b != b':' && b != 0);
            }
        }

        // 2. Symbolic Windows reserved device names (case variations & optional extension).
        let use_com_lpt: bool = kani::any();
        let upper_mask: u8 = kani::any();
        let with_ext: bool = kani::any();
        let mut dev = [0u8; 6];
        let dev_len;
        if use_com_lpt {
            let base = if kani::any() { *b"com" } else { *b"lpt" };
            let digit: u8 = kani::any();
            kani::assume((b'1'..=b'9').contains(&digit));
            for i in 0..3 {
                dev[i] = if (upper_mask & (1 << i)) != 0 {
                    base[i].to_ascii_uppercase()
                } else {
                    base[i]
                };
            }
            dev[3] = digit;
            if with_ext {
                dev[4] = b'.';
                dev[5] = b'a';
                dev_len = 6;
            } else {
                dev_len = 4;
            }
        } else {
            let base = match kani::any::<u8>() & 3 {
                0 => *b"con",
                1 => *b"prn",
                2 => *b"aux",
                _ => *b"nul",
            };
            for i in 0..3 {
                dev[i] = if (upper_mask & (1 << i)) != 0 {
                    base[i].to_ascii_uppercase()
                } else {
                    base[i]
                };
            }
            if with_ext {
                dev[3] = b'.';
                dev[4] = b'a';
                dev_len = 5;
            } else {
                dev_len = 3;
            }
        }
        assert!(!is_valid_relative_component(&dev[..dev_len]));

        // 3. ADS (`:`), traversal (`..`), and `.git`/`.orbit` are always rejected.
        assert!(validate_relative_path_bytes(b"a:s").is_err());
        assert!(validate_relative_path_bytes(b"a/../b").is_err());
        assert!(validate_relative_path_bytes(b".GIT/c").is_err());
        assert!(validate_relative_path_bytes(b".Orbit").is_err());
        assert!(validate_relative_path_bytes(b"a/b.md").is_ok());
    }

    #[kani::proof]
    #[kani::unwind(18)]
    fn proof_codex_archive_path_and_image_extension() {
        let ext_choice: u8 = kani::any();
        let upper_mask: u8 = kani::any();
        let mut ext_buf = [0u8; 4];
        let (ext_len, expected_ok) = match ext_choice % 7 {
            0 => {
                ext_buf[..3].copy_from_slice(b"png");
                (3, true)
            }
            1 => {
                ext_buf[..3].copy_from_slice(b"jpg");
                (3, true)
            }
            2 => {
                ext_buf[..4].copy_from_slice(b"jpeg");
                (4, true)
            }
            3 => {
                ext_buf[..4].copy_from_slice(b"webp");
                (4, true)
            }
            4 => {
                ext_buf[..3].copy_from_slice(b"gif");
                (3, true)
            }
            5 => {
                ext_buf[..3].copy_from_slice(b"svg");
                (3, false)
            }
            _ => {
                ext_buf[..3].copy_from_slice(b"exe");
                (3, false)
            }
        };
        for i in 0..ext_len {
            if (upper_mask & (1 << i)) != 0 {
                ext_buf[i] = ext_buf[i].to_ascii_uppercase();
            }
        }
        let mut path_buf = [0u8; 13];
        path_buf[..9].copy_from_slice(b"assets/a.");
        path_buf[9..9 + ext_len].copy_from_slice(&ext_buf[..ext_len]);

        assert_eq!(
            validate_codex_archive_path_bytes(&path_buf[..9 + ext_len]).is_ok(),
            expected_ok
        );

        assert!(validate_codex_archive_path_bytes(b"rollout.jsonl").is_ok());
        assert!(validate_codex_archive_path_bytes(b"rollout.jsonl.zst").is_ok());
        assert!(validate_codex_archive_path_bytes(b"assets/../a.png").is_err());
        assert!(validate_codex_archive_path_bytes(b"assets/b/a.png").is_err());
        assert!(validate_codex_archive_path_bytes(b"assets/.png").is_err());
        assert!(validate_codex_archive_path_bytes(b"assets/a.png:s").is_err());
        assert!(validate_codex_archive_path_bytes(b"other/a.png").is_err());
    }
}
