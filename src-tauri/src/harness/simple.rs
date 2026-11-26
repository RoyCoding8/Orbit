use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{
    atomic,
    claude::{
        config::{masked_value, ConfigLayer, ConfigSnapshot, EffectiveSetting, SettingsLayer},
        hooks::HookView,
        memories::{MemoryCatalog, MemoryFile, MemoryGroup, MemoryRole, MemorySource},
        skills::{self, SkillEntry},
        SkillOverride,
    },
    credentials, fs_guard, instruction_store, json_store, memory_links, skill_store, Harness,
    HarnessError, HookDraft, InventoryKind, ResourceKind, ResourceRef, Result,
};
use crate::ledger;

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub(crate) fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map(|v| PathBuf::from(v.to_string_lossy().trim()))
}

pub(crate) fn list_inventory<H: Harness>(
    harness: &H,
    skills: std::result::Result<Vec<SkillEntry>, String>,
    mcps: std::result::Result<Vec<super::claude::mcp::ServerView>, String>,
    kind: InventoryKind,
) -> Result<Vec<ResourceRef>> {
    let resources = match kind {
        InventoryKind::Skill => skills.map(|items| {
            items
                .into_iter()
                .map(|item| ResourceRef {
                    kind: ResourceKind::Skill,
                    id: item.id,
                    path: item.path,
                })
                .collect()
        }),
        InventoryKind::McpServer => mcps.map(|items| {
            items
                .into_iter()
                .map(|item| ResourceRef {
                    kind: ResourceKind::McpServer,
                    id: item.name,
                    path: harness.mcp_path(),
                })
                .collect()
        }),
    };
    resources.map_err(|message| HarnessError::Read {
        path: harness.config_root().to_path_buf(),
        source: std::io::Error::other(message),
    })
}

pub(crate) fn settings_layer(name: &str, path: &Path) -> std::result::Result<ConfigLayer, String> {
    let mut values = json_store::read_value(path)?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    for (key, value) in &mut values {
        let secret_root = matches!(
            key.as_str(),
            "env" | "headers" | "http_headers" | "byok" | "proxies"
        );
        *value = masked_value(key, value, 0, secret_root);
    }
    Ok(ConfigLayer {
        name: name.into(),
        path: path.to_string_lossy().into_owned(),
        values,
    })
}

pub(crate) fn load_config_layers(
    specs: &[(&str, PathBuf)],
) -> std::result::Result<ConfigSnapshot, String> {
    let mut layers = Vec::with_capacity(specs.len());
    let mut effective = BTreeMap::new();
    for (name, path) in specs {
        let layer = settings_layer(name, path)?;
        for (key, value) in &layer.values {
            effective.insert(
                key.clone(),
                EffectiveSetting {
                    value: value.clone(),
                    source: (*name).into(),
                },
            );
        }
        layers.push(layer);
    }
    Ok(ConfigSnapshot { layers, effective })
}

pub(crate) fn apply_setting(
    harness: &impl Harness,
    action: &str,
    path: &Path,
    key: &str,
    mut value: Value,
    version_field: Option<(&str, i64)>,
) -> std::result::Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("setting key must not be empty".into());
    }
    json_store::mutate_json(
        harness,
        ResourceKind::Settings,
        key,
        action,
        path,
        |document| {
            let values = document
                .as_object_mut()
                .ok_or("settings file must contain a JSON object")?;
            if let Some(previous) = values.get(key) {
                credentials::restore_masked(previous, &mut value).map_err(|_| {
                    "masked settings arrays cannot be edited; re-enter the credential values"
                })?;
            }
            if credentials::contains_mask(&value) {
                return Err("setting still contains a masked placeholder; enter the real credential or remove the field".into());
            }
            values.insert(key.to_owned(), value);
            if let Some((field, version)) = version_field {
                values
                    .entry(field.to_owned())
                    .or_insert_with(|| Value::from(version));
            }
            Ok(())
        },
    )
}

