//! Single-attempt verified mutations for harness-owned files.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{HarnessError, Result};

static MUTATION_LOCK: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// A non-empty chain of object keys addressing one value in a JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPath(Vec<String>);

impl JsonPath {
    pub fn new(keys: Vec<String>) -> Self {
        assert!(!keys.is_empty(), "JsonPath must address at least one key");
        Self(keys)
    }

    fn keys(&self) -> &[String] {
        &self.0
    }
}

/// Construct a [`JsonPath`] from key literals: `p!["mcpServers", name]`.
macro_rules! p {
    ($($key:expr),+ $(,)?) => {
        $crate::harness::atomic::JsonPath::new(vec![$($key.to_string()),+])
    };
}
pub(crate) use p;

/// One targeted change to a JSON document.
#[derive(Debug, PartialEq)]
pub enum Patch {
    SetPath {
        path: JsonPath,
        value: Value,
    },
    RemovePath {
        path: JsonPath,
    },
    Push {
        path: JsonPath,
        value: Value,
        /// Deduplicate by this field, or by the whole value when absent.
        key: Option<String>,
    },
    ReplaceWhere {
        path: JsonPath,
        matches: Value,
        value: Value,
    },
    /// Replace the leaf at an existing object-key `path` only while the locked
    /// document still holds `matches` there. Unlike [`Patch::SetPath`] it never
    /// creates missing parents, so absent and explicit null never conflate.
    ReplacePath {
        path: JsonPath,
        matches: Value,
        value: Value,
    },
    RemoveWhere {
        path: JsonPath,
        matches: Value,
    },
}

/// Checksums and backup path for one verified write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// Where the exact original bytes were preserved, when a file existed.
    pub backup: Option<PathBuf>,
    /// Hash of the exact original bytes, matching the backup when present.
    pub before: Option<[u8; 32]>,
    /// Hash of the bytes actually read back after the write.
    pub after: [u8; 32],
    /// The file the mutation targeted.
    pub target: PathBuf,
}

/// The state a ledger reservation expects immediately before and after a
/// mutation. `None` means the target is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MutationPlan {
    pub target: PathBuf,
    pub backup: Option<PathBuf>,
    pub before_checksum: Option<String>,
    pub after_checksum: Option<String>,
}

/// Holds the process-wide mutation lock while a reservation and its mutation
/// are committed as one recoverable operation.
pub(crate) struct MutationGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

pub(crate) enum RecordedMutationError<E> {
    Preparation(CommitError),
    Reservation(E),
    Mutation(CommitError),
    Finalization(E),
}

pub(crate) enum RecordedWrite<'a> {
    Patches(Vec<Patch>),
    #[cfg(test)]
    Bytes(&'a [u8]),
    Restore([u8; 32], &'a [u8]),
    /// Create a file only while the locked target is absent. Unlike the test
    /// fixture byte writer, this remains conditional in production.
    Create(&'a [u8]),
}

/// Errors from [`commit`] and [`write_file`].
#[derive(Debug)]
pub enum CommitError {
    Shape {
        path: PathBuf,
    },
    Stale {
        path: PathBuf,
    },
    Io(HarnessError),
    WriteAttempt {
        path: PathBuf,
        source: std::io::Error,
    },
    VerifyRead {
        path: PathBuf,
        source: std::io::Error,
    },
    Conflict {
        path: PathBuf,
        expected: [u8; 32],
        actual: [u8; 32],
    },
    /// The file did not hold the expected digest before a conditional
    /// operation. Nothing was written.
    Drift {
        path: PathBuf,
        expected: [u8; 32],
        actual: Option<[u8; 32]>,
    },
    Exists {
        path: PathBuf,
    },
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape { path } => write!(f, "{} is not the required JSON shape", path.display()),
            Self::Stale { path } => write!(
                f,
                "the target entry in {} is absent or stale",
                path.display()
            ),
            Self::Io(source) => write!(f, "{source}"),
            Self::WriteAttempt { path, source } => write!(
                f,
                "replacing {} failed; the mutation did not land: {source}",
                path.display()
            ),
            Self::VerifyRead { path, source } => write!(
                f,
                "reading back {} failed; the mutation may have landed: {source}",
                path.display()
            ),
            Self::Conflict {
                path,
                expected,
                actual,
            } => {
                write!(
                f,
                "{} changed during the mutation (expected {}, found {}); the write may have landed",
                path.display(), hex(expected), hex(actual)
            )
            }
            Self::Drift {
                path,
                expected,
                actual,
            } => write!(
                f,
                "{} did not hold the expected state (expected {}, found {})",
                path.display(),
                hex(expected),
                actual
                    .as_ref()
                    .map(|digest| hex(digest))
                    .unwrap_or_else(|| "absent".into())
            ),
            Self::Exists { path } => write!(
                f,
                "{} already exists; refusing to create over it",
                path.display()
            ),
        }
    }
}

