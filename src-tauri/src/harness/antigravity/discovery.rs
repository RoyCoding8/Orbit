use super::AntigravityHarness;
use crate::harness::{claude::skills, json_store, Harness};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

pub const PROJECT_DIRS: [&str; 4] = [".agents", ".agent", "_agents", "_agent"];

pub fn ancestors(project: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for folder in project.ancestors() {
        result.push(folder.to_owned());
        if folder.join(".git").exists() {
            break;
        }
    }
    if !result.iter().any(|p| p.join(".git").exists()) {
        result.truncate(1);
    }
    result
}

pub fn roots(harness: &AntigravityHarness, project: &Path) -> Vec<PathBuf> {
    let mut result = vec![harness.config_root().to_owned()];
    for ancestor in ancestors(project) {
        result.extend(PROJECT_DIRS.map(|name| ancestor.join(name)));
    }
    result
}

pub fn files(
    harness: &AntigravityHarness,
    project: &Path,
    kind: &str,
) -> Result<Vec<PathBuf>, String> {
    let mut out = BTreeMap::new();
    let mut visited = HashSet::new();
    let mut excludes = Vec::new();
    let mut remaining = 256;
    for root in roots(harness, project) {
        scan(&root.join(kind), kind, &[], &mut out)?;
        let path = root.join(format!("{kind}.json"));
        if path.exists() {
            let doc = json_store::read_value(&path)?;
            excludes.extend(names(&doc, "exclude")?);
        }
        manifest(&path, kind, &mut visited, &mut out, 0, &mut remaining)?;
    }
    out.retain(|path, _| !matches_name(path, kind, &excludes));
    if kind == "skills" {
        for dir in [
            harness.home().join(".gemini/antigravity-cli/skills"),
            harness.home().join(".gemini/skills"),
        ] {
            scan(&dir, kind, &[], &mut out)?;
        }
    }
    Ok(out.into_keys().collect())
}

fn scan(
    root: &Path,
    kind: &str,
    exclude: &[String],
    out: &mut BTreeMap<PathBuf, ()>,
) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    skills::ensure_plain_components(root)?;
    if root.is_file() {
        if (kind == "rules" && root.extension().is_some_and(|x| x == "md"))
            || (kind == "skills" && root.file_name().is_some_and(|n| n == "SKILL.md"))
        {
            skills::ensure_plain_file_if_present(root)?;
            out.insert(root.canonicalize().map_err(|e| e.to_string())?, ());
        }
        return Ok(());
    }
    if kind == "skills" && root.join("SKILL.md").is_file() {
        return scan(&root.join("SKILL.md"), kind, exclude, out);
    }
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if exclude.contains(&name) {
            continue;
        }
        let path = if kind == "skills" {
            entry.path().join("SKILL.md")
        } else {
            entry.path()
        };
        if path.is_file() {
            scan(&path, kind, &[], out)?;
        }
    }
    Ok(())
}

fn names(doc: &serde_json::Value, key: &str) -> Result<Vec<String>, String> {
    match doc.get(key) {
        None => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("Discovery {key} entries must be strings"))
            })
            .collect(),
        Some(_) => Err(format!("Discovery {key} must be an array")),
    }
}

fn matches_name(path: &Path, kind: &str, names: &[String]) -> bool {
    let item = if kind == "skills" {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let full = item.to_string_lossy().replace('\\', "/");
    names.iter().any(|name| {
        full.ends_with(&format!("/{name}"))
            || item.file_stem().is_some_and(|stem| stem == name.as_str())
    })
}

fn manifest(
    path: &Path,
    kind: &str,
    seen: &mut HashSet<PathBuf>,
    out: &mut BTreeMap<PathBuf, ()>,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if depth > 16 || *remaining == 0 {
        return Err("Antigravity discovery manifests exceed the supported nesting limit.".into());
    }
    *remaining -= 1;
    skills::ensure_plain_components(path)?;
    let identity = path.canonicalize().map_err(|e| e.to_string())?;
    if !seen.insert(identity.clone()) {
        return Ok(());
    }
    let doc = json_store::read_value(path)?;
    let parent = path.parent().ok_or("Manifest has no parent directory")?;
    let base = if parent
        .file_name()
        .is_some_and(|n| PROJECT_DIRS.iter().any(|dir| n == *dir))
    {
        parent.parent().unwrap_or(parent)
    } else {
        parent
    };
    let mut local = BTreeMap::new();
    for key in ["inherits", "entries"] {
        let entries = match doc.get(key) {
            None => continue,
            Some(serde_json::Value::Array(entries)) => entries,
            Some(_) => return Err(format!("Discovery {key} must be an array")),
        };
        if entries.len() > 256 {
            return Err("Too many discovery manifest entries".into());
        }
        for entry in entries {
            let value = entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or("Discovery entry requires a path")?;
            let only = names(entry, "include_only")?;
            let excluded = names(entry, "exclude")?;
            let target = base.join(value);
            let mut found = BTreeMap::new();
            if key == "inherits" {
                manifest(&target, kind, seen, &mut found, depth + 1, remaining)?;
            } else if entry.get("include_only").is_some()
                && target.is_dir()
                && !(kind == "skills" && target.join("SKILL.md").is_file())
            {
                for name in &only {
                    scan(&target.join(name), kind, &[], &mut found)?;
                }
            } else {
                scan(&target, kind, &[], &mut found)?;
            }
            found.retain(|path, _| {
                (entry.get("include_only").is_none() || matches_name(path, kind, &only))
                    && !matches_name(path, kind, &excluded)
            });
            local.extend(found);
        }
    }
    let excluded = names(&doc, "exclude")?;
    local.retain(|path, _| !matches_name(path, kind, &excluded));
    seen.remove(&identity);
    out.extend(local);
    Ok(())
}
