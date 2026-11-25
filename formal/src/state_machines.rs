//! Kani proof harnesses directly verifying `orbit_core::state_machines`.

#[cfg(kani)]
mod verification {
    use orbit_core::state_machines::{
        check_restore_preconditions, classify_history_status, format_backup_filename_buf,
        format_op_id_buf, pending_was_superseded, resolve_operation_state, slices_equal,
        HistoryOperationStatus, LedgerStatus, OperationState, RestorePreconditions,
        RestoreReservation, RowSummary,
    };

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
        let mut rows = [RowSummary {
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
            rows[i] = RowSummary { status, target_id };
        }
        let slice = &rows[..len];
        let idx: usize = kani::any();
        kani::assume(idx < len);

        let superseded = pending_was_superseded(slice, idx, &slice[idx]);

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
    #[kani::unwind(14)]
    fn proof_atomic_sequence_uniqueness() {
        let stamp: u64 = kani::any();
        let seq1: u64 = kani::any();
        let seq2: u64 = kani::any();
        kani::assume(stamp < 100);
        kani::assume(seq1 < 100 && seq2 < 100);
        kani::assume(seq1 != seq2);

        let (op1, op1_len) = format_op_id_buf(b"k", b"i", b"a", stamp, seq1);
        let (op2, op2_len) = format_op_id_buf(b"k", b"i", b"a", stamp, seq2);
        assert!(!slices_equal(&op1[..op1_len], &op2[..op2_len]));

        let (bk1, bk1_len) = format_backup_filename_buf(b"h", b"a", stamp, seq1);
        let (bk2, bk2_len) = format_backup_filename_buf(b"h", b"a", stamp, seq2);
        assert!(!slices_equal(&bk1[..bk1_len], &bk2[..bk2_len]));
    }

    #[kani::proof]
    fn proof_restore_and_undo_preconditions() {
        let p = RestorePreconditions {
            reservation_complete: kani::any(),
            superseded: kani::any(),
            op_state: any_operation_state(),
            is_directory: kani::any(),
            is_tree_v2_checksum: kani::any(),
            expected_current: if kani::any() {
                Some(kani::any::<u8>() % 4)
            } else {
                None
            },
            observed_current: if kani::any() {
                Some(kani::any::<u8>() % 4)
            } else {
                None
            },
            recorded_before: if kani::any() {
                Some(kani::any::<u8>() % 4)
            } else {
                None
            },
            observed_backup: if kani::any() {
                Some(kani::any::<u8>() % 4)
            } else {
                None
            },
        };

        let decision = check_restore_preconditions(&p);

        if p.observed_current != p.expected_current {
            assert!(decision.is_err());
        }
        if p.recorded_before.is_none() || p.observed_backup != p.recorded_before {
            assert!(decision.is_err());
        }
        if p.is_directory && !p.is_tree_v2_checksum {
            assert!(decision.is_err());
        }
        if !p.reservation_complete || p.superseded || p.op_state != OperationState::Applied {
            assert!(decision.is_err());
        }

        let mut res = RestoreReservation::Idle;
        let seq1: u64 = kani::any();
        let seq2: u64 = kani::any();
        assert!(res.complete(seq1).is_err());
        assert!(res.reserve(seq1).is_ok());
        assert!(res.reserve(seq2).is_err());
        if seq2 != seq1 {
            assert!(res.complete(seq2).is_err());
        }
        assert!(res.complete(seq1).is_ok());
        assert!(res.reserve(seq2).is_err());
        assert!(res.complete(seq1).is_err());
    }
}