impl From<CommitError> for HarnessError {
    fn from(error: CommitError) -> Self {
        match error {
            CommitError::Io(source) => source,
            CommitError::WriteAttempt { path, source } => HarnessError::Write {
                path,
                source: std::io::Error::other(format!(
                    "replacement failed; the mutation did not land: {source}"
                )),
            },
            CommitError::VerifyRead { path, source } => HarnessError::Mutation {
                path,
                message: format!("read-back failed; the mutation may have landed: {source}"),
            },
            CommitError::Conflict {
                path,
                expected,
                actual,
            } => HarnessError::Mutation {
                path,
                message: format!(
                    "write conflict; expected {}, found {}",
                    hex(&expected),
                    hex(&actual)
                ),
            },
            CommitError::Drift {
                path,
                expected,
                actual,
            } => HarnessError::Write {
                path,
                source: std::io::Error::other(format!(
                    "the file did not hold the expected state (expected {}, found {})",
                    hex(&expected),
                    actual
                        .as_ref()
                        .map(|digest| hex(digest))
                        .unwrap_or_else(|| "absent".into())
                )),
            },
            CommitError::Exists { path } => HarnessError::Write {
                path,
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "target already exists",
                ),
            },
            CommitError::Shape { path } => HarnessError::Write {
                path,
                source: std::io::Error::other("document has the wrong shape"),
            },
            CommitError::Stale { path } => HarnessError::Write {
                path,
                source: std::io::Error::other("target entry is absent or stale"),
            },
        }
    }
}

#[derive(Debug)]
enum PatchError {
    Shape,
    Stale,
}

impl Patch {
    fn apply(&self, doc: &mut Value) -> std::result::Result<(), PatchError> {
        match self {
            Self::SetPath { path, value } => *slot_for(doc, path.keys())? = value.clone(),
            Self::RemovePath { path } => remove_path(doc, path.keys())?,
            Self::Push { path, value, key } => {
                let array = array_for(doc, path.keys())?;
                if let Some(field) = key {
                    let identity = value.get(field).ok_or(PatchError::Shape)?;
                    if array.iter().any(|entry| entry.get(field) == Some(identity)) {
                        return Ok(());
                    }
                } else if array.contains(value) {
                    return Ok(());
                }
                array.push(value.clone());
            }
            Self::ReplaceWhere {
                path,
                matches,
                value,
            } => {
                let array = array_for(doc, path.keys())?;
                let index = unique_match_index(array, matches)?;
                if array
                    .iter()
                    .enumerate()
                    .any(|(other, entry)| other != index && entry == value)
                {
                    return Err(PatchError::Stale);
                }
                array[index] = value.clone();
            }
            Self::ReplacePath {
                path,
                matches,
                value,
            } => {
                let slot = existing_slot_for(doc, path.keys())?;
                if *slot != *matches {
                    return Err(PatchError::Stale);
                }
                *slot = value.clone();
            }
            Self::RemoveWhere { path, matches } => {
                let array = array_for(doc, path.keys())?;
                let index = unique_match_index(array, matches)?;
                array.remove(index);
            }
        }
        Ok(())
    }
}

fn unique_match_index(array: &[Value], expected: &Value) -> std::result::Result<usize, PatchError> {
    let mut matches = array
        .iter()
        .enumerate()
        .filter(|(_, value)| *value == expected);
    match (matches.next(), matches.next()) {
        (Some((index, _)), None) => Ok(index),
        _ => Err(PatchError::Stale),
    }
}

/// Walk to an existing object-key slot. A missing key or a non-object parent
/// is Stale, never created, so a guard compares against real stored state.
fn existing_slot_for<'a>(
    doc: &'a mut Value,
    keys: &[String],
) -> std::result::Result<&'a mut Value, PatchError> {
    let mut current = doc;
    for key in keys {
        let map = match current {
            Value::Object(map) => map,
            _ => return Err(PatchError::Stale),
        };
        current = map.get_mut(key).ok_or(PatchError::Stale)?;
    }
    Ok(current)
}

