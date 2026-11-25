//! Kani proof harnesses directly verifying `orbit_core::path_safety`.

#[cfg(kani)]
mod verification {
    use orbit_core::path_safety::{
        backup_path_is_contained, contains_seg, is_safe_path_component_bytes,
        is_valid_relative_component, normalize_collision_bytes, normalize_lexical_segments,
        path_is_within_segments, resolve_memory_link_segments, segs_start_with,
        validate_codex_archive_path_bytes, validate_relative_path_bytes, MemoryTargetKind, PathSeg,
        SEG_BACKUPS, SEG_ORBIT,
    };

    fn any_path_seg() -> PathSeg {
        match kani::any::<u8>() & 3 {
            0 => PathSeg::Root,
            1 => PathSeg::Cur,
            2 => PathSeg::Parent,
            _ => PathSeg::Normal(kani::any::<u8>() & 3),
        }
    }

    fn any_memory_target_kind() -> MemoryTargetKind {
        match kani::any::<u8>() % 6 {
            0 => MemoryTargetKind::Empty,
            1 => MemoryTargetKind::ExternalScheme,
            2 => MemoryTargetKind::TildeOnly,
            3 => MemoryTargetKind::TildeSlash,
            4 => MemoryTargetKind::Absolute,
            _ => MemoryTargetKind::Relative,
        }
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn proof_path_is_within_rejects_traversal_and_enforces_prefix() {
        let root_len: usize = kani::any();
        kani::assume(root_len <= 2);
        let mut root_buf = [PathSeg::Root; 2];
        for i in 0..root_len {
            root_buf[i] = any_path_seg();
        }
        let root = &root_buf[..root_len];

        let path_len: usize = kani::any();
        kani::assume(path_len <= 3);
        let mut path_buf = [PathSeg::Root; 3];
        for i in 0..path_len {
            path_buf[i] = any_path_seg();
        }
        let path = &path_buf[..path_len];

        let path_has_link: bool = kani::any();
        let root_has_link: bool = kani::any();

        let within = path_is_within_segments(path, path_has_link, root, root_has_link);

        if contains_seg(path, PathSeg::Parent)
            || contains_seg(root, PathSeg::Parent)
            || path_has_link
            || root_has_link
        {
            assert!(!within);
        }

        if within {
            let norm_path = normalize_lexical_segments::<4>(path);
            let norm_root = normalize_lexical_segments::<4>(root);
            assert!(norm_root.len > 0);
            assert!(segs_start_with(norm_path.as_slice(), norm_root.as_slice()));
            assert!(!contains_seg(norm_path.as_slice(), PathSeg::Parent));
        }

        if path_len >= 1 && path[0] == PathSeg::Parent {
            let norm = normalize_lexical_segments::<4>(path);
            assert!(norm.len > 0 && norm.data[0] == PathSeg::Parent);
        }
    }

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
    #[kani::unwind(7)]
    fn proof_backup_path_containment_under_backup_root() {
        let root_seg: u8 = kani::any();
        kani::assume(root_seg < 4);
        let config_root = [PathSeg::Root, PathSeg::Normal(root_seg)];

        let backup_len: usize = kani::any();
        kani::assume(backup_len <= 5);
        let mut backup_buf = [PathSeg::Root; 5];
        for i in 0..backup_len {
            let kind: u8 = kani::any();
            backup_buf[i] = match kind % 6 {
                0 => PathSeg::Root,
                1 => PathSeg::Cur,
                2 => PathSeg::Parent,
                3 => PathSeg::Normal(SEG_ORBIT),
                4 => PathSeg::Normal(SEG_BACKUPS),
                _ => PathSeg::Normal(kani::any::<u8>() & 3),
            };
        }
        let backup = &backup_buf[..backup_len];
        let backup_has_link: bool = kani::any();
        let root_has_link: bool = kani::any();

        let ok = backup_path_is_contained(&config_root, backup, backup_has_link, root_has_link);
        if ok {
            assert!(!backup_has_link && !root_has_link);
            assert!(!contains_seg(backup, PathSeg::Parent));
            let norm = normalize_lexical_segments::<6>(backup);
            assert_eq!(norm.len, 5);
            assert_eq!(norm.data[0], PathSeg::Root);
            assert_eq!(norm.data[1], PathSeg::Normal(root_seg));
            assert_eq!(norm.data[2], PathSeg::Normal(SEG_ORBIT));
            assert_eq!(norm.data[3], PathSeg::Normal(SEG_BACKUPS));
        }
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

    #[kani::proof]
    #[kani::unwind(8)]
    fn proof_collision_path_normalization() {
        let len: usize = kani::any();
        kani::assume(len <= 5);
        let mut a = [0u8; 5];
        let mut b = [0u8; 5];
        for i in 0..len {
            let ch: u8 = kani::any();
            kani::assume(ch.is_ascii_alphanumeric() || ch == b'/' || ch == b'.');
            a[i] = ch;
            let flip: bool = kani::any();
            b[i] = if flip && ch.is_ascii_alphabetic() {
                ch ^ 0x20
            } else {
                ch
            };
        }

        let (na, na_len) = normalize_collision_bytes::<5>(&a[..len], true);
        let (nb, nb_len) = normalize_collision_bytes::<5>(&b[..len], true);
        assert_eq!(na_len, nb_len);
        assert_eq!(&na[..na_len], &nb[..nb_len]);
        let (na2, na2_len) = normalize_collision_bytes::<5>(&na[..na_len], true);
        assert_eq!(&na2[..na2_len], &na[..na_len]);

        let (sa, sa_len) = normalize_collision_bytes::<5>(&a[..len], false);
        let (sb, sb_len) = normalize_collision_bytes::<5>(&b[..len], false);
        assert_eq!(&sa[..sa_len], &a[..len]);
        assert_eq!(&sb[..sb_len], &b[..len]);
    }

    #[kani::proof]
    #[kani::unwind(6)]
    fn proof_memory_link_tilde_and_leading_parent_normalization() {
        let has_home: bool = kani::any();
        let home_arr = [PathSeg::Root, PathSeg::Normal(1)];
        let home = has_home.then_some(home_arr.as_slice());

        let kind = any_memory_target_kind();
        let tail_len: usize = kani::any();
        kani::assume(tail_len <= 2);
        let mut tail_buf = [PathSeg::Normal(0); 2];
        for i in 0..tail_len {
            tail_buf[i] = match kani::any::<u8>() % 3 {
                0 => PathSeg::Cur,
                1 => PathSeg::Parent,
                _ => PathSeg::Normal(kani::any::<u8>() % 3),
            };
        }
        let tail = &tail_buf[..tail_len];

        let src_parent_empty: bool = kani::any();
        let src_parent: &[PathSeg] = if src_parent_empty {
            &[]
        } else {
            &[PathSeg::Normal(2)]
        };

        let resolved = resolve_memory_link_segments(Some(src_parent), kind, tail, home);

        match kind {
            MemoryTargetKind::Empty | MemoryTargetKind::ExternalScheme => {
                assert!(resolved.is_none());
            }
            MemoryTargetKind::TildeOnly | MemoryTargetKind::TildeSlash => {
                if !has_home {
                    assert!(resolved.is_none());
                } else {
                    let out = resolved.unwrap();
                    assert!(out.len > 0 && out.data[0] == PathSeg::Root);
                    assert!(!contains_seg(out.as_slice(), PathSeg::Parent));
                }
            }
            MemoryTargetKind::Absolute => {
                let out = resolved.unwrap();
                assert!(out.len > 0 && out.data[0] == PathSeg::Root);
                assert!(!contains_seg(out.as_slice(), PathSeg::Parent));
            }
            MemoryTargetKind::Relative => {
                let out = resolved.unwrap();
                if src_parent_empty && tail_len >= 1 && tail[0] == PathSeg::Parent {
                    assert!(out.len > 0 && out.data[0] == PathSeg::Parent);
                }
            }
        }
    }
}