pub(crate) fn undo_setting(
    harness: &impl Harness,
    action: &str,
    path: &Path,
    key: &str,
) -> std::result::Result<(), String> {
    ledger::undo(
        harness.config_root(),
        ResourceKind::Settings,
        key,
        action,
        path,
    )
}

pub(crate) fn skill_in_roots(roots: &[PathBuf], file: &Path) -> bool {
    let Some(resolved) = fs_guard::canonical_or_missing_tail(file) else {
        return false;
    };
    roots.iter().any(|root| {
        let Some(resolved_root) = fs_guard::canonical_or_missing_tail(root) else {
            return false;
        };
        resolved.strip_prefix(&resolved_root).is_ok_and(|rel| {
            let parts: Vec<_> = rel.components().collect();
            parts.iter().all(|c| matches!(c, Component::Normal(_)))
                && (parts.len() == 1
                    || (parts.len() == 2 && rel.file_name().is_some_and(|n| n == "SKILL.md")))
        })
    })
}

pub(crate) fn skill_inventory(
    roots: &[(PathBuf, bool)],
) -> std::result::Result<Vec<SkillEntry>, String> {
    let mut seen: BTreeMap<PathBuf, bool> = BTreeMap::new();
    for (root, editable) in roots {
        if !root.is_dir() {
            continue;
        }
        fs_guard::ensure_plain_components(root)?;
        let entries = fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", root.display()))?;
            let path = entry.path().join("SKILL.md");
            if !path.is_file() {
                continue;
            }
            fs_guard::ensure_plain_components(&path)?;
            let canonical = path.canonicalize().map_err(|e| e.to_string())?;
            seen.entry(canonical)
                .and_modify(|flag| *flag = *flag || *editable)
                .or_insert(*editable);
        }
    }
    seen.into_iter()
        .map(|(path, editable)| {
            let content = skills::read_plain_file(&path)?;
            let fallback = path
                .parent()
                .and_then(Path::file_name)
                .unwrap_or_default()
                .to_string_lossy();
            let meta = skills::parse_frontmatter(&content, &fallback);
            Ok(SkillEntry {
                id: skill_store::hashed_skill_id(&path),
                name: meta.name.unwrap_or_else(|| fallback.into_owned()),
                description: meta.description,
                editable,
                path,
                content,
                state: SkillOverride::On,
            })
        })
        .collect()
}

pub(crate) fn edit_skill(
    harness: &impl Harness,
    entries: Vec<SkillEntry>,
    key: &str,
    expected: &str,
    content: &str,
) -> std::result::Result<(), String> {
    let entry = entries
        .into_iter()
        .find(|e| e.id == key && e.editable)
        .ok_or("Skill is unavailable or read-only")?;
    ledger::write_text(
        harness.config_root(),
        ResourceKind::Skill,
        key,
        "edit",
        &entry.path,
        expected,
        content,
    )
    .map_err(|e| e.to_string())
}

pub(crate) fn install_skill(
    harness: &impl Harness,
    dest_root: &Path,
    source: &Path,
) -> std::result::Result<String, String> {
    skills::validate_plain_tree(source)?;
    let content = skills::read_plain_file(&source.join("SKILL.md"))?;
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Skill folder has no name")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err("Skill folder names use lowercase letters, digits and hyphens.".into());
    }
    if skills::parse_frontmatter(&content, name)
        .description
        .is_none()
    {
        return Err("SKILL.md requires a description in its frontmatter.".into());
    }
    skills::ensure_plain_parent(&dest_root.join(name))?;
    fs::create_dir_all(dest_root).map_err(|e| e.to_string())?;
    let target = dest_root
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(name);
    let key = skill_store::hashed_skill_id(&target.join("SKILL.md"));
    skill_store::install_tree(harness, &key, "install", source, &target)?;
    Ok(key)
}

