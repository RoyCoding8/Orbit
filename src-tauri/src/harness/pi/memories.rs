use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::PiHarness;
use crate::harness::{
    atomic,
    claude::memories::{
        MemoryCatalog, MemoryFile, MemoryGroup, MemoryReference, MemoryRole, MemorySource,
        ReferenceStatus,
    },
    claude::skills,
    memory_links::{parse_references, resolve_reference},
    Harness,
};

#[derive(Clone)]
struct Candidate {
    group: String,
    source: MemorySource,
    editable: bool,
}

pub fn catalog(harness: &PiHarness, project: &Path) -> Result<MemoryCatalog, String> {
    let project = std::fs::canonicalize(project).map_err(|_| "project directory is unavailable")?;
    if !project.is_dir() {
        return Err("project directory is unavailable".into());
    }
    let global = std::fs::canonicalize(harness.config_root())
        .unwrap_or_else(|_| harness.config_root().to_path_buf());
    let groups = vec![
        MemoryGroup {
            id: "pi-global-instructions".into(),
            label: "Global Pi instructions".into(),
            role: MemorySource::UserInstructions,
        },
        MemoryGroup {
            id: "pi-project-instructions".into(),
            label: "Project instructions".into(),
            role: MemorySource::ProjectInstructions,
        },
    ];
    let mut candidates = BTreeMap::<PathBuf, Candidate>::new();
    collect_directory_instructions(
        &global,
        "pi-global-instructions",
        MemorySource::UserInstructions,
        true,
        &mut candidates,
    )?;
    add_if_present(
        &global.join("SYSTEM.md"),
        "pi-global-instructions",
        MemorySource::UserInstructions,
        true,
        &mut candidates,
    )?;
    add_if_present(
        &global.join("APPEND_SYSTEM.md"),
        "pi-global-instructions",
        MemorySource::UserInstructions,
        true,
        &mut candidates,
    )?;
    for (directory, selected) in project_ancestors(&project) {
        collect_directory_instructions(
            &directory,
            "pi-project-instructions",
            MemorySource::ProjectInstructions,
            selected,
            &mut candidates,
        )?;
    }
    let project_pi = project.join(".pi");
    add_if_present(
        &project_pi.join("SYSTEM.md"),
        "pi-project-instructions",
        MemorySource::ProjectInstructions,
        true,
        &mut candidates,
    )?;
    add_if_present(
        &project_pi.join("APPEND_SYSTEM.md"),
        "pi-project-instructions",
        MemorySource::ProjectInstructions,
        true,
        &mut candidates,
    )?;

    let mut files = BTreeMap::new();
    for (path, candidate) in candidates {
        if skills::ensure_plain_file_if_present(&path).is_err() {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
        let id = file_id(&path);
        files.insert(
            path.clone(),
            MemoryFile {
                id,
                group_id: candidate.group,
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: path.to_string_lossy().into_owned(),
                source: candidate.source,
                role: MemoryRole::Instruction,
                load_behavior: behavior(&path),
                editable: candidate.editable,
                content,
                references: Vec::new(),
            },
        );
    }
    let known: BTreeMap<_, _> = files
        .iter()
        .map(|(path, file)| (path.clone(), file.id.clone()))
        .collect();
    let roots = [global.as_path(), project.as_path()];
    for file in files.values_mut() {
        file.references = parse_references(&file.content, false)
            .into_iter()
            .map(|(target, kind)| {
                let resolved = resolve_reference(Path::new(&file.path), &target);
                let inside = resolved
                    .as_deref()
                    .is_some_and(|path| roots.iter().any(|root| path.starts_with(root)));
                let file_id = resolved.as_ref().and_then(|path| known.get(path).cloned());
                MemoryReference {
                    target,
                    kind,
                    status: if !inside {
                        ReferenceStatus::External
                    } else if file_id.is_some() {
                        ReferenceStatus::Known
                    } else {
                        ReferenceStatus::Unresolved
                    },
                    file_id,
                }
            })
            .collect();
    }
    Ok(MemoryCatalog {
        project_root: project.to_string_lossy().into_owned(),
        groups,
        files: files.into_values().collect(),
        excluded_custom_memory_directory: None,
    })
}

pub fn edit(
    harness: &PiHarness,
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

pub fn delete(harness: &PiHarness, project: &Path, id: &str, expected: &str) -> Result<(), String> {
    crate::harness::instruction_store::delete(
        harness,
        &catalog(harness, project)?,
        id,
        expected,
        "edit",
        "delete",
    )
}

pub(crate) fn allowed_instruction_path(harness: &PiHarness, project: &Path, target: &Path) -> bool {
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
    if skills::ensure_plain_components(&target).is_err() {
        return false;
    }
    if target
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }
    let target = canonical_or_deleted(&target);
    let global = std::fs::canonicalize(harness.config_root())
        .unwrap_or_else(|_| harness.config_root().to_path_buf());
    instruction_paths(&global, &project)
        .into_iter()
        .any(|path| canonical_or_deleted(&path) == target)
}

fn instruction_paths(global: &Path, project: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        global.join("SYSTEM.md"),
        global.join("APPEND_SYSTEM.md"),
        project.join(".pi/SYSTEM.md"),
        project.join(".pi/APPEND_SYSTEM.md"),
    ];
    for directory in
        std::iter::once(global.to_path_buf()).chain(std::iter::once(project.to_path_buf()))
    {
        paths.extend(
            [
                "AGENTS.override.md",
                "AGENTS.md",
                "AGENTS.MD",
                "CLAUDE.md",
                "CLAUDE.MD",
            ]
            .into_iter()
            .map(|name| directory.join(name)),
        );
    }
    paths
}

