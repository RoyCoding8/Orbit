//! Kani proof harnesses directly verifying `orbit_core::state_machines`.

#[cfg(kani)]
mod verification {
    use orbit_core::state_machines::{
        classify_history_status, pending_was_superseded_by, resolve_operation_state,
        verify_directory_checksum, HistoryOperationStatus, LedgerStatus, OperationState,
    };

    #[derive(Clone, Copy)]
    struct Row {
        status: Option<LedgerStatus>,
        target_id: Option<u8>,
    }

    fn any_ledger_status() -> LedgerStatus {
        if kani::any() {
            LedgerStatus::Pending
        } else {
            LedgerStatus::Applied
        }
    }

    fn any_opt_ledger_status() -> Option<LedgerStatus> {
        if kani::any() {
            Some(any_ledger_status())
        } else {
            None
        }
    }

    fn any_operation_state() -> OperationState {
        match kani::any::<u8>() % 3 {
            0 => OperationState::Applied,
            1 => OperationState::NotApplied,
            _ => OperationState::Drift,
        }
    }

    fn any_opt_operation_state() -> Option<OperationState> {
        if kani::any() {
            Some(any_operation_state())
        } else {
            None
        }
    }

    #[kani::proof]
    fn proof_resolve_operation_state_invariants() {
        let status = any_opt_ledger_status();
        let before: Option<u8> = if kani::any() {
            Some(kani::any::<u8>() % 4)
        } else {
            None
        };
        let after: Option<u8> = if kani::any() {
            Some(kani::any::<u8>() % 4)
        } else {
            None
        };
        let has_target: bool = kani::any();
        let observed_target: Option<u8> = if kani::any() {
            Some(kani::any::<u8>() % 4)
        } else {
            None
        };
        let has_backup: bool = kani::any();
        let observed_backup: Option<u8> = if kani::any() {
            Some(kani::any::<u8>() % 4)
        } else {
            None
        };

        let result = resolve_operation_state(
            status,
            before,
            after,
            has_target,
            observed_target,
            has_backup,
            observed_backup,
        );

        match status {
            Some(LedgerStatus::Applied) | None => {
                assert_eq!(result, OperationState::Applied);
            }
            Some(LedgerStatus::Pending) => {
                if !has_target {
                    assert_eq!(result, OperationState::Drift);
                } else if observed_target == after {
                    let backup_mismatch =
                        has_backup && before.is_some() && observed_backup != before;
                    if backup_mismatch {
                        assert_eq!(result, OperationState::Drift);
                    } else {
                        assert_eq!(result, OperationState::Applied);
                    }
                } else if observed_target == before {
                    assert_eq!(result, OperationState::NotApplied);
                } else {
                    assert_eq!(result, OperationState::Drift);
                }
            }
        }
    }

    #[kani::proof]
    #[kani::unwind(6)]
    fn proof_pending_was_superseded_invariants() {
        let len: usize = kani::any();
        kani::assume((1..=4).contains(&len));
        let mut rows = [Row {
            status: None,
            target_id: None,
        }; 4];
        for i in 0..len {
            let status = any_opt_ledger_status();
            let target_id: Option<u8> = if kani::any() {
                Some(kani::any::<u8>() % 3)
            } else {
                None
            };
            rows[i] = Row { status, target_id };
        }
        let slice = &rows[..len];
        let idx: usize = kani::any();
        kani::assume(idx < len);

        let superseded =
            pending_was_superseded_by(slice, idx, &slice[idx], |r| r.status, |r| r.target_id);

        if slice[idx].status != Some(LedgerStatus::Pending) || slice[idx].target_id.is_none() {
            assert!(!superseded);
        }

        let mut has_later_same_target = false;
        let mut j = idx + 1;
        while j < len {
            if slice[j].target_id.is_some() && slice[j].target_id == slice[idx].target_id {
                has_later_same_target = true;
            }
            j += 1;
        }
        assert_eq!(
            superseded,
            slice[idx].status == Some(LedgerStatus::Pending) && has_later_same_target
        );
    }

    #[kani::proof]
    fn proof_classify_history_status_invariants() {
        let complete: bool = kani::any();
        let superseded: bool = kani::any();
        let state = any_opt_operation_state();

        let status = classify_history_status(complete, superseded, state);

        assert_ne!(status, HistoryOperationStatus::Pending);

        if !complete {
            assert_eq!(status, HistoryOperationStatus::Legacy);
        } else if superseded {
            assert_eq!(status, HistoryOperationStatus::NotApplied);
        } else {
            match state {
                Some(OperationState::Applied) => {
                    assert_eq!(status, HistoryOperationStatus::Applied)
                }
                Some(OperationState::NotApplied) => {
                    assert_eq!(status, HistoryOperationStatus::NotApplied)
                }
                Some(OperationState::Drift) => assert_eq!(status, HistoryOperationStatus::Drift),
                None => assert_eq!(status, HistoryOperationStatus::Unavailable),
            }
        }
    }

    #[kani::proof]
    #[kani::unwind(24)]
    fn proof_verify_directory_checksum() {
        let checksums = ["", "legacy", "tree-v2-sha256:a", "tree-v2-sha256:b"];
        let actual = usize::from(kani::any::<u8>() % 4);
        let expected = usize::from(kani::any::<u8>() % 4);
        assert_eq!(
            verify_directory_checksum(checksums[actual], checksums[expected]).is_ok(),
            expected >= 2 && actual == expected
        );
    }
}
