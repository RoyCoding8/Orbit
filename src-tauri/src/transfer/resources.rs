use super::{Bundle, Category, Destination, Entry, Manifest, Scope};
use crate::harness::{
    claude::{self, memories::MemorySource, skills},
    opencode,
    selected::SelectedHarness,
    Harness,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, tag = "kind")]
pub(super) enum Resource {
    File { area: Area, path: String },
    Config { slot: Slot, section: Section },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) enum Area {
    Skills,
    Instructions,
    AutoMemory,
    Steering,
    HookFiles,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub(super) enum Slot {
    Settings,
    Mcp,
    Hooks,
    Agent { name: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub(super) enum Section {
    Settings,
    Mcp { name: String },
    Hooks,
    Extensions,
}

pub(super) fn relative(text: &str) -> Result<PathBuf, String> {
    if text.is_empty() || text.len() > 500 || text.contains(['\\', ':', '\0']) {
        return Err("Archive contains an unsafe relative path".into());
    }
    for component in text.split('/') {
        let stem = component.split('.').next().unwrap_or("").to_uppercase();
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|c| c.is_control() || "<>\"|?*".contains(c))
            || matches!(component.to_lowercase().as_str(), ".git" | ".orbit")
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.ends_with(['1', '2', '3', '4', '5', '6', '7', '8', '9']))
        {
            return Err("Archive contains an unsafe relative path".into());
        }
    }
    if text.split('/').count() > 24 {
        return Err("Archive path is too deeply nested".into());
    }
    Ok(PathBuf::from(text))
}

fn project_config(harness: &impl Harness, project: &Path) -> PathBuf {
    match harness.id().0 {
        "claude-code" => project.join(".claude"),
        "codex" => project.join(".codex"),
        "kiro" => project.join(".kiro"),
        "opencode" => project.join(".opencode"),
        "pi" => project.join(".pi"),
        _ => unreachable!(),
    }
}

fn skill_root(harness: &impl Harness, project: &Path, scope: Scope) -> PathBuf {
    if scope != Scope::User {
        if harness.id().0 == "codex" {
            project.join(".agents/skills")
        } else {
            project_config(harness, project).join("skills")
        }
    } else if harness.id().0 == "codex" {
        harness
            .user_home()
            .unwrap_or(harness.config_root())
            .join(".agents/skills")
    } else {
        harness.config_root().join("skills")
    }
}

fn memory_root(harness: &impl Harness, project: &Path) -> Result<PathBuf, String> {
    match harness.id().0 {
        "claude-code" => {
            if let Some(custom) = claude::memories::configured_memory_directory(harness, project) {
                if custom.starts_with(project) {
                    return Ok(custom);
                }
                return Err(
                    "The configured Claude memory directory is outside the selected project".into(),
                );
            }
            let key = project_memory_key(project)?;
            Ok(harness
                .config_root()
                .join("projects")
                .join(key)
                .join("memory"))
        }
        "codex" => Ok(harness.config_root().join("memories")),
        _ => Err("This harness has no local generated-memory directory".into()),
    }
}

pub(super) fn project_memory_key(project: &Path) -> Result<String, String> {
    let text = project.to_string_lossy();
    let native = text
        .strip_prefix(r"\\?\UNC\")
        .map(|tail| format!(r"\\{tail}"))
        .unwrap_or_else(|| text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned());
    claude::project_key(&native).ok_or("Project path cannot be encoded for Claude memory".into())
}

fn config_path(
    harness: &impl Harness,
    project: &Path,
    scope: Scope,
    slot: &Slot,
) -> Result<PathBuf, String> {
    if scope == Scope::Local && harness.id().0 != "claude-code" {
        return Err("This harness has no local settings layer".into());
    }
    let root = if scope == Scope::User {
        harness.config_root().to_path_buf()
    } else {
        project_config(harness, project)
    };
    match slot {
        Slot::Settings => Ok(if scope == Scope::User {
            harness.settings_path()
        } else {
            match harness.id().0 {
                "claude-code" => root.join(if scope == Scope::Local {
                    "settings.local.json"
                } else {
                    "settings.json"
                }),
                "codex" => root.join("config.toml"),
                "kiro" => root.join("settings/cli.json"),
                "opencode" => opencode::config::project_config_path(project),
                "pi" => root.join("settings.json"),
                _ => unreachable!(),
            }
        }),
        Slot::Mcp => Ok(if scope == Scope::User {
            harness.mcp_path()
        } else {
            match harness.id().0 {
                "claude-code" => project.join(".mcp.json"),
                "codex" | "opencode" => {
                    return config_path(harness, project, scope, &Slot::Settings)
                }
                "kiro" => root.join("settings/mcp.json"),
                "pi" => root.join("mcp.json"),
                _ => unreachable!(),
            }
        }),
        Slot::Hooks if harness.id().0 == "codex" => Ok(root.join("hooks.json")),
        Slot::Agent { name } if harness.id().0 == "kiro" => {
            let name = relative(name)?;
            if name.components().count() != 1 || name.extension().is_none_or(|ext| ext != "json") {
                return Err("Invalid Kiro agent filename".into());
            }
            Ok(root.join("agents").join(name))
        }
        _ => Err("Unsupported native configuration slot".into()),
    }
}

pub(super) fn target(
    harness: &impl Harness,
    project: &Path,
    destination: Destination,
    entry: &Entry,
) -> Result<PathBuf, String> {
    let scope = match destination {
        Destination::Original => entry.scope,
        Destination::Project => Scope::Project,
        Destination::User => Scope::User,
    };
    let path = match &entry.resource {
        Resource::File { area, path } => {
            let rel = relative(path)?;
            file_root(harness, project, scope, *area)?.join(rel)
        }
        Resource::Config { slot, .. } => config_path(harness, project, scope, slot)?,
    };
    skills::ensure_plain_components(&path)?;
    skills::ensure_plain_file_if_present(&path)?;
    Ok(path)
}

fn file_root(
    harness: &impl Harness,
    project: &Path,
    scope: Scope,
    area: Area,
) -> Result<PathBuf, String> {
    Ok(match area {
        Area::Skills => skill_root(harness, project, scope),
        Area::AutoMemory => memory_root(harness, project)?,
        Area::Steering => {
            if scope == Scope::User {
                harness.config_root().join("steering")
            } else {
                project_config(harness, project).join("steering")
            }
        }
        Area::Instructions => {
            if scope == Scope::User {
                harness.config_root().to_path_buf()
            } else {
                project.to_path_buf()
            }
        }
        Area::HookFiles => {
            if scope == Scope::User {
                harness.config_root().to_path_buf()
            } else {
                project_config(harness, project)
            }
        }
    })
}

pub(super) fn document(bytes: &[u8], harness: &str, slot: &Slot) -> Result<Value, String> {
    if bytes.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "Configuration is not UTF-8")?
        .trim_start_matches('\u{feff}');
    let value = if harness == "codex" && !matches!(slot, Slot::Hooks) {
        toml_edit::de::from_str::<Value>(text)
            .map_err(|_| "Invalid native TOML configuration".to_string())?
    } else {
        jsonc_parser::cst::CstRootNode::parse(text, &Default::default())
            .map_err(|_| "Invalid native JSON configuration".to_string())?
            .to_serde_value()
            .ok_or("Empty native JSON configuration")?
    };
    if !value.is_object() {
        return Err("Native configuration must be an object".into());
    }
    Ok(value)
}

fn encode(value: &Value, harness: &str, slot: &Slot) -> Result<Vec<u8>, String> {
    if harness == "codex" && !matches!(slot, Slot::Hooks) {
        toml_edit::ser::to_string_pretty(value)
            .map(String::into_bytes)
            .map_err(|e| e.to_string())
    } else {
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())
    }
}

