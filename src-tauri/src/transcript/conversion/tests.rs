use super::*;
use crate::harness::{
    claude::ClaudeCodeHarness, codex::CodexHarness, kiro::KiroHarness, opencode::OpenCodeHarness,
    pi::PiHarness, resolve_paths, Client,
};
use crate::transcript::ingest::list_sessions;

const IMAGE: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";

fn harness(client: Client, root: &Path) -> SelectedHarness {
    fs::create_dir_all(root).unwrap();
    match client {
        Client::ClaudeCode => SelectedHarness::Claude(ClaudeCodeHarness::new(resolve_paths(
            Some(root.to_str().unwrap()),
            root,
        ))),
        Client::Codex => SelectedHarness::Codex(CodexHarness::new(root.into(), root.into())),
        Client::Pi => SelectedHarness::Pi(PiHarness::new(root.into(), root.into())),
        Client::Opencode => {
            SelectedHarness::Opencode(OpenCodeHarness::new(root.into(), root.into()))
        }
        Client::Kiro => SelectedHarness::Kiro(KiroHarness::new(root.into())),
    }
}

fn fixture(cwd: &Path) -> Conversation {
    let mut chat = Conversation {
        cwd: native_path(cwd),
        title: "Inspection".into(),
        ..Default::default()
    };
    chat.message(
        "user",
        &json!([
            {"type":"text","text":"Please inspect café\nsecond line"},
            {"type":"image","mimeType":"image/png","data":IMAGE},
        ]),
    );
    chat.message(
        "assistant",
        &json!([{"type":"text","text":"The image is a pixel."}]),
    );
    chat
}

fn assert_conversation(chat: &Conversation) {
    assert_eq!(chat.turns.len(), 2);
    assert_eq!(chat.turns[0].role, Role::User);
    assert_eq!(chat.turns[1].role, Role::Assistant);
    assert!(
        matches!(&chat.turns[0].content[0], Content::Text(text) if text == "Please inspect café\nsecond line")
    );
    assert!(
        matches!(&chat.turns[0].content[1], Content::Image{mime,data} if mime == "image/png" && data == IMAGE)
    );
    assert!(
        matches!(&chat.turns[1].content[0], Content::Text(text) if text == "The image is a pixel.")
    );
}

#[test]
fn all_four_native_codecs_preserve_conversation_roles_images_and_text() {
    let temp = crate::test_support::tempdir().unwrap();
    for source in [
        Client::ClaudeCode,
        Client::Codex,
        Client::Pi,
        Client::Opencode,
    ] {
        let source = harness(source, &temp.path().join(Uuid::new_v4().to_string()));
        let first = temp.path().join(Uuid::new_v4().to_string());
        fs::create_dir(&first).unwrap();
        encode(&source, &fixture(temp.path()), &first).unwrap();
        let source_chat = decode(&source, &first).unwrap();
        for target in [
            Client::ClaudeCode,
            Client::Codex,
            Client::Pi,
            Client::Opencode,
        ] {
            let target = harness(target, &temp.path().join(Uuid::new_v4().to_string()));
            let next = temp.path().join(Uuid::new_v4().to_string());
            fs::create_dir(&next).unwrap();
            encode(&target, &source_chat, &next).unwrap();
            let converted = decode(&target, &next).unwrap();
            assert_conversation(&converted);
            assert_eq!(converted.title, "Inspection");
            assert_eq!(converted.cwd, native_path(temp.path()));
            if matches!(target, SelectedHarness::Codex(_)) {
                let records = read_lines(&next.join("rollout.jsonl")).unwrap();
                assert!(records[0]["payload"].get("base_instructions").is_none());
                assert!(records[0]["payload"].get("model_provider").is_none());
            }
        }
    }
}

