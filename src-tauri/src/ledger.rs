use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::harness::{atomic, ResourceKind};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum LedgerStatus {
    Pending,
    Applied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerRow {
    pub timestamp: u64,
    pub resource_kind: String,
    pub id: String,
    pub action: String,
    pub backup_path: Option<String>,
    pub before_checksum: Option<String>,
    pub after_checksum: Option<String>,
    /// The file this row's mutation targeted, when a receipt exists. Rows
    /// without a target describe moves that have no single-file undo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// New rows start Pending before the target changes, then become Applied
    /// after verified readback. Missing status means a pre-reservation row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<LedgerStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationState {
    Applied,
    NotApplied,
    Drift,
}

pub fn backup_root(config_root: &Path) -> PathBuf {
    config_root.join(".harness-agent").join("backups")
}

pub fn backup_path(config_root: &Path, id: &str, action: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let encoded = atomic::hex(&Sha256::digest(id.as_bytes()));
    backup_root(config_root).join(format!("{encoded}-{action}-{stamp}"))
}

fn op_id(kind: &str, id: &str, action: &str) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{kind}:{id}:{action}:{stamp}")
}

#[derive(Debug)]
enum WritePhase {
    Preparation,
    Reservation,
    Mutation,
    Finalization,
}

#[derive(Debug)]
pub struct LedgerWriteError {
    op: String,
    phase: WritePhase,
    source: String,
}

impl std::fmt::Display for LedgerWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.phase {
            WritePhase::Preparation => {
                write!(f, "mutation {} was not started: {}", self.op, self.source)
            }
            WritePhase::Reservation => write!(
                f,
                "mutation {} was not started because its ledger reservation failed: {}",
                self.op, self.source
            ),
            WritePhase::Mutation => write!(
                f,
                "mutation {} failed after reservation; its target and backup determine whether it landed: {}",
                self.op, self.source
            ),
            WritePhase::Finalization => write!(
                f,
                "mutation {} finalization failed: {}",
                self.op, self.source
            ),
        }
    }
}

impl std::error::Error for LedgerWriteError {}

