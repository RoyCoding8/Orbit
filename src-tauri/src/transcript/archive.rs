use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use super::{ingest::open_database, SessionTitle};

const MANIFEST_NAME: &str = "manifest.json";
const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveReport {
    pub action: ArchiveAction,
    pub source_session_id: String,
    pub session_id: String,
    pub files: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveAction {
    Exported,
    Imported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    version: u32,
    source_session_id: String,
    source_project_key: String,
    session_file: String,
    family_directory: String,
    directories: Vec<String>,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestFile {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone)]
struct SourceFamily {
    files: Vec<(PathBuf, String)>,
    directories: Vec<String>,
}

/// Copy one native Claude Code session and its session-stem directory into a
/// new directory under `destination`. The archive preserves file bytes; the
/// manifest contains only relative paths and checksums.
pub fn export_archive(
    app_data_dir: &Path,
    projects_root: &Path,
    session_id: &str,
    destination: &Path,
) -> Result<ArchiveReport, String> {
    let (project_key, source_file) = indexed_source(app_data_dir, projects_root, session_id)?;
    let source_name = source_file
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The session filename is not valid UTF-8.".to_owned())?;
    validate_session_filename(source_name)?;
    let stem = source_file
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The session filename has no valid session ID.".to_owned())?;
    let family_path = source_file.with_file_name(stem);
    let destination_root = checked_directory(destination, "archive destination")?;
    if destination_root.starts_with(&family_path) {
        return Err(
            "Choose an archive destination outside this session's data directory.".to_owned(),
        );
    }

    let family = collect_source_family(&source_file, &family_path, stem)?;
    let archive_path = create_export_directory(&destination_root, stem)?;
    let mut created_files = Vec::new();
    let result = write_export(
        &archive_path,
        &source_file,
        &family,
        &project_key,
        session_id,
        &mut created_files,
    );
    match result {
        Ok(manifest) => Ok(ArchiveReport {
            action: ArchiveAction::Exported,
            source_session_id: session_id.to_owned(),
            session_id: session_id.to_owned(),
            files: manifest.files.len(),
            path: archive_path.to_string_lossy().into_owned(),
        }),
        Err(error) => {
            remove_owned_tree(&archive_path, &created_files);
            Err(error)
        }
    }
}

/// Validate a native archive completely before installing it under the
/// selected Claude project directory. Existing chats are never overwritten.
pub fn import_archive(
    projects_root: &Path,
    source: &Path,
    project_root: &Path,
) -> Result<ArchiveReport, String> {
    let archive_root = checked_directory(source, "archive source")?;
    let manifest = read_and_validate_archive(&archive_root)?;
    fs::create_dir_all(projects_root)
        .map_err(|error| format!("Could not create Claude projects directory: {error}"))?;
    let destination_root = checked_directory(projects_root, "Claude projects directory")?;
    let project_root = checked_directory(project_root, "destination project")?;
    let target_key = project_key_for_directory(&project_root)?;
    let project_directory = destination_root.join(&target_key);
    ensure_project_directory(&destination_root, &project_directory)?;

    let source_stem = Path::new(&manifest.session_file)
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "The archive session filename is invalid.".to_owned())?;
    let source_title = read_display_title(&archive_root.join(&manifest.session_file))?;
    let existing = existing_sessions(&project_directory)?;
    let title = source_title
        .as_deref()
        .map(|title| unique_title(title, &existing.titles));

    let mut target_session = source_stem.to_owned();
    let mut target_file = project_directory.join(format!("{target_session}.jsonl"));
    let mut target_family = project_directory.join(&target_session);
    if path_exists(&target_file)? || path_exists(&target_family)? {
        loop {
            target_session = Uuid::new_v4().to_string();
            target_file = project_directory.join(format!("{target_session}.jsonl"));
            target_family = project_directory.join(&target_session);
            if !path_exists(&target_file)? && !path_exists(&target_family)? {
                break;
            }
        }
    }

    let (stage, staged_files, staged_dirs) = stage_import(
        &archive_root,
        &project_directory,
        &manifest,
        source_stem,
        &target_session,
        &project_root,
        title.as_deref(),
    )?;
    let result = install_staged(
        &stage,
        &project_directory,
        &target_family,
        &manifest,
        &staged_files,
        &staged_dirs,
    );
    remove_owned_tree(&stage, &staged_files);
    result?;

    Ok(ArchiveReport {
        action: ArchiveAction::Imported,
        source_session_id: manifest.source_session_id,
        session_id: format!("{target_key}/{target_session}.jsonl"),
        files: manifest.files.len(),
        path: target_file.to_string_lossy().into_owned(),
    })
}