pub(crate) fn uninstall_skill(
    harness: &impl Harness,
    entries: Vec<SkillEntry>,
    key: &str,
) -> std::result::Result<(), String> {
    let entry = entries
        .into_iter()
        .find(|e| e.id == key && e.editable)
        .ok_or("Skill is unavailable or read-only")?;
    skill_store::uninstall_tree(
        harness,
        key,
        "uninstall",
        entry.path.parent().ok_or("Skill has no folder")?,
    )
}

pub(crate) fn restore_skill(
    harness: &impl Harness,
    owned_roots: &[PathBuf],
    key: &str,
    backup: &Path,
) -> std::result::Result<(), String> {
    let row = ledger::read_applied(harness.config_root())?
        .into_iter()
        .rev()
        .find(|r| r.resource_kind == "skill" && r.id == key && r.action == "uninstall")
        .ok_or("No completed removal is available to restore")?;
    let target = PathBuf::from(row.target.ok_or("Removal has no target")?);
    let skill_md = target.join("SKILL.md");
    if !skill_in_roots(owned_roots, &skill_md) || skill_store::hashed_skill_id(&skill_md) != key {
        return Err("Skill removal target does not belong to this project or user".into());
    }
    skills::ensure_backup_path(harness.config_root(), backup)?;
    let recorded = PathBuf::from(row.backup_path.ok_or("Removal has no backup")?);
    if backup.canonicalize().map_err(|e| e.to_string())?
        != recorded.canonicalize().map_err(|e| e.to_string())?
    {
        return Err("This is not the recorded skill backup".into());
    }
    skill_store::restore_tree(
        harness,
        key,
        "restore",
        backup,
        &target,
        &row.before_checksum.ok_or("Removal has no checksum")?,
    )
}

pub(crate) struct MemoryCandidate {
    pub(crate) path: PathBuf,
    pub(crate) group_id: &'static str,
    pub(crate) source: MemorySource,
    pub(crate) role: MemoryRole,
    pub(crate) load_behavior: String,
    pub(crate) editable: bool,
}

pub(crate) fn collect_dir_files(
    dir: &Path,
    exts: &[&str],
) -> std::result::Result<Vec<PathBuf>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    fs_guard::ensure_plain_components(dir)?;
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        {
            fs_guard::ensure_plain_components(&path)?;
            found.push(path.canonicalize().map_err(|e| e.to_string())?);
        }
    }
    found.sort();
    Ok(found)
}

pub(crate) fn build_memory_catalog(
    id_prefix: &str,
    project: &Path,
    home: Option<&Path>,
    config_root: &Path,
    candidates: Vec<MemoryCandidate>,
    groups: Vec<MemoryGroup>,
) -> std::result::Result<MemoryCatalog, String> {
    let project = project.canonicalize().map_err(|e| e.to_string())?;
    let config_root = fs_guard::canonical_or_missing_tail(config_root)
        .unwrap_or_else(|| config_root.to_path_buf());
    let mut files = BTreeMap::new();
    for item in candidates {
        if !item.path.is_file() {
            continue;
        }
        fs_guard::ensure_plain_components(&item.path)?;
        let path = item.path.canonicalize().map_err(|e| e.to_string())?;
        if files.contains_key(&path) {
            continue;
        }
        let content = skills::read_plain_file(&path)?;
        files.insert(
            path.clone(),
            MemoryFile {
                id: format!(
                    "{id_prefix}-{}",
                    atomic::hex(&Sha256::digest(path.to_string_lossy().as_bytes()))
                ),
                group_id: item.group_id.into(),
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: path.to_string_lossy().into_owned(),
                source: item.source,
                role: item.role,
                load_behavior: item.load_behavior,
                editable: item.editable,
                content,
                references: Vec::new(),
            },
        );
    }
    let known: BTreeMap<_, _> = files
        .iter()
        .map(|(path, file)| (path.clone(), file.id.clone()))
        .collect();
    memory_links::populate_references(files.values_mut(), &known, false, home, |_, target| {
        target.is_some_and(|p| p.starts_with(&project) || p.starts_with(&config_root))
    });
    Ok(MemoryCatalog {
        project_root: project.to_string_lossy().into_owned(),
        groups,
        files: files.into_values().collect(),
        excluded_custom_memory_directory: None,
    })
}