fn slot_for<'a>(
    doc: &'a mut Value,
    keys: &[String],
) -> std::result::Result<&'a mut Value, PatchError> {
    let mut current = doc;
    for key in keys {
        let map = match current {
            Value::Object(map) => map,
            Value::Null => {
                *current = Value::Object(serde_json::Map::new());
                current.as_object_mut().expect("replaced with object")
            }
            _ => return Err(PatchError::Shape),
        };
        current = map.entry(key.clone()).or_insert(Value::Null);
    }
    Ok(current)
}

fn remove_path(doc: &mut Value, keys: &[String]) -> std::result::Result<(), PatchError> {
    let mut current = doc;
    for key in &keys[..keys.len() - 1] {
        match current {
            Value::Object(map) => match map.get_mut(key) {
                Some(next) => current = next,
                None => return Ok(()),
            },
            Value::Null => return Ok(()),
            _ => return Err(PatchError::Shape),
        }
    }
    match current {
        Value::Object(map) => {
            map.remove(keys.last().expect("non-empty path"));
            Ok(())
        }
        Value::Null => Ok(()),
        _ => Err(PatchError::Shape),
    }
}

fn array_for<'a>(
    doc: &'a mut Value,
    keys: &[String],
) -> std::result::Result<&'a mut Vec<Value>, PatchError> {
    let (parents, last) = keys.split_at(keys.len() - 1);
    let parent = slot_for(doc, parents)?;
    if parent.is_null() {
        *parent = Value::Object(serde_json::Map::new());
    }
    let slot = parent
        .as_object_mut()
        .ok_or(PatchError::Shape)?
        .entry(last[0].clone())
        .or_insert_with(|| Value::Array(Vec::new()));
    match slot {
        Value::Array(array) => Ok(array),
        _ => Err(PatchError::Shape),
    }
}

/// Read a JSON document, treating an empty or missing file as null.
pub fn read_json(path: &Path) -> Result<Value> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(Value::Null),
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| HarnessError::Parse {
            path: path.to_path_buf(),
            source,
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(source) => Err(HarnessError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub fn read_struct<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    let value = read_json(path)?;
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|source| HarnessError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn serialize_pretty(path: &Path, value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| HarnessError::WriteJson {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn stage_file(path: &Path, bytes: &[u8]) -> std::result::Result<PathBuf, CommitError> {
    let dir = path.parent().ok_or_else(|| {
        CommitError::Io(HarnessError::Write {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no parent directory",
            ),
        })
    })?;
    let name = path.file_name().ok_or_else(|| {
        CommitError::Io(HarnessError::Write {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name"),
        })
    })?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = dir.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        sequence
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|source| {
            CommitError::Io(HarnessError::Write {
                path: temp.clone(),
                source,
            })
        })?;
    if let Err(source) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(CommitError::Io(HarnessError::Write { path: temp, source }));
    }
    Ok(temp)
}

fn replace_via_temp(path: &Path, bytes: &[u8]) -> std::result::Result<(), CommitError> {
    let temp = stage_file(path, bytes)?;
    std::fs::rename(&temp, path).map_err(|source| {
        let _ = std::fs::remove_file(&temp);
        CommitError::WriteAttempt {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn create_via_temp(path: &Path, bytes: &[u8]) -> std::result::Result<(), CommitError> {
    let temp = stage_file(path, bytes)?;
    if let Err(source) = publish_new_file(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(if source.kind() == std::io::ErrorKind::AlreadyExists {
            CommitError::Exists {
                path: path.to_path_buf(),
            }
        } else {
            CommitError::WriteAttempt {
                path: path.to_path_buf(),
                source,
            }
        });
    }
    Ok(())
}

#[cfg(windows)]
fn publish_new_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileW(existing: *const u16, destination: *const u16) -> i32;
    }
    let existing: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // MoveFileW refuses an existing destination; both UTF-16 buffers live through the call.
    if unsafe { MoveFileW(existing.as_ptr(), destination.as_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn publish_new_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    std::fs::hard_link(temp, path)?;
    let _ = std::fs::remove_file(temp);
    Ok(())
}

pub(crate) fn lock_mutations(path: &Path) -> std::result::Result<MutationGuard, CommitError> {
    MUTATION_LOCK
        .lock()
        .map(|lock| MutationGuard { _lock: lock })
        .map_err(|error| {
            CommitError::Io(HarnessError::Write {
                path: path.to_path_buf(),
                source: std::io::Error::other(format!("mutation lock poisoned: {error}")),
            })
        })
}

fn read_original(path: &Path) -> std::result::Result<Option<Vec<u8>>, CommitError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CommitError::Io(HarnessError::Read {
            path: path.to_path_buf(),
            source,
        })),
    }
}

fn parse_document(path: &Path, bytes: Option<&[u8]>) -> std::result::Result<Value, CommitError> {
    let value = match bytes {
        Some(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Value::Null,
        Some(bytes) => serde_json::from_slice(bytes).map_err(|source| {
            CommitError::Io(HarnessError::Parse {
                path: path.to_path_buf(),
                source,
            })
        })?,
        None => Value::Null,
    };
    if !value.is_null() && !value.is_object() {
        return Err(CommitError::Shape {
            path: path.to_path_buf(),
        });
    }
    Ok(value)
}

fn write_backup(path: &Path, bytes: &[u8]) -> std::result::Result<(), CommitError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            CommitError::Io(HarnessError::Write {
                path: parent.to_path_buf(),
                source,
            })
        })?;
    }
    std::fs::write(path, bytes).map_err(|source| {
        CommitError::Io(HarnessError::Write {
            path: path.to_path_buf(),
            source,
        })
    })
}

