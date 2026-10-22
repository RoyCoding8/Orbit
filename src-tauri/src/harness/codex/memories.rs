//! Native Codex instruction and auto-memory files.
//!
//! Generated auto-memory is displayed for inspection but is not an editing
//! surface. AGENTS files are ordinary instructions and remain user-editable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::CodexHarness;
use crate::{
    harness::{
        atomic,
        claude::memories::{
            MemoryCatalog, MemoryFile, MemoryGroup, MemoryReference, MemoryRole, MemorySource,
            ReferenceStatus,
        },
        memory_links::{parse_references, resolve_reference, without_references},
        Harness, ResourceKind,
    },
    ledger,
};

#[derive(Clone)]
struct Candidate {
    path: PathBuf,
    group_id: String,
    source: MemorySource,
    role: MemoryRole,
    editable: bool,
    load_behavior: String,
}

pub fn catalog(harness: &CodexHarness, project: &Path) -> Result<MemoryCatalog, String> {
    let project = canonical_directory(project)?;
    let config_root = optional_directory(harness.config_root())?;
    let mut groups = BTreeMap::<String, MemoryGroup>::new();
    let mut candidates = BTreeMap::<PathBuf, Candidate>::new();

    if let Some(config_root) = &config_root {
        let auto_memory = config_root.join("memories");
        if plain_directory(&auto_memory) {
            add_group(
                &mut groups,
                "global-auto-memory",
                "Codex generated memory",
                MemorySource::GlobalAutoMemory,
            );
            collect_markdown(
                &auto_memory,
                config_root,
                "global-auto-memory",
                MemorySource::GlobalAutoMemory,
                false,
                "Generated Codex memory; relevant contents may be used when memories are enabled. Use /memories to control it; read-only here.",
                &mut candidates,
            )?;
        }
        add_instruction_files(
            &[
                config_root.join("AGENTS.override.md"),
                config_root.join("AGENTS.md"),
            ],
            config_root,
            "user-instructions",
            "Global Codex instructions",
            MemorySource::UserInstructions,
            true,
            &mut groups,
            &mut candidates,
        )?;
    }
    let mut directories = Vec::new();
    let mut cursor = Some(project.as_path());
    let mut found_repo_root = false;
    while let Some(directory) = cursor {
        directories.push(directory.to_path_buf());
        if directory.join(".git").exists() {
            found_repo_root = true;
            break;
        }
        cursor = directory.parent();
    }
    if !found_repo_root {
        directories.truncate(1);
    }
    directories.reverse();
    let scope_root = directories
        .first()
        .cloned()
        .unwrap_or_else(|| project.clone());
    let project_group = "project-instructions";
    add_group(
        &mut groups,
        project_group,
        &format!("Project instructions · {}", project.display()),
        MemorySource::ProjectInstructions,
    );
    let fallback_names = fallback_filenames(harness)?;
    for directory in directories {
        let editable = directory == project;
        let mut paths = vec![
            directory.join("AGENTS.override.md"),
            directory.join("AGENTS.md"),
        ];
        paths.extend(fallback_names.iter().map(|name| directory.join(name)));
        add_instruction_files(
            &paths,
            &scope_root,
            project_group,
            &format!("Project instructions · {}", directory.display()),
            MemorySource::ProjectInstructions,
            editable,
            &mut groups,
            &mut candidates,
        )?;
    }

    let mut files = Vec::with_capacity(candidates.len());
    for candidate in candidates.values() {
        let content = read_plain(&candidate.path)?;
        files.push(MemoryFile {
            id: file_id(&candidate.path),
            group_id: candidate.group_id.clone(),
            name: candidate
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: candidate.path.to_string_lossy().into_owned(),
            source: candidate.source,
            role: candidate.role,
            load_behavior: candidate.load_behavior.clone(),
            editable: candidate.editable,
            content,
            references: Vec::new(),
        });
    }
    files.sort_by(|a, b| a.group_id.cmp(&b.group_id).then(a.path.cmp(&b.path)));
    let lookup = files
        .iter()
        .map(|file| (PathBuf::from(&file.path), file.id.clone()))
        .collect::<BTreeMap<_, _>>();
    for file in &mut files {
        let source = PathBuf::from(&file.path);
        for (target, kind) in parse_references(&file.content, false) {
            let resolved = resolve_reference(&source, &target);
            let file_id = resolved.as_ref().and_then(|path| lookup.get(path)).cloned();
            let status = if file_id.is_some() {
                ReferenceStatus::Known
            } else if is_external(
                &target,
                resolved.as_deref(),
                &project,
                config_root.as_deref(),
            ) {
                ReferenceStatus::External
            } else {
                ReferenceStatus::Unresolved
            };
            file.references.push(MemoryReference {
                target,
                kind,
                status,
                file_id,
            });
        }
    }
    Ok(MemoryCatalog {
        project_root: project.to_string_lossy().into_owned(),
        groups: groups.into_values().collect(),
        files,
        excluded_custom_memory_directory: None,
    })
}