#[test]
fn native_import_conversion_defaults_cwd_and_honors_override_without_changing_source() {
    let temp = crate::test_support::tempdir().unwrap();
    let project_a = temp.path().join("project-a");
    let project_b = temp.path().join("project-b");
    fs::create_dir(&project_a).unwrap();
    fs::create_dir(&project_b).unwrap();
    for from in [Client::ClaudeCode, Client::Codex, Client::Pi] {
        for to in [Client::ClaudeCode, Client::Codex, Client::Pi] {
            let pair = temp.path().join(Uuid::new_v4().to_string());
            let source = harness(from, &pair.join("source"));
            let target = harness(to, &pair.join("target"));
            if source.id() == target.id() {
                continue;
            }
            let app = pair.join("app");
            let seed = pair.join("seed");
            fs::create_dir(&seed).unwrap();
            encode(&source, &fixture(&project_a), &seed).unwrap();
            let original = source.transcript_import(&app, &seed, &project_a).unwrap();
            let before = preview(&source, &app, &original.session_id).unwrap();
            assert_eq!(
                before.working_directory,
                native_path(&project_a.canonicalize().unwrap())
            );
            let source_bytes = fs::read(&original.path).unwrap();
            let mut ids = HashSet::new();
            for override_cwd in [None, Some(project_b.as_path()), Some(project_b.as_path())] {
                let result =
                    convert(&source, &target, &app, &original.session_id, override_cwd).unwrap();
                assert!(ids.insert(result.session_id.clone()));
                let (_, restored) = snapshot(&target, &app, &result.session_id).unwrap();
                assert_conversation(&restored);
                let expected = override_cwd.unwrap_or(&project_a).canonicalize().unwrap();
                assert_eq!(restored.cwd, native_path(&expected));
                assert_eq!(result.working_directory, native_path(&expected));
            }
            assert_eq!(fs::read(&original.path).unwrap(), source_bytes);
            let sessions = list_sessions(&target.transcript_cache(&app)).unwrap();
            let titles: HashSet<_> = sessions.iter().filter_map(|s| s.title.as_deref()).collect();
            assert!(titles.contains("Inspection"));
            assert!(titles.contains("Inspection-1"));
            assert!(!fs::read_dir(&app).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("chat-conversion-")));
        }
    }
}

#[test]
fn branch_selection_excludes_abandoned_messages_and_detects_cycles() {
    let records = vec![
        json!({"type":"session","version":3,"cwd":"/source"}),
        json!({"type":"message","id":"a","parentId":null,"message":{"role":"user","content":"hello"}}),
        json!({"type":"message","id":"old","parentId":"a","message":{"role":"assistant","content":"abandoned"}}),
        json!({"type":"message","id":"new","parentId":"a","message":{"role":"assistant","content":"kept"}}),
        json!({"type":"session_info","id":"title","parentId":"new","name":"Branch"}),
    ];
    let chat = decode_pi(&records).unwrap();
    assert_eq!(chat.turns.len(), 2);
    assert!(matches!(&chat.turns[1].content[0], Content::Text(text) if text == "kept"));
    assert!(chat
        .notes
        .iter()
        .any(|note| note.contains("Alternate branches")));
    let bad = [
        json!({"id":"a","parentId":"b"}),
        json!({"id":"b","parentId":"a"}),
    ];
    assert!(
        branch(&bad, "id", "parentId", Some("a"), &mut BTreeSet::new())
            .unwrap_err()
            .contains("cycle")
    );
}

#[test]
fn claude_branch_tools_and_reasoning_are_not_confused_with_new_executable_calls() {
    let records = vec![
        json!({"type":"user","uuid":"u","parentUuid":null,"cwd":"/project","message":{"content":"question"}}),
        json!({"type":"assistant","uuid":"abandoned","parentUuid":"u","message":{"content":"old answer"}}),
        json!({"type":"assistant","uuid":"a","parentUuid":"u","message":{"content":[
            {"type":"thinking","thinking":"private reasoning","signature":"signed"},
            {"type":"tool_use","id":"call1","name":"Read","input":{"file_path":"src/main.rs"}},
            {"type":"text","text":"final answer"} ]}}),
        json!({"type":"user","uuid":"r","parentUuid":"a","message":{"content":[{"type":"tool_result","tool_use_id":"call1","content":"file contents"}]}}),
        json!({"type":"custom-title","customTitle":"Named"}),
    ];
    let chat = decode_claude(&records).unwrap();
    assert_eq!(chat.turns.len(), 3);
    let text = format!("{:?}", chat.turns);
    assert!(text.contains("Historical tool call"));
    assert!(text.contains("file contents"));
    assert!(text.contains("final answer"));
    assert!(!text.contains("private reasoning"));
    assert!(!text.contains("old answer"));
    assert_eq!(chat.cwd, "/project");
    assert_eq!(chat.title, "Named");
}

#[test]
fn codex_echoes_rollback_and_replacement_history_do_not_resurrect_messages() {
    let records = vec![
        json!({"type":"session_meta","payload":{"cwd":"/original"}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"keep"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"keep"}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}}),
        json!({"type":"event_msg","payload":{"type":"agent_message","message":"answer"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"discard"}]}}),
        json!({"type":"event_msg","payload":{"type":"thread_rolled_back","num_turns":1}}),
    ];
    let chat = decode_codex(&records).unwrap();
    assert_eq!(chat.turns.len(), 2);
    assert!(matches!(&chat.turns[0].content[0], Content::Text(text) if text=="keep"));
    let mut compacted = records;
    compacted.push(json!({"type":"compacted","payload":{"replacement_history":[{"type":"message","role":"user","content":[{"type":"input_text","text":"summary"}]}]}}));
    let chat = decode_codex(&compacted).unwrap();
    assert_eq!(chat.turns.len(), 1);
    assert!(matches!(&chat.turns[0].content[0], Content::Text(text) if text=="summary"));
}

