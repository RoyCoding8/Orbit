//! Ledger-backed, user-facing file and directory history.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    harness::{atomic, Harness, ResourceKind},
    ledger::{self, LedgerRow, OperationState},
};

use crate::harness::claude::ClaudeCodeHarness;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySnapshot {
    pub versions: Vec<HistoryVersion>,
    pub logs: Vec<HistoryLog>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HistoryScope {
    Global,
    Project,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VersionKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HistoryOperationStatus {
    Applied,
    Pending,
    NotApplied,
    Drift,
    Legacy,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BackupStatus {
    Available,
    Missing,
    ChecksumMismatch,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryVersion {
    /// Opaque operation id. The backend resolves it to a ledger row.
    pub id: String,
    pub timestamp: u64,
    pub resource_kind: String,
    pub resource_id: String,
    pub action: String,
    pub scope: HistoryScope,
    /// Stable group key; memory histories use the Claude project slug.
    pub scope_id: String,
    pub scope_label: String,
    pub target: PathBuf,
    pub kind: VersionKind,
    /// Checksum of the backed-up pre-mutation state.
    pub checksum: String,
    /// Checksum of the current target state, or `None` when it is absent.
    pub current_checksum: Option<String>,
    pub backup_status: BackupStatus,
    pub restorable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryLog {
    /// Operation id, or a stable per-load identifier for legacy rows.
    pub id: String,
    pub timestamp: u64,
    pub resource_kind: String,
    pub resource_id: String,
    pub action: String,
    pub scope: HistoryScope,
    pub scope_id: String,
    pub scope_label: String,
    pub target: Option<PathBuf>,
    pub status: HistoryOperationStatus,
    pub backup_status: Option<BackupStatus>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub removed: usize,
    pub missing: usize,
    pub preserved: usize,
}

#[derive(Debug)]
struct TargetScope {
    scope: HistoryScope,
    id: String,
    label: String,
    kind: VersionKind,
}

/// Load current versions and the append-only operation log. Only well-known
/// harness targets enter the versions list; every ledger row remains in logs.
/// Old app-owned payloads are cleaned by the same validated, idempotent rule
/// used by the explicit cleanup command.
pub fn load(harness: &ClaudeCodeHarness, project: &Path) -> Result<HistorySnapshot, String> {
    cleanup_legacy(harness)?;
    let project = std::fs::canonicalize(project).map_err(|error| error.to_string())?;
    if !plain_metadata(&project)?.is_dir() {
        return Err(format!(
            "project root is not a plain directory: {}",
            project.display()
        ));
    }

    let lock_path = harness
        .config_root()
        .join(".harness-agent")
        .join("ledger.json");
    let _guard = atomic::lock_mutations(&lock_path).map_err(|error| error.to_string())?;
    let rows = ledger::read(harness.config_root())?;
    let mut versions = Vec::new();
    let mut logs = Vec::with_capacity(rows.len());

    for (index, row) in rows.iter().enumerate() {
        let scope = row
            .target
            .as_deref()
            .and_then(|target| target_scope(harness, &project, row, Path::new(target)));
        let backup_status = row.backup_path.as_deref().map(|backup| {
            saved_backup_status(
                harness.config_root(),
                Path::new(backup),
                row.before_checksum.as_deref(),
            )
            .unwrap_or(BackupStatus::Unavailable)
        });
        let status = history_status(row);
        let log_scope = scope.as_ref();
        logs.push(HistoryLog {
            id: row
                .op
                .clone()
                .unwrap_or_else(|| format!("legacy:{}:{index}", row.timestamp)),
            timestamp: row.timestamp,
            resource_kind: row.resource_kind.clone(),
            resource_id: row.id.clone(),
            action: row.action.clone(),
            scope: log_scope.map_or(HistoryScope::Other, |value| value.scope),
            scope_id: log_scope.map_or_else(|| "other".into(), |value| value.id.clone()),
            scope_label: log_scope
                .map_or_else(|| "Unclassified target".into(), |value| value.label.clone()),
            target: row.target.as_deref().map(PathBuf::from),
            status,
            backup_status,
        });

        let Some(op) = row.op.as_deref() else {
            continue;
        };
        let Some(target) = row.target.as_deref().map(PathBuf::from) else {
            continue;
        };
        let (Some(backup_status), Some(checksum), Some(scope)) =
            (backup_status, row.before_checksum.as_deref(), scope)
        else {
            continue;
        };
        if !reservation_is_complete(row) || backup_status == BackupStatus::Missing {
            continue;
        }
        let kind = scope.kind;
        let current_result = checksum_if_present(&target);
        let current_checksum = current_result.as_ref().ok().cloned().flatten();
        let restorable = backup_status == BackupStatus::Available
            && current_result.is_ok()
            && matches!(ledger::operation_state(row), Ok(OperationState::Applied));
        versions.push(HistoryVersion {
            id: op.to_owned(),
            timestamp: row.timestamp,
            resource_kind: row.resource_kind.clone(),
            resource_id: row.id.clone(),
            action: row.action.clone(),
            scope: scope.scope,
            scope_id: scope.id,
            scope_label: scope.label,
            target,
            kind,
            checksum: checksum.to_owned(),
            current_checksum,
            backup_status,
            restorable,
        });
    }
    versions.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.id.cmp(&right.id))
    });
    logs.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(HistorySnapshot { versions, logs })
}

/// Restore any previously recorded file or directory state. The caller sends
/// the current checksum returned by `load`; all target changes are checked
/// again while holding the existing mutation lock.
pub fn restore(
    harness: &ClaudeCodeHarness,
    project: &Path,
    id: &str,
    expected_current_checksum: Option<&str>,
) -> Result<(), String> {
    let project = std::fs::canonicalize(project).map_err(|error| error.to_string())?;
    if !plain_metadata(&project)?.is_dir() {
        return Err(format!(
            "project root is not a plain directory: {}",
            project.display()
        ));
    }
    let rows = ledger::read(harness.config_root())?;
    let row = row_by_operation(&rows, id)?.clone();
    if !reservation_is_complete(&row) {
        return Err("this history entry has no complete ledger reservation".into());
    }
    let target_text = row.target.as_deref().ok_or("history entry has no target")?;
    let target_path = PathBuf::from(target_text);
    let scope = target_scope(harness, &project, &row, &target_path).ok_or_else(|| {
        "history target is outside the selected project and known harness files".to_string()
    })?;
    ensure_plain_target_parent(harness, &project, &target_path)?;
    let backup_text = row
        .backup_path
        .as_deref()
        .ok_or("history entry has no backup")?;
    let backup_path = PathBuf::from(backup_text);
    let recorded_checksum = row
        .before_checksum
        .as_deref()
        .ok_or("history entry has no recorded checksum")?;
    match saved_backup_status(harness.config_root(), &backup_path, Some(recorded_checksum))? {
        BackupStatus::Available => {}
        BackupStatus::Missing => return Err("the saved backup is missing".into()),
        BackupStatus::ChecksumMismatch => {
            return Err("the saved backup checksum does not match its recorded checksum".into())
        }
        BackupStatus::Unavailable => return Err("the saved backup is unavailable".into()),
    }
    if !matches!(ledger::operation_state(&row)?, OperationState::Applied) {
        return Err("this history entry did not complete and cannot be restored".into());
    }

    let current = checksum_if_present(&target_path)?;
    if current.as_deref() != expected_current_checksum {
        return Err(stale_error(expected_current_checksum, current.as_deref()));
    }

    match scope.kind {
        VersionKind::File => {
            ensure_plain_file_if_present(&target_path)?;
            let bytes = read_backup_file(harness.config_root(), &backup_path, recorded_checksum)?;
            let expected = expected_current_checksum.map(parse_checksum).transpose()?;
            let restore = match expected {
                Some(expected) => atomic::RecordedWrite::Restore(expected, &bytes),
                None => atomic::RecordedWrite::Create(&bytes),
            };
            let action = "history-restore";
            let backup = if expected_current_checksum.is_some() {
                Some(ledger::backup_path(harness.config_root(), &row.id, action))
            } else {
                None
            };
            ledger::write(
                harness.config_root(),
                resource_kind(&row.resource_kind)?,
                &row.id,
                action,
                &target_path,
                restore,
                backup.as_deref(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
        }
        VersionKind::Directory => restore_directory(
            harness,
            &project,
            &row,
            &target_path,
            &backup_path,
            recorded_checksum,
            expected_current_checksum,
        ),
    }
}

/// Delete only the exact payload named by a complete ledger row. The audit
/// row remains in place and shared payloads cannot be removed independently.
pub fn delete(harness: &ClaudeCodeHarness, id: &str) -> Result<(), String> {
    let rows = ledger::read(harness.config_root())?;
    let row = row_by_operation(&rows, id)?;
    if !reservation_is_complete(row) {
        return Err("this history entry has no complete ledger reservation".into());
    }
    let backup = row
        .backup_path
        .as_deref()
        .ok_or("history entry has no backup")?;
    let checksum = row
        .before_checksum
        .as_deref()
        .ok_or("history entry has no recorded checksum")?;
    let backup = PathBuf::from(backup);
    if rows.iter().any(|other| {
        other.op.as_deref() != Some(id)
            && other.backup_path.as_deref().map(Path::new) == Some(backup.as_path())
    }) {
        return Err(
            "this payload is shared by another ledger entry and cannot be deleted alone".into(),
        );
    }
    let lock_path = ledger::backup_root(harness.config_root());
    let _guard = atomic::lock_mutations(&lock_path).map_err(|error| error.to_string())?;
    let Some(kind) = app_backup_kind(harness.config_root(), &backup)? else {
        return Ok(());
    };
    let actual = artifact_checksum(&backup, kind)?;
    if actual != checksum {
        return Err(
            "the saved payload no longer matches its ledger checksum; preserving it".into(),
        );
    }
    remove_plain_artifact(&backup, kind)
}

/// Remove obsolete payloads only when a ledger row identifies the app-owned
/// backup and no complete modern row shares it. Unknown orphan files are left
/// untouched because their ownership cannot be established.
pub fn cleanup_legacy(harness: &ClaudeCodeHarness) -> Result<CleanupReport, String> {
    let backup_root = ledger::backup_root(harness.config_root());
    let _guard = atomic::lock_mutations(&backup_root).map_err(|error| error.to_string())?;
    let rows = ledger::read(harness.config_root())?;
    if !path_exists_no_follow(&backup_root)? {
        return Ok(CleanupReport::default());
    }
    ensure_plain_directory(&backup_root)?;

    let mut report = CleanupReport::default();
    let mut visited = std::collections::HashSet::new();
    for row in &rows {
        let Some(path) = row.backup_path.as_deref().map(PathBuf::from) else {
            continue;
        };
        if !visited.insert(path.clone()) {
            continue;
        }
        let kind = match app_backup_kind(harness.config_root(), &path) {
            Ok(Some(kind)) => kind,
            Ok(None) => {
                report.missing += 1;
                continue;
            }
            Err(_) => {
                report.preserved += 1;
                continue;
            }
        };
        let linked_rows: Vec<_> = rows
            .iter()
            .filter(|candidate| {
                candidate.backup_path.as_deref().map(Path::new) == Some(path.as_path())
            })
            .collect();
        let legacy_candidate = linked_rows.iter().any(|candidate| {
            !reservation_is_complete(candidate)
                || (kind == VersionKind::Directory
                    && candidate
                        .before_checksum
                        .as_deref()
                        .is_some_and(|checksum| !checksum.starts_with("tree-v2-sha256:")))
        });
        if !legacy_candidate {
            continue;
        }
        let shared_with_modern = linked_rows.iter().any(|candidate| {
            reservation_is_complete(candidate)
                && candidate
                    .before_checksum
                    .as_deref()
                    .is_some_and(|checksum| {
                        checksum.starts_with("tree-v2-sha256:") || kind == VersionKind::File
                    })
                && candidate
                    .before_checksum
                    .as_deref()
                    .is_some_and(|checksum| {
                        saved_backup_status(harness.config_root(), &path, Some(checksum))
                            == Ok(BackupStatus::Available)
                    })
        });
        if shared_with_modern {
            report.preserved += 1;
            continue;
        }
        let expected = linked_rows
            .iter()
            .filter_map(|candidate| candidate.before_checksum.as_deref())
            .find(|checksum| checksum_matches_kind(kind, checksum));
        if kind == VersionKind::File {
            let Some(expected) = expected else {
                report.preserved += 1;
                continue;
            };
            if artifact_checksum(&path, kind)? != expected {
                report.preserved += 1;
                continue;
            }
        } else if !plain_tree_is_valid(&path).unwrap_or(false) {
            report.preserved += 1;
            continue;
        }
        match remove_plain_artifact(&path, kind) {
            Ok(()) => report.removed += 1,
            Err(_) => report.preserved += 1,
        }
    }
    Ok(report)
}

fn restore_directory(
    harness: &ClaudeCodeHarness,
    project: &Path,
    row: &LedgerRow,
    target: &Path,
    source_backup: &Path,
    source_checksum: &str,
    expected_current: Option<&str>,
) -> Result<(), String> {
    let guard = atomic::lock_mutations(target).map_err(|error| error.to_string())?;
    ensure_plain_target_parent(harness, project, target)?;
    let current = checksum_if_present(target)?;
    if current.as_deref() != expected_current {
        return Err(stale_error(expected_current, current.as_deref()));
    }
    if current.is_some() && !plain_tree_is_valid(target)? {
        return Err("current directory contains a link or unsupported file; preserving it".into());
    }
    let source_actual = artifact_checksum(source_backup, VersionKind::Directory)?;
    ledger::verify_directory_checksum(&source_actual, source_checksum)?;

    let parent = target
        .parent()
        .ok_or("history target has no parent directory")?;
    let stage = unique_sibling(target, "history-stage");
    if path_exists_no_follow(&stage)? {
        return Err("temporary restore directory already exists".into());
    }
    copy_plain_directory(source_backup, &stage)?;
    let staged_checksum = ledger::directory_checksum(&stage)?;
    ledger::verify_directory_checksum(&staged_checksum, source_checksum)?;

    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let backup = if current.is_some() {
        let backup = ledger::backup_path(harness.config_root(), &row.id, "history-restore");
        ensure_app_backup_root(harness.config_root())?;
        if path_exists_no_follow(&backup)? {
            let _ = std::fs::remove_dir_all(&stage);
            return Err("history restore backup already exists".into());
        }
        Some(backup)
    } else {
        None
    };
    let plan = atomic::MutationPlan {
        target: target.to_path_buf(),
        backup: backup.clone(),
        before_checksum: current.clone(),
        after_checksum: Some(source_checksum.to_owned()),
    };
    let op = ledger::reserve_mutation(
        harness.config_root(),
        resource_kind(&row.resource_kind)?,
        &row.id,
        "history-restore",
        &plan,
        &guard,
    )
    .inspect_err(|_error| {
        let _ = std::fs::remove_dir_all(&stage);
    })?;

    if let (Some(backup), Some(_)) = (backup.as_deref(), current.as_deref()) {
        if let Err(error) = copy_plain_directory(target, backup) {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(format!(
                "reserved restore {op} could not preserve the current directory: {error}"
            ));
        }
        let backup_checksum = ledger::directory_checksum(backup)?;
        ledger::verify_directory_checksum(&backup_checksum, current.as_deref().unwrap())?;
    }

    if current.is_some() {
        if let Err(error) = std::fs::remove_dir_all(target) {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(format!(
                "reserved restore {op} could not replace {}: {error}",
                target.display()
            ));
        }
    }
    if let Err(error) = std::fs::rename(&stage, target) {
        if let (Some(backup), Some(_)) = (backup.as_deref(), current.as_deref()) {
            let _ = copy_plain_directory(backup, target);
        }
        let _ = std::fs::remove_dir_all(&stage);
        return Err(format!(
            "reserved restore {op} could not install the saved directory: {error}"
        ));
    }
    let restored = ledger::directory_checksum(target)?;
    ledger::verify_directory_checksum(&restored, source_checksum)?;
    ledger::complete_mutation(harness.config_root(), &op, &guard)
}

fn row_by_operation<'a>(rows: &'a [LedgerRow], id: &str) -> Result<&'a LedgerRow, String> {
    if id.trim().is_empty() {
        return Err("history entry id is empty".into());
    }
    let mut matches = rows.iter().filter(|row| row.op.as_deref() == Some(id));
    match (matches.next(), matches.next()) {
        (Some(row), None) => Ok(row),
        (Some(_), Some(_)) => Err("history operation id is ambiguous".into()),
        (None, _) => Err("history entry no longer exists".into()),
    }
}

fn history_status(row: &LedgerRow) -> HistoryOperationStatus {
    if !reservation_is_complete(row) {
        return HistoryOperationStatus::Legacy;
    }
    match ledger::operation_state(row) {
        Ok(OperationState::Applied) => HistoryOperationStatus::Applied,
        Ok(OperationState::NotApplied) => HistoryOperationStatus::NotApplied,
        Ok(OperationState::Drift) => HistoryOperationStatus::Drift,
        Err(_) => HistoryOperationStatus::Unavailable,
    }
}

fn reservation_is_complete(row: &LedgerRow) -> bool {
    row.op.is_some() && row.target.is_some() && row.status.is_some()
}

fn target_scope(
    harness: &ClaudeCodeHarness,
    project: &Path,
    row: &LedgerRow,
    target: &Path,
) -> Option<TargetScope> {
    let canonical_config = canonical_location(harness.config_root())?;
    let config_root = canonical_config.as_path();
    let canonical_target = canonical_location(target)?;
    let target = canonical_target.as_path();
    let kind = row.resource_kind.as_str();
    let global_path = |path: PathBuf, version_kind| {
        (target == path).then(|| TargetScope {
            scope: HistoryScope::Global,
            id: "global".into(),
            label: "Global configuration".into(),
            kind: version_kind,
        })
    };
    if kind == "settings" {
        if let Some(scope) = global_path(config_root.join("settings.json"), VersionKind::File) {
            return Some(scope);
        }
        if target == project.join(".claude/settings.json")
            || target == project.join(".claude/settings.local.json")
        {
            return Some(TargetScope {
                scope: HistoryScope::Project,
                id: project.to_string_lossy().into_owned(),
                label: project_label(project),
                kind: VersionKind::File,
            });
        }
    }
    if kind == "mcpserver" {
        if let Some(scope) = global_path(
            canonical_location(&harness.claude_json_path())?,
            VersionKind::File,
        ) {
            return Some(scope);
        }
    }
    if kind == "globalinstructions" {
        if let Some(scope) = global_path(config_root.join("CLAUDE.md"), VersionKind::File) {
            return Some(scope);
        }
        if markdown_below(&config_root.join("rules"), target) {
            return Some(TargetScope {
                scope: HistoryScope::Global,
                id: "global-instructions".into(),
                label: "User instructions".into(),
                kind: VersionKind::File,
            });
        }
    }
    if kind == "projectinstructions"
        && ([
            "CLAUDE.md",
            "CLAUDE.local.md",
            ".claude/CLAUDE.md",
            ".claude/CLAUDE.local.md",
        ]
        .iter()
        .any(|name| target == project.join(name))
            || markdown_below(&project.join(".claude/rules"), target))
    {
        return Some(TargetScope {
            scope: HistoryScope::Project,
            id: project.to_string_lossy().into_owned(),
            label: project_label(project),
            kind: VersionKind::File,
        });
    }
    if kind == "skill" {
        let skills_root = config_root.join("skills");
        let directory = skills_root.join(&row.id);
        if !safe_single_component(&row.id) {
            return None;
        }
        let version_kind = if target == directory {
            VersionKind::Directory
        } else if target == directory.join("SKILL.md") {
            VersionKind::File
        } else {
            return None;
        };
        return Some(TargetScope {
            scope: HistoryScope::Global,
            id: "global-skills".into(),
            label: "Global skills".into(),
            kind: version_kind,
        });
    }
    if kind == "memory" {
        if let Some(slug) = memory_project_slug(config_root, target) {
            return Some(TargetScope {
                scope: HistoryScope::Project,
                id: format!("claude-memory:{slug}"),
                label: format!("Project memory · {slug}"),
                kind: VersionKind::File,
            });
        }
        let custom =
            crate::harness::claude::memories::configured_memory_directory(harness, project)
                .and_then(|path| path.canonicalize().ok());
        if custom
            .as_ref()
            .is_some_and(|root| root.starts_with(project) && markdown_below(root, target))
        {
            return Some(TargetScope {
                scope: HistoryScope::Project,
                id: project.to_string_lossy().into_owned(),
                label: format!("Project memory · {}", project_label(project)),
                kind: VersionKind::File,
            });
        }
    }
    None
}

fn canonical_location(path: &Path) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if let Ok(root) = ancestor.canonicalize() {
            let relative = path.strip_prefix(ancestor).ok()?;
            if relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            {
                return Some(root.join(relative));
            }
            return None;
        }
    }
    None
}