pub(crate) fn edit_memory(
    harness: &impl Harness,
    catalog: MemoryCatalog,
    id: &str,
    expected: &str,
    content: &str,
) -> std::result::Result<(), String> {
    let file = catalog
        .files
        .into_iter()
        .find(|f| f.id == id)
        .ok_or("Instruction is no longer available")?;
    instruction_store::edit(harness, &file, expected, content, "edit")
}

pub(crate) fn delete_memory(
    harness: &impl Harness,
    catalog: &MemoryCatalog,
    id: &str,
    expected: &str,
) -> std::result::Result<(), String> {
    instruction_store::delete(harness, catalog, id, expected, "edit", "delete")
}

#[derive(Clone, Copy)]
pub(crate) enum HookFormat {
    MatcherGroups,
    FlatList,
}

#[derive(Clone, Copy)]
pub(crate) struct HookSchema {
    pub(crate) action: &'static str,
    pub(crate) events: &'static [&'static str],
    pub(crate) format: HookFormat,
    pub(crate) version_field: Option<(&'static str, i64)>,
}

struct HookRow {
    view: HookView,
    event: String,
    group_index: usize,
    command_index: Option<usize>,
}

fn hook_rows(doc: &Value, layer: SettingsLayer, source: &Path, schema: HookSchema) -> Vec<HookRow> {
    let mut rows = Vec::new();
    let Some(hooks_obj) = doc.get("hooks").and_then(Value::as_object) else {
        return rows;
    };
    for &event in schema.events {
        let Some(entries) = hooks_obj.get(event).and_then(Value::as_array) else {
            continue;
        };
        for (i, entry) in entries.iter().enumerate() {
            match schema.format {
                HookFormat::MatcherGroups => {
                    let matcher = entry
                        .get("matcher")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let list = entry
                        .get("hooks")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    for (j, hook) in list.iter().enumerate() {
                        if hook
                            .get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|t| t != "command")
                        {
                            continue;
                        }
                        let Some(cmd) = hook.get("command").and_then(Value::as_str) else {
                            continue;
                        };
                        let id = atomic::hex(&Sha256::digest(
                            format!("{}\0{event}\0{i}\0{j}\0{hook}", source.display()).as_bytes(),
                        ));
                        rows.push(HookRow {
                            event: event.into(),
                            group_index: i,
                            command_index: Some(j),
                            view: HookView {
                                id,
                                event: event.into(),
                                matcher: matcher.clone(),
                                commands: vec![cmd.into()],
                                enabled: true,
                                layer: layer.label().into(),
                                source_path: source.to_string_lossy().into_owned(),
                                editable: true,
                            },
                        });
                    }
                }
                HookFormat::FlatList => {
                    let Some(cmd) = entry.get("command").and_then(Value::as_str) else {
                        continue;
                    };
                    let matcher = entry
                        .get("matcher")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let id = atomic::hex(&Sha256::digest(
                        format!("{}\0{event}\0{i}\0{entry}", source.display()).as_bytes(),
                    ));
                    rows.push(HookRow {
                        event: event.into(),
                        group_index: i,
                        command_index: None,
                        view: HookView {
                            id,
                            event: event.into(),
                            matcher,
                            commands: vec![cmd.into()],
                            enabled: true,
                            layer: layer.label().into(),
                            source_path: source.to_string_lossy().into_owned(),
                            editable: true,
                        },
                    });
                }
            }
        }
    }
    rows
}

