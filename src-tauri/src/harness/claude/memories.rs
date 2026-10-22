use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ClaudeCodeHarness;
use crate::{
    harness::{atomic, Harness, ResourceKind},
    ledger,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCatalog {
    pub project_root: String,
    pub groups: Vec<MemoryGroup>,
    pub files: Vec<MemoryFile>,
    pub excluded_custom_memory_directory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGroup {
    pub id: String,
    pub label: String,
    pub role: MemorySource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MemorySource {
    ProjectAutoMemory,
    ProjectInstructions,
    UserInstructions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFile {
    pub id: String,
    pub group_id: String,
    pub name: String,
    pub path: String,
    pub source: MemorySource,
    pub role: MemoryRole,
    pub load_behavior: String,
    pub editable: bool,
    pub content: String,
    pub references: Vec<MemoryReference>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MemoryRole {
    Index,
    Topic,
    Instruction,
    Rule,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReference {
    pub target: String,
    pub kind: ReferenceKind,
    pub status: ReferenceStatus,
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum ReferenceKind {
    MarkdownLink,
    Import,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReferenceStatus {
    Known,
    External,
    Unresolved,
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    group_id: String,
    source: MemorySource,
    role: MemoryRole,
}

pub fn catalog(harness: &ClaudeCodeHarness, project: &Path) -> Result<MemoryCatalog, String> {
    let project = canonical_plain_dir(project)?;
    let config_root = canonical_plain_dir_optional(harness.config_root())?;
    let mut groups = BTreeMap::<String, MemoryGroup>::new();
    let mut candidates = BTreeMap::<PathBuf, Candidate>::new();
    let mut excluded_custom_memory_directory = None;

    // Auto memory is project-scoped, even though Claude Code stores each
    // project's directory under its config root. Slugs are labels only.
    if let Some(config_root) = &config_root {
        let projects = config_root.join("projects");
        if plain_dir(&projects) {
            for project_entry in safe_entries(&projects)? {
                let metadata =
                    std::fs::symlink_metadata(&project_entry).map_err(|error| error.to_string())?;
                if !metadata.is_dir() || atomic::is_link_or_reparse(&metadata) {
                    continue;
                }
                let slug = project_entry
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let memory_dir = project_entry.join("memory");
                if !plain_dir(&memory_dir) {
                    continue;
                }
                let group_id = format!("auto:{slug}");
                let label = format!("Project auto memory · {slug}");
                let group = MemoryGroup {
                    id: group_id.clone(),
                    label: label.clone(),
                    role: MemorySource::ProjectAutoMemory,
                };
                groups.insert(group_id.clone(), group);
                collect_markdown(
                    &memory_dir,
                    &memory_dir,
                    &group_id,
                    MemorySource::ProjectAutoMemory,
                    &mut candidates,
                )?;
            }
        }
    }

    let user_group = "user-instructions".to_owned();
    add_group(
        &mut groups,
        &user_group,
        "User instructions",
        MemorySource::UserInstructions,
    );
    if let Some(user_root) = &config_root {
        add_if_plain(
            &user_root.join("CLAUDE.md"),
            user_root,
            &user_group,
            MemorySource::UserInstructions,
            MemoryRole::Instruction,
            &mut candidates,
        )?;
        collect_markdown_optional(
            &user_root.join("rules"),
            user_root,
            &user_group,
            MemorySource::UserInstructions,
            &mut candidates,
        )?;
    }

    let project_group = "selected-project".to_owned();
    let project_label = format!("Selected project · {}", project.display());
    add_group(
        &mut groups,
        &project_group,
        &project_label,
        MemorySource::ProjectInstructions,
    );
    for name in ["CLAUDE.md", "CLAUDE.local.md"] {
        add_if_plain(
            &project.join(name),
            &project,
            &project_group,
            MemorySource::ProjectInstructions,
            MemoryRole::Instruction,
            &mut candidates,
        )?;
    }
    let claude_dir = project.join(".claude");
    for name in ["CLAUDE.md", "CLAUDE.local.md"] {
        add_if_plain(
            &claude_dir.join(name),
            &project,
            &project_group,
            MemorySource::ProjectInstructions,
            MemoryRole::Instruction,
            &mut candidates,
        )?;
    }
    collect_markdown_optional(
        &claude_dir.join("rules"),
        &project,
        &project_group,
        MemorySource::ProjectInstructions,
        &mut candidates,
    )?;

    // Custom auto-memory paths are only read when they resolve inside the
    // already-authorized project. A setting never grants access by itself.
    if let Some(configured) = configured_memory_directory(harness, &project) {
        match canonical_plain_dir(&configured) {
            Ok(custom) if custom.starts_with(&project) => {
                let group = "project-auto-memory-custom";
                add_group(
                    &mut groups,
                    group,
                    "Project auto memory · custom directory",
                    MemorySource::ProjectAutoMemory,
                );
                collect_markdown(
                    &custom,
                    &custom,
                    group,
                    MemorySource::ProjectAutoMemory,
                    &mut candidates,
                )?;
            }
            _ => excluded_custom_memory_directory = Some(configured.to_string_lossy().into_owned()),
        }
    }
    let mut files = Vec::new();
    for candidate in candidates.values() {
        let content = read_plain(&candidate.path)?;
        let id = file_id(&candidate.path);
        let name = candidate
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let load_behavior = match (candidate.source, candidate.role) {
            (MemorySource::ProjectAutoMemory, MemoryRole::Index) => "Claude Code loads the first 200 lines or 25 KB at startup; topic files are on demand".to_owned(),
            (MemorySource::ProjectAutoMemory, _) => "Topic file; referenced or read on demand, not auto-loaded at startup".to_owned(),
            (_, MemoryRole::Rule) => "Instruction rule; loaded by Claude Code when its path or frontmatter applies".to_owned(),
            _ => "Instruction file; included in Claude Code project/user context subject to its normal limits".to_owned(),
        };
        files.push(MemoryFile {
            id,
            group_id: candidate.group_id.clone(),
            name,
            path: candidate.path.to_string_lossy().into_owned(),
            source: candidate.source,
            role: candidate.role,
            load_behavior,
            editable: true,
            content,
            references: Vec::new(),
        });
    }
    files.sort_by(|a, b| {
        a.group_id
            .cmp(&b.group_id)
            .then_with(|| role_order(a.role).cmp(&role_order(b.role)))
            .then(a.path.cmp(&b.path))
    });
    let lookup: BTreeMap<PathBuf, String> = candidates
        .keys()
        .map(|path| (path.clone(), file_id(path)))
        .collect();
    for file in &mut files {
        let source_path = PathBuf::from(&file.path);
        for (target, kind) in parse_references(&file.content) {
            let resolved = resolve_reference(&source_path, &target);
            let known = resolved.as_ref().and_then(|path| lookup.get(path));
            let mut allowed_roots = vec![project.as_path()];
            if let Some(config_root) = &config_root {
                allowed_roots.push(config_root.as_path());
            }
            let external = is_external_reference(&target, resolved.as_deref(), &allowed_roots);
            let (status, file_id) = if let Some(id) = known {
                (ReferenceStatus::Known, Some(id.clone()))
            } else if external {
                (ReferenceStatus::External, None)
            } else {
                (ReferenceStatus::Unresolved, None)
            };
            file.references.push(MemoryReference {
                target,
                kind,
                status,
                file_id,
            });
        }
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(MemoryCatalog {
        project_root: project.to_string_lossy().into_owned(),
        groups,
        files,
        excluded_custom_memory_directory,
    })
}

pub fn edit(
    harness: &ClaudeCodeHarness,
    project: &Path,
    id: &str,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    let project = canonical_plain_dir(project)?;
    let catalog = catalog(harness, &project)?;
    let file = catalog
        .files
        .iter()
        .find(|file| file.id == id && file.editable)
        .ok_or("memory file is no longer available for editing")?;
    let path = PathBuf::from(&file.path);
    let allowed_config = canonical_plain_dir_optional(harness.config_root())?;
    if !path.starts_with(&project)
        && !allowed_config
            .as_ref()
            .is_some_and(|root| path.starts_with(root))
    {
        return Err("memory file is outside the authorized memory roots".into());
    }
    ensure_plain_file(&path)?;
    let backup = ledger::backup_path(harness.config_root(), id, "edit");
    let expected = Sha256::digest(expected_content.as_bytes()).into();
    let kind = match file.source {
        MemorySource::ProjectAutoMemory => ResourceKind::Memory,
        MemorySource::ProjectInstructions => ResourceKind::ProjectInstructions,
        MemorySource::UserInstructions => ResourceKind::GlobalInstructions,
    };
    ledger::write(harness.config_root(), kind, id, "edit", &path,
        atomic::RecordedWrite::Restore(expected, content.as_bytes()), Some(&backup))
        .map(|_| ())
        .map_err(|error| {
            let message = error.to_string();
            if message.contains("did not hold the expected state") {
                "Memory file changed since this editor opened. Refresh the memory catalog and try again.".to_owned()
            } else { message }
        })
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

fn add_if_plain(
    path: &Path,
    boundary: &Path,
    group: &str,
    source: MemorySource,
    role: MemoryRole,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    if path.exists() {
        if has_link_component(path)? {
            return Ok(());
        }
        ensure_plain_file(path)?;
        let canonical = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
        if !canonical.starts_with(boundary) {
            return Err("instruction file escaped its authorized root".into());
        }
        out.insert(
            canonical.clone(),
            Candidate {
                path: canonical,
                group_id: group.to_owned(),
                source,
                role,
            },
        );
    }
    Ok(())
}

fn collect_markdown_optional(
    root: &Path,
    boundary: &Path,
    group: &str,
    source: MemorySource,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    if plain_dir(root) {
        collect_markdown(root, boundary, group, source, out)?;
    }
    Ok(())
}

fn collect_markdown(
    root: &Path,
    boundary: &Path,
    group: &str,
    source: MemorySource,
    out: &mut BTreeMap<PathBuf, Candidate>,
) -> Result<(), String> {
    for path in safe_entries(root)? {
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if atomic::is_link_or_reparse(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                continue;
            }
            collect_markdown(&path, boundary, group, source, out)?;
        } else if metadata.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            let canonical = std::fs::canonicalize(&path).map_err(|error| error.to_string())?;
            if !canonical.starts_with(boundary) {
                continue;
            }
            let role = if source == MemorySource::ProjectAutoMemory
                && path
                    .file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case("MEMORY.md"))
            {
                MemoryRole::Index
            } else if source == MemorySource::ProjectAutoMemory {
                MemoryRole::Topic
            } else {
                MemoryRole::Rule
            };
            out.insert(
                canonical.clone(),
                Candidate {
                    path: canonical,
                    group_id: group.to_owned(),
                    source,
                    role,
                },
            );
        }
    }
    Ok(())
}

fn safe_entries(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|error| error.to_string())? {
        entries.push(entry.map_err(|error| error.to_string())?.path());
    }
    entries.sort();
    Ok(entries)
}

fn plain_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.is_dir() && !atomic::is_link_or_reparse(&meta))
}

fn canonical_plain_dir(path: &Path) -> Result<PathBuf, String> {
    ensure_plain_components(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || atomic::is_link_or_reparse(&metadata) {
        return Err(format!("expected a plain directory: {}", path.display()));
    }
    std::fs::canonicalize(path).map_err(|error| error.to_string())
}

fn canonical_plain_dir_optional(path: &Path) -> Result<Option<PathBuf>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => canonical_plain_dir(path).map(Some),
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
    if has_link_component(path)? {
        return Err(format!("refusing linked path: {}", path.display()));
    }
    Ok(())
}

fn has_link_component(path: &Path) -> Result<bool, String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                current.push(prefix.as_os_str());
                continue;
            }
            Component::RootDir => {
                current.push(component.as_os_str());
                continue;
            }
            Component::Normal(name) => current.push(name),
            Component::CurDir => continue,
            Component::ParentDir => return Err("parent traversal is not allowed".into()),
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if atomic::is_link_or_reparse(&metadata) => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(false)
}

fn read_plain(path: &Path) -> Result<String, String> {
    ensure_plain_file(path)?;
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn file_id(path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    format!("memory-{}", crate::harness::atomic::hex(&digest))
}

pub(crate) fn configured_memory_directory(
    harness: &ClaudeCodeHarness,
    project: &Path,
) -> Option<PathBuf> {
    let snapshot = super::config::load_config(harness, project).ok()?;
    let value = snapshot
        .effective
        .get("autoMemoryDirectory")?
        .value
        .as_str()?
        .trim();
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        Some(PathBuf::from(home).join(rest))
    } else {
        Some(PathBuf::from(value))
    }
}

fn is_within_any(path: &Path, roots: &[&Path]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

fn role_order(role: MemoryRole) -> u8 {
    match role {
        MemoryRole::Index => 0,
        MemoryRole::Topic => 1,
        MemoryRole::Instruction => 2,
        MemoryRole::Rule => 3,
    }
}

fn is_external_reference(target: &str, resolved: Option<&Path>, roots: &[&Path]) -> bool {
    let target = target.trim();
    target.starts_with("http:")
        || target.starts_with("https:")
        || target.starts_with("mailto:")
        || resolved.is_some_and(|path| !is_within_any(path, roots))
}

fn resolve_reference(source: &Path, target: &str) -> Option<PathBuf> {
    let raw = target
        .trim()
        .trim_matches('<')
        .trim_matches('>')
        .split('#')
        .next()?
        .split('?')
        .next()?;
    if raw.is_empty()
        || raw.starts_with("http:")
        || raw.starts_with("https:")
        || raw.starts_with("mailto:")
    {
        return None;
    }
    let candidate = if raw.starts_with("~/") {
        return Some(PathBuf::from(raw));
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        source.parent()?.join(raw)
    };
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
        }
    }
    Some(normalized)
}

fn parse_references(content: &str) -> Vec<(String, ReferenceKind)> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut result = BTreeSet::new();
    let mut code_block = false;
    for (event, range) in Parser::new(content).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_block = true,
            Event::End(TagEnd::CodeBlock) => code_block = false,
            Event::Start(Tag::Link { dest_url, .. }) => {
                result.insert((dest_url.into_string(), ReferenceKind::MarkdownLink));
            }
            Event::Text(_) if !code_block => {
                for target in context_imports(&content[range]) {
                    result.insert((target, ReferenceKind::Import));
                }
            }
            _ => {}
        }
    }
    result.into_iter().collect()
}

