use super::resources::Slot;
use super::*;
use crate::harness::{
    claude::ClaudeCodeHarness, codex::CodexHarness, kiro::KiroHarness, opencode::OpenCodeHarness,
    pi::PiHarness, resolve_paths,
};
use serde_json::{json, Value};

struct Fixture {
    _dir: tempfile::TempDir,
    harness: SelectedHarness,
    project: PathBuf,
    archive: PathBuf,
}

impl Fixture {
    fn new(id: &str) -> Self {
        let dir = crate::test_support::tempdir().unwrap();
        let home = dir.path().join("home");
        let config = home.join(format!(".{id}"));
        let project = dir.path().join("project");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();
        let project = project.canonicalize().unwrap();
        let harness = match id {
            "claude-code" => SelectedHarness::Claude(ClaudeCodeHarness::new(resolve_paths(
                Some(config.to_str().unwrap()),
                &home,
            ))),
            "codex" => SelectedHarness::Codex(CodexHarness::new(config, home)),
            "kiro" => SelectedHarness::Kiro(KiroHarness::new(config)),
            "opencode" => SelectedHarness::Opencode(OpenCodeHarness::new(config, home)),
            "pi" => SelectedHarness::Pi(PiHarness::new(config, home)),
            _ => unreachable!(),
        };
        Self {
            archive: dir.path().join("bundle.zip"),
            _dir: dir,
            harness,
            project,
        }
    }

    fn write(&self, path: &Path, bytes: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn export(&self, categories: &[Category]) {
        export(&self.harness, &self.project, categories, &self.archive).unwrap();
    }

    fn preview(&self) -> Preview {
        preview(
            &self.harness,
            &self.project,
            &self.archive,
            Destination::Original,
        )
        .unwrap()
    }

    fn apply(&self, preview: &Preview, ids: &[String], replace: bool) -> Result<Report, String> {
        let choices = preview
            .entries
            .iter()
            .filter(|e| ids.contains(&e.id))
            .map(|e| Choice {
                id: e.id.clone(),
                replace,
                expected: e.expected.clone(),
            })
            .collect::<Vec<_>>();
        apply(
            &self.harness,
            &self.project,
            &self.archive,
            &preview.checksum,
            Destination::Original,
            &choices,
        )
    }

    fn settings(&self, text: &str) {
        self.write(&self.harness.settings_path(), text);
    }
}

#[test]
fn skills_round_trip_binary_assets_and_history_for_every_harness() {
    for id in ["claude-code", "codex", "kiro", "opencode", "pi"] {
        let f = Fixture::new(id);
        let root = if id == "codex" {
            f.harness.user_home().unwrap().join(".agents/skills")
        } else {
            f.harness.config_root().join("skills")
        };
        let skill = root.join("example/SKILL.md");
        let asset = root.join("example/assets/example.bin");
        f.write(
            &skill,
            "---\nname: example\ndescription: Example skill\n---\nOriginal\n",
        );
        f.write(&asset, [0, 255, 128, 7]);
        f.export(&[Category::Skills]);
        let (bundle, _) = load(&f.harness, &f.archive).unwrap();
        assert_eq!(bundle.manifest.harness, id);
        assert_eq!(bundle.manifest.entries.len(), 2);
        assert!(bundle
            .manifest
            .entries
            .iter()
            .all(|e| e.category == Category::Skills));
        f.write(&skill, "Edited outside Orbit");
        fs::remove_file(&asset).unwrap();
        let preview = f.preview();
        let conflict = preview
            .entries
            .iter()
            .find(|e| e.status == Status::Conflict)
            .unwrap();
        let ids = preview
            .entries
            .iter()
            .map(|e| e.id.clone())
            .collect::<Vec<_>>();
        let report = f.apply(&preview, &ids, false).unwrap();
        assert_eq!((report.files, report.skipped), (1, 1));
        assert_eq!(fs::read(&asset).unwrap(), [0, 255, 128, 7]);
        let preview = f.preview();
        assert_eq!(
            f.apply(&preview, std::slice::from_ref(&conflict.id), true)
                .unwrap()
                .files,
            1
        );
        assert!(fs::read_to_string(&skill).unwrap().contains("Original"));
        let history = crate::history::load(&f.harness, &f.project).unwrap();
        let version = history
            .versions
            .iter()
            .find(|v| v.target.file_name() == Some(std::ffi::OsStr::new("SKILL.md")))
            .unwrap();
        assert!(version.restorable, "{id}");
        crate::history::restore(
            &f.harness,
            &f.project,
            &version.id,
            version.current_checksum.as_deref(),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&skill).unwrap(), "Edited outside Orbit");
        let unchanged = f.preview();
        assert_eq!(f.apply(&unchanged, &ids, true).unwrap().files, 1);
        let unchanged = f.preview();
        let report = f.apply(&unchanged, &ids, true).unwrap();
        assert_eq!((report.files, report.skipped), (0, 2));
    }
}