fn write_error<E: std::fmt::Display>(
    op: &str,
    error: atomic::RecordedMutationError<E>,
) -> LedgerWriteError {
    let (phase, source) = match error {
        atomic::RecordedMutationError::Preparation(source) => {
            (WritePhase::Preparation, source.to_string())
        }
        atomic::RecordedMutationError::Reservation(source) => {
            (WritePhase::Reservation, source.to_string())
        }
        atomic::RecordedMutationError::Mutation(source) => {
            (WritePhase::Mutation, source.to_string())
        }
        atomic::RecordedMutationError::Finalization(source) => {
            (WritePhase::Finalization, source.to_string())
        }
    };
    LedgerWriteError {
        op: op.to_owned(),
        phase,
        source,
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn append_row_locked(
    config_root: &Path,
    row: &LedgerRow,
    guard: &atomic::MutationGuard,
) -> Result<(), String> {
    let path = config_root.join(".harness-agent").join("ledger.json");
    std::fs::create_dir_all(path.parent().ok_or("ledger path has no parent")?)
        .map_err(|error| error.to_string())?;
    let row_value = serde_json::to_value(row).map_err(|error| error.to_string())?;
    atomic::commit_locked(
        &path,
        vec![atomic::Patch::Push {
            path: atomic::p!["rows"],
            value: row_value,
            key: Some("op".into()),
        }],
        None,
        guard,
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn row_for_plan(
    kind: &str,
    id: &str,
    action: &str,
    op: &str,
    plan: &atomic::MutationPlan,
) -> LedgerRow {
    LedgerRow {
        timestamp: now_millis(),
        resource_kind: kind.to_owned(),
        id: id.to_owned(),
        action: action.to_owned(),
        backup_path: plan
            .backup
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        before_checksum: plan.before_checksum.clone(),
        after_checksum: plan.after_checksum.clone(),
        target: Some(plan.target.to_string_lossy().into_owned()),
        op: Some(op.to_owned()),
        status: Some(LedgerStatus::Pending),
    }
}

fn reserve_row_locked(
    config_root: &Path,
    row: &LedgerRow,
    guard: &atomic::MutationGuard,
) -> Result<(), String> {
    reconcile_pending_for_target(config_root, row.target.as_deref().unwrap(), guard)?;
    append_row_locked(config_root, row, guard)
}

/// Reserve a filesystem mutation while the caller holds the atomic mutation
/// lock. Used for directory moves whose verified checksum is computed by the
/// owning subsystem.
pub(crate) fn reserve_mutation(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    plan: &atomic::MutationPlan,
    guard: &atomic::MutationGuard,
) -> Result<String, String> {
    let context = MutationContext::new(config_root, kind, id, action);
    context.reserve(plan, guard)?;
    Ok(context.op)
}

/// Mark a verified reservation Applied while the caller still holds the same
/// mutation lock that protected the target change.
pub(crate) fn complete_mutation(
    config_root: &Path,
    op: &str,
    guard: &atomic::MutationGuard,
) -> Result<(), String> {
    let path = config_root.join(".harness-agent").join("ledger.json");
    let value = atomic::read_json(&path).map_err(|error| error.to_string())?;
    let rows = value
        .get("rows")
        .and_then(Value::as_array)
        .ok_or("ledger has no rows array")?;
    let current = rows
        .iter()
        .find(|row| row.get("op").and_then(Value::as_str) == Some(op))
        .cloned()
        .ok_or_else(|| format!("ledger reservation {op} disappeared"))?;
    let mut updated: LedgerRow =
        serde_json::from_value(current.clone()).map_err(|error| error.to_string())?;
    updated.status = Some(LedgerStatus::Applied);
    let updated = serde_json::to_value(updated).map_err(|error| error.to_string())?;
    atomic::commit_locked(
        &path,
        vec![atomic::Patch::ReplaceWhere {
            path: atomic::p!["rows"],
            matches: current,
            value: updated,
        }],
        None,
        guard,
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

struct MutationContext<'a> {
    config_root: &'a Path,
    kind: String,
    id: &'a str,
    action: &'a str,
    op: String,
}

impl<'a> MutationContext<'a> {
    fn new(config_root: &'a Path, kind: ResourceKind, id: &'a str, action: &'a str) -> Self {
        let kind = format!("{kind:?}").to_lowercase();
        let op = op_id(&kind, id, action);
        Self {
            config_root,
            kind,
            id,
            action,
            op,
        }
    }

    fn reserve(
        &self,
        plan: &atomic::MutationPlan,
        guard: &atomic::MutationGuard,
    ) -> Result<(), String> {
        let row = row_for_plan(&self.kind, self.id, self.action, &self.op, plan);
        reserve_row_locked(self.config_root, &row, guard)
    }

    fn complete(&self, guard: &atomic::MutationGuard) -> Result<(), String> {
        complete_mutation(self.config_root, &self.op, guard)
    }
}

type MutationCallback<'a> =
    dyn Fn(&atomic::MutationPlan, &atomic::MutationGuard) -> Result<(), String> + 'a;

fn recorded_mutation<T>(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    operation: impl FnOnce(
        &MutationCallback<'_>,
        &MutationCallback<'_>,
    ) -> std::result::Result<T, atomic::RecordedMutationError<String>>,
) -> Result<T, LedgerWriteError> {
    let context = MutationContext::new(config_root, kind, id, action);
    let reserve = |plan, guard| context.reserve(plan, guard);
    let complete = |_, guard| context.complete(guard);
    operation(&reserve, &complete).map_err(|error| write_error(&context.op, error))
}

pub fn commit(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
    patches: Vec<atomic::Patch>,
    backup: Option<&Path>,
) -> Result<atomic::Receipt, LedgerWriteError> {
    recorded_mutation(config_root, kind, id, action, |reserve, complete| {
        atomic::commit_recorded(path, patches, backup, reserve, complete)
    })
}

pub fn write_file(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
    bytes: &[u8],
    backup: Option<&Path>,
) -> Result<atomic::Receipt, LedgerWriteError> {
    recorded_mutation(config_root, kind, id, action, |reserve, complete| {
        atomic::write_file_recorded(path, bytes, backup, reserve, complete)
    })
}

pub fn restore_file(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
    expected_current: [u8; 32],
    bytes: &[u8],
    backup: Option<&Path>,
) -> Result<atomic::Receipt, LedgerWriteError> {
    recorded_mutation(config_root, kind, id, action, |reserve, complete| {
        atomic::restore_recorded(path, expected_current, bytes, backup, reserve, complete)
    })
}

pub fn remove_file(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
    expected_current: [u8; 32],
    backup: &Path,
) -> Result<(), LedgerWriteError> {
    recorded_mutation(config_root, kind, id, action, |reserve, complete| {
        atomic::remove_current_recorded(path, expected_current, backup, reserve, complete)
    })
}

pub fn read(config_root: &Path) -> Result<Vec<LedgerRow>, String> {
    let value = atomic::read_json(&config_root.join(".harness-agent").join("ledger.json"))
        .map_err(|error| error.to_string())?;
    serde_json::from_value(
        value
            .get("rows")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| error.to_string())
}

fn checksum_if_present(path: &Path) -> Result<Option<String>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        return Err(format!("refusing to checksum a link: {}", path.display()));
    }
    if metadata.is_dir() {
        directory_checksum(path).map(Some)
    } else if metadata.is_file() {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        Ok(Some(atomic::hex(&Sha256::digest(bytes))))
    } else {
        Err(format!("unsupported ledger target: {}", path.display()))
    }
}

/// Resolve whether a reservation landed from the recorded target and backup.
/// Applied rows remain historical receipts even after later mutations.
pub fn operation_state(row: &LedgerRow) -> Result<OperationState, String> {
    match row.status {
        Some(LedgerStatus::Applied) | None => return Ok(OperationState::Applied),
        Some(LedgerStatus::Pending) => {}
    }
    let target = match row.target.as_deref() {
        Some(target) => checksum_if_present(Path::new(target))?,
        None => return Ok(OperationState::Drift),
    };
    if target == row.after_checksum {
        if let (Some(path), Some(expected)) =
            (row.backup_path.as_deref(), row.before_checksum.as_deref())
        {
            if checksum_if_present(Path::new(path))?.as_deref() != Some(expected) {
                return Ok(OperationState::Drift);
            }
        }
        return Ok(OperationState::Applied);
    }
    if target == row.before_checksum {
        return Ok(OperationState::NotApplied);
    }
    Ok(OperationState::Drift)
}

/// Completed history for views that should not surface abandoned reservations.
/// `read` stays the raw audit log and includes every reserved row.
pub fn read_applied(config_root: &Path) -> Result<Vec<LedgerRow>, String> {
    let rows = read(config_root)?;
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| {
            if pending_was_superseded(&rows, index, row) {
                return None;
            }
            match operation_state(row) {
                Ok(OperationState::Applied) => Some(Ok(row)),
                Ok(OperationState::NotApplied | OperationState::Drift) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .map(|result| result.map(|row| row.clone()))
        .collect()
}

fn pending_was_superseded(rows: &[LedgerRow], index: usize, row: &LedgerRow) -> bool {
    row.status == Some(LedgerStatus::Pending)
        && row.target.as_deref().is_some_and(|target| {
            rows[index + 1..]
                .iter()
                .any(|later| later.target.as_deref() == Some(target))
        })
}

fn reconcile_pending_for_target(
    config_root: &Path,
    target: &str,
    guard: &atomic::MutationGuard,
) -> Result<(), String> {
    for row in read(config_root)? {
        if row.target.as_deref() == Some(target)
            && row.status == Some(LedgerStatus::Pending)
            && operation_state(&row)? == OperationState::Applied
        {
            complete_mutation(
                config_root,
                row.op.as_deref().ok_or("pending row has no operation id")?,
                guard,
            )?;
        }
    }
    Ok(())
}

pub fn latest_row(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    target: Option<&Path>,
) -> Result<Option<LedgerRow>, String> {
    let kind_label = format!("{kind:?}").to_lowercase();
    let target_label = target.map(|path| path.to_string_lossy().into_owned());
    Ok(read(config_root)?
        .into_iter()
        .filter(|row| {
            row.op.is_some()
                && row.resource_kind == kind_label
                && row.id == id
                && row.action == action
                && row.target == target_label
        })
        .max_by_key(|row| row.timestamp))
}

pub fn latest_applied_row(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    target: &Path,
) -> Result<Option<LedgerRow>, String> {
    let kind_label = format!("{kind:?}").to_lowercase();
    let target_label = target.to_string_lossy().into_owned();
    let rows = read(config_root)?;
    let matches = rows
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, row)| {
            row.op.is_some()
                && row.resource_kind == kind_label
                && row.id == id
                && row.action == action
                && row.target.as_deref() == Some(target_label.as_str())
        })
        .filter_map(|(index, row)| {
            if pending_was_superseded(&rows, index, &row) {
                return None;
            }
            match operation_state(&row) {
                Ok(OperationState::Applied) => Some(Ok((index, row))),
                Ok(OperationState::NotApplied | OperationState::Drift) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(matches
        .into_iter()
        .max_by_key(|(index, row)| (row.timestamp, *index))
        .map(|(_, row)| row))
}

const DIRECTORY_CHECKSUM_PREFIX: &str = "tree-v2-sha256:";

pub fn directory_checksum(path: &Path) -> Result<String, String> {
    fn update_entry(hasher: &mut Sha256, kind: u8, path: &[u8], contents: &[u8]) {
        hasher.update([kind]);
        hasher.update((path.len() as u64).to_be_bytes());
        hasher.update(path);
        hasher.update((contents.len() as u64).to_be_bytes());
        hasher.update(contents);
    }

    fn visit(root: &Path, current: &Path, hasher: &mut Sha256) -> Result<(), String> {
        let mut entries = std::fs::read_dir(current)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative =
                path.strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .components()
                    .map(|component| {
                        component.as_os_str().to_str().ok_or_else(|| {
                            format!("non-UTF-8 path in skill tree: {}", path.display())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .join("/");
            let relative = relative.as_bytes();
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_dir() {
                update_entry(hasher, b'd', relative, &[]);
                visit(root, &path, hasher)?;
            } else if file_type.is_file() {
                let contents = std::fs::read(path).map_err(|error| error.to_string())?;
                update_entry(hasher, b'f', relative, &contents);
            } else {
                return Err(format!(
                    "unsupported entry in directory checksum: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }
    let mut hasher = Sha256::new();
    hasher.update(b"Orbit directory checksum v2\0");
    visit(path, path, &mut hasher)?;
    Ok(format!(
        "{DIRECTORY_CHECKSUM_PREFIX}{:x}",
        hasher.finalize()
    ))
}

pub fn verify_directory_checksum(actual: &str, expected: &str) -> Result<(), String> {
    if !expected.starts_with(DIRECTORY_CHECKSUM_PREFIX) {
        return Err(
            "legacy directory checksum cannot distinguish path and content boundaries; automatic restore is refused and the backup is preserved".into(),
        );
    }
    if actual != expected {
        return Err("directory checksum does not match the recorded backup".into());
    }
    Ok(())
}

pub fn undo(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
) -> Result<(), String> {
    let row = latest_applied_row(config_root, kind, id, action, path)?
        .ok_or_else(|| format!("no recorded {action} for {id}"))?;
    let recorded_after = row
        .after_checksum
        .as_deref()
        .ok_or("the recorded mutation has no verified after state")?;
    let current = std::fs::read(path).map_err(|error| error.to_string())?;
    let current_digest: [u8; 32] = Sha256::digest(&current).into();
    let current_hash = atomic::hex(&current_digest);
    if current_hash != recorded_after {
        return Err(format!(
            "the file changed after the recorded mutation (expected {recorded_after}, found {current_hash}); refusing to undo"
        ));
    }

    match row.before_checksum.as_deref() {
        None => {
            let backup = backup_path(config_root, id, "undo");
            remove_file(config_root, kind, id, "undo", path, current_digest, &backup)
                .map_err(|error| format!("refusing to undo: {error}"))?;
        }
        Some(recorded_before) => {
            let backup = row
                .backup_path
                .as_ref()
                .map(PathBuf::from)
                .ok_or_else(|| "the recorded mutation has no backup to restore".to_string())?;
            let restore = std::fs::read(&backup).map_err(|error| error.to_string())?;
            let backup_hash = atomic::hex(&Sha256::digest(&restore));
            if backup_hash != recorded_before {
                return Err(format!(
                    "backup {} does not match the recorded before state (expected {recorded_before}, found {backup_hash}); refusing to undo",
                    backup.display()
                ));
            }
            let undo_backup = backup_path(config_root, id, "undo");
            restore_file(
                config_root,
                kind,
                id,
                "undo",
                path,
                current_digest,
                &restore,
                Some(&undo_backup),
            )
            .map_err(|error| format!("refusing to undo: {error}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::claude::ClaudeCodeHarness;
    use crate::harness::Harness;

    fn fixture() -> (tempfile::TempDir, ClaudeCodeHarness) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("claude");
        std::fs::create_dir_all(&root).unwrap();
        let paths = crate::harness::HarnessPaths {
            config_root: root,
            claude_json: dir.path().join(".claude.json"),
        };
        (dir, ClaudeCodeHarness::new(paths))
    }

    fn write_project_file(root: &std::path::Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn write_reserves_checksums_backup_bytes_and_target() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{\"v\":1}").unwrap();
        let backup = backup_path(harness.config_root(), "resource", "edit");
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            b"{\"v\":2}",
            Some(&backup),
        )
        .unwrap();
        let row = read(harness.config_root()).unwrap().remove(0);
        assert_eq!(
            row.before_checksum.as_deref(),
            Some(atomic::hex(&Sha256::digest(b"{\"v\":1}")).as_str())
        );
        assert_eq!(
            row.after_checksum.as_deref(),
            Some(atomic::hex(&Sha256::digest(b"{\"v\":2}")).as_str())
        );
        assert_eq!(row.target.as_deref(), Some(path.to_string_lossy().as_ref()));
        assert_eq!(std::fs::read(backup).unwrap(), b"{\"v\":1}");
    }

    #[test]
    fn failed_reservation_leaves_target_unchanged_and_can_be_retried() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"ready":false}"#).unwrap();
        let agent_dir = harness.config_root().join(".harness-agent");
        std::fs::write(&agent_dir, b"blocks ledger directory").unwrap();

        let failed = commit(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            vec![atomic::Patch::SetPath {
                path: atomic::p!["ready"],
                value: serde_json::json!(true),
            }],
            None,
        )
        .unwrap_err();
        assert!(failed.to_string().contains("reservation failed"));
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"ready":false}"#);

        std::fs::remove_file(&agent_dir).unwrap();
        std::fs::create_dir(&agent_dir).unwrap();
        assert!(read_applied(harness.config_root()).unwrap().is_empty());
        commit(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            vec![atomic::Patch::SetPath {
                path: atomic::p!["ready"],
                value: serde_json::json!(true),
            }],
            None,
        )
        .unwrap();
        assert_eq!(
            atomic::read_json(&path).unwrap(),
            serde_json::json!({"ready":true})
        );
        assert_eq!(read_applied(harness.config_root()).unwrap().len(), 1);
    }

    #[test]
    fn pending_failed_write_does_not_hide_the_latest_applied_undo() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"before").unwrap();
        let original_backup = backup_path(harness.config_root(), "resource", "edit");
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            b"applied",
            Some(&original_backup),
        )
        .unwrap();

        let blocker = backup_root(harness.config_root()).join("blocker");
        std::fs::create_dir_all(blocker.parent().unwrap()).unwrap();
        std::fs::write(&blocker, b"not a directory").unwrap();
        let failed = write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            b"not applied",
            Some(&blocker.join("backup")),
        )
        .unwrap_err();
        assert!(failed.to_string().contains("failed after reservation"));
        assert_eq!(std::fs::read(&path).unwrap(), b"applied");

        let rows = read(harness.config_root()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            operation_state(&rows[1]).unwrap(),
            OperationState::NotApplied
        );
        assert_eq!(read_applied(harness.config_root()).unwrap().len(), 1);
        let latest = latest_applied_row(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            latest.after_checksum.as_deref(),
            Some(atomic::hex(&Sha256::digest(b"applied")).as_str())
        );

        undo(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
        )
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"before");
    }

    #[test]
    fn finalization_failure_recovers_from_target_and_backup() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"before").unwrap();
        let backup = backup_path(harness.config_root(), "resource", "edit");
        let op = "settings:resource:edit:simulated";
        let result = atomic::write_file_recorded(
            &path,
            b"after",
            Some(&backup),
            |plan, guard| {
                let row = row_for_plan("settings", "resource", "edit", op, plan);
                reserve_row_locked(harness.config_root(), &row, guard)
            },
            |_, _| Err("simulated ledger finalization failure".to_owned()),
        );
        assert!(matches!(
            result,
            Err(atomic::RecordedMutationError::Finalization(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"after");
        let rows = read(harness.config_root()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, Some(LedgerStatus::Pending));
        assert_eq!(operation_state(&rows[0]).unwrap(), OperationState::Applied);
        assert_eq!(read_applied(harness.config_root()).unwrap().len(), 1);

        let next_backup = backup_path(harness.config_root(), "resource", "edit");
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            b"next",
            Some(&next_backup),
        )
        .unwrap();
        let rows = read(harness.config_root()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, Some(LedgerStatus::Applied));
        assert_eq!(rows[1].status, Some(LedgerStatus::Applied));
        assert_eq!(read_applied(harness.config_root()).unwrap().len(), 2);
    }

    #[test]
    fn undo_targets_the_recorded_file_not_another_project_with_the_same_key() {
        let (dir, harness) = fixture();
        let id = "model";
        let action = "config-project";
        let project_a = write_project_file(dir.path(), "project-a.json", b"{\"before\":\"a\"}");
        let project_b = write_project_file(dir.path(), "project-b.json", b"{\"before\":\"b\"}");
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            id,
            action,
            &project_a,
            b"{\"after\":\"same\"}",
            Some(&backup_path(harness.config_root(), "model-a", action)),
        )
        .unwrap();
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            id,
            action,
            &project_b,
            b"{\"after\":\"same\"}",
            Some(&backup_path(harness.config_root(), "model-b", action)),
        )
        .unwrap();

        undo(
            harness.config_root(),
            ResourceKind::Settings,
            id,
            action,
            &project_a,
        )
        .unwrap();

        assert_eq!(std::fs::read(&project_a).unwrap(), b"{\"before\":\"a\"}");
        assert_eq!(std::fs::read(&project_b).unwrap(), b"{\"after\":\"same\"}");
        let error = undo(
            harness.config_root(),
            ResourceKind::Settings,
            id,
            action,
            &project_a,
        )
        .unwrap_err();
        assert!(error.contains("the file changed after the recorded mutation"));
    }

    #[test]
    fn undo_refuses_when_the_file_drifted_after_the_record() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"before").unwrap();
        let backup = backup_path(harness.config_root(), "resource", "edit");
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            b"after",
            Some(&backup),
        )
        .unwrap();
        std::fs::write(&path, b"drifted").unwrap();

        let error = undo(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
        )
        .unwrap_err();
        assert!(error.contains("the file changed after the recorded mutation"));
        assert_eq!(std::fs::read(path).unwrap(), b"drifted");
    }

    #[test]
    fn undo_refuses_when_no_row_was_recorded() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"anything").unwrap();

        let error = undo(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
        )
        .unwrap_err();
        assert!(error.contains("no recorded edit for resource"));
        assert_eq!(std::fs::read(path).unwrap(), b"anything");
    }

    #[test]
    fn undo_refuses_when_the_recorded_mutation_has_no_backup() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"before").unwrap();
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            b"after",
            None,
        )
        .unwrap();

        let error = undo(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
        )
        .unwrap_err();
        assert!(error.contains("no backup to restore"));
        assert_eq!(std::fs::read(path).unwrap(), b"after");
    }

    #[test]
    fn undo_refuses_a_backup_that_does_not_match_before_checksum() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"before").unwrap();
        let backup = backup_path(harness.config_root(), "resource", "edit");
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
            b"after",
            Some(&backup),
        )
        .unwrap();
        std::fs::write(&backup, b"corrupted").unwrap();

        let error = undo(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &path,
        )
        .unwrap_err();
        assert!(error.contains("does not match the recorded before state"));
        assert_eq!(std::fs::read(path).unwrap(), b"after");
    }

    #[test]
    fn creation_undo_removes_only_the_unchanged_created_file_and_records_the_removal() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "create",
            &path,
            b"created",
            None,
        )
        .unwrap();
        undo(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "create",
            &path,
        )
        .unwrap();
        assert!(!path.exists());
        let rows = read(harness.config_root()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].action, "undo");
        let removal_backup = rows[1].backup_path.as_deref().unwrap();
        assert_eq!(std::fs::read(removal_backup).unwrap(), b"created");
        assert!(rows[1].after_checksum.is_none());
        assert_eq!(
            rows[1].before_checksum.as_deref(),
            Some(atomic::hex(&Sha256::digest(b"created")).as_str())
        );
    }

    #[test]
    fn undo_refuses_a_creation_that_was_drifted_instead_of_removing_it() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        write_file(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "create",
            &path,
            b"created",
            None,
        )
        .unwrap();
        std::fs::write(&path, b"modified after creation").unwrap();

        let error = undo(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "create",
            &path,
        )
        .unwrap_err();
        assert!(error.contains("the file changed after the recorded mutation"));
        assert_eq!(std::fs::read(path).unwrap(), b"modified after creation");
    }

    #[test]
    fn legacy_rows_load_but_are_not_undo_candidates() {
        let (_dir, harness) = fixture();
        let ledger = harness
            .config_root()
            .join(".harness-agent")
            .join("ledger.json");
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        std::fs::write(&ledger, br#"{"rows":[{"timestamp":1,"resourceKind":"settings","id":"resource","action":"edit","backupPath":null,"beforeChecksum":null,"afterChecksum":"aa"}]}"#).unwrap();
        assert_eq!(read(harness.config_root()).unwrap().len(), 1);
        assert!(latest_row(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            Some(&harness.settings_path())
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn backup_paths_stay_under_the_backup_root_for_hostile_ids() {
        let (_dir, harness) = fixture();
        for hostile in [
            "../escape",
            r"..\\..\\escape",
            "a/b",
            r"a\\b",
            "C:\\evil",
            "",
        ] {
            let backup = backup_path(harness.config_root(), hostile, "edit");
            assert_eq!(
                backup.parent().unwrap(),
                backup_root(harness.config_root()),
                "id {hostile:?} escaped the backup root"
            );
        }
    }
}