fn context_imports(text: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for (offset, character) in text.char_indices() {
        if character != '@'
            || text[..offset]
                .chars()
                .next_back()
                .is_some_and(|previous| !previous.is_whitespace() && !matches!(previous, '(' | '['))
        {
            continue;
        }
        let mut remaining = text[offset + 1..].chars().peekable();
        if remaining
            .peek()
            .is_some_and(|character| matches!(character, '\'' | '"'))
        {
            continue;
        }
        let mut target = String::new();
        while let Some(character) = remaining.next() {
            if character == '\\' && remaining.peek().is_some_and(|next| next.is_whitespace()) {
                target.push(remaining.next().unwrap());
            } else if character.is_whitespace()
                || matches!(character, ')' | ']' | ',' | ';' | '\'' | '"' | '<' | '>')
            {
                break;
            } else {
                target.push(character);
            }
        }
        let target = target.trim_end_matches('.');
        if !target.is_empty() {
            imports.push(target.to_owned());
        }
    }
    imports
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_distinguish_imports_from_links_and_ignore_code_and_quoted_paths() {
        let content = "[topic][ref]\n\n[ref]: topic.md\n\n@folder/space\\ name.md @other.md `@inline.md` email@example.md @\"quoted.md\"\n\n~~~md\n@fenced.md [hidden](hidden.md)\n~~~\n\n    @indented.md\n";
        assert_eq!(
            parse_references(content),
            vec![
                ("folder/space name.md".to_owned(), ReferenceKind::Import),
                ("other.md".to_owned(), ReferenceKind::Import),
                ("topic.md".to_owned(), ReferenceKind::MarkdownLink),
            ]
        );
    }
    use crate::harness::HarnessPaths;

    fn fixture() -> (tempfile::TempDir, ClaudeCodeHarness, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("claude");
        let project = temp.path().join("project");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(project.join(".claude/rules")).unwrap();
        let harness = ClaudeCodeHarness::new(HarnessPaths {
            config_root: config,
            claude_json: temp.path().join("claude.json"),
        });
        (temp, harness, project)
    }

    #[cfg(any(unix, windows))]
    fn link_directory(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let link = link.to_string_lossy().replace('/', "\\");
            let target = target.to_string_lossy().replace('/', "\\");
            let result = std::process::Command::new("cmd")
                .args(["/D", "/C", "mklink", "/J", &link, &target])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    fn catalog_groups_project_auto_memory_and_resolves_only_known_edges() {
        let (_temp, harness, project) = fixture();
        let memory = harness.config_root().join("projects/slug-project/memory");
        std::fs::create_dir_all(&memory).unwrap();
        std::fs::write(
            memory.join("MEMORY.md"),
            "See [topic](topic.md) and @topic.md\n[away](../../../../outside.md)",
        )
        .unwrap();
        std::fs::write(memory.join("topic.md"), "topic").unwrap();
        std::fs::write(project.join("CLAUDE.md"), "project instruction").unwrap();
        let catalog = catalog(&harness, &project).unwrap();
        let index = catalog
            .files
            .iter()
            .find(|f| f.name == "MEMORY.md")
            .unwrap();
        assert_eq!(index.source, MemorySource::ProjectAutoMemory);
        assert_eq!(index.role, MemoryRole::Index);
        assert!(
            index
                .references
                .iter()
                .filter(|r| r.status == ReferenceStatus::Known)
                .count()
                >= 2
        );
        assert!(index
            .references
            .iter()
            .any(|r| r.status == ReferenceStatus::External));
        assert!(catalog
            .groups
            .iter()
            .any(|group| group.label.contains("slug-project")));
        let group_files: Vec<_> = catalog
            .files
            .iter()
            .filter(|file| file.group_id == index.group_id)
            .collect();
        assert_eq!(group_files.first().unwrap().role, MemoryRole::Index);
    }

    #[test]
    fn catalog_keeps_nested_topics_separates_project_groups_and_skips_poisoned_entries() {
        let (_temp, harness, project) = fixture();
        let first = harness.config_root().join("projects/slug-one/memory");
        let second = harness.config_root().join("projects/slug-two/memory");
        std::fs::create_dir_all(first.join("topics/deep")).unwrap();
        std::fs::create_dir_all(first.join("poison.md")).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("MEMORY.md"), "index").unwrap();
        std::fs::write(first.join("topics/deep/nested.md"), "nested").unwrap();
        std::fs::write(first.join("poison.md/hidden.md"), "must not appear").unwrap();
        std::fs::write(first.join("notes.txt"), "not markdown").unwrap();
        std::fs::write(second.join("MEMORY.md"), "other project").unwrap();
        let catalog = catalog(&harness, &project).unwrap();
        let groups: BTreeSet<_> = catalog
            .files
            .iter()
            .filter(|file| file.source == MemorySource::ProjectAutoMemory)
            .map(|file| file.group_id.as_str())
            .collect();
        assert!(groups.contains("auto:slug-one"));
        assert!(groups.contains("auto:slug-two"));
        assert!(catalog.files.iter().any(|file| file.name == "nested.md"));
        assert!(!catalog.files.iter().any(|file| file.name == "hidden.md"));
        assert!(!catalog.files.iter().any(|file| file.name == "notes.txt"));
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn catalog_does_not_follow_project_memory_or_nested_directory_links() {
        let (_temp, harness, project) = fixture();
        let projects = harness.config_root().join("projects");
        let outside = harness.config_root().join("outside");
        std::fs::create_dir_all(outside.join("memory")).unwrap();
        std::fs::write(outside.join("memory/foreign.md"), "foreign").unwrap();

        let nested_group = projects.join("safe-project/memory");
        std::fs::create_dir_all(&nested_group).unwrap();
        link_directory(&outside.join("memory"), &nested_group.join("linked"));
        let project_link = projects.join("linked-project");
        link_directory(&outside, &project_link);

        let catalog = catalog(&harness, &project).unwrap();
        assert!(!catalog.files.iter().any(|file| file.name == "foreign.md"));
        assert!(!catalog
            .files
            .iter()
            .any(|file| file.path.contains("linked")));
    }

    #[test]
    fn edit_uses_stale_guard_backup_and_refuses_a_reference_as_a_path() {
        let (_temp, harness, project) = fixture();
        let file = project.join("CLAUDE.md");
        std::fs::write(&file, "before").unwrap();
        let catalog = catalog(&harness, &project).unwrap();
        let id = catalog
            .files
            .iter()
            .find(|entry| Path::new(&entry.path) == file.canonicalize().unwrap())
            .unwrap()
            .id
            .clone();
        edit(&harness, &project, &id, "before", "after").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "after");
        let rows = ledger::read(harness.config_root()).unwrap();
        assert_eq!(rows.last().unwrap().action, "edit");
        assert!(edit(&harness, &project, &id, "before", "stale").is_err());
        assert!(edit(&harness, &project, "../../outside", "before", "bad").is_err());
    }

    #[test]
    fn custom_directory_outside_authorized_project_is_not_discovered() {
        let (_temp, harness, project) = fixture();
        let outside = project.parent().unwrap().join("outside-memory");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("topic.md"), "outside").unwrap();
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(
            project.join(".claude/settings.json"),
            format!(
                "{{\"autoMemoryDirectory\": {:?}}}",
                outside.to_string_lossy()
            ),
        )
        .unwrap();
        let catalog = catalog(&harness, &project).unwrap();
        assert!(catalog.excluded_custom_memory_directory.is_some());
        assert!(!catalog.files.iter().any(|file| file.name == "topic.md"));
    }

    #[test]
    fn missing_user_config_root_leaves_project_instructions_available() {
        let (_temp, harness, project) = fixture();
        std::fs::remove_dir_all(harness.config_root()).unwrap();
        std::fs::write(project.join("CLAUDE.md"), "project guidance").unwrap();
        let catalog = catalog(&harness, &project).unwrap();
        assert!(catalog.files.iter().any(
            |file| file.name == "CLAUDE.md" && file.source == MemorySource::ProjectInstructions
        ));
        assert!(catalog
            .files
            .iter()
            .all(|file| file.source != MemorySource::UserInstructions));
    }
}
