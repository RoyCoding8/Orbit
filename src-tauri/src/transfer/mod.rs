mod resources;
pub(crate) use resources::history_scope;
#[cfg(test)]
mod tests;

use crate::{
    harness::{atomic, claude::skills, selected::SelectedHarness, Harness, ResourceKind},
    ledger,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

const LIMIT: u64 = 128 * 1024 * 1024;
const FILE_LIMIT: u64 = 16 * 1024 * 1024;
const COUNT_LIMIT: usize = 10_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Skills,
    Hooks,
    Mcps,
    Memories,
    Settings,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    User,
    Project,
    Local,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Destination {
    Original,
    Project,
    User,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    id: String,
    category: Category,
    scope: Scope,
    label: String,
    blob: String,
    checksum: String,
    resource: resources::Resource,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    harness: String,
    orbit_version: String,
    entries: Vec<Entry>,
}

struct Bundle {
    manifest: Manifest,
    data: BTreeMap<String, Vec<u8>>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    id: String,
    category: Category,
    label: String,
    scope: Scope,
    target: String,
    target_key: String,
    status: Status,
    expected: String,
}

#[derive(Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Status {
    Add,
    Identical,
    Conflict,
    Merge,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub checksum: String,
    pub project_root: String,
    pub entries: Vec<Item>,
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Choice {
    pub id: String,
    pub replace: bool,
    pub expected: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub files: usize,
    pub skipped: usize,
    pub path: String,
}

fn digest(bytes: &[u8]) -> String {
    atomic::hex(&Sha256::digest(bytes))
}

fn categories() -> [Category; 5] {
    [
        Category::Skills,
        Category::Hooks,
        Category::Mcps,
        Category::Memories,
        Category::Settings,
    ]
}

pub fn inventory(harness: &SelectedHarness, project: &Path) -> Result<Preview, String> {
    let bundle = resources::collect(harness, project, &categories())?;
    Ok(Preview {
        checksum: String::new(),
        project_root: project.to_string_lossy().into_owned(),
        entries: bundle
            .manifest
            .entries
            .iter()
            .map(|entry| Item {
                id: entry.id.clone(),
                category: entry.category,
                label: entry.label.clone(),
                scope: entry.scope,
                target: String::new(),
                target_key: String::new(),
                status: Status::Add,
                expected: String::new(),
            })
            .collect(),
        warnings: bundle.warnings,
    })
}

pub fn export(
    harness: &SelectedHarness,
    project: &Path,
    selected: &[Category],
    destination: &Path,
) -> Result<Report, String> {
    if selected.is_empty() {
        return Err("Select at least one category to export".into());
    }
    let bundle = resources::collect(harness, project, selected)?;
    if bundle.manifest.entries.is_empty() {
        return Err("No local resources were found in the selected categories".into());
    }
    let metadata = serde_json::to_vec_pretty(&bundle.manifest).map_err(|e| e.to_string())?;
    if metadata.len() as u64 > FILE_LIMIT {
        return Err("Selected resources exceed the metadata size limit".into());
    }
    if metadata.len() as u64
        + bundle
            .data
            .values()
            .map(|bytes| bytes.len() as u64)
            .sum::<u64>()
        > LIMIT
    {
        return Err("Selected resources exceed the archive size limit".into());
    }
    skills::ensure_plain_parent(destination)?;
    let parent = destination
        .parent()
        .ok_or("Archive has no parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    {
        let mut writer = ZipWriter::new(temporary.as_file_mut());
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer
            .start_file("metadata.json", options)
            .map_err(|e| e.to_string())?;
        writer.write_all(&metadata).map_err(|e| e.to_string())?;
        for (name, data) in &bundle.data {
            writer
                .start_file(name, options)
                .map_err(|e| e.to_string())?;
            writer.write_all(data).map_err(|e| e.to_string())?;
        }
        writer.finish().map_err(|e| e.to_string())?;
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    if temporary
        .as_file()
        .metadata()
        .map_err(|e| e.to_string())?
        .len()
        > LIMIT
    {
        return Err("Selected resources exceed the archive size limit".into());
    }
    temporary
        .persist_noclobber(destination)
        .map_err(|e| format!("Could not save archive; choose a new filename: {e}"))?;
    Ok(Report {
        files: bundle.manifest.entries.len(),
        skipped: 0,
        path: destination.to_string_lossy().into_owned(),
    })
}

fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    skills::ensure_plain_components(path)?;
    skills::ensure_plain_file_if_present(path)?;
    let mut data = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > limit {
        return Err(format!(
            "File exceeds the archive size limit: {}",
            path.display()
        ));
    }
    Ok(data)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    if skills::path_exists_no_follow(path)? {
        read_file(path, FILE_LIMIT).map(Some)
    } else {
        Ok(None)
    }
}

fn load(harness: &impl Harness, path: &Path) -> Result<(Bundle, String), String> {
    let bytes = read_file(path, LIMIT)?;
    let checksum = digest(&bytes);
    let mut zip = ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Not an Orbit ZIP archive: {e}"))?;
    if zip.len() > COUNT_LIMIT + 1 {
        return Err("Archive contains too many files".into());
    }
    let mut data = BTreeMap::new();
    let mut names = HashSet::new();
    let mut total = 0;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(|e| e.to_string())?;
        resources::relative(file.name().trim_end_matches('/'))?;
        if file.is_dir() {
            continue;
        }
        if file.is_symlink()
            || file
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
        {
            return Err("Archive contains a link or special file".into());
        }
        let name = file.name().to_owned();
        if !names.insert(name.to_lowercase()) {
            return Err("Archive contains duplicate file names".into());
        }
        let mut content = Vec::new();
        (&mut file)
            .take(FILE_LIMIT + 1)
            .read_to_end(&mut content)
            .map_err(|e| e.to_string())?;
        total += content.len() as u64;
        if content.len() as u64 > FILE_LIMIT || total > LIMIT {
            return Err("Archive exceeds the unpacked size limit".into());
        }
        data.insert(name, content);
    }
    let metadata = data
        .remove("metadata.json")
        .ok_or("Orbit archive is missing metadata.json")?;
    let manifest: Manifest =
        serde_json::from_slice(&metadata).map_err(|_| "Invalid Orbit metadata.json".to_string())?;
    if manifest.format != "orbit-harness" || manifest.version != 1 {
        return Err("Unsupported Orbit archive format or version".into());
    }
    if manifest.harness != harness.id().0 {
        return Err(format!("This archive contains {} resources. Cross-harness import is planned for a future version. Select the matching harness to import it.", manifest.harness));
    }
    let mut ids = HashSet::new();
    let mut blobs = HashSet::new();
    for entry in &manifest.entries {
        if !ids.insert(&entry.id) || !blobs.insert(&entry.blob) {
            return Err("Archive metadata contains duplicate resources".into());
        }
        let content = data
            .get(&entry.blob)
            .ok_or("Archive is missing a resource file")?;
        if digest(content) != entry.checksum {
            return Err(format!("Archive checksum does not match: {}", entry.label));
        }
        resources::validate(entry, content, harness.id().0)?;
    }
    if blobs.len() != data.len() {
        return Err("Archive contains files not listed in metadata.json".into());
    }
    Ok((
        Bundle {
            manifest,
            data,
            warnings: Vec::new(),
        },
        checksum,
    ))
}