fn indexed_source(
    app_data_dir: &Path,
    projects_root: &Path,
    session_id: &str,
) -> Result<(String, PathBuf), String> {
    let projects_root = checked_directory(projects_root, "Claude projects directory")?;
    let connection = open_database(app_data_dir).map_err(|error| error.to_string())?;
    let path: String = connection
        .query_row(
            "SELECT source_path FROM sessions WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("Could not find the indexed session: {error}"))?;
    let stored_file = PathBuf::from(path);
    let stored_metadata = fs::symlink_metadata(&stored_file)
        .map_err(|error| format!("Could not inspect the indexed session: {error}"))?;
    if crate::harness::atomic::is_link_or_reparse(&stored_metadata) || !stored_metadata.is_file() {
        return Err("The indexed session must be a plain file.".to_owned());
    }
    let source_file = stored_file
        .canonicalize()
        .map_err(|error| format!("Could not open the indexed session: {error}"))?;
    let source_parent = source_file
        .parent()
        .ok_or_else(|| "The indexed session has no project directory.".to_owned())?;
    let canonical_parent = source_parent
        .canonicalize()
        .map_err(|error| format!("Could not open the session project: {error}"))?;
    if !canonical_parent.starts_with(&projects_root)
        || canonical_parent.parent() != Some(projects_root.as_path())
    {
        return Err(
            "The indexed session is outside the configured Claude projects directory.".to_owned(),
        );
    }
    let file_name = source_file
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "The indexed session filename is invalid.".to_owned())?;
    let project_key = canonical_parent
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "The indexed project key is invalid.".to_owned())?
        .to_owned();
    if session_id != format!("{project_key}/{file_name}") {
        return Err("The indexed session identity no longer matches its source file.".to_owned());
    }
    Ok((project_key, source_file))
}

fn validate_session_filename(file_name: &str) -> Result<(), String> {
    let path = Path::new(file_name);
    if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
        return Err("Only native Claude `.jsonl` sessions can be archived.".to_owned());
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "The session filename has no valid session ID.".to_owned())?;
    Uuid::parse_str(stem).map(|_| ()).map_err(|_| {
        "The selected transcript does not have a Claude session UUID filename.".to_owned()
    })
}

fn collect_source_family(
    source_file: &Path,
    family_path: &Path,
    stem: &str,
) -> Result<SourceFamily, String> {
    let mut files = vec![(source_file.to_owned(), format!("{stem}.jsonl"))];
    let mut directories = Vec::new();
    match fs::symlink_metadata(family_path) {
        Ok(metadata) if crate::harness::atomic::is_link_or_reparse(&metadata) => {
            return Err("The session data directory cannot be a symbolic link.".to_owned());
        }
        Ok(metadata) if metadata.is_dir() => {
            collect_tree(family_path, stem, &mut files, &mut directories)?;
        }
        Ok(_) => return Err("The session data path is not a directory.".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not inspect session data: {error}")),
    }
    files.sort_by(|left, right| left.1.cmp(&right.1));
    directories.sort();
    Ok(SourceFamily { files, directories })
}

fn collect_tree(
    root: &Path,
    relative: &str,
    files: &mut Vec<(PathBuf, String)>,
    directories: &mut Vec<String>,
) -> Result<(), String> {
    directories.push(relative.to_owned());
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("Could not read session data: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not read session data entry: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "Session data contains a filename that is not valid UTF-8.".to_owned())?;
        validate_path_component(&name)?;
        let child = entry.path();
        let child_relative = format!("{relative}/{name}");
        let metadata = fs::symlink_metadata(&child)
            .map_err(|error| format!("Could not inspect session data: {error}"))?;
        if crate::harness::atomic::is_link_or_reparse(&metadata) {
            return Err("Session data contains a symbolic link or reparse point.".to_owned());
        }
        if metadata.is_dir() {
            collect_tree(&child, &child_relative, files, directories)?;
        } else if metadata.is_file() {
            files.push((child, child_relative));
        } else {
            return Err("Session data contains a non-regular file.".to_owned());
        }
    }
    Ok(())
}

fn write_export(
    stage: &Path,
    source_file: &Path,
    family: &SourceFamily,
    project_key: &str,
    source_session_id: &str,
    created_files: &mut Vec<PathBuf>,
) -> Result<Manifest, String> {
    let stem = Path::new(source_file)
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "The session filename is invalid.".to_owned())?;
    for directory in &family.directories {
        let relative = directory
            .strip_prefix(stem)
            .ok_or_else(|| "Session directory escaped its session family.".to_owned())?;
        let target = stage.join(stem).join(relative.trim_start_matches('/'));
        fs::create_dir_all(&target)
            .map_err(|error| format!("Could not create archive directory: {error}"))?;
    }
    let mut manifest_files = Vec::with_capacity(family.files.len());
    for (source, relative) in &family.files {
        let target = stage.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create archive directory: {error}"))?;
        }
        created_files.push(target.clone());
        let (digest, _) = copy_file(source, &target)?;
        manifest_files.push(ManifestFile {
            path: relative.clone(),
            sha256: digest,
        });
    }
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        source_session_id: source_session_id.to_owned(),
        source_project_key: project_key.to_owned(),
        session_file: format!("{stem}.jsonl"),
        family_directory: stem.to_owned(),
        directories: family.directories.clone(),
        files: manifest_files,
    };
    let manifest_path = stage.join(MANIFEST_NAME);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest_path)
        .map_err(|error| format!("Could not write archive manifest: {error}"))?;
    created_files.push(manifest_path);
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &manifest)
        .map_err(|error| format!("Could not encode archive manifest: {error}"))?;
    writer
        .flush()
        .map_err(|error| format!("Could not finish archive manifest: {error}"))?;
    Ok(manifest)
}