fn memory_project_slug<'a>(config_root: &Path, target: &'a Path) -> Option<&'a str> {
    let relative = target.strip_prefix(config_root.join("projects")).ok()?;
    let mut components = relative.components();
    let slug = match components.next()? {
        Component::Normal(value) => value.to_str()?,
        _ => return None,
    };
    if !matches!(components.next()?, Component::Normal(value) if value == "memory") {
        return None;
    }
    if !markdown_below(
        &config_root.join("projects").join(slug).join("memory"),
        target,
    ) || slug.is_empty()
    {
        return None;
    }
    Some(slug)
}

fn markdown_below(root: &Path, target: &Path) -> bool {
    target.strip_prefix(root).is_ok_and(|relative| {
        relative.components().count() > 0
            && relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
    }) && target
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn project_label(project: &Path) -> String {
    project
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| project.to_str().unwrap_or("Project"))
        .to_owned()
}

fn safe_single_component(value: &str) -> bool {
    !value.is_empty()
        && Path::new(value).components().count() == 1
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && !value.contains(['/', '\\', ':'])
}

fn resource_kind(value: &str) -> Result<ResourceKind, String> {
    match value {
        "skill" => Ok(ResourceKind::Skill),
        "memory" => Ok(ResourceKind::Memory),
        "globalinstructions" => Ok(ResourceKind::GlobalInstructions),
        "projectinstructions" => Ok(ResourceKind::ProjectInstructions),
        "settings" => Ok(ResourceKind::Settings),
        "mcpserver" => Ok(ResourceKind::McpServer),
        _ => Err(format!("unsupported history resource kind: {value}")),
    }
}

