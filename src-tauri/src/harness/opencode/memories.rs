use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use super::{config, OpenCodeHarness};
use crate::harness::{
    claude::memories::{
        MemoryCatalog, MemoryFile, MemoryGroup, MemoryReference, MemoryRole, MemorySource,
        ReferenceStatus,
    },
    memory_links::{parse_references, resolve_reference},
    Harness,
};

pub fn catalog(harness: &OpenCodeHarness, project: &Path) -> Result<MemoryCatalog, String> {
    let project = project
        .canonicalize()
        .map_err(|error| format!("could not resolve project: {error}"))?;
    let mut paths = BTreeMap::<PathBuf, (String, MemorySource, bool)>::new();
    let groups = vec![
        MemoryGroup {
            id: "opencode-instructions-global".into(),
            label: "OpenCode global instructions".into(),
            role: MemorySource::UserInstructions,
        },
        MemoryGroup {
            id: "opencode-instructions-project".into(),
            label: "OpenCode project instructions".into(),
            role: MemorySource::ProjectInstructions,
        },
    ];

    let global_root = harness
        .config_root()
        .canonicalize()
        .unwrap_or_else(|_| harness.config_root().to_path_buf());
    add_if_file(
        &mut paths,
        global_root.join("AGENTS.md"),
        "opencode-instructions-global",
        MemorySource::UserInstructions,
        true,
    )?;
    let ancestors = project_ancestors(&project, harness.home());
    for directory in &ancestors {
        let editable = same_or_child(&project, directory);
        add_if_file(
            &mut paths,
            directory.join("AGENTS.md"),
            "opencode-instructions-project",
            MemorySource::ProjectInstructions,
            editable,
        )?;
    }
    collect_nested_agents(&project, 0, &mut paths)?;

    // Catalog local literal instruction paths. OpenCode also accepts URLs and
    // globs; neither is represented as an editable file here.
    for (path, source, group) in configured_instruction_paths(harness, &global_root, &project)? {
        let editable = (same_or_child(&project, &path) || same_or_child(&global_root, &path))
            && (source == MemorySource::UserInstructions || same_or_child(&project, &path));
        add_if_file(&mut paths, path, group, source, editable)?;
    }

    let mut files = Vec::new();
    let mut by_path = HashMap::<PathBuf, String>::new();
    for (path, (group_id, source, editable)) in paths {
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_file() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
        let id = path.to_string_lossy().into_owned();
        by_path.insert(path.clone(), id.clone());
        files.push(MemoryFile {
            id,
            group_id,
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: path.to_string_lossy().into_owned(),
            source,
            role: MemoryRole::Instruction,
            load_behavior: if source == MemorySource::UserInstructions {
                "Loaded through OpenCode instructions configuration."
            } else {
                "Loaded as OpenCode project instructions."
            }
            .into(),
            editable,
            content,
            references: Vec::new(),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for file in &mut files {
        let references = parse_references(&file.content, false);
        file.references = references
            .into_iter()
            .map(|(target, kind)| {
                let resolved = resolve_reference(Path::new(&file.path), &target);
                let file_id = resolved
                    .as_ref()
                    .and_then(|path| by_path.get(path))
                    .cloned();
                let status = if target.starts_with("http:")
                    || target.starts_with("https:")
                    || target.starts_with("mailto:")
                {
                    ReferenceStatus::External
                } else if file_id.is_some() {
                    ReferenceStatus::Known
                } else {
                    ReferenceStatus::Unresolved
                };
                MemoryReference {
                    target,
                    kind,
                    status,
                    file_id,
                }
            })
            .collect();
    }
    Ok(MemoryCatalog {
        project_root: project.to_string_lossy().into_owned(),
        groups,
        files,
        excluded_custom_memory_directory: None,
    })
}

pub fn edit(
    harness: &OpenCodeHarness,
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
    crate::harness::instruction_store::edit(
        harness,
        &file,
        expected,
        content,
        "opencode-instruction-edit",
    )
}

pub fn delete(
    harness: &OpenCodeHarness,
    project: &Path,
    id: &str,
    expected: &str,
) -> Result<(), String> {
    crate::harness::instruction_store::delete(
        harness,
        &catalog(harness, project)?,
        id,
        expected,
        "opencode-instruction-edit",
        "opencode-instruction-delete",
    )
}

fn add_if_file(
    paths: &mut BTreeMap<PathBuf, (String, MemorySource, bool)>,
    path: PathBuf,
    group: &str,
    source: MemorySource,
    editable: bool,
) -> Result<(), String> {
    if crate::harness::claude::skills::ensure_plain_components(&path).is_err() {
        return Ok(());
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata)
            if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_file() =>
        {
            Ok(())
        }
        Ok(_) => {
            let path = path.canonicalize().map_err(|error| error.to_string())?;
            paths.insert(path, (group.into(), source, editable));
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not inspect {}: {error}", path.display())),
    }
}

fn collect_nested_agents(
    directory: &Path,
    depth: usize,
    paths: &mut BTreeMap<PathBuf, (String, MemorySource, bool)>,
) -> Result<(), String> {
    if depth >= 4 {
        return Ok(());
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "could not inspect {}: {error}",
                directory.display()
            ))
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|error| error.to_string())?;
        if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() {
            continue;
        }
        if child.join("AGENTS.md").is_file() {
            add_if_file(
                paths,
                child.join("AGENTS.md"),
                "opencode-instructions-project",
                MemorySource::ProjectInstructions,
                true,
            )?;
        }
        collect_nested_agents(&child, depth + 1, paths)?;
    }
    Ok(())
}

