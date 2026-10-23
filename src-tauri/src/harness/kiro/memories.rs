use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::KiroHarness;
use crate::harness::{
    atomic,
    claude::memories::{
        MemoryCatalog, MemoryFile, MemoryGroup, MemoryReference, MemoryRole, MemorySource,
        ReferenceStatus,
    },
    memory_links::{parse_references, resolve_reference},
};

#[derive(Clone)]
struct Candidate {
    group_id: String,
    source: MemorySource,
}

pub fn catalog(harness: &KiroHarness, project: &Path) -> Result<MemoryCatalog, String> {
    let project = std::fs::canonicalize(project).map_err(|error| error.to_string())?;
    if !project.is_dir() {
        return Err("project directory is unavailable".into());
    }
    let mut groups = BTreeMap::new();
    let mut candidates = BTreeMap::<PathBuf, Candidate>::new();
    let global_root = harness.home().join("steering");
    groups.insert(
        "global-steering".to_owned(),
        MemoryGroup {
            id: "global-steering".into(),
            label: "Global Kiro steering".into(),
            role: MemorySource::UserInstructions,
        },
    );
    collect_markdown(
        &global_root,
        &global_root,
        "global-steering",
        MemorySource::UserInstructions,
        &mut candidates,
    )?;

    let project_root = harness.project_root(&project);
    let project_steering = project_root.join("steering");
    groups.insert(
        "project-steering".to_owned(),
        MemoryGroup {
            id: "project-steering".into(),
            label: "Project Kiro steering".into(),
            role: MemorySource::ProjectInstructions,
        },
    );
    groups.insert(
        "project-instructions".to_owned(),
        MemoryGroup {
            id: "project-instructions".into(),
            label: "Project instructions".into(),
            role: MemorySource::ProjectInstructions,
        },
    );
    collect_markdown(
        &project_steering,
        &project_steering,
        "project-steering",
        MemorySource::ProjectInstructions,
        &mut candidates,
    )?;
    collect_agents_files(&project, &mut candidates)?;

    let mut files_by_path = BTreeMap::<PathBuf, MemoryFile>::new();
    for (path, candidate) in candidates {
        let content = read_plain_file(&path)?;
        let id = memory_id(&path);
        files_by_path.insert(
            path.clone(),
            MemoryFile {
                id,
                group_id: candidate.group_id,
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: path.to_string_lossy().into_owned(),
                source: candidate.source,
                role: MemoryRole::Instruction,
                load_behavior: steering_behavior(&content),
                editable: true,
                content,
                references: Vec::new(),
            },
        );
    }
    let known: BTreeMap<PathBuf, String> = files_by_path
        .iter()
        .map(|(path, file)| (path.clone(), file.id.clone()))
        .collect();
    for file in files_by_path.values_mut() {
        file.references = parse_references(&file.content, false)
            .into_iter()
            .map(|(target, kind)| {
                let resolved = resolve_reference(Path::new(&file.path), &target);
                let known_target = resolved
                    .as_ref()
                    .is_some_and(|path| known.contains_key(path));
                MemoryReference {
                    target,
                    kind,
                    status: if resolved.as_deref().is_some_and(|path| {
                        path.starts_with(harness.home()) || path.starts_with(&project)
                    }) {
                        if known_target {
                            ReferenceStatus::Known
                        } else {
                            ReferenceStatus::Unresolved
                        }
                    } else {
                        ReferenceStatus::External
                    },
                    file_id: resolved.and_then(|path| known.get(&path).cloned()),
                }
            })
            .collect();
    }
    Ok(MemoryCatalog {
        project_root: project.to_string_lossy().into_owned(),
        groups: groups.into_values().collect(),
        files: files_by_path.into_values().collect(),
        excluded_custom_memory_directory: None,
    })
}

pub fn edit(
    harness: &KiroHarness,
    project: &Path,
    id: &str,
    expected: &str,
    content: &str,
) -> Result<(), String> {
    let file = catalog(harness, project)?
        .files
        .into_iter()
        .find(|file| file.id == id && file.editable)
        .ok_or("instruction file is no longer available for editing")?;
    crate::harness::instruction_store::edit(harness, &file, expected, content, "edit")
}

pub fn delete(
    harness: &KiroHarness,
    project: &Path,
    id: &str,
    expected: &str,
) -> Result<(), String> {
    crate::harness::instruction_store::delete(
        harness,
        &catalog(harness, project)?,
        id,
        expected,
        "edit",
        "delete",
    )
}

fn collect_markdown(
    root: &Path,
    boundary: &Path,
    group_id: &str,
    source: MemorySource,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    for path in bounded_files(root)? {
        if path.extension().is_some_and(|ext| ext == "md") {
            add_candidate(path, boundary, group_id, source, out)?;
        }
    }
    Ok(())
}

fn collect_agents_files(
    project: &Path,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    if project.join("AGENTS.md").is_file() {
        add_candidate(
            project.join("AGENTS.md"),
            project,
            "project-instructions",
            MemorySource::ProjectInstructions,
            out,
        )?;
    }
    collect_named_agents(project, project, out)
}