pub fn edit(
    harness: &CodexHarness,
    project: &Path,
    id: &str,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    let catalog = catalog(harness, project)?;
    let file = catalog
        .files
        .iter()
        .find(|file| file.id == id && file.editable)
        .ok_or("instruction file is no longer available for editing")?;
    edit_file(harness, file, expected_content, content)
}

pub fn delete(
    harness: &CodexHarness,
    project: &Path,
    id: &str,
    expected_content: &str,
) -> Result<(), String> {
    let catalog = catalog(harness, project)?;
    let file = catalog
        .files
        .iter()
        .find(|file| file.id == id && file.editable)
        .ok_or("instruction file is no longer available for deletion")?;
    if file.content != expected_content {
        return Err(
            "Instruction file changed since this editor opened. Refresh the catalog and try again."
                .into(),
        );
    }
    let target = PathBuf::from(&file.path);
    for source in catalog
        .files
        .iter()
        .filter(|source| source.id != id && source.editable)
    {
        let content = without_references(source, &target, false);
        if content != source.content {
            edit_file(harness, source, &source.content, &content).map_err(|error| {
                format!(
                    "Could not remove references from {}. The instruction was not deleted. {error}",
                    source.name
                )
            })?;
        }
    }
    ensure_plain_file(&target)?;
    ledger::remove_file(
        harness.config_root(),
        resource_kind(file.source),
        id,
        "delete",
        &target,
        Sha256::digest(expected_content.as_bytes()).into(),
        &ledger::backup_path(harness.config_root(), id, "delete"),
    )
    .map_err(|error| error.to_string())
}

fn edit_file(
    harness: &CodexHarness,
    file: &MemoryFile,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    let path = Path::new(&file.path);
    ensure_plain_file(path)?;
    let backup = ledger::backup_path(harness.config_root(), &file.id, "edit");
    ledger::write(
        harness.config_root(),
        resource_kind(file.source),
        &file.id,
        "edit",
        path,
        atomic::RecordedWrite::Restore(
            Sha256::digest(expected_content.as_bytes()).into(),
            content.as_bytes(),
        ),
        Some(&backup),
    )
    .map(|_| ())
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("did not hold the expected state") {
            "Instruction file changed since this editor opened. Refresh the catalog and try again."
                .to_owned()
        } else {
            message
        }
    })
}

fn add_instruction_files(
    paths: &[PathBuf],
    boundary: &Path,
    group: &str,
    label: &str,
    source: MemorySource,
    editable: bool,
    groups: &mut BTreeMap<String, MemoryGroup>,
    candidates: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    add_group(groups, group, label, source);
    let selected = selected_instruction(paths)?;
    for path in paths {
        if !path_exists_no_follow(path)? {
            continue;
        }
        let content = read_plain(path)?;
        let canonical = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
        let load_behavior = if content.trim().is_empty() {
            "Empty instruction file; Codex falls through to the next instruction filename in this directory."
        } else if selected.as_ref() == Some(&canonical) {
            if source == MemorySource::UserInstructions {
                "Loaded global instruction file; Codex applies it across projects."
            } else {
                "Loaded instruction file; applicable files are combined from the repository root to the current directory."
            }
        } else {
            "Not loaded: a higher-priority nonempty instruction filename applies in this directory."
        };
        add_candidate_if_plain(
            path,
            boundary,
            group,
            source,
            MemoryRole::Instruction,
            editable,
            load_behavior,
            candidates,
        )?;
    }
    Ok(())
}

fn selected_instruction(paths: &[PathBuf]) -> Result<Option<PathBuf>, String> {
    for path in paths {
        if !path_exists_no_follow(path)? {
            continue;
        }
        if !read_plain(path)?.trim().is_empty() {
            return std::fs::canonicalize(path)
                .map(Some)
                .map_err(|error| error.to_string());
        }
    }
    Ok(None)
}

fn add_candidate_if_plain(
    path: &Path,
    boundary: &Path,
    group_id: &str,
    source: MemorySource,
    role: MemoryRole,
    editable: bool,
    load_behavior: &str,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    if !path_exists_no_follow(path)? {
        return Ok(());
    }
    ensure_plain_file(path)?;
    let canonical = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !canonical.starts_with(boundary) {
        return Ok(());
    }
    out.insert(
        canonical.clone(),
        Candidate {
            path: canonical,
            group_id: group_id.to_owned(),
            source,
            role,
            editable,
            load_behavior: load_behavior.to_owned(),
        },
    );
    Ok(())
}