fn apply_and_write(
    path: &Path,
    original: Option<Vec<u8>>,
    bytes: &[u8],
    backup_to: Option<&Path>,
) -> std::result::Result<Receipt, CommitError> {
    let before = original.as_deref().map(sha256);
    let backup = match (backup_to, original.as_deref()) {
        (Some(backup), Some(original)) => {
            write_backup(backup, original)?;
            Some(backup.to_path_buf())
        }
        _ => None,
    };
    if original.as_deref() != Some(bytes) {
        replace_via_temp(path, bytes)?;
    }
    let landed = std::fs::read(path).map_err(|source| CommitError::VerifyRead {
        path: path.to_path_buf(),
        source,
    })?;
    if landed != bytes {
        return Err(CommitError::Conflict {
            path: path.to_path_buf(),
            expected: sha256(bytes),
            actual: sha256(&landed),
        });
    }
    Ok(Receipt {
        backup,
        before,
        after: sha256(&landed),
        target: path.to_path_buf(),
    })
}

/// Apply a batch once, preserve the exact original bytes, write once, and
/// verify the complete candidate once. The in-process lock prevents this
/// application's concurrent mutations from losing one another. It is not a
/// compare-and-swap against other processes.
pub fn commit(
    path: &Path,
    patches: Vec<Patch>,
    backup_to: Option<&Path>,
) -> std::result::Result<Receipt, CommitError> {
    let guard = lock_mutations(path)?;
    commit_locked(path, patches, backup_to, &guard)
}

/// Apply a JSON patch while the caller already owns the mutation lock.
pub(crate) fn commit_locked(
    path: &Path,
    patches: Vec<Patch>,
    backup_to: Option<&Path>,
    _guard: &MutationGuard,
) -> std::result::Result<Receipt, CommitError> {
    let original = read_original(path)?;
    let candidate = patch_candidate(path, original.as_deref(), &patches)?;
    apply_and_write(path, original, &candidate, backup_to)
}

fn patch_candidate(
    path: &Path,
    original: Option<&[u8]>,
    patches: &[Patch],
) -> std::result::Result<Vec<u8>, CommitError> {
    let mut document = parse_document(path, original)?;
    for patch in patches {
        patch.apply(&mut document).map_err(|error| match error {
            PatchError::Shape => CommitError::Shape {
                path: path.to_path_buf(),
            },
            PatchError::Stale => CommitError::Stale {
                path: path.to_path_buf(),
            },
        })?;
    }
    serialize_pretty(path, &document).map_err(CommitError::Io)
}

/// Write exact bytes once and verify the bytes actually read back.
pub fn write_file(
    path: &Path,
    bytes: &[u8],
    backup_to: Option<&Path>,
) -> std::result::Result<Receipt, CommitError> {
    let _guard = lock_mutations(path)?;
    let original = read_original(path)?;
    apply_and_write(path, original, bytes, backup_to)
}