#[test]
fn settings_replacement_preserves_unchecked_hooks_and_servers() {
    for id in ["claude-code", "codex", "kiro", "opencode", "pi"] {
        let f = Fixture::new(id);
        let (source, target) = if id == "codex" {
            ("model = 'source'\n[mcp_servers.docs]\ncommand = 'original'\n[hooks]\nStop = []\n",
             "# keep this comment\nmodel = 'target'\nextra = true\n[mcp_servers.docs]\ncommand = 'local'\n[hooks]\nStop = ['keep']\n")
        } else if id == "opencode" {
            (r#"{"model":"source","mcp":{"servers":{"docs":{"type":"local","command":["original"]}}},"plugins":["original"]}"#,
             "{\n// keep this comment\n\"model\":\"target\",\"extra\":true,\"mcp\":{\"servers\":{\"docs\":{\"type\":\"local\",\"command\":[\"local\"]}}},\"plugins\":[\"keep\"]\n}")
        } else {
            (r#"{"model":"source","mcpServers":{"docs":{"command":"original"}},"hooks":{"Stop":[]},"extensions":["original"]}"#,
             "{\n// keep this comment\n\"model\":\"target\",\"extra\":true,\"mcpServers\":{\"docs\":{\"command\":\"local\"}},\"hooks\":{\"Stop\":[\"keep\"]},\"extensions\":[\"keep\"]\n}")
        };
        f.settings(source);
        f.export(&[Category::Settings]);
        let (bundle, _) = load(&f.harness, &f.archive).unwrap();
        assert_eq!(bundle.manifest.entries.len(), 1);
        let archived =
            resources::document(bundle.data.values().next().unwrap(), id, &Slot::Settings).unwrap();
        for &key in resources::managed_keys(id) {
            assert!(archived.get(key).is_none(), "{id} exported {key}");
        }
        f.settings(target);
        let preview = f.preview();
        f.apply(&preview, &[preview.entries[0].id.clone()], true)
            .unwrap();
        let bytes = fs::read(f.harness.settings_path()).unwrap();
        let saved = resources::document(&bytes, id, &Slot::Settings).unwrap();
        assert_eq!(saved["model"], "source", "{id}");
        assert!(saved.get("extra").is_none());
        let before = resources::document(target.as_bytes(), id, &Slot::Settings).unwrap();
        for &key in resources::managed_keys(id) {
            assert_eq!(saved.get(key), before.get(key), "{id} {key}");
        }
        assert!(
            String::from_utf8_lossy(&bytes).contains("keep this comment"),
            "{id}"
        );
    }
}