fn create_export_directory(destination: &Path, session_stem: &str) -> Result<PathBuf, String> {
    let mut index = 0usize;
    loop {
        let name = if index == 0 {
            format!("claude-session-{session_stem}")
        } else {
            format!("claude-session-{session_stem}-{index}")
        };
        let target = destination.join(name);
        match fs::create_dir(&target) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("Could not create archive directory: {error}")),
        }
        index += 1;
    }
}

fn read_and_validate_archive(root: &Path) -> Result<Manifest, String> {
    let manifest_path = root.join(MANIFEST_NAME);
    let metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|error| format!("The archive has no readable manifest: {error}"))?;
    if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err("The archive manifest must be a plain file.".to_owned());
    }
    let manifest: Manifest = serde_json::from_reader(
        File::open(&manifest_path)
            .map_err(|error| format!("Could not read archive manifest: {error}"))?,
    )
    .map_err(|error| format!("The archive manifest is invalid: {error}"))?;
    validate_manifest_shape(&manifest)?;

    let mut expected_files = HashSet::new();
    let mut expected_directories = HashSet::new();
    expected_files.insert(MANIFEST_NAME.to_owned());
    for directory in &manifest.directories {
        validate_archive_relative(directory, &manifest.family_directory)?;
        if !expected_directories.insert(directory.clone()) {
            return Err("The archive manifest contains a duplicate directory.".to_owned());
        }
    }
    for file in &manifest.files {
        validate_archive_relative(&file.path, &manifest.family_directory)?;
        if !expected_files.insert(file.path.clone()) {
            return Err("The archive manifest contains a duplicate file.".to_owned());
        }
        if expected_directories.contains(&file.path) {
            return Err(
                "The archive manifest uses one path as both a file and directory.".to_owned(),
            );
        }
        let path = root.join(file.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("The archive is missing a listed file: {error}"))?;
        if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
            return Err("The archive contains a symlink or non-regular file.".to_owned());
        }
        let (digest, _) = hash_file(&path)?;
        if digest != file.sha256 {
            return Err(format!(
                "The archive checksum does not match for `{}`.",
                file.path
            ));
        }
    }
    if !expected_files.contains(&manifest.session_file) {
        return Err("The archive does not contain its main session transcript.".to_owned());
    }
    validate_archive_inventory(root, &expected_files, &expected_directories)?;
    Ok(manifest)
}

fn validate_manifest_shape(manifest: &Manifest) -> Result<(), String> {
    if manifest.version != MANIFEST_VERSION {
        return Err(format!(
            "Unsupported transcript archive version {}.",
            manifest.version
        ));
    }
    validate_session_filename(&manifest.session_file)?;
    let stem = Path::new(&manifest.session_file)
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "The archive session filename is invalid.".to_owned())?;
    if manifest.family_directory != stem
        || manifest.source_project_key.is_empty()
        || manifest.source_project_key.contains('/')
        || manifest.source_project_key.contains('\\')
        || manifest.source_session_id
            != format!("{}/{}", manifest.source_project_key, manifest.session_file)
    {
        return Err("The archive session identity is inconsistent.".to_owned());
    }
    Ok(())
}

fn validate_archive_relative(path: &str, family_directory: &str) -> Result<(), String> {
    if path == format!("{family_directory}.jsonl") {
        return Ok(());
    }
    if path == family_directory || path.starts_with(&format!("{family_directory}/")) {
        let components = path.split('/').collect::<Vec<_>>();
        if components.iter().all(|component| {
            !component.is_empty()
                && *component != "."
                && *component != ".."
                && !component.contains('\\')
                && !component.contains(':')
                && !component.contains('\0')
        }) {
            return Ok(());
        }
    }
    Err("The archive contains a path outside its session family.".to_owned())
}