/// Restore `bytes` only while the file still holds `expected_current`, under
/// the same mutation lock as commit and write_file. A drift refusal happens
/// before any write; the original file is untouched.
pub fn restore(
    path: &Path,
    expected_current: [u8; 32],
    bytes: &[u8],
    backup_to: Option<&Path>,
) -> std::result::Result<Receipt, CommitError> {
    let _guard = lock_mutations(path)?;
    let current = read_original(path)?;
    ensure_current(path, &current, expected_current)?;
    apply_and_write(path, current, bytes, backup_to)
}

/// Remove the file only while it still holds `expected_current`, under the
/// same mutation lock. Nothing is recorded; the caller records after removal.
pub fn remove_current(
    path: &Path,
    expected_current: [u8; 32],
) -> std::result::Result<(), CommitError> {
    let guard = lock_mutations(path)?;
    remove_current_locked(path, expected_current, None, &guard)
}

pub(crate) fn remove_current_locked(
    path: &Path,
    expected_current: [u8; 32],
    backup_to: Option<&Path>,
    _guard: &MutationGuard,
) -> std::result::Result<(), CommitError> {
    let current = read_original(path)?;
    ensure_current(path, &current, expected_current)?;
    remove_prepared(
        path,
        current.as_deref().expect("validated current bytes"),
        backup_to,
    )
}

fn ensure_current(
    path: &Path,
    current: &Option<Vec<u8>>,
    expected_current: [u8; 32],
) -> std::result::Result<(), CommitError> {
    let actual = current.as_deref().map(sha256);
    if actual != Some(expected_current) {
        return Err(CommitError::Drift {
            path: path.to_path_buf(),
            expected: expected_current,
            actual,
        });
    }
    Ok(())
}

fn remove_prepared(
    path: &Path,
    bytes: &[u8],
    backup_to: Option<&Path>,
) -> std::result::Result<(), CommitError> {
    if let Some(backup) = backup_to {
        write_backup(backup, bytes)?;
    }
    std::fs::remove_file(path).map_err(|source| {
        CommitError::Io(HarnessError::Write {
            path: path.to_path_buf(),
            source,
        })
    })
}

