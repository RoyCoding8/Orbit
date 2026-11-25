use super::{discovery, AntigravityHarness};
use crate::harness::{
    atomic,
    claude::{
        memories::{MemoryCatalog, MemoryFile, MemoryGroup, MemoryRole, MemorySource},
        skills,
    },
    instruction_store, Harness,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub fn allowed_instruction_path(harness: &AntigravityHarness, project: &Path, path: &Path) -> bool {
    let Some(path) = crate::harness::fs_guard::canonical_or_missing_tail(path) else {
        return false;
    };
    let resolve = |p: &Path| {
        crate::harness::fs_guard::canonical_or_missing_tail(p).unwrap_or_else(|| p.to_path_buf())
    };
    let home_gemini = resolve(&harness.home().join(".gemini"));
    let config_root = resolve(harness.config_root());
    let project_root = resolve(project);
    let top_roots = [&home_gemini, &config_root, &project_root];
    if top_roots.iter().any(|root| {
        ["AGENTS.md", "GEMINI.md"]
            .iter()
            .any(|name| path == root.join(name))
    }) {
        return true;
    }
    let mut rule_roots = vec![config_root];
    rule_roots.extend(discovery::PROJECT_DIRS.map(|name| project_root.join(name)));
    rule_roots.iter().any(|root| {
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
    let home_gemini =
        crate::harness::fs_guard::canonical_or_missing_tail(&harness.home().join(".gemini"))
            .unwrap_or_else(|| harness.home().join(".gemini"));
    let mut files = BTreeMap::new();
    for path in paths {
        let content = skills::read_plain_file(&path)?;
        let user = path.starts_with(&home_gemini);
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
    crate::harness::memory_links::populate_references(
        files.values_mut(),
        &known,
        false,
        Some(harness.home()),
        |_, path| path.is_some_and(|p| p.starts_with(&project) || p.starts_with(&home_gemini)),
    );
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
    let Some(frontmatter) = crate::harness::skill_store::extract_frontmatter_body(content) else {
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