fn collect_markdown(
    root: &Path,
    boundary: &Path,
    group_id: &str,
    source: MemorySource,
    editable: bool,
    load_behavior: &str,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    for path in safe_entries(root)? {
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if atomic::is_link_or_reparse(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            collect_markdown(
                &path,
                boundary,
                group_id,
                source,
                editable,
                load_behavior,
                out,
            )?;
        } else if metadata.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            let canonical = std::fs::canonicalize(&path).map_err(|error| error.to_string())?;
            if canonical.starts_with(boundary) {
                out.insert(
                    canonical.clone(),
                    Candidate {
                        path: canonical,
                        group_id: group_id.to_owned(),
                        source,
                        role: MemoryRole::Topic,
                        editable,
                        load_behavior: load_behavior.to_owned(),
                    },
                );
            }
        }
    }
    Ok(())
}

fn add_group(
    groups: &mut BTreeMap<String, MemoryGroup>,
    id: &str,
    label: &str,
    role: MemorySource,
) {
    groups.entry(id.to_owned()).or_insert_with(|| MemoryGroup {
        id: id.to_owned(),
        label: label.to_owned(),
        role,
    });
}

pub(crate) fn fallback_filenames(harness: &CodexHarness) -> Result<Vec<String>, String> {
    let value = super::store::read_value(&harness.settings_path())?;
    let Some(names) = value
        .get("project_doc_fallback_filenames")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(Vec::new());
    };
    Ok(names
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| {
            !name.is_empty()
                && Path::new(name).components().count() == 1
                && !matches!(
                    Path::new(name).components().next(),
                    Some(
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                )
        })
        .map(str::to_owned)
        .collect())
}

fn resource_kind(source: MemorySource) -> ResourceKind {
    match source {
        MemorySource::GlobalAutoMemory => ResourceKind::Memory,
        MemorySource::ProjectInstructions => ResourceKind::ProjectInstructions,
        MemorySource::UserInstructions => ResourceKind::GlobalInstructions,
        MemorySource::ProjectAutoMemory => ResourceKind::Memory,
    }
}

fn file_id(path: &Path) -> String {
    format!(
        "memory-{}",
        atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
    )
}

fn safe_entries(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = std::fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort();
    Ok(entries)
}

fn plain_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !atomic::is_link_or_reparse(&metadata))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    ensure_plain_components(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || atomic::is_link_or_reparse(&metadata) {
        return Err(format!("expected a plain directory: {}", path.display()));
    }
    std::fs::canonicalize(path).map_err(|error| error.to_string())
}

fn optional_directory(path: &Path) -> Result<Option<PathBuf>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => canonical_directory(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn ensure_plain_file(path: &Path) -> Result<(), String> {
    ensure_plain_components(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || atomic::is_link_or_reparse(&metadata) {
        return Err(format!("expected a plain file: {}", path.display()));
    }
    Ok(())
}

fn ensure_plain_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            std::path::Component::RootDir => current.push(component.as_os_str()),
            std::path::Component::Normal(name) => current.push(name),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => return Err("parent traversal is not allowed".into()),
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            if atomic::is_link_or_reparse(&metadata) {
                return Err(format!("refusing linked path: {}", current.display()));
            }
        }
    }
    Ok(())
}