fn stale_error(expected: Option<&str>, actual: Option<&str>) -> String {
    format!(
        "target changed since history loaded (expected {}, found {}); refresh history before restoring",
        expected.unwrap_or("absent"),
        actual.unwrap_or("absent")
    )
}

fn parse_checksum(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err("file checksum is malformed".into());
    }
    let mut bytes = [0_u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "file checksum is malformed")?;
    }
    Ok(bytes)
}

fn checksum_if_present(path: &Path) -> Result<Option<String>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if atomic::is_link_or_reparse(&metadata) {
        return Err(format!("refusing to read a link: {}", path.display()));
    }
    if metadata.is_file() {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        Ok(Some(atomic::hex(&Sha256::digest(bytes))))
    } else if metadata.is_dir() {
        Ok(Some(ledger::directory_checksum(path)?))
    } else {
        Err(format!("unsupported history target: {}", path.display()))
    }
}

fn saved_backup_status(
    config_root: &Path,
    path: &Path,
    expected: Option<&str>,
) -> Result<BackupStatus, String> {
    let Some(kind) = app_backup_kind(config_root, path)? else {
        return Ok(BackupStatus::Missing);
    };
    let Some(expected) = expected else {
        return Ok(BackupStatus::Unavailable);
    };
    if !checksum_matches_kind(kind, expected) || artifact_checksum(path, kind)? != expected {
        return Ok(BackupStatus::ChecksumMismatch);
    }
    Ok(BackupStatus::Available)
}