pub(crate) fn list_hooks(
    sources: &[(SettingsLayer, PathBuf)],
    schema: HookSchema,
) -> std::result::Result<Vec<HookView>, String> {
    let mut out = Vec::new();
    for (layer, path) in sources {
        let doc = json_store::read_value(path)?;
        out.extend(
            hook_rows(&doc, *layer, path, schema)
                .into_iter()
                .map(|r| r.view),
        );
    }
    Ok(out)
}

pub(crate) fn save_hook(
    harness: &impl Harness,
    path: &Path,
    schema: HookSchema,
    draft: HookDraft,
) -> std::result::Result<(), String> {
    if !schema.events.contains(&draft.event.as_str()) {
        return Err(format!("Unsupported hook event: {}", draft.event));
    }
    if draft.command.trim().is_empty() {
        return Err("Hook command cannot be empty".into());
    }
    json_store::mutate_json(
        harness,
        ResourceKind::Settings,
        "hooks",
        schema.action,
        path,
        |doc| {
            let root = doc
                .as_object_mut()
                .ok_or("Hook configuration must contain a JSON object")?;
            if let Some((field, version)) = schema.version_field {
                root.entry(field.to_owned())
                    .or_insert_with(|| Value::from(version));
            }
            if let Some(id) = &draft.edit_id {
                let row = hook_rows(doc, draft.layer, path, schema)
                    .into_iter()
                    .find(|r| &r.view.id == id)
                    .ok_or("Hook changed. Refresh hooks before editing.")?;
                if row.event != draft.event {
                    return Err("Remove and recreate a hook to change its event".into());
                }
                let entries = doc
                    .get_mut("hooks")
                    .and_then(Value::as_object_mut)
                    .and_then(|h| h.get_mut(&row.event))
                    .and_then(Value::as_array_mut)
                    .ok_or("Hook event list is missing")?;
                let entry = entries
                    .get_mut(row.group_index)
                    .and_then(Value::as_object_mut)
                    .ok_or("Hook entry is missing")?;
                if let Some(matcher) = draft.matcher.filter(|m| !m.trim().is_empty()) {
                    entry.insert("matcher".into(), Value::String(matcher));
                } else {
                    entry.remove("matcher");
                }
                match (schema.format, row.command_index) {
                    (HookFormat::MatcherGroups, Some(j)) => {
                        let hook = entry
                            .get_mut("hooks")
                            .and_then(Value::as_array_mut)
                            .and_then(|list| list.get_mut(j))
                            .and_then(Value::as_object_mut)
                            .ok_or("Hook command is missing")?;
                        hook.insert("command".into(), Value::String(draft.command));
                    }
                    (HookFormat::FlatList, _) => {
                        entry.insert("command".into(), Value::String(draft.command));
                    }
                    _ => return Err("Invalid hook structure".into()),
                }
            } else {
                let hooks_obj = root
                    .entry("hooks")
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .ok_or("`hooks` must be a JSON object")?;
                let entries = hooks_obj
                    .entry(draft.event)
                    .or_insert_with(|| Value::Array(Vec::new()))
                    .as_array_mut()
                    .ok_or("Hook event must be an array")?;
                let matcher = draft.matcher.filter(|m| !m.trim().is_empty());
                let new_entry = match schema.format {
                    HookFormat::MatcherGroups => {
                        let mut obj = Map::new();
                        if let Some(m) = matcher {
                            obj.insert("matcher".into(), Value::String(m));
                        }
                        obj.insert(
                            "hooks".into(),
                            json!([{ "type": "command", "command": draft.command }]),
                        );
                        Value::Object(obj)
                    }
                    HookFormat::FlatList => {
                        let mut obj = Map::new();
                        obj.insert("command".into(), Value::String(draft.command));
                        if let Some(m) = matcher {
                            obj.insert("matcher".into(), Value::String(m));
                        }
                        Value::Object(obj)
                    }
                };
                entries.push(new_entry);
            }
            Ok(())
        },
    )
}