fn mcp_pointer(harness: &str) -> &'static str {
    match harness {
        "codex" => "/mcp_servers",
        "opencode" => "/mcp/servers",
        _ => "/mcpServers",
    }
}

fn extensions_key(harness: &str) -> &'static str {
    if harness == "pi" {
        "extensions"
    } else {
        "plugins"
    }
}

pub(super) fn managed_keys(harness: &str) -> &'static [&'static str] {
    match harness {
        "codex" => &["hooks", "mcp_servers"],
        "opencode" => &["mcp", "plugins"],
        "pi" => &["extensions"],
        _ => &["hooks", "mcpServers"],
    }
}

fn settings_only(mut value: Value, harness: &str) -> Value {
    if let Some(object) = value.as_object_mut() {
        for &key in managed_keys(harness) {
            object.remove(key);
        }
    }
    value
}

fn section(value: &Value, part: &Section, harness: &str) -> Option<Value> {
    match part {
        Section::Settings => Some(settings_only(value.clone(), harness)),
        Section::Mcp { name } => value.pointer(mcp_pointer(harness))?.get(name).cloned(),
        Section::Hooks => value.get("hooks").cloned(),
        Section::Extensions => value.get(extensions_key(harness)).cloned(),
    }
}

pub(super) fn part(entry: &Entry, bytes: &[u8], harness: &str) -> Result<Value, String> {
    match &entry.resource {
        Resource::File { .. } => Ok(Value::String(super::digest(bytes))),
        Resource::Config {
            slot,
            section: part,
        } => section(&document(bytes, harness, slot)?, part, harness)
            .ok_or("Archive is missing its declared configuration section".into()),
    }
}