fn validate_archive_inventory(
    root: &Path,
    expected_files: &HashSet<String>,
    expected_directories: &HashSet<String>,
) -> Result<(), String> {
    let mut actual_files = HashSet::new();
    let mut actual_directories = HashSet::new();
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("Could not inspect archive contents: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not inspect archive contents: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "The archive contains a filename that is not valid UTF-8.".to_owned())?;
        validate_path_component(&name)?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("Could not inspect archive contents: {error}"))?;
        if crate::harness::atomic::is_link_or_reparse(&metadata) {
            return Err("The archive contains a symbolic link or reparse point.".to_owned());
        }
        if metadata.is_dir() {
            inventory_tree(&path, &name, &mut actual_files, &mut actual_directories)?;
            actual_directories.insert(name);
        } else if metadata.is_file() {
            actual_files.insert(name);
        } else {
            return Err("The archive contains a non-regular file.".to_owned());
        }
    }
    if &actual_files != expected_files || &actual_directories != expected_directories {
        return Err(
            "The archive contains files or directories not listed in its manifest.".to_owned(),
        );
    }
    Ok(())
}

fn inventory_tree(
    directory: &Path,
    relative: &str,
    files: &mut HashSet<String>,
    directories: &mut HashSet<String>,
) -> Result<(), String> {
    directories.insert(relative.to_owned());
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("Could not inspect archive directory: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not inspect archive directory: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "The archive contains a filename that is not valid UTF-8.".to_owned())?;
        validate_path_component(&name)?;
        let child = entry.path();
        let path = format!("{relative}/{name}");
        let metadata = fs::symlink_metadata(&child)
            .map_err(|error| format!("Could not inspect archive contents: {error}"))?;
        if crate::harness::atomic::is_link_or_reparse(&metadata) {
            return Err("The archive contains a symbolic link or reparse point.".to_owned());
        }
        if metadata.is_dir() {
            inventory_tree(&child, &path, files, directories)?;
        } else if metadata.is_file() {
            files.insert(path);
        } else {
            return Err("The archive contains a non-regular file.".to_owned());
        }
    }
    Ok(())
}