fn path_exists_no_follow(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn read_plain(path: &Path) -> Result<String, String> {
    ensure_plain_file(path)?;
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn is_external(
    target: &str,
    resolved: Option<&Path>,
    project: &Path,
    global: Option<&Path>,
) -> bool {
    let target = target.trim();
    target.starts_with("http:")
        || target.starts_with("https:")
        || target.starts_with("mailto:")
        || resolved.is_some_and(|path| {
            !path.starts_with(project) && global.is_none_or(|root| !path.starts_with(root))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture() -> (tempfile::TempDir, CodexHarness, PathBuf) {
        let temp = tempdir().unwrap();
        let config_root = temp.path().join("codex-home");
        let home = temp.path().join("profile");
        let project = temp.path().join("repo");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(project.join(".git")).unwrap();
        (temp, CodexHarness::new(config_root, home), project)
    }

    #[test]
    fn catalog_separates_generated_memory_from_editable_instructions_and_indexes_links() {
        let (_temp, harness, project) = fixture();
        let generated = harness.config_root().join("memories/notes/topic.md");
        std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
        std::fs::write(&generated, "Generated notes.").unwrap();
        let generated = std::fs::canonicalize(generated).unwrap();
        let root_instruction = project.join("AGENTS.md");
        let nested = project.join("child");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&root_instruction, "# Repo rules\n").unwrap();
        std::fs::write(
            nested.join("AGENTS.override.md"),
            "See [repo](../AGENTS.md) and @../AGENTS.md.\n",
        )
        .unwrap();
        let root_instruction = std::fs::canonicalize(root_instruction).unwrap();

        let catalog = catalog(&harness, &nested).unwrap();
        let generated_entry = catalog
            .files
            .iter()
            .find(|file| file.path == generated.to_string_lossy())
            .unwrap();
        assert_eq!(generated_entry.source, MemorySource::GlobalAutoMemory);
        assert!(!generated_entry.editable);
        assert!(edit(
            &harness,
            &nested,
            &generated_entry.id,
            "Generated notes.",
            "changed"
        )
        .is_err());
        let link_file = catalog
            .files
            .iter()
            .find(|file| file.path.ends_with("AGENTS.override.md"))
            .unwrap();
        assert_eq!(link_file.references.len(), 1);
        assert_eq!(link_file.references[0].status, ReferenceStatus::Known);
        assert_eq!(
            link_file.references[0].kind,
            crate::harness::claude::memories::ReferenceKind::MarkdownLink
        );
        let ancestor_instruction = catalog
            .files
            .iter()
            .find(|file| file.path == root_instruction.to_string_lossy())
            .unwrap();
        assert!(!ancestor_instruction.editable);
    }

    #[test]
    fn manual_instruction_edit_is_stale_checked_and_delete_cleans_plain_links() {
        let (_temp, harness, project) = fixture();
        let root_instruction = project.join("AGENTS.md");
        let nested = project.join("AGENTS.override.md");
        std::fs::write(&root_instruction, "Root guidance.\n").unwrap();
        std::fs::write(&nested, "See [root](AGENTS.md).\n").unwrap();
        let root_instruction = std::fs::canonicalize(root_instruction).unwrap();
        let initial = catalog(&harness, &project).unwrap();
        let root = initial
            .files
            .iter()
            .find(|file| file.path == root_instruction.to_string_lossy())
            .unwrap();
        edit(
            &harness,
            &project,
            &root.id,
            "Root guidance.\n",
            "Updated guidance.\n",
        )
        .unwrap();
        assert!(
            edit(&harness, &project, &root.id, "Root guidance.\n", "stale")
                .unwrap_err()
                .contains("changed since")
        );
        let updated = catalog(&harness, &project).unwrap();
        let root = updated
            .files
            .iter()
            .find(|file| file.path == root_instruction.to_string_lossy())
            .unwrap();
        delete(&harness, &project, &root.id, &root.content).unwrap();
        assert!(!root_instruction.exists());
        assert_eq!(std::fs::read_to_string(nested).unwrap(), "See .\n");
    }

    #[test]
    fn configured_project_document_fallback_is_discovered_only_without_agents_file() {
        let (_temp, harness, project) = fixture();
        std::fs::write(
            harness.settings_path(),
            "project_doc_fallback_filenames = [\"TEAM.md\"]\n",
        )
        .unwrap();
        let fallback = project.join("TEAM.md");
        std::fs::write(&fallback, "Team instructions.").unwrap();
        let fallback = std::fs::canonicalize(fallback).unwrap();
        let initial = catalog(&harness, &project).unwrap();
        assert!(initial
            .files
            .iter()
            .any(|file| file.path == fallback.to_string_lossy()));

        std::fs::write(project.join("AGENTS.md"), "Canonical instructions.").unwrap();
        let canonical = catalog(&harness, &project).unwrap();
        let fallback_entry = canonical
            .files
            .iter()
            .find(|file| file.path == fallback.to_string_lossy())
            .unwrap();
        assert!(fallback_entry.load_behavior.starts_with("Not loaded"));
        let canonical_entry = canonical
            .files
            .iter()
            .find(|file| file.path.ends_with("AGENTS.md"))
            .unwrap();
        assert!(canonical_entry.load_behavior.starts_with("Loaded"));
    }

    #[test]
    fn empty_override_falls_through_to_nonempty_agents_file() {
        let (_temp, harness, project) = fixture();
        std::fs::write(project.join("AGENTS.override.md"), "\n  \n").unwrap();
        std::fs::write(project.join("AGENTS.md"), "Canonical instructions.").unwrap();
        let snapshot = catalog(&harness, &project).unwrap();
        let override_file = snapshot
            .files
            .iter()
            .find(|file| file.name == "AGENTS.override.md")
            .unwrap();
        let agents_file = snapshot
            .files
            .iter()
            .find(|file| file.name == "AGENTS.md")
            .unwrap();
        assert!(override_file.load_behavior.starts_with("Empty instruction"));
        assert!(agents_file.load_behavior.starts_with("Loaded"));
    }
}