fn collect_directory_instructions(
    dir: &Path,
    group: &str,
    source: MemorySource,
    editable: bool,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    for path in directory_instruction_paths(dir) {
        add_if_present(&path, group, source, editable, out)?;
    }
    Ok(())
}

fn directory_instruction_paths(dir: &Path) -> Vec<PathBuf> {
    let names = [
        "AGENTS.override.md",
        "AGENTS.md",
        "AGENTS.MD",
        "CLAUDE.md",
        "CLAUDE.MD",
    ];
    names
        .into_iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .take(1)
        .collect()
}

fn add_if_present(
    path: &Path,
    group: &str,
    source: MemorySource,
    editable: bool,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    if path.exists() {
        skills::ensure_plain_components(path)?;
        skills::ensure_plain_file_if_present(path)?;
        let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
        out.insert(
            path,
            Candidate {
                group: group.into(),
                source,
                editable,
            },
        );
    }
    Ok(())
}

fn project_ancestors(project: &Path) -> Vec<(PathBuf, bool)> {
    let mut ancestors: Vec<_> = project.ancestors().map(Path::to_path_buf).collect();
    ancestors.reverse();
    ancestors
        .into_iter()
        .map(|path| {
            let selected = path == project;
            (path, selected)
        })
        .collect()
}

fn canonical_or_deleted(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn behavior(path: &Path) -> String {
    match path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
    {
        "SYSTEM.md" => "replaces the system prompt".into(),
        "APPEND_SYSTEM.md" => "appends to the system prompt".into(),
        _ => "loaded as agent instructions".into(),
    }
}

fn file_id(path: &Path) -> String {
    format!(
        "instruction-{}",
        atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_delete_checks_stale_content_and_removes_known_inbound_links() {
        let root = tempfile::tempdir().unwrap();
        let harness = PiHarness::new(root.path().join(".pi/agent"), root.path().join("home"));
        let project = root.path().join("project");
        let pi = project.join(".pi");
        std::fs::create_dir_all(pi.join(".git")).unwrap();
        std::fs::write(
            project.join("AGENTS.md"),
            "Read [system](.pi/APPEND_SYSTEM.md)\nKeep this line.\n",
        )
        .unwrap();
        std::fs::write(pi.join("APPEND_SYSTEM.md"), "Additional system rules.\n").unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "Ancestor rules\n").unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();

        let catalog = catalog(&harness, &project).unwrap();
        assert!(catalog.files.iter().any(|file| Path::new(&file.path)
            == root.path().join("AGENTS.md").canonicalize().unwrap()
            && !file.editable));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            &root.path().join("AGENTS.md")
        ));
        let system = catalog
            .files
            .iter()
            .find(|file| file.name == "APPEND_SYSTEM.md")
            .unwrap();
        assert!(system.editable);
        assert!(allowed_instruction_path(
            &harness,
            &project,
            Path::new(".pi/APPEND_SYSTEM.md")
        ));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            Path::new(".pi/unlisted.md")
        ));
        let stale = delete(&harness, &project, &system.id, "stale content").unwrap_err();
        assert!(stale.contains("changed since this editor opened"));
        assert!(Path::new(&system.path).exists());

        delete(&harness, &project, &system.id, &system.content).unwrap();
        assert!(!Path::new(&system.path).exists());
        assert_eq!(
            std::fs::read_to_string(project.join("AGENTS.md")).unwrap(),
            "Read \nKeep this line.\n"
        );
    }
}