fn stage_import(
    archive_root: &Path,
    project_directory: &Path,
    manifest: &Manifest,
    source_stem: &str,
    target_session: &str,
    target_cwd: &Path,
    title: Option<&str>,
) -> Result<(PathBuf, Vec<PathBuf>, Vec<PathBuf>), String> {
    let stage = loop {
        let path = project_directory.join(format!(".orbit-import-{}", Uuid::new_v4()));
        match fs::create_dir(&path) {
            Ok(()) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not create import staging folder: {error}")),
        }
    };
    let mut written_files = Vec::new();
    let mut written_dirs = Vec::new();
    let result = (|| {
        for directory in &manifest.directories {
            let relative = remap_relative(directory, source_stem, target_session)?;
            let target = stage.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            fs::create_dir_all(&target)
                .map_err(|error| format!("Could not stage imported session data: {error}"))?;
            written_dirs.push(target);
        }
        for file in &manifest.files {
            let relative = remap_relative(&file.path, source_stem, target_session)?;
            let source = archive_root.join(file.path.replace('/', std::path::MAIN_SEPARATOR_STR));
            let target = stage.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("Could not stage imported session data: {error}"))?;
            }
            written_files.push(target.clone());
            if Path::new(&file.path)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
            {
                transform_jsonl(
                    &source,
                    &target,
                    source_stem,
                    target_session,
                    &claude_path_string(target_cwd)?,
                    if file.path == manifest.session_file {
                        title
                    } else {
                        None
                    },
                )?;
            } else {
                copy_file(&source, &target)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        remove_owned_tree(&stage, &written_files);
        return Err(error);
    }
    Ok((stage, written_files, written_dirs))
}

fn install_staged(
    stage: &Path,
    project_directory: &Path,
    target_family: &Path,
    manifest: &Manifest,
    staged_files: &[PathBuf],
    staged_dirs: &[PathBuf],
) -> Result<(), String> {
    let mut installed_files = Vec::new();
    let mut installed_dirs = Vec::new();
    let result = (|| {
        let family_exists = !manifest.directories.is_empty();
        if family_exists {
            fs::create_dir(target_family)
                .map_err(|error| format!("The destination session directory already exists or cannot be created: {error}"))?;
            installed_dirs.push(target_family.to_owned());
        }
        for directory in staged_dirs {
            let relative = directory
                .strip_prefix(stage)
                .map_err(|_| "Staged session directory escaped its staging folder.".to_owned())?;
            let destination = project_directory.join(relative);
            if destination != target_family {
                fs::create_dir(&destination).map_err(|error| {
                    format!("Could not create imported session directory: {error}")
                })?;
                installed_dirs.push(destination);
            }
        }
        for file in staged_files {
            let relative = file
                .strip_prefix(stage)
                .map_err(|_| "Staged session file escaped its staging folder.".to_owned())?;
            let destination = project_directory.join(relative);
            copy_file_exclusive(file, &destination)?;
            installed_files.push(destination);
        }
        Ok(())
    })();
    if result.is_err() {
        for path in installed_files.iter().rev() {
            let _ = fs::remove_file(path);
        }
        for path in installed_dirs.iter().rev() {
            let _ = fs::remove_dir(path);
        }
    }
    result
}

fn transform_jsonl(
    source: &Path,
    target: &Path,
    source_session: &str,
    target_session: &str,
    target_cwd: &str,
    appended_title: Option<&str>,
) -> Result<(), String> {
    let input = File::open(source)
        .map_err(|error| format!("Could not read archived transcript: {error}"))?;
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("Could not stage archived transcript: {error}"))?;
    let mut reader = BufReader::new(input);
    let mut writer = BufWriter::new(output);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("Could not read archived transcript: {error}"))?;
        if read == 0 {
            break;
        }
        let line_ending_len = if line.ends_with(b"\r\n") {
            2
        } else if line.ends_with(b"\n") {
            1
        } else {
            0
        };
        let content_end = line.len() - line_ending_len;
        let parsed = serde_json::from_slice::<Value>(&line[..content_end]);
        match parsed {
            Ok(Value::Object(mut object)) => {
                let mut changed = false;
                changed |= update_string(&mut object, "sessionId", source_session, target_session);
                changed |= replace_string(&mut object, "cwd", target_cwd);
                if changed {
                    serde_json::to_writer(&mut writer, &Value::Object(object)).map_err(
                        |error| format!("Could not rewrite transcript metadata: {error}"),
                    )?;
                    if line_ending_len == 2 {
                        writer.write_all(b"\r\n").map_err(|error| {
                            format!("Could not rewrite transcript metadata: {error}")
                        })?;
                    } else if line_ending_len == 1 {
                        writer.write_all(b"\n").map_err(|error| {
                            format!("Could not rewrite transcript metadata: {error}")
                        })?;
                    }
                } else {
                    writer
                        .write_all(&line)
                        .map_err(|error| format!("Could not write staged transcript: {error}"))?;
                }
            }
            _ => writer
                .write_all(&line)
                .map_err(|error| format!("Could not write staged transcript: {error}"))?,
        }
    }
    if let Some(title) = appended_title {
        writer
            .write_all(b"\n")
            .map_err(|error| format!("Could not add imported chat title: {error}"))?;
        let record = serde_json::json!({
            "type": "custom-title",
            "customTitle": title,
            "sessionId": target_session,
        });
        serde_json::to_writer(&mut writer, &record)
            .map_err(|error| format!("Could not add imported chat title: {error}"))?;
        writer
            .write_all(b"\n")
            .map_err(|error| format!("Could not add imported chat title: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("Could not finish staged transcript: {error}"))
}

fn update_string(object: &mut Map<String, Value>, key: &str, from: &str, to: &str) -> bool {
    if object.get(key).and_then(Value::as_str) == Some(from) {
        object.insert(key.to_owned(), Value::String(to.to_owned()));
        true
    } else {
        false
    }
}

fn replace_string(object: &mut Map<String, Value>, key: &str, value: &str) -> bool {
    if object.get(key).is_some_and(Value::is_string)
        && object.get(key).and_then(Value::as_str) != Some(value)
    {
        object.insert(key.to_owned(), Value::String(value.to_owned()));
        true
    } else {
        false
    }
}

fn read_display_title(path: &Path) -> Result<Option<String>, String> {
    let file =
        File::open(path).map_err(|error| format!("Could not read archived transcript: {error}"))?;
    let mut title = SessionTitle::default();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| format!("Could not read archived transcript: {error}"))?;
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            title.observe(&value);
        }
    }
    Ok(title.finish())
}

#[derive(Default)]
struct ExistingSessions {
    titles: HashSet<String>,
}

fn existing_sessions(project_directory: &Path) -> Result<ExistingSessions, String> {
    let mut existing = ExistingSessions::default();
    let entries = match fs::read_dir(project_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(existing),
        Err(error) => return Err(format!("Could not inspect destination project: {error}")),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Could not inspect destination project: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("Could not inspect destination project: {error}"))?;
        if crate::harness::atomic::is_link_or_reparse(&metadata)
            || !metadata.is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("jsonl")
        {
            continue;
        }
        if let Some(title) = read_display_title(&path)? {
            existing.titles.insert(title);
        }
    }
    Ok(existing)
}