fn same_or_child(root: &Path, path: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path == root || path.starts_with(root)
}

/// True only for native AGENTS.md locations or literal local paths configured
/// through OpenCode's `instructions` setting. This also covers deleted leaves.
pub(crate) fn allowed_instruction_path(
    harness: &OpenCodeHarness,
    project: &Path,
    target: &Path,
) -> bool {
    if crate::harness::claude::skills::ensure_plain_components(target).is_err() {
        return false;
    }
    let Ok(project) = project.canonicalize() else {
        return false;
    };
    let config_root = harness
        .config_root()
        .canonicalize()
        .unwrap_or_else(|_| harness.config_root().to_path_buf());
    let Some(target) = resolved_or_deleted_path(target) else {
        return false;
    };
    let Ok(allowed) = native_instruction_allowlist(harness, &config_root, &project) else {
        return false;
    };
    let owned = same_or_child(&config_root, &target) || same_or_child(&project, &target);
    owned
        && allowed
            .into_iter()
            .any(|path| resolved_or_deleted_path(&path).as_deref() == Some(target.as_path()))
}

fn project_ancestors(project: &Path, home: &Path) -> Vec<PathBuf> {
    let mut ancestors = Vec::new();
    let mut cursor = Some(project);
    let mut found_root = false;
    while let Some(directory) = cursor {
        ancestors.push(directory.to_path_buf());
        if !project.starts_with(home) && directory.join(".git").exists() {
            found_root = true;
            break;
        }
        if directory == home {
            break;
        }
        cursor = directory.parent();
    }
    if !found_root && !project.starts_with(home) {
        ancestors.truncate(1);
    }
    ancestors.reverse();
    ancestors
}

fn configured_instruction_paths(
    harness: &OpenCodeHarness,
    global_root: &Path,
    project: &Path,
) -> Result<Vec<(PathBuf, MemorySource, &'static str)>, String> {
    let root = project
        .ancestors()
        .find(|directory| directory.join(".git").exists())
        .unwrap_or(project);
    let ancestors = project_ancestors(project, root);
    let mut result = Vec::new();
    let global_config = config::config_path(global_root);
    let mut layers = vec![(
        global_config,
        MemorySource::UserInstructions,
        "opencode-instructions-global",
    )];
    layers.extend(
        config::project_config_paths(project)
            .into_iter()
            .map(|path| {
                (
                    path,
                    MemorySource::ProjectInstructions,
                    "opencode-instructions-project",
                )
            }),
    );
    for (config_path, source, group) in layers {
        if !config_path.is_file() {
            continue;
        }
        if crate::harness::claude::skills::ensure_plain_components(&config_path).is_err() {
            continue;
        }
        let value = config::read_config_value(&config_path)?;
        let Some(value) = value.get("instructions") else {
            continue;
        };
        let items: Vec<&str> = value
            .as_str()
            .into_iter()
            .chain(
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.as_str()),
            )
            .collect();
        for item in items {
            result.extend(
                resolve_literal_instruction(item, source, global_root, &ancestors, harness.home())
                    .into_iter()
                    .map(|path| (path, source, group)),
            );
        }
    }
    Ok(result)
}

fn resolve_literal_instruction(
    item: &str,
    source: MemorySource,
    global_root: &Path,
    ancestors: &[PathBuf],
    home: &Path,
) -> Vec<PathBuf> {
    if item.starts_with("http://")
        || item.starts_with("https://")
        || item
            .chars()
            .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}'))
    {
        return Vec::new();
    }
    let expanded = item.strip_prefix("~/").map(|tail| home.join(tail));
    let path = expanded.as_deref().unwrap_or_else(|| Path::new(item));
    if path.is_absolute() {
        return vec![path.to_path_buf()];
    }
    if source == MemorySource::UserInstructions
        && std::env::var_os("OPENCODE_DISABLE_PROJECT_CONFIG").as_deref()
            == Some(std::ffi::OsStr::new("1"))
    {
        return vec![global_root.join(path)];
    }
    ancestors
        .iter()
        .rev()
        .map(|directory| directory.join(path))
        .collect()
}