pub(super) fn current(
    bytes: Option<&[u8]>,
    entry: &Entry,
    harness: &str,
) -> Result<Option<Value>, String> {
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    match &entry.resource {
        Resource::File { .. } => Ok(Some(Value::String(super::digest(bytes)))),
        Resource::Config {
            slot,
            section: part,
        } => Ok(section(&document(bytes, harness, slot)?, part, harness)),
    }
}

pub(super) fn merges(entry: &Entry) -> bool {
    matches!(
        entry.resource,
        Resource::Config {
            section: Section::Hooks | Section::Extensions,
            ..
        }
    )
}

fn merge(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Array(target), Value::Array(source)) => {
            for entry in source {
                if !target.contains(&entry) {
                    target.push(entry);
                }
            }
        }
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                merge(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target @ Value::Null, source) => *target = source,
        _ => {}
    }
}

fn install(value: &mut Value, part: &Section, source: Value, harness: &str) -> Result<(), String> {
    match part {
        Section::Settings => {
            let preserved = value
                .as_object()
                .ok_or("Invalid native config")?
                .iter()
                .filter(|(key, _)| managed_keys(harness).contains(&key.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Map<_, _>>();
            *value = source;
            value
                .as_object_mut()
                .ok_or("Settings must be an object")?
                .extend(preserved);
        }
        Section::Mcp { name } => {
            let keys: Vec<_> = mcp_pointer(harness)
                .trim_start_matches('/')
                .split('/')
                .collect();
            let mut cursor = value;
            for key in keys {
                cursor = cursor
                    .as_object_mut()
                    .ok_or("MCP configuration has an invalid shape")?
                    .entry(key)
                    .or_insert_with(|| serde_json::json!({}));
            }
            cursor
                .as_object_mut()
                .ok_or("MCP server map must be an object")?
                .insert(name.clone(), source);
        }
        Section::Hooks | Section::Extensions => {
            let key = if matches!(part, Section::Hooks) {
                "hooks"
            } else {
                extensions_key(harness)
            };
            merge(
                value
                    .as_object_mut()
                    .ok_or("Invalid native config")?
                    .entry(key)
                    .or_insert(Value::Null),
                source,
            );
        }
    }
    Ok(())
}

pub(super) fn updated(
    entry: &Entry,
    before: &[u8],
    bytes: &[u8],
    harness: &str,
) -> Result<Vec<u8>, String> {
    match &entry.resource {
        Resource::File { .. } => Ok(bytes.to_vec()),
        Resource::Config { slot, section } => {
            let original = document(before, harness, slot)?;
            let mut value = original.clone();
            install(&mut value, section, part(entry, bytes, harness)?, harness)?;
            if let Slot::Agent { name } = slot {
                value
                    .as_object_mut()
                    .ok_or("Invalid Kiro profile")?
                    .entry("name")
                    .or_insert_with(|| Value::String(name.trim_end_matches(".json").into()));
            }
            if before.is_empty() {
                return encode(&value, harness, slot);
            }
            let text = std::str::from_utf8(before)
                .map_err(|_| "Configuration is not UTF-8")?
                .trim_start_matches('\u{feff}');
            if harness == "codex" && !matches!(slot, Slot::Hooks) {
                let mut document = text
                    .parse::<toml_edit::DocumentMut>()
                    .map_err(|e| e.to_string())?;
                let after = toml_edit::ser::to_document(&value).map_err(|e| e.to_string())?;
                patch_toml(document.as_table_mut(), after.as_table(), &original, &value);
                Ok(document.to_string().into_bytes())
            } else {
                let root = jsonc_parser::cst::CstRootNode::parse(text, &Default::default())
                    .map_err(|e| e.to_string())?;
                patch_json(&root.object_value_or_set(), &original, &value);
                Ok(root.to_string().into_bytes())
            }
        }
    }
}

fn patch_toml(
    target: &mut dyn toml_edit::TableLike,
    source: &dyn toml_edit::TableLike,
    before: &Value,
    after: &Value,
) {
    let removed: Vec<_> = target
        .iter()
        .filter(|(key, _)| !source.contains_key(key))
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in removed {
        target.remove(&key);
    }
    for (key, value) in source.iter() {
        if before.get(key) == after.get(key) {
            continue;
        }
        match (
            target
                .get_mut(key)
                .and_then(toml_edit::Item::as_table_like_mut),
            value.as_table_like(),
        ) {
            (Some(target), Some(source)) => patch_toml(target, source, &before[key], &after[key]),
            _ => {
                let mut value = value.clone();
                if let (Some(previous), Some(next)) = (
                    target.get(key).and_then(toml_edit::Item::as_value),
                    value.as_value_mut(),
                ) {
                    *next.decor_mut() = previous.decor().clone();
                }
                if let Some(target) = target.get_mut(key) {
                    *target = value;
                } else {
                    target.insert(key, value);
                }
            }
        }
    }
}

fn patch_json(target: &jsonc_parser::cst::CstObject, before: &Value, after: &Value) {
    if let (Some(before), Some(after)) = (before.as_object(), after.as_object()) {
        for key in before.keys().filter(|key| !after.contains_key(*key)) {
            if let Some(property) = target.get(key) {
                property.remove();
            }
        }
        for (key, value) in after {
            if before.get(key) == Some(value) {
                continue;
            }
            if value.is_object() && before.get(key).is_some_and(Value::is_object) {
                patch_json(&target.object_value_or_set(key), &before[key], value);
            } else if let Some(property) = target.get(key) {
                property.set_value(opencode::config::cst_input_value(value.clone()));
            } else {
                target.append(key, opencode::config::cst_input_value(value.clone()));
            }
        }
    }
}

pub(super) fn target_key(entry: &Entry) -> String {
    match &entry.resource {
        Resource::File { .. } => "file".into(),
        Resource::Config { section, .. } => serde_json::to_string(section).unwrap_or_default(),
    }
}

pub(super) fn validate(entry: &Entry, bytes: &[u8], harness: &str) -> Result<(), String> {
    if entry.id.len() > 80 || entry.label.len() > 1000 {
        return Err("Invalid archive resource metadata".into());
    }
    let category = match &entry.resource {
        Resource::File { area, path } => {
            relative(path)?;
            match area {
                Area::Skills => Category::Skills,
                Area::HookFiles => {
                    if !path.starts_with(match harness {
                        "opencode" => "plugins/",
                        "pi" => "extensions/",
                        _ => "hooks/",
                    }) {
                        return Err("Invalid native hook file path".into());
                    }
                    Category::Hooks
                }
                Area::Instructions | Area::AutoMemory | Area::Steering => {
                    if !path.ends_with(".md") {
                        return Err("Memory resources must be Markdown files".into());
                    }
                    if *area == Area::AutoMemory && !matches!(harness, "codex" | "claude-code") {
                        return Err("Unsupported generated-memory format".into());
                    }
                    if *area == Area::Steering && harness != "kiro" {
                        return Err("Unsupported steering format".into());
                    }
                    Category::Memories
                }
            }
        }
        Resource::Config { slot, section } => {
            let value = part(entry, bytes, harness)?;
            match section {
                Section::Settings if matches!(slot, Slot::Settings) && value.is_object() => {
                    Category::Settings
                }
                Section::Mcp { name }
                    if matches!(slot, Slot::Mcp | Slot::Settings)
                        && !name.trim().is_empty()
                        && value.is_object() =>
                {
                    Category::Mcps
                }
                Section::Hooks
                    if matches!(harness, "claude-code" | "codex" | "kiro")
                        && (value.is_array() || value.is_object()) =>
                {
                    Category::Hooks
                }
                Section::Extensions
                    if matches!(harness, "opencode" | "pi")
                        && matches!(slot, Slot::Settings)
                        && value.is_array() =>
                {
                    Category::Hooks
                }
                _ => return Err("Invalid native configuration section".into()),
            }
        }
    };
    if category != entry.category {
        return Err("Archive resource category does not match its payload".into());
    }
    Ok(())
}

impl Bundle {
    fn add(
        &mut self,
        harness: &str,
        category: Category,
        scope: Scope,
        label: String,
        resource: Resource,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        if self.manifest.entries.len() >= super::COUNT_LIMIT
            || bytes.len() as u64 > super::FILE_LIMIT
            || self
                .data
                .values()
                .map(|data| data.len() as u64)
                .sum::<u64>()
                + bytes.len() as u64
                > super::LIMIT
        {
            return Err("Selected resources exceed the archive limits".into());
        }
        let id = format!("resource-{}", self.manifest.entries.len());
        let filename = match &resource {
            Resource::File { path, .. } => path.rsplit('/').next().unwrap_or("resource"),
            Resource::Config { slot, .. } => {
                if harness == "codex" && !matches!(slot, Slot::Hooks) {
                    "config.toml"
                } else {
                    "config.json"
                }
            }
        };
        let blob = format!("files/{id}/{filename}");
        let entry = Entry {
            id,
            category,
            scope,
            label,
            blob: blob.clone(),
            checksum: super::digest(&bytes),
            resource,
        };
        validate(&entry, &bytes, harness)?;
        self.data.insert(blob, bytes);
        self.manifest.entries.push(entry);
        Ok(())
    }
}

fn files(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !skills::path_exists_no_follow(root)? {
        return Ok(Vec::new());
    }
    skills::ensure_plain_components(root)?;
    fn visit(root: &Path, depth: usize, output: &mut Vec<PathBuf>) -> Result<(), String> {
        if depth > 24 || output.len() >= super::COUNT_LIMIT {
            return Err("Resource directory exceeds archive limits".into());
        }
        for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if matches!(
                entry.file_name().to_str(),
                Some(".git" | ".orbit" | "node_modules")
            ) {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            if crate::harness::atomic::is_link_or_reparse(&metadata) {
                return Err(format!(
                    "Linked resources cannot be archived: {}",
                    entry.path().display()
                ));
            }
            if metadata.is_dir() {
                visit(&entry.path(), depth + 1, output)?;
            } else if metadata.is_file() {
                output.push(entry.path());
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    visit(root, 0, &mut output)?;
    output.sort();
    Ok(output)
}

fn add_tree(
    bundle: &mut Bundle,
    harness: &str,
    root: &Path,
    scope: Scope,
    area: Area,
    prefix: &str,
) -> Result<(), String> {
    for path in files(root)? {
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let rel = format!("{prefix}{rel}");
        let category = if area == Area::Skills {
            Category::Skills
        } else {
            Category::Hooks
        };
        bundle.add(
            harness,
            category,
            scope,
            rel.clone(),
            Resource::File { area, path: rel },
            super::read_file(&path, super::FILE_LIMIT)?,
        )?;
    }
    Ok(())
}

fn collect_config(
    bundle: &mut Bundle,
    harness: &impl Harness,
    project: &Path,
    scope: Scope,
    slot: Slot,
    selected: &[Category],
) -> Result<(), String> {
    let path = config_path(harness, project, scope, &slot)?;
    if !skills::path_exists_no_follow(&path)? {
        return Ok(());
    }
    let doc = document(
        &super::read_file(&path, super::FILE_LIMIT)?,
        harness.id().0,
        &slot,
    )?;
    let mut parts = Vec::new();
    if matches!(slot, Slot::Settings) && selected.contains(&Category::Settings) {
        parts.push((Category::Settings, Section::Settings, "Settings".to_owned()));
    }
    if selected.contains(&Category::Mcps) && matches!(slot, Slot::Mcp | Slot::Settings) {
        if let Some(servers) = doc
            .pointer(mcp_pointer(harness.id().0))
            .and_then(Value::as_object)
        {
            parts.extend(servers.keys().map(|name| {
                (
                    Category::Mcps,
                    Section::Mcp { name: name.clone() },
                    name.clone(),
                )
            }));
        }
    }
    if selected.contains(&Category::Hooks) {
        if matches!(harness.id().0, "claude-code" | "codex" | "kiro") && doc.get("hooks").is_some()
        {
            parts.push((Category::Hooks, Section::Hooks, "Hooks".into()));
        }
        if matches!(harness.id().0, "opencode" | "pi")
            && doc.get(extensions_key(harness.id().0)).is_some()
        {
            parts.push((
                Category::Hooks,
                Section::Extensions,
                "Extension/plugin list".into(),
            ));
        }
    }
    for (category, part, label) in parts {
        let source = section(&doc, &part, harness.id().0).ok_or("Missing configuration section")?;
        let mut minimal = serde_json::json!({});
        install(&mut minimal, &part, source, harness.id().0)?;
        let bytes = encode(&minimal, harness.id().0, &slot)?;
        bundle.add(
            harness.id().0,
            category,
            scope,
            format!(
                "{label} · {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            Resource::Config {
                slot: slot.clone(),
                section: part,
            },
            bytes,
        )?;
    }
    Ok(())
}

pub(super) fn collect(
    harness: &SelectedHarness,
    project: &Path,
    selected: &[Category],
) -> Result<Bundle, String> {
    let mut bundle = Bundle {
        manifest: Manifest {
            format: "orbit-harness".into(),
            version: 1,
            harness: harness.id().0.into(),
            orbit_version: env!("CARGO_PKG_VERSION").into(),
            entries: Vec::new(),
        },
        data: BTreeMap::new(),
        warnings: notes(harness),
    };
    for scope in [Scope::User, Scope::Project] {
        if selected.contains(&Category::Skills) {
            let mut roots = vec![skill_root(harness, project, scope)];
            if harness.id().0 == "codex" && scope == Scope::User {
                roots.push(harness.config_root().join("skills"));
            }
            for root in roots {
                add_tree(&mut bundle, harness.id().0, &root, scope, Area::Skills, "")?;
            }
        }
        collect_config(
            &mut bundle,
            harness,
            project,
            scope,
            Slot::Settings,
            selected,
        )?;
        if matches!(harness.id().0, "claude-code" | "kiro" | "pi")
            && selected.contains(&Category::Mcps)
        {
            collect_config(&mut bundle, harness, project, scope, Slot::Mcp, selected)?;
        }
        if selected.contains(&Category::Hooks) {
            if harness.id().0 == "codex" {
                collect_config(
                    &mut bundle,
                    harness,
                    project,
                    scope,
                    Slot::Hooks,
                    &[Category::Hooks],
                )?;
            }
            let root = if scope == Scope::User {
                harness.config_root().to_path_buf()
            } else {
                project_config(harness, project)
            };
            let folder = match harness.id().0 {
                "opencode" => "plugins",
                "pi" => "extensions",
                _ => "hooks",
            };
            if harness.id().0 != "codex" {
                add_tree(
                    &mut bundle,
                    harness.id().0,
                    &root.join(folder),
                    scope,
                    Area::HookFiles,
                    &format!("{folder}/"),
                )?;
            }
            if harness.id().0 == "kiro" {
                for path in files(&root.join("agents"))? {
                    if path.extension().is_some_and(|ext| ext == "json") {
                        collect_config(
                            &mut bundle,
                            harness,
                            project,
                            scope,
                            Slot::Agent {
                                name: path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                            },
                            &[Category::Hooks],
                        )?;
                    } else {
                        bundle.warnings.push("Kiro Markdown agent profiles are not included; only native JSON hook sources are supported.".into());
                    }
                }
            }
        }
    }
    if harness.id().0 == "claude-code" {
        collect_config(
            &mut bundle,
            harness,
            project,
            Scope::Local,
            Slot::Settings,
            selected,
        )?;
    }
    if selected.contains(&Category::Memories) {
        let catalog = harness.memory_catalog(project)?;
        let auto = if matches!(harness.id().0, "claude-code" | "codex") {
            match memory_root(harness, project) {
                Ok(root) => Some(root.canonicalize().unwrap_or(root)),
                Err(error) => {
                    bundle
                        .warnings
                        .push(format!("Generated memory excluded: {error}"));
                    None
                }
            }
        } else {
            None
        };
        for file in catalog.files {
            let path = PathBuf::from(&file.path);
            if !file.editable {
                continue;
            }
            let (scope, area, root) = match file.source {
                MemorySource::ProjectAutoMemory | MemorySource::GlobalAutoMemory => {
                    let Some(root) = auto.as_ref() else {
                        continue;
                    };
                    if !path.starts_with(root) {
                        continue;
                    }
                    (
                        if harness.id().0 == "codex" {
                            Scope::User
                        } else {
                            Scope::Project
                        },
                        Area::AutoMemory,
                        root.clone(),
                    )
                }
                MemorySource::UserInstructions => {
                    let steering = harness.config_root().join("steering");
                    let steering = steering.canonicalize().unwrap_or(steering);
                    if harness.id().0 == "kiro" && path.starts_with(&steering) {
                        (Scope::User, Area::Steering, steering)
                    } else {
                        (
                            Scope::User,
                            Area::Instructions,
                            harness.config_root().to_path_buf(),
                        )
                    }
                }
                MemorySource::ProjectInstructions => {
                    let steering = project_config(harness, project).join("steering");
                    let steering = steering.canonicalize().unwrap_or(steering);
                    if harness.id().0 == "kiro" && path.starts_with(&steering) {
                        (Scope::Project, Area::Steering, steering)
                    } else {
                        (Scope::Project, Area::Instructions, project.to_path_buf())
                    }
                }
            };
            let root = root.canonicalize().unwrap_or(root);
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            bundle.add(
                harness.id().0,
                Category::Memories,
                scope,
                relative.clone(),
                Resource::File {
                    area,
                    path: relative,
                },
                file.content.into_bytes(),
            )?;
        }
        collect_memory_links(&mut bundle, harness, project)?;
    }
    if selected.contains(&Category::Skills) && harness.id().0 != "codex" {
        collect_discovered_skills(&mut bundle, harness, project)?;
    }
    let mut identities = HashSet::new();
    bundle.manifest.entries.retain(|entry| {
        identities.insert((
            entry.scope as u8,
            serde_json::to_string(&entry.resource).unwrap_or_default(),
            entry.checksum.clone(),
        ))
    });
    let used: HashSet<_> = bundle
        .manifest
        .entries
        .iter()
        .map(|entry| entry.blob.clone())
        .collect();
    bundle.data.retain(|key, _| used.contains(key));
    if bundle.data.values().map(Vec::len).sum::<usize>() as u64 > super::LIMIT {
        return Err("Selected resources exceed the archive size limit".into());
    }
    Ok(bundle)
}

fn collect_memory_links(
    bundle: &mut Bundle,
    harness: &impl Harness,
    project: &Path,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    let mut index = 0;
    while index < bundle.manifest.entries.len() {
        let entry = bundle.manifest.entries[index].clone();
        index += 1;
        if entry.category != Category::Memories {
            continue;
        }
        let Resource::File { area, path } = &entry.resource else {
            continue;
        };
        let root = file_root(harness, project, entry.scope, *area)?;
        let root = root.canonicalize().unwrap_or(root);
        let source = root.join(relative(path)?);
        if !seen.insert(source.clone()) {
            continue;
        }
        let text =
            std::str::from_utf8(&bundle.data[&entry.blob]).map_err(|_| "Memory is not UTF-8")?;
        for (link, _) in
            crate::harness::memory_links::parse_references(text, harness.id().0 == "claude-code")
        {
            let Some(path) = crate::harness::memory_links::resolve_reference(&source, &link) else {
                continue;
            };
            if !path.starts_with(&root)
                || path.extension().is_none_or(|ext| ext != "md")
                || seen.contains(&path)
                || !path.is_file()
            {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if bundle.manifest.entries.iter().any(|e| e.scope == entry.scope && matches!(&e.resource, Resource::File { area: other, path } if other == area && path == &rel)) { continue; }
            bundle.add(
                harness.id().0,
                Category::Memories,
                entry.scope,
                rel.clone(),
                Resource::File {
                    area: *area,
                    path: rel,
                },
                super::read_file(&path, super::FILE_LIMIT)?,
            )?;
        }
    }
    Ok(())
}

fn collect_discovered_skills(
    bundle: &mut Bundle,
    harness: &SelectedHarness,
    project: &Path,
) -> Result<(), String> {
    let home = harness
        .user_home()
        .and_then(|path| path.canonicalize().ok());
    let config = harness
        .config_root()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    for skill in harness.skills_inventory(project)? {
        let path = skill.path;
        let canonical = path.canonicalize().map_err(|e| e.to_string())?;
        let scope = if canonical.starts_with(project) {
            Scope::Project
        } else if canonical.starts_with(&config)
            || home
                .as_ref()
                .is_some_and(|home| canonical.starts_with(home))
        {
            Scope::User
        } else {
            Scope::Project
        };
        let native = skill_root(harness, project, scope);
        if native
            .canonicalize()
            .is_ok_and(|root| canonical.starts_with(root))
        {
            continue;
        }
        if path.file_name().is_some_and(|name| name == "SKILL.md") {
            let folder = path.parent().ok_or("Skill file has no directory")?;
            let prefix = format!(
                "{}/",
                folder
                    .file_name()
                    .ok_or("Skill folder has no name")?
                    .to_string_lossy()
            );
            add_tree(bundle, harness.id().0, folder, scope, Area::Skills, &prefix)?;
        } else {
            let name = path
                .file_name()
                .ok_or("Skill file has no name")?
                .to_string_lossy()
                .into_owned();
            bundle.add(
                harness.id().0,
                Category::Skills,
                scope,
                name.clone(),
                Resource::File {
                    area: Area::Skills,
                    path: name,
                },
                super::read_file(&path, super::FILE_LIMIT)?,
            )?;
        }
    }
    Ok(())
}

pub(super) fn notes(harness: &impl Harness) -> Vec<String> {
    let mut notes = vec![
        "Backups include the selected files and saved connection details. Login files, chats, caches, and previous Orbit backups are left out.".into(),
        "Saved file paths and hook commands stay as written. Import does not download packages or run scripts. Paths from another computer may need updating.".into(),
        "Settings and hooks come from your user setup and selected project. Parent-project settings are left out. Shared skills are copied into this agent's skills folder.".into(),
    ];
    if harness.id().0 == "codex" {
        notes.push("Codex generated memory stays user-wide. Codex still controls project trust and hook approval.".into());
    }
    notes
}

pub(crate) fn history_scope(
    harness: &impl Harness,
    project: &Path,
    target_path: &Path,
) -> Option<bool> {
    let canonical = |p: PathBuf| crate::history::canonical_location(&p);
    for scope in [Scope::User, Scope::Project, Scope::Local] {
        if scope == Scope::Local && harness.id().0 != "claude-code" {
            continue;
        }
        for slot in [Slot::Settings, Slot::Mcp, Slot::Hooks] {
            if config_path(harness, project, scope, &slot)
                .ok()
                .and_then(canonical)
                .is_some_and(|p| p == target_path)
            {
                return Some(scope == Scope::User);
            }
        }
        let native = if scope == Scope::User {
            harness.config_root().to_path_buf()
        } else {
            project_config(harness, project)
        };
        let roots = [
            skill_root(harness, project, scope),
            native.join("steering"),
            native.join("hooks"),
            native.join("plugins"),
            native.join("extensions"),
        ];
        for root in roots {
            if canonical(root).is_some_and(|root| target_path.starts_with(root)) {
                return Some(scope == Scope::User);
            }
        }
        let instructions = if scope == Scope::User {
            harness.config_root().to_path_buf()
        } else {
            project.to_path_buf()
        };
        if canonical(instructions).is_some_and(|root| target_path.starts_with(root))
            && target_path.extension().is_some_and(|ext| ext == "md")
        {
            return Some(scope == Scope::User);
        }
        if harness.id().0 == "kiro"
            && canonical(native.join("agents"))
                .is_some_and(|root| target_path.parent() == Some(root.as_path()))
            && target_path.extension().is_some_and(|ext| ext == "json")
        {
            return Some(scope == Scope::User);
        }
    }
    if memory_root(harness, project)
        .ok()
        .and_then(canonical)
        .is_some_and(|root| target_path.starts_with(root))
    {
        return Some(harness.id().0 == "codex");
    }
    None
}