#[test]
fn selective_mcp_import_preserves_settings_other_servers_and_comments() {
    let f = Fixture::new("codex");
    f.settings("model = 'source'\n[mcp_servers.docs]\ncommand = 'exported'\n[mcp_servers.unselected]\ncommand = 'never-import'\n");
    f.export(&[Category::Mcps]);
    f.settings("# keep\nmodel = 'local'\n[mcp_servers.docs]\ncommand = 'local'\n[mcp_servers.other]\n# keep server comment\ncommand = 'other'\n");
    let preview = f.preview();
    let docs = preview
        .entries
        .iter()
        .find(|e| e.label.starts_with("docs"))
        .unwrap();
    f.apply(&preview, std::slice::from_ref(&docs.id), true)
        .unwrap();
    let bytes = fs::read(f.harness.settings_path()).unwrap();
    let value = resources::document(&bytes, "codex", &Slot::Settings).unwrap();
    assert_eq!(value["model"], "local");
    assert_eq!(value["mcp_servers"]["docs"]["command"], "exported");
    assert_eq!(value["mcp_servers"]["other"]["command"], "other");
    assert!(value["mcp_servers"].get("unselected").is_none());
    assert!(String::from_utf8_lossy(&bytes).contains("# keep server comment"));
}

#[test]
fn hooks_merge_without_duplicates_and_settings_can_share_the_write() {
    let f = Fixture::new("claude-code");
    f.settings(r#"{"model":"source","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo source"}]}]}}"#);
    f.export(&[Category::Hooks, Category::Settings]);
    f.settings(r#"{"model":"target","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo target"}]}]}}"#);
    let preview = f.preview();
    let ids = preview
        .entries
        .iter()
        .map(|e| e.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(f.apply(&preview, &ids, true).unwrap().files, 1);
    let value: Value =
        serde_json::from_slice(&fs::read(f.harness.settings_path()).unwrap()).unwrap();
    assert_eq!(value["model"], "source");
    assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 2);
    let preview = f.preview();
    assert_eq!(f.apply(&preview, &ids, true).unwrap().files, 0);
}

#[test]
fn stale_preview_preflights_every_selection_before_writing() {
    let f = Fixture::new("claude-code");
    let a = f.harness.config_root().join("skills/a/SKILL.md");
    let b = f.harness.config_root().join("skills/b/SKILL.md");
    f.write(&a, "source a");
    f.write(&b, "source b");
    f.export(&[Category::Skills]);
    f.write(&a, "local a");
    f.write(&b, "local b");
    let preview = f.preview();
    f.write(&b, "changed after preview");
    let ids = preview
        .entries
        .iter()
        .map(|e| e.id.clone())
        .collect::<Vec<_>>();
    assert!(f
        .apply(&preview, &ids, true)
        .unwrap_err()
        .contains("changed after preview"));
    assert_eq!(fs::read_to_string(&a).unwrap(), "local a");
    assert!(crate::history::load(&f.harness, &f.project)
        .unwrap()
        .logs
        .is_empty());
}

fn rewrite_archive(f: &Fixture, mutate: impl FnOnce(&mut Value, &mut BTreeMap<String, Vec<u8>>)) {
    let (bundle, _) = load(&f.harness, &f.archive).unwrap();
    let mut metadata = serde_json::to_value(bundle.manifest).unwrap();
    let mut data = bundle.data;
    mutate(&mut metadata, &mut data);
    let mut zip = ZipWriter::new(fs::File::create(&f.archive).unwrap());
    let options = SimpleFileOptions::default();
    zip.start_file("metadata.json", options).unwrap();
    zip.write_all(&serde_json::to_vec(&metadata).unwrap())
        .unwrap();
    for (name, bytes) in data {
        zip.start_file(name, options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn untrusted_archives_are_rejected_before_any_destination_write() {
    for invalid in [
        "harness", "version", "checksum", "path", "category", "extra", "missing",
    ] {
        let f = Fixture::new("claude-code");
        f.write(
            &f.harness.config_root().join("skills/example/SKILL.md"),
            "hello",
        );
        f.export(&[Category::Skills]);
        rewrite_archive(&f, |metadata, data| match invalid {
            "harness" => metadata["harness"] = json!("codex"),
            "version" => metadata["version"] = json!(99),
            "checksum" => {
                data.values_mut().next().unwrap().push(42);
            }
            "path" => metadata["entries"][0]["resource"]["path"] = json!("../../outside.md"),
            "category" => metadata["entries"][0]["category"] = json!("memories"),
            "extra" => {
                data.insert("unlisted.txt".into(), vec![]);
            }
            "missing" => {
                data.clear();
            }
            _ => unreachable!(),
        });
        let error = preview(&f.harness, &f.project, &f.archive, Destination::Original)
            .err()
            .unwrap();
        if invalid == "harness" {
            assert!(error.contains("Cross-harness import is planned for a future version"));
        }
        assert!(crate::history::load(&f.harness, &f.project)
            .unwrap()
            .logs
            .is_empty());
    }
}

#[test]
fn archive_changes_and_scope_collisions_require_a_new_choice() {
    let f = Fixture::new("claude-code");
    f.settings(r#"{"model":"user"}"#);
    f.write(
        &f.project.join(".claude/settings.json"),
        r#"{"model":"project"}"#,
    );
    f.export(&[Category::Settings]);
    f.write(
        &f.project.join(".claude/settings.json"),
        r#"{"model":"local"}"#,
    );
    let p = preview(&f.harness, &f.project, &f.archive, Destination::Project).unwrap();
    let choices = p
        .entries
        .iter()
        .map(|e| Choice {
            id: e.id.clone(),
            replace: true,
            expected: e.expected.clone(),
        })
        .collect::<Vec<_>>();
    assert!(apply(
        &f.harness,
        &f.project,
        &f.archive,
        &p.checksum,
        Destination::Project,
        &choices
    )
    .err()
    .unwrap()
    .contains("same destination"));
    assert_eq!(
        fs::read_to_string(f.project.join(".claude/settings.json")).unwrap(),
        r#"{"model":"local"}"#
    );
    rewrite_archive(&f, |metadata, _| {
        metadata["orbitVersion"] = json!("different")
    });
    assert!(apply(
        &f.harness,
        &f.project,
        &f.archive,
        &p.checksum,
        Destination::Project,
        &choices
    )
    .err()
    .unwrap()
    .contains("Archive changed"));
}

#[test]
fn instructions_and_project_auto_memory_follow_the_destination_project() {
    let f = Fixture::new("claude-code");
    f.write(&f.project.join("CLAUDE.md"), "@notes.md\n");
    f.write(&f.project.join("notes.md"), "Linked instructions\n");
    let key = resources::project_memory_key(&f.project).unwrap();
    f.write(
        &f.harness
            .config_root()
            .join("projects")
            .join(key)
            .join("memory/MEMORY.md"),
        "Project memory\n",
    );
    f.export(&[Category::Memories]);
    let target = f._dir.path().join("other-project");
    fs::create_dir_all(&target).unwrap();
    let target = target.canonicalize().unwrap();
    let p = preview(&f.harness, &target, &f.archive, Destination::Project).unwrap();
    assert_eq!(p.entries.len(), 3);
    let choices = p
        .entries
        .iter()
        .map(|e| Choice {
            id: e.id.clone(),
            replace: false,
            expected: e.expected.clone(),
        })
        .collect::<Vec<_>>();
    apply(
        &f.harness,
        &target,
        &f.archive,
        &p.checksum,
        Destination::Project,
        &choices,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(target.join("CLAUDE.md")).unwrap(),
        "@notes.md\n"
    );
    assert_eq!(
        fs::read_to_string(target.join("notes.md")).unwrap(),
        "Linked instructions\n"
    );
    let key = resources::project_memory_key(&target).unwrap();
    assert_eq!(
        fs::read_to_string(
            f.harness
                .config_root()
                .join("projects")
                .join(key)
                .join("memory/MEMORY.md")
        )
        .unwrap(),
        "Project memory\n"
    );
}

#[test]
fn native_configuration_and_memories_import_into_another_home_for_every_harness() {
    for id in ["claude-code", "codex", "kiro", "opencode", "pi"] {
        let source = Fixture::new(id);
        let config = source.harness.config_root();
        let main = match id {
            "codex" => "model = 'portable'\n[mcp_servers.docs]\ncommand = 'docs-server'\n[hooks]\nStop = [{hooks = [{type = 'command', command = 'echo portable'}]}]\n",
            "opencode" => r#"{"model":"portable","mcp":{"servers":{"docs":{"type":"local","command":["docs-server"]}}},"plugins":["example-plugin"]}"#,
            "pi" => r#"{"model":"portable","extensions":["./extensions/example.ts"]}"#,
            "claude-code" => r#"{"model":"portable","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo portable"}]}]}}"#,
            _ => r#"{"model":"portable"}"#,
        };
        source.settings(main);
        if matches!(id, "claude-code" | "kiro" | "pi") {
            source.write(
                &source.harness.mcp_path(),
                r#"{"mcpServers":{"docs":{"command":"docs-server"}}}"#,
            );
        }
        if id == "kiro" {
            source.write(
                &config.join("agents/reviewer.json"),
                r#"{"name":"reviewer","hooks":{"agentSpawn":[{"command":"echo portable"}]}}"#,
            );
            source.write(
                &config.join("steering/example.md"),
                "Portable instructions\n",
            );
        } else {
            source.write(
                &config.join(if id == "claude-code" {
                    "CLAUDE.md"
                } else {
                    "AGENTS.md"
                }),
                "Portable instructions\n",
            );
        }
        source.export(&[
            Category::Hooks,
            Category::Mcps,
            Category::Memories,
            Category::Settings,
        ]);
        let destination = Fixture::new(id);
        fs::copy(&source.archive, &destination.archive).unwrap();
        let preview = destination.preview();
        for category in [
            Category::Hooks,
            Category::Mcps,
            Category::Memories,
            Category::Settings,
        ] {
            assert!(
                preview.entries.iter().any(|e| e.category == category),
                "{id} missing {category:?}"
            );
        }
        assert!(
            preview.entries.iter().all(|e| e.status == Status::Add),
            "{id}"
        );
        let ids = preview
            .entries
            .iter()
            .map(|e| e.id.clone())
            .collect::<Vec<_>>();
        destination.apply(&preview, &ids, false).unwrap();
        let bytes = fs::read(destination.harness.settings_path()).unwrap();
        let main = resources::document(&bytes, id, &Slot::Settings).unwrap();
        assert_eq!(main["model"], "portable", "{id}");
        assert_eq!(
            destination.harness.mcp_list(&destination.project).unwrap()[0].name,
            "docs",
            "{id}"
        );
        assert!(
            destination
                .harness
                .memory_catalog(&destination.project)
                .unwrap()
                .files
                .iter()
                .any(|m| m.content == "Portable instructions\n"),
            "{id}"
        );
        let repeated = destination.preview();
        assert!(
            repeated
                .entries
                .iter()
                .all(|e| e.status == Status::Identical),
            "{id}"
        );
    }
}

#[test]
fn malformed_zip_entries_are_rejected_without_extracting_them() {
    for name in [
        "../outside.txt",
        "/absolute.txt",
        "C:/drive.txt",
        "unsafe/../",
        "oversized.txt",
        "symlink",
    ] {
        let f = Fixture::new("claude-code");
        let mut zip = ZipWriter::new(fs::File::create(&f.archive).unwrap());
        if name == "symlink" {
            zip.add_symlink(name, "../outside", SimpleFileOptions::default())
                .unwrap();
        } else if name.ends_with('/') {
            zip.add_directory(name, SimpleFileOptions::default())
                .unwrap();
        } else {
            zip.start_file(
                name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            if name == "oversized.txt" {
                zip.write_all(&vec![0; FILE_LIMIT as usize + 1]).unwrap();
            }
        }
        zip.finish().unwrap();
        let error = preview(&f.harness, &f.project, &f.archive, Destination::Original)
            .err()
            .unwrap();
        assert!(
            error.contains("unsafe relative path")
                || error.contains("special file")
                || error.contains("size limit"),
            "{name}: {error}"
        );
        assert!(crate::history::load(&f.harness, &f.project)
            .unwrap()
            .logs
            .is_empty());
    }
}
