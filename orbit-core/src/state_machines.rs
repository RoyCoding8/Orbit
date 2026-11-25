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

/// Abstract checksum token (`u8` represents a distinct SHA-256 digest in bounded proofs).
pub type DigestId = u8;

/// Prefix marking version-2 tree-boundary-safe directory checksums.
pub const DIRECTORY_CHECKSUM_PREFIX: &str = "tree-v2-sha256:";

/// Pure state-machine transition resolver for a ledger row given observed
/// target and backup checksums. Generic over checksum representation `T` so
/// both `src-tauri` (`&str`) and `formal/` (`DigestId`) execute the exact same logic.
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

/// Minimal ledger row representation for bounded supersession proofs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSummary {
    pub status: Option<LedgerStatus>,
    pub target_id: Option<u8>,
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

/// Specialized supersession check over `RowSummary` slices.
pub fn pending_was_superseded(rows: &[RowSummary], index: usize, row: &RowSummary) -> bool {
    pending_was_superseded_by(rows, index, row, |r| r.status, |r| r.target_id)
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

fn push_bytes<const N: usize>(buf: &mut [u8; N], pos: &mut usize, src: &[u8]) {
    let mut i = 0;
    while i < src.len() && *pos < N {
        buf[*pos] = src[i];
        *pos += 1;
        i += 1;
    }
}

fn push_u64_dec<const N: usize>(buf: &mut [u8; N], pos: &mut usize, mut val: u64) {
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    if val == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while val > 0 {
            tmp[n] = b'0' + (val % 10) as u8;
            val /= 10;
            n += 1;
        }
    }
    while n > 0 && *pos < N {
        n -= 1;
        buf[*pos] = tmp[n];
        *pos += 1;
    }
}

/// Stack-allocated formatter for backup filenames: `"{encoded_id}-{action}-{stamp}-{seq}"`.
pub fn format_backup_filename_buf(
    encoded_id: &[u8],
    action: &[u8],
    stamp: u64,
    seq: u64,
) -> ([u8; 24], usize) {
    let mut buf = [0u8; 24];
    let mut pos = 0;
    push_bytes(&mut buf, &mut pos, encoded_id);
    push_bytes(&mut buf, &mut pos, b"-");
    push_bytes(&mut buf, &mut pos, action);
    push_bytes(&mut buf, &mut pos, b"-");
    push_u64_dec(&mut buf, &mut pos, stamp);
    push_bytes(&mut buf, &mut pos, b"-");
    push_u64_dec(&mut buf, &mut pos, seq);
    (buf, pos)
}

/// Format a backup filename: `"{encoded_id}-{action}-{stamp}-{seq}"`.
pub fn format_backup_filename(encoded_id: &str, action: &str, stamp: u128, seq: u64) -> String {
    format!("{encoded_id}-{action}-{stamp}-{seq}")
}

/// Stack-allocated formatter for operation IDs: `"{kind}:{id}:{action}:{stamp}:{seq}"`.
pub fn format_op_id_buf(
    kind: &[u8],
    id: &[u8],
    action: &[u8],
    stamp: u64,
    seq: u64,
) -> ([u8; 24], usize) {
    let mut buf = [0u8; 24];
    let mut pos = 0;
    push_bytes(&mut buf, &mut pos, kind);
    push_bytes(&mut buf, &mut pos, b":");
    push_bytes(&mut buf, &mut pos, id);
    push_bytes(&mut buf, &mut pos, b":");
    push_bytes(&mut buf, &mut pos, action);
    push_bytes(&mut buf, &mut pos, b":");
    push_u64_dec(&mut buf, &mut pos, stamp);
    push_bytes(&mut buf, &mut pos, b":");
    push_u64_dec(&mut buf, &mut pos, seq);
    (buf, pos)
}

/// Format an operation ID: `"{kind}:{id}:{action}:{stamp}:{seq}"`.
pub fn format_op_id(kind: &str, id: &str, action: &str, stamp: u128, seq: u64) -> String {
    format!("{kind}:{id}:{action}:{stamp}:{seq}")
}

pub fn slices_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
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

/// Pure single-use reservation state machine for restore/undo operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreReservation {
    Idle,
    Reserved { op_seq: u64 },
    Completed { op_seq: u64 },
}

impl RestoreReservation {
    pub fn reserve(&mut self, op_seq: u64) -> Result<(), &'static str> {
        match *self {
            RestoreReservation::Idle => {
                *self = RestoreReservation::Reserved { op_seq };
                Ok(())
            }
            RestoreReservation::Reserved { .. } | RestoreReservation::Completed { .. } => {
                Err("restore reservation is single-use")
            }
        }
    }

    pub fn complete(&mut self, op_seq: u64) -> Result<(), &'static str> {
        match *self {
            RestoreReservation::Reserved { op_seq: active } if active == op_seq => {
                *self = RestoreReservation::Completed { op_seq };
                Ok(())
            }
            _ => Err("cannot complete an unreserved or mismatched restore"),
        }
    }
}

/// Precondition gate for restore/undo operations.
#[derive(Debug, Clone, Copy)]
pub struct RestorePreconditions {
    pub reservation_complete: bool,
    pub superseded: bool,
    pub op_state: OperationState,
    pub is_directory: bool,
    pub is_tree_v2_checksum: bool,
    pub expected_current: Option<DigestId>,
    pub observed_current: Option<DigestId>,
    pub recorded_before: Option<DigestId>,
    pub observed_backup: Option<DigestId>,
}

/// Validate all preconditions required before executing a restore or undo.
pub fn check_restore_preconditions(p: &RestorePreconditions) -> Result<(), &'static str> {
    if !p.reservation_complete {
        return Err("incomplete ledger reservation");
    }
    if p.superseded || p.op_state != OperationState::Applied {
        return Err("history entry did not complete and cannot be restored");
    }
    let Some(recorded_before) = p.recorded_before else {
        return Err("history entry has no recorded before checksum");
    };
    if p.is_directory && !p.is_tree_v2_checksum {
        return Err("legacy directory checksum refused");
    }
    if p.observed_backup != Some(recorded_before) {
        return Err("backup checksum does not match recorded preimage");
    }
    if p.observed_current != p.expected_current {
        return Err("stale target checksum; refresh before restoring");
    }
    Ok(())
}