fn collect_named_agents(
    directory: &Path,
    boundary: &Path,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    for path in bounded_files(directory)? {
        if path.file_name().is_some_and(|name| name == "AGENTS.md")
            && path != boundary.join("AGENTS.md")
        {
            add_candidate(
                path,
                boundary,
                "project-instructions",
                MemorySource::ProjectInstructions,
                out,
            )?;
        }
    }
    Ok(())
}

const MAX_DISCOVERY_DIRECTORIES: usize = 4_096;
const MAX_DISCOVERY_ENTRIES: usize = 50_000;
const MAX_DISCOVERY_DEPTH: usize = 32;

fn bounded_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Ok(Vec::new());
    }

    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut files = Vec::new();
    let mut inspected_directories = 0usize;
    let mut inspected_entries = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        inspected_directories += 1;
        if inspected_directories > MAX_DISCOVERY_DIRECTORIES {
            return Err(format!(
                "Kiro instruction discovery exceeded its {MAX_DISCOVERY_DIRECTORIES}-directory safety limit"
            ));
        }
        for entry in std::fs::read_dir(&directory).map_err(|error| error.to_string())? {
            inspected_entries += 1;
            if inspected_entries > MAX_DISCOVERY_ENTRIES {
                return Err(format!(
                    "Kiro instruction discovery exceeded its {MAX_DISCOVERY_ENTRIES}-entry safety limit"
                ));
            }
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if atomic::is_link_or_reparse(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                if skipped_directory(entry.file_name().to_string_lossy().as_ref()) {
                    continue;
                }
                if depth >= MAX_DISCOVERY_DEPTH {
                    return Err(format!(
                        "Kiro instruction discovery reached its maximum depth of {MAX_DISCOVERY_DEPTH} at {}",
                        path.display()
                    ));
                }
                pending.push((path, depth + 1));
            } else if metadata.is_file() {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn skipped_directory(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "node_modules"
            | "vendor"
            | "deps"
            | "dependencies"
            | "third_party"
            | "third-party"
            | "thirdparty"
            | "target"
            | "dist"
            | "build"
            | "out"
            | "coverage"
            | "bower_components"
            | "venv"
    )
}

pub(crate) fn allowed_instruction_path(
    _harness: &KiroHarness,
    project: &Path,
    target: &Path,
) -> bool {
    let Ok(project) = std::fs::canonicalize(project) else {
        return false;
    };
    if !project.is_dir() {
        return false;
    }
    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        project.join(target)
    };
    if target
        .components()
        .any(|component| matches!(component, Component::ParentDir))
        || crate::harness::claude::skills::ensure_plain_components(&target).is_err()
    {
        return false;
    }

    let mut nearest = target.clone();
    let mut missing_tail = Vec::new();
    let nearest_metadata = loop {
        match std::fs::symlink_metadata(&nearest) {
            Ok(metadata) => break metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = nearest.file_name() else {
                    return false;
                };
                missing_tail.push(name.to_os_string());
                let Some(parent) = nearest.parent() else {
                    return false;
                };
                nearest = parent.to_path_buf();
            }
            Err(_) => return false,
        }
    };
    if atomic::is_link_or_reparse(&nearest_metadata)
        || (!missing_tail.is_empty() && !nearest_metadata.is_dir())
        || (missing_tail.is_empty() && !nearest_metadata.is_file())
    {
        return false;
    }
    let Ok(mut resolved) = std::fs::canonicalize(&nearest) else {
        return false;
    };
    for component in missing_tail.iter().rev() {
        resolved.push(component);
    }
    if !resolved.starts_with(&project) {
        return false;
    }
    let Ok(relative) = resolved.strip_prefix(&project) else {
        return false;
    };
    let components: Vec<_> = relative.components().collect();
    if components.len() >= 3
        && components[0].as_os_str() == ".kiro"
        && components[1].as_os_str() == "steering"
        && relative
            .extension()
            .is_some_and(|extension| extension == "md")
    {
        return components[2..components.len() - 1]
            .iter()
            .all(|component| match component {
                Component::Normal(name) => !skipped_directory(&name.to_string_lossy()),
                _ => false,
            });
    }
    if relative.file_name().is_some_and(|name| name == "AGENTS.md") {
        return components[..components.len() - 1]
            .iter()
            .all(|component| match component {
                Component::Normal(name) => !skipped_directory(&name.to_string_lossy()),
                _ => false,
            });
    }
    false
}

fn add_candidate(
    path: PathBuf,
    boundary: &Path,
    group_id: &str,
    source: MemorySource,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    ensure_plain_file(&path)?;
    let canonical = std::fs::canonicalize(&path).map_err(|error| error.to_string())?;
    let boundary = std::fs::canonicalize(boundary).unwrap_or_else(|_| boundary.to_path_buf());
    if !canonical.starts_with(boundary) {
        return Err(format!(
            "instruction file escaped its authorized root: {}",
            path.display()
        ));
    }
    out.insert(
        canonical.clone(),
        Candidate {
            group_id: group_id.into(),
            source,
        },
    );
    Ok(())
}