pub fn preview(
    harness: &SelectedHarness,
    project: &Path,
    archive: &Path,
    destination: Destination,
) -> Result<Preview, String> {
    let (bundle, checksum) = load(harness, archive)?;
    let entries = bundle
        .manifest
        .entries
        .iter()
        .map(|entry| {
            describe(
                harness,
                project,
                destination,
                entry,
                &bundle.data[&entry.blob],
            )
        })
        .collect::<Result<_, _>>()?;
    Ok(Preview {
        checksum,
        project_root: project.to_string_lossy().into_owned(),
        entries,
        warnings: resources::notes(harness),
    })
}

fn describe(
    harness: &SelectedHarness,
    project: &Path,
    destination: Destination,
    entry: &Entry,
    bytes: &[u8],
) -> Result<Item, String> {
    let target = resources::target(harness, project, destination, entry)?;
    describe_content(
        harness.id().0,
        &target,
        entry,
        read_optional(&target)?.as_deref(),
        bytes,
    )
}

fn describe_content(
    harness: &str,
    target: &Path,
    entry: &Entry,
    before: Option<&[u8]>,
    bytes: &[u8],
) -> Result<Item, String> {
    let current = resources::current(before, entry, harness)?;
    let after = resources::part(entry, bytes, harness)?;
    let status = if current.as_ref() == Some(&after) {
        Status::Identical
    } else if current.is_none() {
        Status::Add
    } else if resources::merges(entry) {
        Status::Merge
    } else {
        Status::Conflict
    };
    Ok(Item {
        id: entry.id.clone(),
        category: entry.category,
        label: entry.label.clone(),
        scope: entry.scope,
        target: target.to_string_lossy().into_owned(),
        target_key: resources::target_key(entry),
        status,
        expected: digest(&serde_json::to_vec(&current).map_err(|e| e.to_string())?),
    })
}