#[test]
fn kiro_chunks_join_without_inventing_spaces_and_keep_native_cwd() {
    let chat = decode_kiro(&json!({"cwd":"/kiro","title":"Kiro"}), &[
        json!({"kind":"Prompt","data":{"message_id":"u","content":{"type":"text","text":"question"}}}),
            json!({"kind":"AssistantMessage","data":{"message_id":"a","content":{"kind":"Text","data":"hel"}}}),
            json!({"kind":"AssistantMessage","data":{"message_id":"a","content":{"kind":"Text","data":{"text":"lo"}}}}),
    ]).unwrap();
    assert_eq!(chat.turns.len(), 2);
    assert!(matches!(&chat.turns[1].content[0], Content::Text(text) if text=="hello"));
    assert_eq!(chat.cwd, "/kiro");
}

#[test]
fn conversion_rejects_unsupported_destination_before_creating_any_chat() {
    let temp = crate::test_support::tempdir().unwrap();
    let claude = harness(Client::ClaudeCode, &temp.path().join("claude"));
    let kiro = harness(Client::Kiro, &temp.path().join("kiro"));
    let app = temp.path().join("app");
    assert!(convert(&claude, &kiro, &app, "does-not-exist", None)
        .unwrap_err()
        .contains("verified transcript import"));
    assert!(convert(&claude, &claude, &app, "does-not-exist", None)
        .unwrap_err()
        .contains("different destination"));
    assert!(!app.exists());
}

fn isolated_command(executable: &std::ffi::OsStr, root: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(executable);
    command.current_dir(root);
    for name in [
        "CODEX_SQLITE_HOME",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "ANTHROPIC_API_KEY",
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_SERVER_PASSWORD",
        "OPENCODE_SERVER_USERNAME",
    ] {
        command.env_remove(name);
    }
    for (name, directory) in [
        ("HOME", "profile"),
        ("USERPROFILE", "profile"),
        ("APPDATA", "roaming"),
        ("LOCALAPPDATA", "local"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
        ("CODEX_HOME", "codex"),
        ("TMP", "temp"),
        ("TEMP", "temp"),
        ("TMPDIR", "temp"),
    ] {
        let path = root.join(directory);
        fs::create_dir_all(&path).unwrap();
        command.env(name, path);
    }
    command.env("OPENCODE_DB", root.join("opencode.sqlite"));
    command
}

#[test]
#[ignore = "requires an installed Codex CLI; starts only an isolated app-server, no model turn"]
fn native_codex_reads_and_resumes_converted_history_without_inference() {
    use crate::harness::process_lifetime::ChildTree;
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let temp = crate::test_support::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let target = harness(Client::Codex, &temp.path().join("codex"));
    let archive = temp.path().join("archive");
    fs::create_dir(&archive).unwrap();
    encode(&target, &fixture(&project), &archive).unwrap();
    let report = target
        .transcript_import(&temp.path().join("app"), &archive, &project)
        .unwrap();
    let mut command = isolated_command(std::ffi::OsStr::new("codex"), temp.path());
    command
        .args(["app-server", "--listen", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut server = ChildTree::spawn(&mut command).expect("Codex CLI must be installed");
    let stdout = server.child_mut().stdout.take().unwrap();
    let mut stdin = server.child_mut().stdin.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if sender.send(value).is_err() {
                    break;
                }
            }
        }
    });
    let mut request = |id: u64, method: &str, params: Value| {
        writeln!(
            stdin,
            "{}",
            json!({"id":id,"method":method,"params":params})
        )
        .unwrap();
        stdin.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let response = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("Native app-server response timed out");
            if response["id"] == id {
                assert!(response.get("error").is_none(), "{response}");
                break response["result"].clone();
            }
        }
    };
    request(
        1,
        "initialize",
        json!({"clientInfo":{"name":"orbit-test","version":"0.1.0"},"capabilities":{"experimentalApi":true}}),
    );
    let read = request(
        2,
        "thread/read",
        json!({"threadId":report.session_id,"includeTurns":true}),
    );
    let text = read.to_string();
    assert!(text.contains("Please inspect café"), "{read}");
    assert!(text.contains("The image is a pixel."), "{read}");
    assert!(
        text.contains(IMAGE),
        "native thread/read omitted the inline image"
    );
    let resumed = request(3, "thread/resume", json!({"threadId":report.session_id}));
    assert_eq!(resumed["thread"]["id"], report.session_id);
    assert!(resumed.to_string().contains("The image is a pixel."));
    server.cleanup();
    let backup = crate::transcript::deletion::delete(
        &target,
        &temp.path().join("app"),
        &report.session_id,
        &archive,
    )
    .unwrap();
    assert!(
        !Path::new(&report.path).exists(),
        "native delete must remove the rollout"
    );
    let restored = target
        .transcript_import(&temp.path().join("app"), Path::new(&backup.path), &project)
        .unwrap();
    assert_ne!(restored.session_id, report.session_id);
    assert_conversation(
        &snapshot(&target, &temp.path().join("app"), &restored.session_id)
            .unwrap()
            .1,
    );
}