fn unique_title(source: &str, existing: &HashSet<String>) -> String {
    if !existing.contains(source) {
        return source.to_owned();
    }
    let mut index = 1u64;
    loop {
        let candidate = format!("{source}-{index}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn ensure_project_directory(projects_root: &Path, project_directory: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(project_directory) {
        if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() {
            return Err("The Claude project storage path is not a plain directory.".to_owned());
        }
        let canonical = project_directory
            .canonicalize()
            .map_err(|error| format!("Could not inspect Claude project storage: {error}"))?;
        if canonical.parent() != Some(projects_root) {
            return Err(
                "The Claude project storage path escapes the configured projects directory."
                    .to_owned(),
            );
        }
    } else {
        fs::create_dir(project_directory)
            .map_err(|error| format!("Could not create Claude project storage: {error}"))?;
    }
    Ok(())
}

/// Match the SDK's documented project key rule for the normal-length path
/// case. The SDK explicitly notes that its long-path hash differs from the
/// CLI's Bun hash, so this boundary rejects paths where the SDK cannot prove
/// the CLI key.
fn project_key_for_directory(path: &Path) -> Result<String, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Could not resolve the destination project path: {error}"))?;
    let path_string = claude_path_string(&canonical)?;
    let normalized = path_string.nfc().collect::<String>();
    let sanitized = normalized
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.chars().count() > 200 {
        return Err(
            "This project path is too long to map safely to Claude's project folder.".to_owned(),
        );
    }
    Ok(sanitized)
}

fn claude_path_string(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or_else(|| "The destination project path is not valid UTF-8.".to_owned())?;
    #[cfg(windows)]
    {
        Ok(strip_windows_extended_prefix(value))
    }
    #[cfg(not(windows))]
    {
        Ok(value.to_owned())
    }
}

#[cfg(windows)]
fn strip_windows_extended_prefix(path: &str) -> String {
    if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else if let Some(drive) = path.strip_prefix("\\\\?\\") {
        drive.to_owned()
    } else {
        path.to_owned()
    }
}

fn checked_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect {label}: {error}"))?;
    if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!("The {label} must be a plain directory."));
    }
    path.canonicalize()
        .map_err(|error| format!("Could not resolve {label}: {error}"))
}

fn validate_path_component(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value.contains('\0')
    {
        return Err("The archive contains an unsafe path component.".to_owned());
    }
    Ok(())
}

fn remap_relative(path: &str, source_stem: &str, target_stem: &str) -> Result<String, String> {
    let first = path
        .split_once('/')
        .map(|(first, rest)| (first, Some(rest)))
        .unwrap_or((path, None));
    if first.0 != source_stem && first.0 != format!("{source_stem}.jsonl") {
        return Err("The archive path does not belong to its session family.".to_owned());
    }
    let remapped = if first.0 == source_stem {
        match first.1 {
            Some(rest) => format!("{target_stem}/{rest}"),
            None => target_stem.to_owned(),
        }
    } else {
        format!("{target_stem}.jsonl")
    };
    validate_archive_relative(&remapped, target_stem)?;
    Ok(remapped)
}

fn path_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not inspect path: {error}")),
    }
}

fn copy_file(source: &Path, target: &Path) -> Result<(String, u64), String> {
    let source_metadata =
        fs::symlink_metadata(source).map_err(|error| format!("Could not inspect file: {error}"))?;
    if crate::harness::atomic::is_link_or_reparse(&source_metadata) || !source_metadata.is_file() {
        return Err("Archive data must contain only plain files.".to_owned());
    }
    let mut input = File::open(source).map_err(|error| format!("Could not read file: {error}"))?;
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("Could not create file: {error}"))?;
    let mut output = BufWriter::new(output);
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| format!("Could not read file: {error}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("Could not write file: {error}"))?;
        size += count as u64;
    }
    output
        .flush()
        .map_err(|error| format!("Could not finish file: {error}"))?;
    Ok((format!("{:x}", hasher.finalize()), size))
}

fn copy_file_exclusive(source: &Path, target: &Path) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("Could not inspect staged file: {error}"))?;
    if crate::harness::atomic::is_link_or_reparse(&source_metadata) || !source_metadata.is_file() {
        return Err("Staged archive data must contain only plain files.".to_owned());
    }
    let mut input =
        File::open(source).map_err(|error| format!("Could not read staged file: {error}"))?;
    copy_to_new_file(&mut input, target)
}

fn copy_to_new_file(input: &mut impl Read, target: &Path) -> Result<(), String> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("Could not install file without overwriting: {error}"))?;
    let result = std::io::copy(input, &mut output).and_then(|_| output.flush());
    drop(output);
    result.map_err(|error| {
        let _ = fs::remove_file(target);
        format!("Could not install file: {error}")
    })
}