pub fn apply(
    harness: &SelectedHarness,
    project: &Path,
    archive: &Path,
    checksum: &str,
    destination: Destination,
    choices: &[Choice],
) -> Result<Report, String> {
    let (bundle, actual) = load(harness, archive)?;
    if actual != checksum {
        return Err("Archive changed after preview. Open it again before importing.".into());
    }
    if choices.is_empty() {
        return Err("Select at least one resource to import".into());
    }
    let mut writes = BTreeMap::<PathBuf, (Option<Vec<u8>>, Vec<u8>)>::new();
    let mut seen = HashSet::new();
    let mut targets = HashSet::new();
    let mut skipped = 0;
    for choice in choices {
        if !seen.insert(&choice.id) {
            return Err("Duplicate import selection".into());
        }
        let entry = bundle
            .manifest
            .entries
            .iter()
            .find(|entry| entry.id == choice.id)
            .ok_or("Unknown archive resource")?;
        let bytes = &bundle.data[&entry.blob];
        let path = resources::target(harness, project, destination, entry)?;
        let (before, after) = match writes.entry(path.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                let before = read_optional(&path)?;
                slot.insert((before.clone(), before.unwrap_or_default()))
            }
            std::collections::btree_map::Entry::Occupied(slot) => slot.into_mut(),
        };
        let item = describe_content(harness.id().0, &path, entry, before.as_deref(), bytes)?;
        if choice.expected != item.expected {
            return Err(format!(
                "{} changed after preview. Refresh the preview before importing.",
                item.label
            ));
        }
        if item.status == Status::Identical || (item.status == Status::Conflict && !choice.replace)
        {
            skipped += 1;
            continue;
        }
        let key = resources::target_key(entry);
        let identity = path.to_string_lossy().into_owned();
        let identity = if cfg!(windows) {
            identity.to_lowercase()
        } else {
            identity
        };
        if !targets.insert((identity, key)) {
            return Err("Multiple selected resources have the same destination. Choose one source for each destination, or preserve original scopes.".into());
        }
        *after = resources::updated(entry, after, bytes, harness.id().0)?;
    }
    let mut files = 0;
    for (path, (before, after)) in writes {
        if before.as_deref() == Some(&after) {
            continue;
        }
        skills::ensure_plain_parent(&path)?;
        fs::create_dir_all(path.parent().ok_or("Import target has no parent")?)
            .map_err(|e| e.to_string())?;
        let id = digest(path.to_string_lossy().as_bytes());
        let backup = ledger::backup_path(harness.config_root(), &id, "archive-import");
        let change = match &before {
            Some(bytes) => atomic::RecordedWrite::Restore(Sha256::digest(bytes).into(), &after),
            None => atomic::RecordedWrite::Create(&after),
        };
        ledger::write(
            harness.config_root(),
            ResourceKind::HarnessImport,
            &id,
            "archive-import",
            &path,
            change,
            Some(&backup),
        )
        .map_err(|e| {
            format!(
                "Import stopped after {files} files. Earlier writes are recorded in History. {e}"
            )
        })?;
        files += 1;
    }
    Ok(Report {
        files,
        skipped,
        path: archive.to_string_lossy().into_owned(),
    })
}