fn read_plain_file(path: &Path) -> Result<String, String> {
    ensure_plain_file(path)?;
    std::fs::read_to_string(path)
        .map_err(|error| format!("failed reading {}: {error}", path.display()))
}

fn ensure_plain_file(path: &Path) -> Result<(), String> {
    crate::harness::claude::skills::ensure_plain_components(path)?;
    crate::harness::claude::skills::ensure_plain_file_if_present(path)?;
    if !path.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    Ok(())
}

fn memory_id(path: &Path) -> String {
    format!(
        "kiro-instructions-{}",
        atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
    )
}

fn steering_behavior(content: &str) -> String {
    let Some(rest) = content.strip_prefix("---\n") else {
        return "always".into();
    };
    let Some((frontmatter, _)) = rest.split_once("\n---") else {
        return "always".into();
    };
    frontmatter
        .lines()
        .find_map(|line| line.strip_prefix("inclusion:").map(str::trim))
        .filter(|value| matches!(*value, "always" | "auto" | "manual" | "fileMatch"))
        .unwrap_or("always")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steering_and_agents_files_have_link_graph_and_checked_delete() {
        let root = tempfile::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));
        let project = root.path().join("project");
        let steering = project.join(".kiro/steering");
        std::fs::create_dir_all(&steering).unwrap();
        std::fs::create_dir_all(project.join("src")).unwrap();
        for ignored in [
            ".hidden",
            "node_modules",
            "vendor",
            "target",
            "dist",
            "build",
        ] {
            let ignored_dir = project.join(ignored);
            std::fs::create_dir_all(&ignored_dir).unwrap();
            std::fs::write(ignored_dir.join("AGENTS.md"), "must not be scanned\n").unwrap();
        }
        std::fs::create_dir_all(harness.home().join("steering")).unwrap();
        std::fs::write(
            steering.join("guide.md"),
            "---\ninclusion: fileMatch\n---\nGuide.\n",
        )
        .unwrap();
        std::fs::write(
            project.join("AGENTS.md"),
            "Read [the guide](.kiro/steering/guide.md).\n",
        )
        .unwrap();
        std::fs::write(project.join("src/AGENTS.md"), "Nested rules.\n").unwrap();
        std::fs::write(harness.home().join("steering/global.md"), "Global rules.\n").unwrap();

        let initial = catalog(&harness, &project).unwrap();
        for file in &initial.files {
            assert!(
                initial.groups.iter().any(|group| group.id == file.group_id),
                "{} refers to missing group {}",
                file.path,
                file.group_id
            );
            assert!(![
                ".hidden",
                "node_modules",
                "vendor",
                "target",
                "dist",
                "build"
            ]
            .iter()
            .any(|ignored| file.path.contains(ignored)));
        }
        let guide = initial
            .files
            .iter()
            .find(|file| file.name == "guide.md")
            .unwrap();
        assert_eq!(guide.load_behavior, "fileMatch");
        let index = initial
            .files
            .iter()
            .find(|file| {
                file.name == "AGENTS.md"
                    && Path::new(&file.path)
                        == std::fs::canonicalize(project.join("AGENTS.md")).unwrap()
            })
            .unwrap();
        assert_eq!(index.references[0].status, ReferenceStatus::Known);
        let guide_path = PathBuf::from(&guide.path);
        assert!(allowed_instruction_path(&harness, &project, &guide_path));
        assert!(allowed_instruction_path(
            &harness,
            &project,
            &project.join("src/AGENTS.md")
        ));
        assert!(allowed_instruction_path(
            &harness,
            &project,
            &project.join(".kiro/steering/deleted-history.md")
        ));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            &project.join("README.md")
        ));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            &project.join("target/AGENTS.md")
        ));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            &project.join("../outside/AGENTS.md")
        ));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            &harness.home().join("steering/global.md")
        ));
        delete(&harness, &project, &guide.id, &guide.content).unwrap();
        assert!(!guide_path.exists());
        assert_eq!(std::fs::read_to_string(&index.path).unwrap(), "Read .\n");
    }

    #[test]
    fn learned_memory_is_not_misrepresented_as_editable_steering() {
        let root = tempfile::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let catalog = catalog(&harness, &project).unwrap();
        assert!(catalog.files.is_empty());
        assert!(catalog.excluded_custom_memory_directory.is_none());
    }

    #[test]
    fn instruction_discovery_stops_at_the_depth_limit() {
        let root = tempfile::tempdir().unwrap();
        let harness = KiroHarness::new(root.path().join("home/.kiro"));
        let project = root.path().join("project");
        let mut directory = project.clone();
        for _ in 0..=MAX_DISCOVERY_DEPTH {
            directory.push("nested");
            std::fs::create_dir_all(&directory).unwrap();
        }
        std::fs::write(directory.join("AGENTS.md"), "deep rules\n").unwrap();

        let error = catalog(&harness, &project).unwrap_err();
        assert!(error.contains("maximum depth"));
    }
}
