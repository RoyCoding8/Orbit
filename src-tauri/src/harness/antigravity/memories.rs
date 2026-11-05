use super::{discovery, AntigravityHarness};
use crate::harness::{
    atomic,
    claude::{
        memories::{
            MemoryCatalog, MemoryFile, MemoryGroup, MemoryReference, MemoryRole, MemorySource,
            ReferenceStatus,
        },
        skills,
    },
    instruction_store,
    memory_links::{parse_references, resolve_reference},
    Harness,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub fn allowed_instruction_path(harness: &AntigravityHarness, project: &Path, path: &Path) -> bool {
    let roots = [
        harness.home().join(".gemini"),
        harness.config_root().to_owned(),
        project.to_owned(),
    ];
    if roots.iter().any(|root| {
        ["AGENTS.md", "GEMINI.md"]
            .iter()
            .any(|name| path == root.join(name))
    }) {
        return true;
    }
    let mut roots = vec![harness.config_root().to_owned()];
    roots.extend(discovery::PROJECT_DIRS.map(|name| project.join(name)));
    roots.iter().any(|root| {
        ["AGENTS.md", "GEMINI.md"]
            .iter()
            .any(|name| path == root.join(name))
            || path.parent() == Some(root.join("rules").as_path())
                && path.extension().is_some_and(|e| e == "md")
    })
}

pub fn catalog(harness: &AntigravityHarness, project: &Path) -> Result<MemoryCatalog, String> {
    let project = project.canonicalize().map_err(|e| e.to_string())?;
    let mut paths = discovery::files(harness, &project, "rules")?;
    let mut roots = discovery::roots(harness, &project);
    roots.push(harness.home().join(".gemini"));
    roots.extend(discovery::ancestors(&project));
    for root in roots {
        for name in ["AGENTS.md", "GEMINI.md"] {
            let path = root.join(name);
            if path.is_file() {
                skills::ensure_plain_components(&path)?;
                paths.push(path.canonicalize().map_err(|e| e.to_string())?);
            }
        }
    }
    paths.sort();
    paths.dedup();
    let mut files = BTreeMap::new();
    for path in paths {
        let content = skills::read_plain_file(&path)?;
        let user = path.starts_with(harness.home().join(".gemini"));
        let source = if user {
            MemorySource::UserInstructions
        } else {
            MemorySource::ProjectInstructions
        };
        let behavior = if matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("AGENTS.md" | "GEMINI.md")
        ) {
            "Always active in scope".into()
        } else {
            behavior(&content)
        };
        files.insert(
            path.clone(),
            MemoryFile {
                id: format!(
                    "antigravity-{}",
                    atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
                ),
                group_id: if user {
                    "user".into()
                } else {
                    "project".into()
                },
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: path.to_string_lossy().into_owned(),
                source,
                role: MemoryRole::Instruction,
                load_behavior: behavior,
                editable: allowed_instruction_path(harness, &project, &path),
                content,
                references: vec![],
            },
        );
    }
    let known: BTreeMap<_, _> = files
        .iter()
        .map(|(path, file)| (path.clone(), file.id.clone()))
        .collect();
    for file in files.values_mut() {
        file.references = parse_references(&file.content, false)
            .into_iter()
            .map(|(target, kind)| {
                let path = resolve_reference(Path::new(&file.path), &target);
                let file_id = path.as_ref().and_then(|p| known.get(p).cloned());
                let status = if file_id.is_some() {
                    ReferenceStatus::Known
                } else if path.is_some_and(|p| {
                    p.starts_with(&project) || p.starts_with(harness.home().join(".gemini"))
                }) {
                    ReferenceStatus::Unresolved
                } else {
                    ReferenceStatus::External
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
        groups: vec![
            MemoryGroup {
                id: "user".into(),
                label: "User instructions and rules".into(),
                role: MemorySource::UserInstructions,
            },
            MemoryGroup {
                id: "project".into(),
                label: "Project instructions and rules".into(),
                role: MemorySource::ProjectInstructions,
            },
        ],
        files: files.into_values().collect(),
        excluded_custom_memory_directory: None,
    })
}

fn behavior(content: &str) -> String {
    let Some(frontmatter) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .and_then(|s| s.split("\n---").next())
    else {
        return "Missing rule frontmatter; Antigravity may ignore this file".into();
    };
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter).unwrap_or_default();
    match value.get("trigger").and_then(serde_yaml::Value::as_str) {
        Some("always_on") => "Always active in scope".into(),
        Some("manual") => "Loaded when explicitly requested in Antigravity".into(),
        Some("glob") => format!(
            "File pattern: {}",
            value
                .get("globs")
                .or_else(|| value.get("glob"))
                .and_then(serde_yaml::Value::as_str)
                .unwrap_or("missing pattern")
        ),
        Some("model_decision") => "Antigravity selects this rule using its description".into(),
        _ => "Missing or unsupported rule trigger".into(),
    }
}

pub fn edit(
    harness: &AntigravityHarness,
    project: &Path,
    id: &str,
    expected: &str,
    content: &str,
) -> Result<(), String> {
    let file = catalog(harness, project)?
        .files
        .into_iter()
        .find(|f| f.id == id)
        .ok_or("Instruction is no longer available")?;
    instruction_store::edit(harness, &file, expected, content, "edit")
}

pub fn delete(
    harness: &AntigravityHarness,
    project: &Path,
    id: &str,
    expected: &str,
) -> Result<(), String> {
    instruction_store::delete(
        harness,
        &catalog(harness, project)?,
        id,
        expected,
        "edit",
        "delete",
    )
}