#[test]
fn codex_same_text_in_different_turns_is_not_globally_deduplicated() {
    let chat = decode_codex(&[
        json!({"type":"session_meta","payload":{"cwd":"/project"}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"repeat"}}),
        json!({"type":"event_msg","payload":{"type":"task_started"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"repeat"}]}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"repeat"}}),
        json!({"type":"event_msg","payload":{"type":"thread_rolled_back","num_turns":1}}),
        json!({"type":"event_msg","payload":{"type":"task_started"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"repeat"}]}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"repeat","local_images":["/missing.png"]}}),
    ]).unwrap();
    assert_eq!(chat.turns.len(), 2);
    assert!(matches!(&chat.turns[0].content[0], Content::Text(text) if text == "repeat"));
    assert!(
        matches!(&chat.turns[1].content[1], Content::Text(text) if text.contains("/missing.png"))
    );
    assert!(chat
        .notes
        .iter()
        .any(|note| note.contains("External attachments")));
}

#[test]
fn opencode_partial_reverts_keep_only_the_confirmed_prefix() {
    let transfer = json!({"info":{"location":{"directory":"/project"},"revert":{"messageID":"a","partID":"cut"}},"messages":[
        {"id":"u","type":"user","text":"question"},
        {"id":"a","type":"assistant","content":[{"type":"text","text":"keep"},{"id":"cut","type":"tool","name":"discard"},{"type":"text","text":"discard too"}]},
        {"id":"u2","type":"user","text":"abandoned"}
    ]});
    let chat = decode_opencode(&transfer).unwrap();
    assert_eq!(chat.turns.len(), 2);
    assert!(matches!(&chat.turns[1].content[0], Content::Text(text) if text == "keep"));
    assert_eq!(chat.turns[1].content.len(), 1);
    let mut invalid = transfer;
    invalid["info"]["revert"]["partID"] = json!("absent");
    assert!(decode_opencode(&invalid)
        .unwrap_err()
        .contains("partial revert boundary"));
}