pub(crate) fn delete_hook(
    harness: &impl Harness,
    path: &Path,
    layer: SettingsLayer,
    schema: HookSchema,
    id: &str,
) -> std::result::Result<(), String> {
    json_store::mutate_json(
        harness,
        ResourceKind::Settings,
        "hooks",
        schema.action,
        path,
        |doc| {
            let row = hook_rows(doc, layer, path, schema)
                .into_iter()
                .find(|r| r.view.id == id)
                .ok_or("Hook changed. Refresh hooks.")?;
            let root = doc
                .as_object_mut()
                .ok_or("Hook configuration must contain a JSON object")?;
            if let Some((field, version)) = schema.version_field {
                root.entry(field.to_owned())
                    .or_insert_with(|| Value::from(version));
            }
            let hooks_obj = root
                .get_mut("hooks")
                .and_then(Value::as_object_mut)
                .ok_or("Hook map is missing")?;
            let entries = hooks_obj
                .get_mut(&row.event)
                .and_then(Value::as_array_mut)
                .ok_or("Hook event list is missing")?;
            match (schema.format, row.command_index) {
                (HookFormat::MatcherGroups, Some(j)) => {
                    let remove_group = {
                        let list = entries
                            .get_mut(row.group_index)
                            .and_then(Value::as_object_mut)
                            .and_then(|e| e.get_mut("hooks"))
                            .and_then(Value::as_array_mut)
                            .ok_or("Hook command list is missing")?;
                        list.remove(j);
                        list.is_empty()
                    };
                    if remove_group {
                        entries.remove(row.group_index);
                    }
                }
                (HookFormat::FlatList, _) => {
                    entries.remove(row.group_index);
                }
                _ => return Err("Invalid hook structure".into()),
            }
            if entries.is_empty() {
                hooks_obj.remove(&row.event);
            }
            Ok(())
        },
    )
}

pub(crate) fn unsupported_toggle(label: &str) -> std::result::Result<(), String> {
    Err(format!(
        "{label} does not support toggling individual hooks; remove or edit the hook instead."
    ))
}