fn native_instruction_allowlist(
    harness: &OpenCodeHarness,
    global_root: &Path,
    project: &Path,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = vec![global_root.join("AGENTS.md")];
    let ancestors = project_ancestors(project, harness.home());
    paths.extend(
        ancestors
            .iter()
            .map(|directory| directory.join("AGENTS.md")),
    );
    collect_agent_paths(project, 0, &mut paths)?;
    paths.extend(
        configured_instruction_paths(harness, global_root, project)?
            .into_iter()
            .map(|(path, _, _)| path),
    );
    Ok(paths)
}

fn collect_agent_paths(root: &Path, depth: usize, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    if depth >= 4 {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|error| error.to_string())?;
        if crate::harness::atomic::is_link_or_reparse(&metadata) || !metadata.is_dir() {
            continue;
        }
        paths.push(child.join("AGENTS.md"));
        collect_agent_paths(&child, depth + 1, paths)?;
    }
    Ok(())
}

fn resolved_or_deleted_path(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return path.canonicalize().ok();
    }
    let parent = path.parent()?.canonicalize().ok()?;
    Some(parent.join(path.file_name()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tempdir;

    fn file_at<'a>(catalog: &'a MemoryCatalog, path: &Path) -> &'a MemoryFile {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        catalog
            .files
            .iter()
            .find(|file| Path::new(&file.path) == canonical)
            .unwrap()
    }

    #[test]
    fn restore_allowlist_accepts_only_native_agents_and_configured_literal_paths() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".config/opencode");
        let project = temp.path().join("project");
        fs::create_dir_all(config.join("notes")).unwrap();
        fs::create_dir_all(project.join("notes")).unwrap();
        fs::write(
            project.join("opencode.json"),
            r#"{"instructions":["notes/guide.md", "https://example.com/instructions.md", "notes/*.md"]}"#,
        )
        .unwrap();
        fs::write(
            config.join("opencode.json"),
            r#"{"instructions":"notes/global.md"}"#,
        )
        .unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(project.join("notes")).unwrap();
        fs::write(project.join("notes/AGENTS.md"), "nested instructions").unwrap();
        let harness = OpenCodeHarness::new(config.clone(), home);

        assert!(allowed_instruction_path(
            &harness,
            &project,
            &project.join("notes/guide.md")
        ));
        assert!(allowed_instruction_path(
            &harness,
            &project,
            &project.join("notes/global.md")
        ));
        assert!(allowed_instruction_path(
            &harness,
            &project,
            &project.join("notes/AGENTS.md")
        ));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            &project.join("notes/other.md")
        ));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            &config.join("notes/other.md")
        ));
        assert!(!allowed_instruction_path(
            &harness,
            &project,
            &project.join("notes/../other.md")
        ));
    }

    #[test]
    fn ordinary_at_mentions_are_not_reported_as_native_imports() {
        let parsed = parse_references("Use @notes/guide.md when useful.", false);
        assert!(parsed.is_empty());
    }

    #[test]
    fn catalog_reads_workspace_relative_configured_files_and_keeps_external_paths_read_only() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let config = home.join(".config/opencode");
        let repository = temp.path().join("repository");
        let project = repository.join("packages/app");
        let inherited = repository.join("notes/inherited.md");
        let local = project.join("notes/local.md");
        let guide = project.join("notes/guide.md");
        let external = temp.path().join("outside/external.md");
        fs::create_dir_all(repository.join(".git")).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(inherited.parent().unwrap()).unwrap();
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::create_dir_all(external.parent().unwrap()).unwrap();
        fs::write(&inherited, "inherited content").unwrap();
        fs::write(&local, "[guide](guide.md) @guide.md").unwrap();
        fs::write(&guide, "guide content").unwrap();
        fs::write(&external, "outside content").unwrap();
        fs::write(
            repository.join("opencode.json"),
            r#"{"instructions":["notes/inherited.md"]}"#,
        )
        .unwrap();
        let external_json = serde_json::to_string(&external.to_string_lossy()).unwrap();
        fs::write(
            config.join("opencode.json"),
            format!(r#"{{"instructions":{external_json}}}"#),
        )
        .unwrap();
        fs::write(
            project.join("opencode.json"),
            r#"{"instructions":["notes/local.md", "notes/guide.md"]}"#,
        )
        .unwrap();
        let harness = OpenCodeHarness::new(config, home);

        let catalog = catalog(&harness, &project).unwrap();
        assert_eq!(file_at(&catalog, &inherited).content, "inherited content");
        assert_eq!(
            file_at(&catalog, &local).content,
            "[guide](guide.md) @guide.md"
        );
        assert!(!file_at(&catalog, &external).editable);
        assert!(file_at(&catalog, &local).editable);
        let guide_id = file_at(&catalog, &guide).id.clone();
        delete(&harness, &project, &guide_id, "guide content").unwrap();
        assert!(!guide.exists());
        assert_eq!(fs::read_to_string(&local).unwrap(), " @guide.md");
    }
}
