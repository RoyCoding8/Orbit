use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::harness::{atomic, ResourceKind};

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

pub fn record(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    receipt: &atomic::Receipt,
) -> Result<(), RecordError> {
    let kind_label = format!("{kind:?}").to_lowercase();
    let op = op_id(&kind_label, id, action);
    let row = LedgerRow {
        timestamp: now_millis(),
        resource_kind: kind_label,
        id: id.to_owned(),
        action: action.to_owned(),
        backup_path: receipt
            .backup
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        before_checksum: receipt.before.as_ref().map(|digest| atomic::hex(digest)),
        after_checksum: Some(atomic::hex(&receipt.after)),
        target: Some(receipt.target.to_string_lossy().into_owned()),
        op: Some(op.clone()),
    };
    append_for_operation(config_root, row, op)
}

#[derive(Debug)]
pub struct RecordError {
    pub op: String,
    pub source: String,
    config_root: PathBuf,
    row: Box<LedgerRow>,
}

impl RecordError {
    pub fn retry(self) -> Result<(), Self> {
        append_row(&self.config_root, &self.row).map_err(|source| Self { source, ..self })
    }
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the mutation landed, but recording operation {} failed: {}",
            self.op, self.source
        )
    }
}

fn append_for_operation(config_root: &Path, row: LedgerRow, op: String) -> Result<(), RecordError> {
    append_row(config_root, &row).map_err(|source| RecordError {
        op,
        source,
        config_root: config_root.to_path_buf(),
        row: Box::new(row),
    })
}

pub fn record_moved(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    backup: Option<&Path>,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<(), RecordError> {
    let kind_label = format!("{kind:?}").to_lowercase();
    let op = op_id(&kind_label, id, action);
    let row = LedgerRow {
        timestamp: now_millis(),
        resource_kind: kind_label,
        id: id.to_owned(),
        action: action.to_owned(),
        backup_path: backup.map(|path| path.to_string_lossy().into_owned()),
        before_checksum: before.map(str::to_owned),
        after_checksum: after.map(str::to_owned),
        target: None,
        op: Some(op.clone()),
    };
    append_for_operation(config_root, row, op)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn append_row(config_root: &Path, row: &LedgerRow) -> Result<(), String> {
    let path = config_root.join(".harness-agent").join("ledger.json");
    std::fs::create_dir_all(path.parent().ok_or("ledger path has no parent")?)
        .map_err(|error| error.to_string())?;
    let row_value = serde_json::to_value(row).map_err(|error| error.to_string())?;
    atomic::commit(
        &path,
        vec![atomic::Patch::Push {
            path: atomic::p!["rows"],
            value: row_value,
            key: Some("op".into()),
        }],
        None,
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
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

pub fn directory_checksum(path: &Path) -> Result<String, String> {
    fn visit(root: &Path, current: &Path, hasher: &mut Sha256) -> Result<(), String> {
        let mut entries = std::fs::read_dir(current)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy();
            hasher.update(relative.as_bytes());
            if path.is_dir() {
                visit(root, &path, hasher)?;
            } else {
                hasher.update(std::fs::read(path).map_err(|error| error.to_string())?);
            }
        }
        Ok(())
    }
    let mut hasher = Sha256::new();
    visit(path, path, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn file_checksum(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    Ok(atomic::hex(&Sha256::digest(&bytes)))
}

pub fn undo(
    config_root: &Path,
    kind: ResourceKind,
    id: &str,
    action: &str,
    path: &Path,
) -> Result<(), String> {
    let row = latest_row(config_root, kind, id, action, Some(path))?
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
            atomic::remove_current(path, current_digest)
                .map_err(|error| format!("refusing to undo: {error}"))?;
            record_moved(
                config_root,
                kind,
                id,
                "undo",
                None,
                Some(recorded_after),
                None,
            )
            .map_err(|error| error.to_string())?;
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
            let receipt = atomic::restore(path, current_digest, &restore, Some(&undo_backup))
                .map_err(|error| format!("refusing to undo: {error}"))?;
            record(config_root, kind, id, "undo", &receipt).map_err(|error| error.to_string())?;
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
    fn record_uses_receipt_checksums_backup_bytes_and_target() {
        let (_dir, harness) = fixture();
        let path = harness.settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{\"v\":1}").unwrap();
        let backup = backup_path(harness.config_root(), "resource", "edit");
        let receipt = atomic::write_file(&path, b"{\"v\":2}", Some(&backup)).unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &receipt,
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
    fn retrying_the_same_audit_row_keeps_one_operation() {
        let (_dir, harness) = fixture();
        let receipt = atomic::write_file(&harness.settings_path(), b"done", None).unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &receipt,
        )
        .unwrap();
        let row = read(harness.config_root()).unwrap().remove(0);
        let error = RecordError {
            op: row.op.clone().unwrap(),
            source: "simulated ambiguous append result".into(),
            config_root: harness.config_root().to_path_buf(),
            row: Box::new(row),
        };
        let op = error.op.clone();
        error.retry().unwrap();
        let rows = read(harness.config_root()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].op.as_deref(), Some(op.as_str()));
    }

    #[test]
    fn undo_targets_the_recorded_file_not_another_project_with_the_same_key() {
        let (dir, harness) = fixture();
        let id = "model";
        let action = "config-project";
        let project_a = write_project_file(dir.path(), "project-a.json", b"{\"before\":\"a\"}");
        let project_b = write_project_file(dir.path(), "project-b.json", b"{\"before\":\"b\"}");
        let receipt_a = atomic::write_file(
            &project_a,
            b"{\"after\":\"same\"}",
            Some(&backup_path(harness.config_root(), "model-a", action)),
        )
        .unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            id,
            action,
            &receipt_a,
        )
        .unwrap();
        let receipt_b = atomic::write_file(
            &project_b,
            b"{\"after\":\"same\"}",
            Some(&backup_path(harness.config_root(), "model-b", action)),
        )
        .unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            id,
            action,
            &receipt_b,
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
        let receipt = atomic::write_file(&path, b"after", Some(&backup)).unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &receipt,
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
        let receipt = atomic::write_file(&path, b"after", None).unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &receipt,
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
        let receipt = atomic::write_file(&path, b"after", Some(&backup)).unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "edit",
            &receipt,
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
        let receipt = atomic::write_file(&path, b"created", None).unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "create",
            &receipt,
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
        assert!(rows[1].backup_path.is_none());
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
        let receipt = atomic::write_file(&path, b"created", None).unwrap();
        record(
            harness.config_root(),
            ResourceKind::Settings,
            "resource",
            "create",
            &receipt,
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