/// Reserve the candidate before writing and finalize only after verified readback.
pub(crate) fn write_recorded<E>(
    path: &Path,
    change: RecordedWrite<'_>,
    backup_to: Option<&Path>,
    reserve: impl FnOnce(&MutationPlan, &MutationGuard) -> std::result::Result<(), E>,
    complete: impl FnOnce(&MutationPlan, &MutationGuard) -> std::result::Result<(), E>,
) -> std::result::Result<Receipt, RecordedMutationError<E>> {
    let guard = lock_mutations(path).map_err(RecordedMutationError::Preparation)?;
    let create_only = matches!(&change, RecordedWrite::Create(_));
    if create_only {
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(RecordedMutationError::Preparation(CommitError::Exists {
                    path: path.to_path_buf(),
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(RecordedMutationError::Preparation(CommitError::Io(
                    HarnessError::Read {
                        path: path.to_path_buf(),
                        source,
                    },
                )));
            }
        }
    }
    let original = if create_only {
        None
    } else {
        read_original(path).map_err(RecordedMutationError::Preparation)?
    };
    let candidate = match change {
        RecordedWrite::Patches(patches) => patch_candidate(path, original.as_deref(), &patches)
            .map_err(RecordedMutationError::Preparation)?,
        #[cfg(test)]
        RecordedWrite::Bytes(bytes) => bytes.to_vec(),
        RecordedWrite::Restore(expected, bytes) => {
            ensure_current(path, &original, expected)
                .map_err(RecordedMutationError::Preparation)?;
            bytes.to_vec()
        }
        RecordedWrite::Create(bytes) => bytes.to_vec(),
    };
    let plan = MutationPlan {
        target: path.to_path_buf(),
        backup: original.as_ref().and(backup_to).map(Path::to_path_buf),
        before_checksum: original.as_deref().map(|value| hex(&sha256(value))),
        after_checksum: Some(hex(&sha256(&candidate))),
    };
    reserve(&plan, &guard).map_err(RecordedMutationError::Reservation)?;
    let receipt = if create_only {
        create_via_temp(path, &candidate).map_err(RecordedMutationError::Mutation)?;
        let landed = std::fs::read(path).map_err(|source| {
            RecordedMutationError::Mutation(CommitError::VerifyRead {
                path: path.to_path_buf(),
                source,
            })
        })?;
        if landed != candidate {
            return Err(RecordedMutationError::Mutation(CommitError::Conflict {
                path: path.to_path_buf(),
                expected: sha256(&candidate),
                actual: sha256(&landed),
            }));
        }
        Receipt {
            backup: None,
            before: None,
            after: sha256(&landed),
            target: path.to_path_buf(),
        }
    } else {
        apply_and_write(path, original, &candidate, backup_to)
            .map_err(RecordedMutationError::Mutation)?
    };
    complete(&plan, &guard).map_err(RecordedMutationError::Finalization)?;
    Ok(receipt)
}

pub(crate) fn remove_current_recorded<E>(
    path: &Path,
    expected_current: [u8; 32],
    backup_to: &Path,
    reserve: impl FnOnce(&MutationPlan, &MutationGuard) -> std::result::Result<(), E>,
    complete: impl FnOnce(&MutationPlan, &MutationGuard) -> std::result::Result<(), E>,
) -> std::result::Result<(), RecordedMutationError<E>> {
    let guard = lock_mutations(path).map_err(RecordedMutationError::Preparation)?;
    let original = read_original(path).map_err(RecordedMutationError::Preparation)?;
    ensure_current(path, &original, expected_current)
        .map_err(RecordedMutationError::Preparation)?;
    let plan = MutationPlan {
        target: path.to_path_buf(),
        backup: Some(backup_to.to_path_buf()),
        before_checksum: Some(hex(&expected_current)),
        after_checksum: None,
    };
    reserve(&plan, &guard).map_err(RecordedMutationError::Reservation)?;
    remove_prepared(
        path,
        original.as_deref().expect("validated current bytes"),
        Some(backup_to),
    )
    .map_err(RecordedMutationError::Mutation)?;
    complete(&plan, &guard).map_err(RecordedMutationError::Finalization)?;
    Ok(())
}

pub fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_file_publication_never_replaces_an_existing_destination() {
        let directory = crate::test_support::tempdir().unwrap();
        let target = directory.path().join("memory.md");
        let _guard = lock_mutations(&target).unwrap();
        create_via_temp(&target, b"saved memory").unwrap();
        assert!(matches!(
            create_via_temp(&target, b"replacement"),
            Err(CommitError::Exists { .. })
        ));
        assert_eq!(std::fs::read(&target).unwrap(), b"saved memory");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn staging_refuses_an_existing_temp_link_without_changing_its_target() {
        let directory = crate::test_support::tempdir().unwrap();
        let target = directory.path().join("settings.json");
        let _guard = lock_mutations(&target).unwrap();
        let outside = directory.path().join("preserve.txt");
        std::fs::write(&outside, b"preserve").unwrap();
        let temp = directory.path().join(format!(
            ".settings.json.{}.{}.tmp",
            std::process::id(),
            TEMP_SEQUENCE.load(std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::hard_link(&outside, &temp).unwrap();
        assert!(stage_file(&target, b"replacement").is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"preserve");
        assert!(!target.exists());
    }
    use serde_json::json;

    fn write_fixture(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn mixed_set_and_unkeyed_push_land_once() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("ledger.json");
        write_fixture(&path, br#"{"rows":[]}"#);
        let receipt = commit(
            &path,
            vec![
                Patch::SetPath {
                    path: p!["ready"],
                    value: json!(true),
                },
                Patch::Push {
                    path: p!["rows"],
                    value: json!({"id":"one"}),
                    key: None,
                },
            ],
            None,
        )
        .unwrap();
        assert_eq!(receipt.after, sha256(&std::fs::read(&path).unwrap()));
        assert_eq!(
            read_json(&path).unwrap(),
            json!({"ready":true,"rows":[{"id":"one"}]})
        );
    }

    #[test]
    fn concurrent_whole_value_pushes_keep_one_addressable_entry() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_fixture(&path, br#"{"hooks":[]}"#);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2).map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                commit(&path, vec![Patch::Push {
                    path: p!["hooks"],
                    value: json!({"matcher":"Read","hooks":[{"type":"command","command":"echo checked"}]}),
                    key: None,
                }], None).unwrap();
            })
        }).collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            read_json(&path).unwrap(),
            json!({"hooks":[{
                "matcher":"Read","hooks":[{"type":"command","command":"echo checked"}]
            }]})
        );
    }

    #[test]
    fn replacement_cannot_collide_with_an_entry_added_since_the_read() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let backup = dir.path().join("backup.json");
        write_fixture(&path, br#"{"hooks":[{"command":"old"}]}"#);
        let edit = Patch::ReplaceWhere {
            path: p!["hooks"],
            matches: json!({"command":"old"}),
            value: json!({"command":"new"}),
        };
        commit(
            &path,
            vec![Patch::Push {
                path: p!["hooks"],
                value: json!({"command":"new"}),
                key: None,
            }],
            None,
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let result = commit(&path, vec![edit], Some(&backup));
        assert!(matches!(result, Err(CommitError::Stale { .. })));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!backup.exists());
    }

    #[test]
    fn where_patches_change_only_the_matching_element() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_fixture(
            &path,
            br#"{"hooks":[{"name":"a","value":1},{"name":"b","value":2}]}"#,
        );
        commit(
            &path,
            vec![Patch::ReplaceWhere {
                path: p!["hooks"],
                matches: json!({"name":"a","value":1}),
                value: json!({"name":"a","value":3}),
            }],
            None,
        )
        .unwrap();
        assert_eq!(
            read_json(&path).unwrap(),
            json!({"hooks":[{"name":"a","value":3},{"name":"b","value":2}]})
        );
        commit(
            &path,
            vec![Patch::RemoveWhere {
                path: p!["hooks"],
                matches: json!({"name":"a","value":3}),
            }],
            None,
        )
        .unwrap();
        assert_eq!(
            read_json(&path).unwrap(),
            json!({"hooks":[{"name":"b","value":2}]})
        );
    }

    #[test]
    fn ambiguous_where_batches_leave_original_bytes_and_backup_untouched() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let backup = dir.path().join("backup");
        let original = br#"{"hooks":[{"command":"same"},{"command":"same"}]}"#;
        for patch in [
            Patch::ReplaceWhere {
                path: p!["hooks"],
                matches: json!({"command":"same"}),
                value: json!({"command":"changed"}),
            },
            Patch::RemoveWhere {
                path: p!["hooks"],
                matches: json!({"command":"same"}),
            },
        ] {
            write_fixture(&path, original);
            let result = commit(
                &path,
                vec![
                    Patch::SetPath {
                        path: p!["mustNotLand"],
                        value: json!(true),
                    },
                    patch,
                ],
                Some(&backup),
            );
            assert!(matches!(result, Err(CommitError::Stale { .. })));
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert!(!backup.exists());
        }
    }

    #[test]
    fn removing_an_absent_nested_path_does_not_create_parents() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_fixture(&path, br#"{}"#);
        commit(
            &path,
            vec![Patch::RemovePath {
                path: p!["missing", "entry"],
            }],
            None,
        )
        .unwrap();
        assert_eq!(read_json(&path).unwrap(), json!({}));
    }

    #[test]
    fn stale_or_bad_shape_is_rejected_before_mutating() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_fixture(&path, br#"{"hooks":[]}"#);
        let original = std::fs::read(&path).unwrap();
        let result = commit(
            &path,
            vec![Patch::ReplaceWhere {
                path: p!["hooks"],
                matches: json!("absent"),
                value: json!("new"),
            }],
            None,
        );
        assert!(matches!(result, Err(CommitError::Stale { .. })));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        write_fixture(&path, b"[1]");
        let result = commit(
            &path,
            vec![Patch::SetPath {
                path: p!["x"],
                value: json!(1),
            }],
            None,
        );
        assert!(matches!(result, Err(CommitError::Shape { .. })));
        assert_eq!(std::fs::read(&path).unwrap(), b"[1]");
    }

    #[test]
    fn receipt_before_matches_exact_backup_and_after_matches_readback() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let backup = dir.path().join("backup").join("settings");
        let original = b"{ \"theme\": \"dark\" }\r\n";
        write_fixture(&path, original);
        let receipt = commit(
            &path,
            vec![Patch::SetPath {
                path: p!["model"],
                value: json!("opus"),
            }],
            Some(&backup),
        )
        .unwrap();
        let landed = std::fs::read(&path).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), original);
        assert_eq!(receipt.before, Some(sha256(original)));
        assert_eq!(receipt.after, sha256(&landed));
    }

    #[test]
    fn creation_receipt_has_no_backup_or_before_checksum() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("new.json");
        let backup = dir.path().join("backup");
        let receipt = write_file(&path, b"created", Some(&backup)).unwrap();
        assert_eq!(receipt.before, None);
        assert_eq!(receipt.backup, None);
        assert!(!backup.exists());
        assert_eq!(std::fs::read(path).unwrap(), b"created");
    }

    #[test]
    fn post_write_readback_errors_preserve_uncertainty_in_display_and_conversion() {
        let error = CommitError::VerifyRead {
            path: PathBuf::from("target"),
            source: std::io::Error::other("denied"),
        };
        assert!(error.to_string().contains("may have landed"));
        assert!(HarnessError::from(error)
            .to_string()
            .contains("may have landed"));
    }

    #[test]
    fn write_file_preserves_exact_bytes_and_checksums() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("skill.md");
        let backup = dir.path().join("backup");
        write_fixture(&path, b"original\r\n");
        let receipt = write_file(&path, b"replacement\n", Some(&backup)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement\n");
        assert_eq!(std::fs::read(&backup).unwrap(), b"original\r\n");
        assert_eq!(receipt.before, Some(sha256(b"original\r\n")));
        assert_eq!(receipt.after, sha256(b"replacement\n"));
    }

    #[test]
    fn windows_rename_replaces_an_existing_file() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("replace.json");
        let _guard = lock_mutations(&path).unwrap();
        write_fixture(&path, b"old");
        replace_via_temp(&path, b"new").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"new");
    }

    #[test]
    fn failed_rename_leaves_no_temp_file_and_no_landed_mutation() {
        let dir = crate::test_support::tempdir().unwrap();
        let occupied = dir.path().join("occupied");
        let _guard = lock_mutations(&occupied).unwrap();
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("keep.txt"), b"occupied").unwrap();
        let error = replace_via_temp(&occupied, b"new").unwrap_err();
        assert!(matches!(error, CommitError::WriteAttempt { .. }));
        assert!(error.to_string().contains("did not land"));
        assert!(HarnessError::from(error)
            .to_string()
            .contains("did not land"));
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn restore_writes_only_when_the_file_still_holds_the_expected_digest() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_fixture(&path, b"current");
        let receipt = restore(&path, sha256(b"current"), b"restored", None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"restored");
        assert_eq!(receipt.before, Some(sha256(b"current")));
        assert_eq!(receipt.after, sha256(b"restored"));
        assert_eq!(receipt.target, path);
    }

    #[test]
    fn restore_refuses_without_writing_when_the_file_drifted() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_fixture(&path, b"changed by someone else");
        let error = restore(&path, sha256(b"current"), b"restored", None).unwrap_err();
        match &error {
            CommitError::Drift {
                path: error_path,
                expected,
                actual,
            } => {
                assert_eq!(error_path, &path);
                assert_eq!(*expected, sha256(b"current"));
                assert_eq!(*actual, Some(sha256(b"changed by someone else")));
            }
            other => panic!("expected drift refusal, got {other:?}"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"changed by someone else");
        assert!(error
            .to_string()
            .contains("did not hold the expected state"));
        assert!(HarnessError::from(error)
            .to_string()
            .contains("did not hold the expected state"));
    }

    #[test]
    fn restore_refuses_an_absent_file() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let error = restore(&path, sha256(b"current"), b"restored", None).unwrap_err();
        match &error {
            CommitError::Drift { actual, .. } => assert_eq!(*actual, None),
            other => panic!("expected drift refusal, got {other:?}"),
        }
        assert!(!path.exists());
        assert!(error.to_string().contains("absent"));
    }

    #[test]
    fn remove_current_deletes_only_the_unchanged_file() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("created.json");
        write_fixture(&path, b"created");
        remove_current(&path, sha256(b"created")).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn remove_current_refuses_when_the_file_drifted_or_is_absent() {
        let dir = crate::test_support::tempdir().unwrap();
        let path = dir.path().join("created.json");
        write_fixture(&path, b"changed");
        let error = remove_current(&path, sha256(b"created")).unwrap_err();
        assert!(matches!(error, CommitError::Drift { .. }));
        assert_eq!(std::fs::read(&path).unwrap(), b"changed");
        remove_current(&path, sha256(b"changed")).unwrap();
        let error = remove_current(&path, sha256(b"created")).unwrap_err();
        match error {
            CommitError::Drift { actual, .. } => assert_eq!(actual, None),
            other => panic!("expected drift refusal, got {other:?}"),
        }
        assert!(!path.exists());
    }
}