fn checksum_matches_kind(kind: VersionKind, checksum: &str) -> bool {
    match kind {
        VersionKind::File => checksum.len() == 64 && !checksum.starts_with("tree-v2-sha256:"),
        VersionKind::Directory => checksum.starts_with("tree-v2-sha256:"),
    }
}

fn artifact_checksum(path: &Path, kind: VersionKind) -> Result<String, String> {
    match kind {
        VersionKind::File => {
            ensure_plain_file(path)?;
            let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
            Ok(atomic::hex(&Sha256::digest(bytes)))
        }
        VersionKind::Directory => {
            ensure_plain_tree(path)?;
            ledger::directory_checksum(path)
        }
    }
}

fn app_backup_kind(config_root: &Path, path: &Path) -> Result<Option<VersionKind>, String> {
    let backup_root = ledger::backup_root(config_root);
    let relative = match path.strip_prefix(&backup_root) {
        Ok(relative) => relative,
        Err(_) => return Err("backup path is outside the app-owned backup directory".into()),
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("backup path is not a direct app-owned artifact".into());
    }
    let Some(root_metadata) = metadata_if_present(&backup_root)? else {
        return Ok(None);
    };
    if atomic::is_link_or_reparse(&root_metadata) || !root_metadata.is_dir() {
        return Err("app-owned backup directory is not a plain directory".into());
    }
    let mut current = backup_root;
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err("backup path contains an unsupported component".into());
        };
        current.push(name);
        let Some(metadata) = metadata_if_present(&current)? else {
            return Ok(None);
        };
        if atomic::is_link_or_reparse(&metadata) {
            return Err(format!(
                "refusing to follow a backup link: {}",
                current.display()
            ));
        }
        let last = index + 1 == components.len();
        if !last && !metadata.is_dir() {
            return Err("backup path parent is not a directory".into());
        }
        if last {
            if metadata.is_file() {
                return Ok(Some(VersionKind::File));
            }
            if metadata.is_dir() {
                return Ok(Some(VersionKind::Directory));
            }
            return Err("backup payload has an unsupported type".into());
        }
    }
    Ok(None)
}

