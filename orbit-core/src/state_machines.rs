//! Pure ledger and history state-machine logic shared between `src-tauri` and `formal/`.

/// Reservation status recorded on a ledger row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerStatus {
    Pending,
    Applied,
}

/// Resolved execution state of a ledger operation against observed disk state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationState {
    Applied,
    NotApplied,
    Drift,
}

/// User-facing status classification for a history log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryOperationStatus {
    Applied,
    Pending,
    NotApplied,
    Drift,
    Legacy,
    Unavailable,
}

/// Prefix marking version-2 tree-boundary-safe directory checksums.
pub const DIRECTORY_CHECKSUM_PREFIX: &str = "tree-v2-sha256:";

/// Pure state-machine transition resolver for a ledger row given observed
/// target and backup checksums. Generic over checksum representation `T` so
/// both `src-tauri` (`&str`) and `formal/` (small integer tokens) execute the exact same logic.
pub fn resolve_operation_state<T: PartialEq>(
    status: Option<LedgerStatus>,
    before_checksum: Option<T>,
    after_checksum: Option<T>,
    has_target: bool,
    observed_target: Option<T>,
    has_backup: bool,
    observed_backup: Option<T>,
) -> OperationState {
    match status {
        Some(LedgerStatus::Applied) | None => return OperationState::Applied,
        Some(LedgerStatus::Pending) => {}
    }
    if !has_target {
        return OperationState::Drift;
    }
    if observed_target == after_checksum {
        if has_backup {
            if let Some(expected) = before_checksum {
                if observed_backup != Some(expected) {
                    return OperationState::Drift;
                }
            }
        }
        return OperationState::Applied;
    }
    if observed_target == before_checksum {
        return OperationState::NotApplied;
    }
    OperationState::Drift
}

/// Generic supersession check: returns true iff `row` is `Pending` with a target
/// and some strictly later row (`rows[index + 1..]`) targets the same path/ID.
pub fn pending_was_superseded_by<'a, R, T: PartialEq>(
    rows: &'a [R],
    index: usize,
    row: &'a R,
    status_of: impl Fn(&'a R) -> Option<LedgerStatus>,
    target_of: impl Fn(&'a R) -> Option<T>,
) -> bool {
    status_of(row) == Some(LedgerStatus::Pending)
        && target_of(row).is_some_and(|target| {
            rows[index + 1..]
                .iter()
                .any(|later| target_of(later).is_some_and(|later_target| later_target == target))
        })
}

/// Pure status classifier for a history log row given reservation completeness,
/// whether a pending row was superseded by a later operation, and the resolved
/// operation state.
pub fn classify_history_status(
    reservation_complete: bool,
    superseded_pending: bool,
    state: Option<OperationState>,
) -> HistoryOperationStatus {
    if !reservation_complete {
        return HistoryOperationStatus::Legacy;
    }
    if superseded_pending {
        return HistoryOperationStatus::NotApplied;
    }
    match state {
        Some(OperationState::Applied) => HistoryOperationStatus::Applied,
        Some(OperationState::NotApplied) => HistoryOperationStatus::NotApplied,
        Some(OperationState::Drift) => HistoryOperationStatus::Drift,
        None => HistoryOperationStatus::Unavailable,
    }
}

/// Format a backup filename: `"{encoded_id}-{action}-{stamp}-{seq}"`.
pub fn format_backup_filename(encoded_id: &str, action: &str, stamp: u128, seq: u64) -> String {
    format!("{encoded_id}-{action}-{stamp}-{seq}")
}

/// Format an operation ID: `"{kind}:{id}:{action}:{stamp}:{seq}"`.
pub fn format_op_id(kind: &str, id: &str, action: &str, stamp: u128, seq: u64) -> String {
    format!("{kind}:{id}:{action}:{stamp}:{seq}")
}

/// Verify a directory checksum against a recorded expected checksum, refusing
/// legacy (non-`tree-v2-sha256:`) checksums.
pub fn verify_directory_checksum(actual: &str, expected: &str) -> Result<(), &'static str> {
    if !expected.starts_with(DIRECTORY_CHECKSUM_PREFIX) {
        return Err(
            "legacy directory checksum cannot distinguish path and content boundaries; automatic restore is refused and the backup is preserved",
        );
    }
    if actual != expected {
        return Err("directory checksum does not match the recorded backup");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{format_backup_filename, format_op_id};

    #[test]
    fn formatter_preserves_delimiters_and_decimal_boundaries() {
        let encoded = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        for (seq, decimal) in [
            (0, "0"),
            (9, "9"),
            (10, "10"),
            (u64::MAX - 1, "18446744073709551614"),
            (u64::MAX, "18446744073709551615"),
        ] {
            let timestamp = "340282366920938463463374607431768211455";
            assert_eq!(
                format_backup_filename(encoded, "history-restore", u128::MAX, seq),
                [encoded, "-history-restore-", timestamp, "-", decimal].concat()
            );
            assert_eq!(
                format_op_id("skill", "\u{914d}\u{7f6e}:nightly", "edit", u128::MAX, seq),
                [
                    "skill:\u{914d}\u{7f6e}:nightly:edit:",
                    timestamp,
                    ":",
                    decimal
                ]
                .concat()
            );
        }
    }
}