pub(crate) fn unsupported_skill_override(label: &str) -> std::result::Result<(), String> {
    Err(format!(
        "{label} has no native per-skill enable switch. Uninstall an owned skill to keep a restorable copy in History."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::muse::MuseHarness;

    fn hook_schema() -> HookSchema {
        HookSchema {
            action: "test-hooks",
            events: &["PreToolUse", "Stop"],
            format: HookFormat::FlatList,
            version_field: Some(("version", 1)),
        }
    }

    #[test]
    fn malformed_json_mask_and_stale_preconditions_are_refused() {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        fs::create_dir_all(&project).unwrap();
        let harness = MuseHarness::new(home.join(".config/muse"), home);
        let settings = harness.settings_path();
        let hooks = project.join(".muse/hooks.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::create_dir_all(hooks.parent().unwrap()).unwrap();

        fs::write(&settings, "{broken").unwrap();
        assert!(settings_layer("user", &settings).is_err());
        assert!(apply_setting(&harness, "act", &settings, "model", json!("a"), None).is_err());
        assert!(
            apply_setting(&harness, "act", &settings, "  ", json!("a"), None)
                .unwrap_err()
                .contains("empty")
        );

        fs::write(&settings, "{}").unwrap();
        assert!(apply_setting(
            &harness,
            "act",
            &settings,
            "env",
            json!({"TOKEN": "••••••••"}),
            None,
        )
        .unwrap_err()
        .contains("masked placeholder"));
        apply_setting(
            &harness,
            "act",
            &settings,
            "env",
            json!({"TOKEN": "real-secret"}),
            None,
        )
        .unwrap();
        apply_setting(
            &harness,
            "act",
            &settings,
            "env",
            json!({"TOKEN": "••••••••", "EXTRA": "ok"}),
            None,
        )
        .unwrap();
        let raw = json_store::read_value(&settings).unwrap();
        assert_eq!(raw["env"]["TOKEN"], "real-secret");
        assert_eq!(raw["env"]["EXTRA"], "ok");

        fs::write(&hooks, "[not-an-object").unwrap();
        assert!(list_hooks(&[(SettingsLayer::Project, hooks.clone())], hook_schema()).is_err());
        assert!(save_hook(
            &harness,
            &hooks,
            hook_schema(),
            HookDraft {
                layer: SettingsLayer::Project,
                event: "Stop".into(),
                matcher: None,
                command: "echo hi".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .is_err());
        fs::write(&hooks, "{}").unwrap();
        assert!(save_hook(
            &harness,
            &hooks,
            hook_schema(),
            HookDraft {
                layer: SettingsLayer::Project,
                event: "Unknown".into(),
                matcher: None,
                command: "echo hi".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .unwrap_err()
        .contains("Unsupported"));
        assert!(save_hook(
            &harness,
            &hooks,
            hook_schema(),
            HookDraft {
                layer: SettingsLayer::Project,
                event: "Stop".into(),
                matcher: None,
                command: "  ".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .unwrap_err()
        .contains("empty"));
        assert!(save_hook(
            &harness,
            &hooks,
            hook_schema(),
            HookDraft {
                layer: SettingsLayer::Project,
                event: "Stop".into(),
                matcher: None,
                command: "echo hi".into(),
                edit_id: Some("stale-id".into()),
                source_id: None,
            },
        )
        .unwrap_err()
        .contains("Hook changed"));
        assert!(delete_hook(
            &harness,
            &hooks,
            SettingsLayer::Project,
            hook_schema(),
            "stale-id",
        )
        .unwrap_err()
        .contains("Hook changed"));

        let mem = project.join("AGENTS.md");
        fs::write(&mem, "Original\n").unwrap();
        let cat = build_memory_catalog(
            "t",
            &project,
            Some(harness.home()),
            harness.config_root(),
            vec![MemoryCandidate {
                path: mem.clone(),
                group_id: "p",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "always".into(),
                editable: true,
            }],
            vec![MemoryGroup {
                id: "p".into(),
                label: "P".into(),
                role: MemorySource::ProjectInstructions,
            }],
        )
        .unwrap();
        let id = cat.files[0].id.clone();
        assert!(edit_memory(&harness, cat, &id, "stale", "new")
            .unwrap_err()
            .contains("changed since"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_settings_hooks_and_memories_are_refused() {
        use std::os::unix::fs::symlink;
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("repo");
        let outside = dir.path().join("outside.json");
        fs::create_dir_all(home.join(".config/muse")).unwrap();
        fs::create_dir_all(project.join(".muse")).unwrap();
        fs::write(&outside, "{}").unwrap();
        let harness = MuseHarness::new(home.join(".config/muse"), home);
        let settings = harness.settings_path();
        let hooks = project.join(".muse/hooks.json");
        let mem = project.join("AGENTS.md");
        symlink(&outside, &settings).unwrap();
        symlink(&outside, &hooks).unwrap();
        symlink(&outside, &mem).unwrap();

        assert!(settings_layer("user", &settings).is_err());
        assert!(apply_setting(&harness, "act", &settings, "model", json!("x"), None).is_err());
        assert!(list_hooks(&[(SettingsLayer::Project, hooks.clone())], hook_schema()).is_err());
        assert!(save_hook(
            &harness,
            &hooks,
            hook_schema(),
            HookDraft {
                layer: SettingsLayer::Project,
                event: "Stop".into(),
                matcher: None,
                command: "echo x".into(),
                edit_id: None,
                source_id: None,
            },
        )
        .is_err());
        assert!(build_memory_catalog(
            "t",
            &project,
            Some(harness.home()),
            harness.config_root(),
            vec![MemoryCandidate {
                path: mem,
                group_id: "p",
                source: MemorySource::ProjectInstructions,
                role: MemoryRole::Instruction,
                load_behavior: "always".into(),
                editable: true,
            }],
            vec![],
        )
        .is_err());
    }
}