fn hash_file(path: &Path) -> Result<(String, u64), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect archive file: {error}"))?;
    if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err("The archive contains a symlink or non-regular file.".to_owned());
    }
    let mut input =
        File::open(path).map_err(|error| format!("Could not read archive file: {error}"))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| format!("Could not read archive file: {error}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

fn remove_owned_tree(root: &Path, files: &[PathBuf]) {
    for file in files.iter().rev() {
        let _ = fs::remove_file(file);
    }
    let mut directories = Vec::new();
    if root.exists() {
        collect_empty_dirs(root, &mut directories);
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        let _ = fs::remove_dir(directory);
    }
    let _ = fs::remove_dir(root);
}

fn collect_empty_dirs(path: &Path, directories: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&child) else {
            continue;
        };
        if crate::harness::atomic::is_link_or_reparse(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            collect_empty_dirs(&child, directories);
            directories.push(child);
        }
    }
    directories.push(path.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn interrupted_install_removes_its_partial_file_without_overwriting_existing_files() {
        struct InterruptedReader(bool);
        impl Read for InterruptedReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::other("interrupted source"));
                }
                self.0 = true;
                buffer[..4].copy_from_slice(b"part");
                Ok(4)
            }
        }
        let temp = tempdir().unwrap();
        let target = temp.path().join("session.jsonl");
        assert!(copy_to_new_file(&mut InterruptedReader(false), &target).is_err());
        assert!(!target.exists());
        fs::write(&target, b"existing chat").unwrap();
        assert!(copy_to_new_file(&mut InterruptedReader(false), &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"existing chat");
    }

    fn init_source(root: &Path) -> (PathBuf, PathBuf, String, String, Vec<u8>, Vec<u8>) {
        let projects = root.join("claude").join("projects");
        let project_root = root.join("source-project");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&project_root).unwrap();
        let project_key = project_key_for_directory(&project_root).unwrap();
        let bucket = projects.join(&project_key);
        fs::create_dir_all(&bucket).unwrap();
        let session = "11111111-2222-4333-8444-555555555555";
        let session_id = format!("{project_key}/{session}.jsonl");
        let session_bytes = b"{\"type\":\"user\",\"sessionId\":\"11111111-2222-4333-8444-555555555555\",\"cwd\":\"source\",\"uuid\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n{\"type\":\"custom-title\",\"sessionId\":\"11111111-2222-4333-8444-555555555555\",\"customTitle\":\"Agent\"}\n".to_vec();
        let subagent_bytes = b"{\"type\":\"assistant\",\"sessionId\":\"11111111-2222-4333-8444-555555555555\",\"cwd\":\"source\",\"uuid\":\"a1\"}\n".to_vec();
        let file = bucket.join(format!("{session}.jsonl"));
        fs::write(&file, &session_bytes).unwrap();
        let family = bucket.join(session);
        fs::create_dir_all(family.join("subagents")).unwrap();
        fs::write(family.join("subagents/agent-a1.jsonl"), &subagent_bytes).unwrap();
        fs::write(
            family.join("subagents/agent-a1.meta.json"),
            br#"{"agentId":"a1"}"#,
        )
        .unwrap();
        (
            projects,
            project_root,
            session_id,
            file.to_string_lossy().into_owned(),
            session_bytes,
            subagent_bytes,
        )
    }

    fn register_source(app_data: &Path, session_id: &str, source_path: &str) {
        let connection = open_database(app_data).unwrap();
        connection.execute(
            "INSERT INTO sessions(session_id, project, source_path, updated_at, message_count) VALUES (?1, ?2, ?3, NULL, 0)",
            rusqlite::params![session_id, session_id.split('/').next().unwrap(), source_path],
        ).unwrap();
    }

    #[test]
    fn archive_round_trip_preserves_native_files_and_remaps_cwd() {
        let temp = tempdir().unwrap();
        let (projects, source_root, session_id, source_path, source_bytes, subagent_bytes) =
            init_source(temp.path());
        let app_data = temp.path().join("app-data");
        register_source(&app_data, &session_id, &source_path);
        let archive_destination = temp.path().join("archive-destination");
        fs::create_dir_all(&archive_destination).unwrap();
        let exported =
            export_archive(&app_data, &projects, &session_id, &archive_destination).unwrap();
        let archive = PathBuf::from(&exported.path);
        assert_eq!(
            fs::read(archive.join("11111111-2222-4333-8444-555555555555.jsonl")).unwrap(),
            source_bytes
        );
        assert_eq!(
            fs::read(archive.join("11111111-2222-4333-8444-555555555555/subagents/agent-a1.jsonl"))
                .unwrap(),
            subagent_bytes
        );
        assert_eq!(exported.files, 3);

        let destination = temp.path().join("destination-project");
        fs::create_dir_all(&destination).unwrap();
        let imported_projects = temp.path().join("new-config/projects");
        let imported = import_archive(&imported_projects, &archive, &destination).unwrap();
        let imported_file = PathBuf::from(&imported.path);
        let imported_json = fs::read_to_string(&imported_file).unwrap();
        let first_record: Value =
            serde_json::from_str(imported_json.lines().next().unwrap()).unwrap();
        assert_eq!(
            first_record["cwd"],
            claude_path_string(&destination.canonicalize().unwrap()).unwrap()
        );
        assert!(imported_json.contains("\"customTitle\":\"Agent\""));
        let imported_subagent = imported_file
            .parent()
            .unwrap()
            .join(
                imported
                    .session_id
                    .split('/')
                    .next_back()
                    .unwrap()
                    .trim_end_matches(".jsonl"),
            )
            .join("subagents/agent-a1.jsonl");
        let subagent: Value = serde_json::from_str(
            fs::read_to_string(imported_subagent)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            subagent["cwd"],
            claude_path_string(&destination.canonicalize().unwrap()).unwrap()
        );
        assert_eq!(
            fs::read(archive.join("11111111-2222-4333-8444-555555555555.jsonl")).unwrap(),
            source_bytes
        );
        let _ = source_root;
    }

    #[test]
    fn import_collision_keeps_existing_files_and_suffixes_title() {
        let temp = tempdir().unwrap();
        let (projects, _, session_id, source_path, _, _) = init_source(temp.path());
        let app_data = temp.path().join("app-data");
        register_source(&app_data, &session_id, &source_path);
        let archive_destination = temp.path().join("archive-destination");
        fs::create_dir_all(&archive_destination).unwrap();
        let archive =
            export_archive(&app_data, &projects, &session_id, &archive_destination).unwrap();
        let archive = PathBuf::from(archive.path);
        let destination = temp.path().join("destination-project");
        fs::create_dir_all(&destination).unwrap();
        let target_key = project_key_for_directory(&destination).unwrap();
        let target_bucket = projects.join(&target_key);
        fs::create_dir_all(&target_bucket).unwrap();
        let existing_id = "11111111-2222-4333-8444-555555555555";
        fs::write(
            target_bucket.join(format!("{existing_id}.jsonl")),
            "{\"type\":\"custom-title\",\"customTitle\":\"Agent\"}\n",
        )
        .unwrap();
        fs::create_dir_all(target_bucket.join(existing_id)).unwrap();
        fs::write(target_bucket.join(existing_id).join("untouched"), "keep").unwrap();
        let imported = import_archive(&projects, &archive, &destination).unwrap();
        assert_ne!(imported.session_id, session_id);
        assert_eq!(
            fs::read(target_bucket.join(existing_id).join("untouched")).unwrap(),
            b"keep"
        );
        let imported_text = fs::read_to_string(imported.path).unwrap();
        assert!(imported_text.contains("\"customTitle\":\"Agent-1\""));
        super::super::ingest::ingest_projects(projects.parent().unwrap(), &app_data).unwrap();
        let sessions = super::super::ingest::list_sessions(&app_data).unwrap();
        assert_eq!(
            sessions
                .iter()
                .find(|session| session.session_id == imported.session_id)
                .unwrap()
                .title
                .as_deref(),
            Some("Agent-1")
        );
    }

    #[test]
    fn import_rejects_manifest_traversal_checksum_changes_and_symlinks() {
        let temp = tempdir().unwrap();
        let (projects, _, session_id, source_path, _, _) = init_source(temp.path());
        let app_data = temp.path().join("app-data");
        register_source(&app_data, &session_id, &source_path);
        let destination = temp.path().join("archive-destination");
        fs::create_dir_all(&destination).unwrap();
        let exported = export_archive(&app_data, &projects, &session_id, &destination).unwrap();
        let archive = PathBuf::from(exported.path);
        let manifest_path = archive.join(MANIFEST_NAME);
        let original = fs::read(&manifest_path).unwrap();
        let mut manifest: Manifest = serde_json::from_slice(&original).unwrap();
        manifest.files[0].path = "../outside".to_owned();
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let project = temp.path().join("destination-project");
        fs::create_dir_all(&project).unwrap();
        assert!(import_archive(&projects, &archive, &project).is_err());
        fs::write(&manifest_path, original).unwrap();

        let transcript = archive.join("11111111-2222-4333-8444-555555555555.jsonl");
        fs::write(&transcript, b"changed\n").unwrap();
        assert!(import_archive(&projects, &archive, &project)
            .unwrap_err()
            .contains("checksum"));
    }

    #[cfg(unix)]
    #[test]
    fn import_rejects_archive_symlinks() {
        use std::os::unix::fs::symlink;
        let temp = tempdir().unwrap();
        let (projects, _, session_id, source_path, _, _) = init_source(temp.path());
        let app_data = temp.path().join("app-data");
        register_source(&app_data, &session_id, &source_path);
        let destination = temp.path().join("archive-destination");
        fs::create_dir_all(&destination).unwrap();
        let exported = export_archive(&app_data, &projects, &session_id, &destination).unwrap();
        let archive = PathBuf::from(exported.path);
        let outside = temp.path().join("outside.jsonl");
        fs::write(&outside, b"outside").unwrap();
        let link = archive.join("11111111-2222-4333-8444-555555555555/linked.jsonl");
        symlink(&outside, &link).unwrap();
        let project = temp.path().join("destination-project");
        fs::create_dir_all(&project).unwrap();
        assert!(import_archive(&projects, &archive, &project)
            .unwrap_err()
            .contains("symbolic link"));
    }
}