fn ensure_app_backup_root(config_root: &Path) -> Result<PathBuf, String> {
    let root = ledger::backup_root(config_root);
    ensure_plain_directory(&root)?;
    Ok(root)
}

fn read_backup_file(config_root: &Path, path: &Path, expected: &str) -> Result<Vec<u8>, String> {
    if app_backup_kind(config_root, path)? != Some(VersionKind::File) {
        return Err("saved file version is unavailable".into());
    }
    ensure_plain_file(path)?;
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if atomic::hex(&Sha256::digest(&bytes)) != expected {
        return Err("saved file version no longer matches its checksum".into());
    }
    Ok(bytes)
}

fn path_exists_no_follow(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn metadata_if_present(path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn ensure_plain_file(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!("expected a plain file: {}", path.display()));
    }
    Ok(())
}

fn ensure_plain_file_if_present(path: &Path) -> Result<(), String> {
    match metadata_if_present(path)? {
        Some(metadata) if atomic::is_link_or_reparse(&metadata) || !metadata.is_file() => {
            Err(format!("expected a plain file: {}", path.display()))
        }
        _ => Ok(()),
    }
}

fn ensure_plain_directory(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!("expected a plain directory: {}", path.display()));
    }
    Ok(())
}

fn plain_metadata(path: &Path) -> Result<std::fs::Metadata, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if atomic::is_link_or_reparse(&metadata) {
        return Err(format!(
            "refusing a link or reparse point: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

fn ensure_plain_target_parent(
    harness: &ClaudeCodeHarness,
    project: &Path,
    target: &Path,
) -> Result<(), String> {
    let canonical_config =
        canonical_location(harness.config_root()).ok_or("configuration root cannot be resolved")?;
    let config_root = canonical_config.as_path();
    let claude_json =
        canonical_location(&harness.claude_json_path()).ok_or("state path cannot be resolved")?;
    let canonical_target = canonical_location(target).ok_or("history target cannot be resolved")?;
    let target = canonical_target.as_path();
    let root = if target.starts_with(config_root) {
        config_root.to_path_buf()
    } else if target.starts_with(project) {
        project.to_path_buf()
    } else if target == claude_json {
        claude_json
            .parent()
            .ok_or("global state file has no parent")?
            .to_path_buf()
    } else {
        return Err("history target parent is outside a known root".into());
    };
    let parent = target.parent().ok_or("history target has no parent")?;
    let relative = parent
        .strip_prefix(&root)
        .map_err(|_| "history target parent is outside a known root")?;
    let mut current = root;
    ensure_plain_directory(&current)?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("history target parent has an unsupported path component".into());
        };
        current.push(name);
        match metadata_if_present(&current)? {
            Some(metadata) if atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() => {
                return Err(format!(
                    "history target parent is not a plain directory: {}",
                    current.display()
                ))
            }
            Some(_) => {}
            None => break,
        }
    }
    Ok(())
}

fn ensure_plain_tree(path: &Path) -> Result<(), String> {
    ensure_plain_directory(path)?;
    for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let child = entry.path();
        let metadata = plain_metadata(&child)?;
        if metadata.is_dir() {
            ensure_plain_tree(&child)?;
        } else if !metadata.is_file() {
            return Err(format!("unsupported backup entry: {}", child.display()));
        }
    }
    Ok(())
}