#[test]
#[ignore = "requires installed OpenCode v2 CLI; uses disposable native storage, no model prompt"]
fn native_opencode_accepts_converted_history_and_inline_images() {
    use std::process::Stdio;
    let temp = crate::test_support::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let target = harness(Client::Opencode, &temp.path().join("opencode-config"));
    let archive = temp.path().join("archive");
    fs::create_dir(&archive).unwrap();
    encode(&target, &fixture(&project), &archive).unwrap();
    let input = read_json(&archive.join("session.json")).unwrap();
    let executable = crate::transcript::opencode::native_executable().unwrap();
    let output = isolated_command(executable.as_os_str(), temp.path())
        .args(["session", "import"])
        .arg(archive.join("session.json"))
        .args(["--standalone", "--directory"])
        .arg(&project)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = isolated_command(executable.as_os_str(), temp.path())
        .args([
            "session",
            "export",
            input["info"]["id"].as_str().unwrap(),
            "--standalone",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output: Value =
        serde_json::from_slice(&output.stdout).expect("Native OpenCode session JSON");
    assert_conversation(&decode_opencode(&output).unwrap());
    assert!(output["info"].get("model").is_none());
    assert_eq!(output["messages"][1]["model"]["providerID"], "orbit");
}

#[test]
#[ignore = "requires installed Pi CLI; resumes disposable converted sessions without a model prompt"]
fn native_pi_resumes_converted_history_and_inline_images_without_inference() {
    use crate::harness::process_lifetime::ChildTree;
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let temp = crate::test_support::tempdir().unwrap();
    let original_project = temp.path().join("original-project");
    let chosen_project = temp.path().join("chosen-project");
    fs::create_dir(&original_project).unwrap();
    fs::create_dir(&chosen_project).unwrap();
    let source = harness(Client::ClaudeCode, &temp.path().join("claude"));
    let target = harness(Client::Pi, &temp.path().join("pi-agent"));
    let app = temp.path().join("app");
    let archive = temp.path().join("archive");
    fs::create_dir(&archive).unwrap();
    encode(&source, &fixture(&original_project), &archive).unwrap();
    let original = source
        .transcript_import(&app, &archive, &original_project)
        .unwrap();
    let source_bytes = fs::read(&original.path).unwrap();

    for override_cwd in [None, Some(chosen_project.as_path())] {
        let converted =
            convert(&source, &target, &app, &original.session_id, override_cwd).unwrap();
        let connection = open_database(&target.transcript_cache(&app)).unwrap();
        let path: String = connection
            .query_row(
                "SELECT source_path FROM sessions WHERE session_id = ?1",
                [&converted.session_id],
                |row| row.get(0),
            )
            .unwrap();
        let project = override_cwd
            .unwrap_or(&original_project)
            .canonicalize()
            .unwrap();
        let executable = if cfg!(windows) { "pi.cmd" } else { "pi" };
        let mut command = isolated_command(std::ffi::OsStr::new(executable), temp.path());
        command
            .current_dir(&project)
            .env("PI_CODING_AGENT_DIR", temp.path().join("pi-agent"))
            .env_remove("PI_CODING_AGENT_SESSION_DIR")
            .args([
                "--offline",
                "--mode",
                "rpc",
                "--session",
                &path,
                "--no-approve",
                "--no-extensions",
                "--no-mcp",
                "--no-tools",
                "--no-skills",
                "--no-context-files",
                "--no-prompt-templates",
                "--no-themes",
            ])
            .arg("--session-dir")
            .arg(Path::new(&path).parent().unwrap())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut server = ChildTree::spawn(&mut command).expect("Pi CLI must be installed");
        let stdout = server.child_mut().stdout.take().unwrap();
        let mut stdin = server.child_mut().stdin.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let value: Value = serde_json::from_str(&line).expect("Pi RPC JSON record");
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let mut request = |id: &str, mut payload: Value| {
            payload["id"] = json!(id);
            writeln!(stdin, "{payload}").unwrap();
            stdin.flush().unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let response = receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .expect("Pi RPC response timed out");
                assert!(
                    !matches!(
                        response["type"].as_str(),
                        Some("agent_start" | "message_start")
                    ),
                    "Unexpected model activity: {response}"
                );
                if response["id"] == id {
                    assert_eq!(response["success"], true, "{response}");
                    break response["data"].clone();
                }
            }
        };
        let state = request("state", json!({"type":"get_state"}));
        assert_eq!(
            state["sessionId"],
            converted.session_id.trim_start_matches("pi:")
        );
        assert_eq!(state["sessionName"], "Inspection");
        assert_eq!(
            Path::new(state["sessionFile"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            Path::new(&path).canonicalize().unwrap()
        );
        assert_eq!(state["isStreaming"], false);
        assert_eq!(state["messageCount"], 2);
        assert_ne!(state["model"]["provider"], "orbit");
        let messages = request("messages", json!({"type":"get_messages"}));
        let messages = messages["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(
            messages[0]["content"][0]["text"],
            "Please inspect café\nsecond line"
        );
        assert_eq!(messages[0]["content"][1]["mimeType"], "image/png");
        assert_eq!(messages[0]["content"][1]["data"], IMAGE);
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["text"], "The image is a pixel.");
        let html = temp.path().join("converted.html");
        let exported = request("export", json!({"type":"export_html","outputPath":html}));
        assert_eq!(Path::new(exported["path"].as_str().unwrap()), html);
        let html = fs::read_to_string(html).unwrap();
        let payload = html
            .split_once("<script id=\"session-data\" type=\"application/json\">")
            .unwrap()
            .1
            .split_once("</script>")
            .unwrap()
            .0
            .trim();
        let exported: Value = serde_json::from_slice(&STANDARD.decode(payload).unwrap()).unwrap();
        assert_eq!(
            Path::new(exported["header"]["cwd"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            project
        );
        assert_eq!(
            exported["entries"][0]["message"]["content"][0]["text"],
            "Please inspect café\nsecond line"
        );
        assert_eq!(
            exported["entries"][0]["message"]["content"][1]["data"],
            IMAGE
        );
        server.cleanup();
    }
    assert_eq!(fs::read(&original.path).unwrap(), source_bytes);
}