fn plain_tree_is_valid(path: &Path) -> Result<bool, String> {
    match ensure_plain_tree(path) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

fn copy_plain_directory(source: &Path, destination: &Path) -> Result<(), String> {
    ensure_plain_tree(source)?;
    std::fs::create_dir(destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = plain_metadata(&from)?;
        if metadata.is_dir() {
            copy_plain_directory(&from, &to)?;
        } else if metadata.is_file() {
            std::fs::copy(&from, &to).map_err(|error| error.to_string())?;
            std::fs::set_permissions(&to, metadata.permissions())
                .map_err(|error| error.to_string())?;
        } else {
            return Err(format!("unsupported directory entry: {}", from.display()));
        }
    }
    Ok(())
}

fn unique_sibling(target: &Path, label: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let digest = atomic::hex(&Sha256::digest(target.to_string_lossy().as_bytes()));
    target
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{label}-{}-{stamp}", &digest[..12]))
}

fn remove_plain_artifact(path: &Path, kind: VersionKind) -> Result<(), String> {
    match kind {
        VersionKind::File => {
            ensure_plain_file(path)?;
            std::fs::remove_file(path).map_err(|error| error.to_string())
        }
        VersionKind::Directory => {
            ensure_plain_tree(path)?;
            std::fs::remove_dir_all(path).map_err(|error| error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{claude::skills, HarnessPaths};

    fn fixture() -> (tempfile::TempDir, ClaudeCodeHarness, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("claude");
        let project = dir.path().join("project");
        std::fs::create_dir_all(config_root.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            config_root.join("skills/alpha/SKILL.md"),
            "original skill\n",
        )
        .unwrap();
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            claude_json: config_root.join(".claude.json"),
            config_root,
        });
        (dir, harness, project)
    }

    #[test]
    fn restores_an_older_file_version_after_later_edits_and_records_the_replaced_state() {
        let (_dir, harness, project) = fixture();
        let target = harness.config_root().join("skills/alpha/SKILL.md");
        skills::edit_skill(&harness, "alpha", "original skill\n", "second version\n").unwrap();
        skills::edit_skill(&harness, "alpha", "second version\n", "latest version\n").unwrap();
        let snapshot = load(&harness, &project).unwrap();
        let first = snapshot
            .versions
            .iter()
            .find(|version| version.checksum == atomic::hex(&Sha256::digest(b"original skill\n")))
            .unwrap();
        let id = first.id.clone();
        let expected_current = first.current_checksum.clone();

        restore(&harness, &project, &id, expected_current.as_deref()).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"original skill\n");
        let rows = ledger::read(harness.config_root()).unwrap();
        assert_eq!(rows.last().unwrap().action, "history-restore");
        let replacement_backup =
            PathBuf::from(rows.last().unwrap().backup_path.as_deref().unwrap());
        assert_eq!(
            std::fs::read(replacement_backup).unwrap(),
            b"latest version\n"
        );
    }

    #[test]
    fn refuses_stale_current_checksum_without_recording_or_writing() {
        let (_dir, harness, project) = fixture();
        let target = harness.config_root().join("skills/alpha/SKILL.md");
        skills::edit_skill(&harness, "alpha", "original skill\n", "recorded edit\n").unwrap();
        let version = load(&harness, &project).unwrap().versions.remove(0);
        std::fs::write(&target, b"external edit\n").unwrap();
        let row_count = ledger::read(harness.config_root()).unwrap().len();

        let error = restore(
            &harness,
            &project,
            &version.id,
            version.current_checksum.as_deref(),
        )
        .unwrap_err();

        assert!(error.contains("target changed since history loaded"));
        assert_eq!(std::fs::read(target).unwrap(), b"external edit\n");
        assert_eq!(
            ledger::read(harness.config_root()).unwrap().len(),
            row_count
        );
    }

    #[test]
    fn deleting_a_payload_keeps_its_log_row() {
        let (_dir, harness, project) = fixture();
        skills::edit_skill(&harness, "alpha", "original skill\n", "edited\n").unwrap();
        let before = load(&harness, &project).unwrap();
        let version = before.versions.first().unwrap();
        let id = version.id.clone();
        let log_id = id.clone();

        delete(&harness, &id).unwrap();

        let after = load(&harness, &project).unwrap();
        assert!(after
            .logs
            .iter()
            .any(|log| log.id == log_id && log.action == "edit"));
        assert!(!after.versions.iter().any(|entry| entry.id == id));
        assert_eq!(
            after
                .logs
                .iter()
                .find(|log| log.id == id)
                .unwrap()
                .backup_status,
            Some(BackupStatus::Missing)
        );
        delete(&harness, &id).unwrap();
        let error =
            restore(&harness, &project, &id, version.current_checksum.as_deref()).unwrap_err();
        assert_eq!(error, "the saved backup is missing");
    }

    #[test]
    fn a_changed_backup_reports_checksum_mismatch_and_is_preserved() {
        let (_dir, harness, project) = fixture();
        skills::edit_skill(&harness, "alpha", "original skill\n", "new skill\n").unwrap();
        let row = ledger::read(harness.config_root()).unwrap().remove(0);
        let backup = Path::new(row.backup_path.as_deref().unwrap());
        std::fs::write(backup, "externally changed backup").unwrap();
        let snapshot = load(&harness, &project).unwrap();
        let version = snapshot.versions.first().unwrap();
        assert_eq!(version.backup_status, BackupStatus::ChecksumMismatch);
        assert!(!version.restorable);
        assert_eq!(
            snapshot.logs[0].backup_status,
            Some(BackupStatus::ChecksumMismatch)
        );
        assert_eq!(
            restore(
                &harness,
                &project,
                &version.id,
                version.current_checksum.as_deref()
            )
            .unwrap_err(),
            "the saved backup checksum does not match its recorded checksum"
        );
        assert!(delete(&harness, &version.id)
            .unwrap_err()
            .contains("checksum"));
        assert_eq!(
            std::fs::read_to_string(backup).unwrap(),
            "externally changed backup"
        );
    }

    #[test]
    fn restores_directory_versions_over_present_and_absent_targets() {
        let (_dir, harness, project) = fixture();
        let target = harness.config_root().join("skills/alpha");
        skills::uninstall(&harness, "alpha").unwrap();
        let version = load(&harness, &project).unwrap().versions.remove(0);
        assert_eq!(version.kind, VersionKind::Directory);
        assert_eq!(version.current_checksum, None);

        restore(&harness, &project, &version.id, None).unwrap();
        assert_eq!(
            std::fs::read(target.join("SKILL.md")).unwrap(),
            b"original skill\n"
        );

        let current = ledger::directory_checksum(&target).unwrap();
        std::fs::write(target.join("SKILL.md"), b"later skill\n").unwrap();
        let changed = ledger::directory_checksum(&target).unwrap();
        assert_ne!(changed, current);
        restore(&harness, &project, &version.id, Some(&changed)).unwrap();
        assert_eq!(
            std::fs::read(target.join("SKILL.md")).unwrap(),
            b"original skill\n"
        );
        let rows = ledger::read(harness.config_root()).unwrap();
        assert_eq!(rows.last().unwrap().action, "history-restore");
    }

    #[test]
    fn cleanup_removes_only_known_legacy_payloads_and_preserves_logs_and_modern_backups() {
        let (_dir, harness, _project) = fixture();
        let root = ledger::backup_root(harness.config_root());
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("old-file");
        let outside = harness
            .config_root()
            .parent()
            .unwrap()
            .join("outside-backup");
        let modern = root.join("modern-file");
        let orphan = root.join("orphan-file");
        std::fs::write(&old, b"legacy").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::write(&modern, b"modern").unwrap();
        std::fs::write(&orphan, b"orphan").unwrap();
        let ledger_path = harness.config_root().join(".harness-agent/ledger.json");
        std::fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
        let modern_hash = atomic::hex(&Sha256::digest(b"modern"));
        let old_hash = atomic::hex(&Sha256::digest(b"legacy"));
        let outside_hash = atomic::hex(&Sha256::digest(b"outside"));
        std::fs::write(
            &ledger_path,
            serde_json::to_vec(&serde_json::json!({"rows": [
                {"timestamp":1,"resourceKind":"skill","id":"alpha","action":"edit","backupPath":old,"beforeChecksum":old_hash,"afterChecksum":"aa"},
                {"timestamp":2,"resourceKind":"skill","id":"beta","action":"edit","backupPath":outside,"beforeChecksum":outside_hash,"afterChecksum":"bb"},
                {"timestamp":3,"resourceKind":"skill","id":"gamma","action":"edit","backupPath":modern,"beforeChecksum":modern_hash,"afterChecksum":"cc","target":harness.config_root().join("skills/gamma/SKILL.md"),"op":"op-modern","status":"applied"}
            ]})).unwrap(),
        )
        .unwrap();

        let report = cleanup_legacy(&harness).unwrap();

        assert_eq!(report.removed, 1);
        assert!(!old.exists());
        assert!(outside.exists());
        assert!(modern.exists());
        assert!(orphan.exists());
        assert_eq!(ledger::read(harness.config_root()).unwrap().len(), 3);
    }

    #[test]
    fn memory_and_instruction_edits_have_restorable_versions_in_their_actual_scope() {
        use crate::harness::claude::memories;
        let (_dir, harness, project) = fixture();
        let custom = project.join("custom-memory");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(
            harness.config_root().join("settings.json"),
            serde_json::json!({"autoMemoryDirectory": custom}).to_string(),
        )
        .unwrap();
        for (target, scope, kind) in [
            (
                harness
                    .config_root()
                    .join("projects/project-alpha/memory/nested/topic.md"),
                HistoryScope::Project,
                "memory",
            ),
            (
                harness.config_root().join("CLAUDE.md"),
                HistoryScope::Global,
                "globalinstructions",
            ),
            (
                harness.config_root().join("rules/nested/lesson.md"),
                HistoryScope::Global,
                "globalinstructions",
            ),
            (
                project.join("CLAUDE.md"),
                HistoryScope::Project,
                "projectinstructions",
            ),
            (
                project.join(".claude/rules/test.md"),
                HistoryScope::Project,
                "projectinstructions",
            ),
            (custom.join("topic.md"), HistoryScope::Project, "memory"),
        ] {
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, "before").unwrap();
            let target = target.canonicalize().unwrap();
            let catalog = memories::catalog(&harness, &project).unwrap();
            let file = catalog
                .files
                .iter()
                .find(|file| Path::new(&file.path) == target)
                .unwrap();
            memories::edit(&harness, &project, &file.id, "before", "after").unwrap();
            let snapshot = load(&harness, &project).unwrap();
            let version = snapshot
                .versions
                .iter()
                .find(|version| version.target == target && version.action == "edit")
                .unwrap();
            assert_eq!(version.scope, scope);
            assert_eq!(version.resource_kind, kind);
            assert!(version.restorable);
            if target.to_string_lossy().contains("project-alpha") {
                assert_eq!(version.scope_id, "claude-memory:project-alpha");
            }
            restore(
                &harness,
                &project,
                &version.id,
                version.current_checksum.as_deref(),
            )
            .unwrap();
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "before");
        }
    }
}
