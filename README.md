<div align="center">
  <img src="assets/orbit-readme.svg" alt="Orbit. Your coding agent setup, in one place." width="900" />
  <br />
  <a href="https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml"><img src="https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-6750A4.svg" alt="Apache License 2.0" /></a>
</div>

<br />

Orbit puts Claude Code, Codex, Kiro, OpenCode v2, and Pi tools and context in one desktop app. Manage skills, memories, MCP servers, hooks, settings, and transcripts without hunting through configuration files.

Choose the coding agent in the header. Claude Code is selected initially. Orbit is a local desktop app and keeps each agent's configuration, backups, and transcript index separate.

## What you can manage

| | Area | What you can do |
|:--:|---|---|
| <img src="assets/icons/skills.svg" alt="" width="24" /> | **Skills** | Browse and edit skills, control visibility, install or remove folders, and discover skills copied in outside Orbit. |
| | **Memories** | Edit or delete owned memories and instructions, follow links, and find incoming references. Browse Codex memory sections grouped by project hints, with a whole-file editor available. |
| <img src="assets/icons/mcp.svg" alt="" width="24" /> | **MCP servers** | Manage local stdio and remote HTTP servers, then test their connections. |
| <img src="assets/icons/hooks.svg" alt="" width="24" /> | **Hooks** | Inspect user, project, and inherited hooks. Edit supported command hooks and test commands with sample JSON input. |
| <img src="assets/icons/settings.svg" alt="" width="24" /> | **Settings** | Inspect configuration layers, edit one key at a time, and undo a recorded change when the file has not changed since. |
| <img src="assets/icons/transcripts.svg" alt="" width="24" /> | **Transcripts** | Index sessions, search messages, follow conversation branches, and export or import native chat archives to a chosen project. Markdown export is also available. |
| | **History** | Browse saved versions separately from operation logs, restore a version, or delete a saved backup. |

Support follows each agent's native formats:

| Area | Claude Code | Codex | Kiro | OpenCode v2 | Pi |
|---|---|---|---|---|---|
| Skills | User skills and visibility controls | Native user/project/plugin discovery, enabled state | User/project skills | Native and shared user/project skills, including Markdown skills | Native user/project folders and Markdown skills; shared and configured paths read-only |
| Memories and instructions | Project auto-memory and Claude instructions | Shared memory Markdown and scoped AGENTS instructions | User/project steering and AGENTS instructions | AGENTS and configured local instruction files | AGENTS/CLAUDE instruction hierarchy and system prompt files |
| MCP | User servers, local and HTTP transports | User servers, local and HTTP transports | User servers, local and HTTP transports | User servers in the v2 `mcp.servers` format | User servers, local and streamable HTTP transports |
| Hooks | Command hooks, enable/disable, test | Command hooks and test; approval in Codex | V2 agent hooks and V3 hooks; Markdown profiles read-only | Native JavaScript plugins; configure their list in Settings | Native JavaScript/TypeScript extensions; configure their list in Settings |
| Settings | User/project/local | User/trusted project TOML | User/project CLI JSON | User/project JSON or JSONC, preserving comments | User/project JSON; nested values merge and resource lists combine |
| Transcripts | Search and native archive import/export | Search and native archive import/export | V2 ACP search and source backup export | Index selected projects, search, native CLI archive import/export | Index selected projects, search branches, native v3 JSONL archive import/export |
| History | Versions, restore, backup deletion, separate logs | Same | Same for supported local resources | Same for supported local resources | Same for supported local resources |

Kiro's learned memories belong to its account service. OpenCode and Pi have no built-in generated-memory file catalog. Kiro, OpenCode, and Pi have no native per-skill enable switch, so Orbit offers editing and removal with restorable backups. Inherited skills and instructions outside the selected project are read-only. MCP editing currently covers user configuration; project and agent overrides can change the running session.

Kiro V3, terminal, and cloud transcripts are not indexed. V2 ACP source backups preserve native files but cannot be imported through Orbit. Kiro's interactive `/chat save` and `/chat load` remain its supported archive workflow. OpenCode plugin code is managed by OpenCode rather than the command-hook builder.

For OpenCode, choose a project on the Transcripts page before indexing. Chats from previously indexed projects remain searchable. Imports use the selected destination project and add a numeric suffix when a chat title already exists. Archives preserve the native session payload; external files referenced by a chat are not bundled.

Pi also indexes the selected project's chats and retains previously indexed projects for search. Its archives preserve native tree records, unknown entry fields, and inline images. Import assigns a fresh session ID, changes the working directory, preserves entry links, and suffixes duplicate titles. Session paths follow Pi's environment override, project/user `sessionDir`, or default project directory. Explicit CLI session-directory overrides are outside Orbit's settings preview.

Pi project trust, package expansion, and resource filters determine what Pi actually loads. Orbit discovers local skill files and literal configured paths; it does not evaluate package or glob rules. Extension code, trust decisions, and authentication state stay with Pi. MCP tests refuse OAuth, dynamic environment/command references, and relative working directories that need Pi's runtime context.

## Requirements

- Node.js 22
- The stable Rust toolchain
- Platform prerequisites for [Tauri v2](https://v2.tauri.app/start/prerequisites/)
- Codex CLI on `PATH` for Codex skill discovery. Native skill discovery and imported chat resume are verified against Codex 0.163.0.
- OpenCode v2 CLI on `PATH` for its transcript features. Native format checks target OpenCode v2.1.3; v1 is not supported.
- Kiro native file adapters target CLI 2.32.0, including V2 and opt-in V3 hook formats.
- Pi adapters target the [v1.4.0 native formats](https://github.com/earendil-works/pi/tree/v1.4.0/packages/coding-agent/docs), including v3 sessions. Fixture checks pass locally; live Pi acceptance remains unverified because Pi is not installed in this environment.

On Ubuntu, Tauri's native build dependencies include WebKitGTK 4.1, GTK 3, and Ayatana AppIndicator. Install the packages before building:

```sh
sudo apt-get update
sudo apt-get install --yes build-essential curl wget file \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev libxdo-dev \
  libssl-dev librsvg2-dev patchelf
```

Windows builds use WebView2 and a native compiler matching your default Rust toolchain. For GNU, use `stable-x86_64-pc-windows-gnu` with MinGW-w64 GCC and G++ on `PATH`. For MSVC, use Microsoft C++ Build Tools. Windows CI uses MSVC.

macOS builds require Xcode Command Line Tools.

## Run locally

```sh
npm ci
npm run tauri dev
```

Orbit reads Claude Code's normal configuration directories and honors `CLAUDE_CONFIG_DIR`. For Codex, it honors `CODEX_HOME`, which defaults to `~/.codex`. Select a project folder to inspect that project's instructions, skills, and settings. The launch directory is the initial project.

For Kiro, Orbit honors `KIRO_HOME`, defaulting to `~/.kiro`, and reads project files under `.kiro`. For OpenCode v2, user configuration lives under `XDG_CONFIG_HOME/opencode`, defaulting to `~/.config/opencode`; project configuration uses `opencode.json[c]` and `.opencode/opencode.json[c]`. The settings preview covers local files rather than remote or runtime plugin configuration. Configured instruction URLs and globs are not expanded by Orbit's file editor.

For Pi, Orbit honors `PI_CODING_AGENT_DIR`, defaulting to `~/.pi/agent`, and reads project settings under `.pi`. Instruction discovery follows Pi's one-file-per-directory precedence from filesystem ancestors to the selected project. Project `SYSTEM.md` replaces the global system prompt; `APPEND_SYSTEM.md` adds instructions. The skill installer accepts folders; standalone skills copied into native skill directories are also discovered and editable.

Codex settings remain TOML, and Orbit preserves unrelated keys and comments. The effective settings view previews user and trusted project files. Profiles, command-line options, environment variables, and system policy can change Codex's runtime settings. Codex skill discovery uses the installed CLI's native inventory, including plugin skills and folders copied outside Orbit. Plugin and system skill files are read-only.

Codex instructions follow `AGENTS.override.md`, `AGENTS.md`, and configured fallback precedence through the repository hierarchy. Ancestor instructions are read-only until their project is selected. Orbit edits Markdown under `CODEX_HOME/memories`, including the memory index, summary, raw inputs, and supporting notes, with stale-content checks and restorable versions in History. Structured files offer section navigation grouped by explicit project details; shared or unclear sections stay visible. Section edits share one file draft, and the whole-file editor remains available. These groups help navigation without changing Codex's shared memory scope. Git metadata and linked paths are excluded. Codex can regenerate these files and replace manual edits; editing a file does not reset its memory generation state. Codex hook trust stays with its native `/hooks` review flow.

The header and navigation stay visible while pages scroll. Long lists, editors, messages, and configuration values scroll within bounded panels. Memory details, connections, server diagnostics, hook output, and skill backups expand when needed; editing dialogs fit the window.

Orbit writes configuration changes directly to disk. Backups and change history are kept under `.harness-agent` in each agent's configuration directory. Claude's disabled MCP and hook records and the transcript indexes are stored in the app-data directory. Codex uses native enablement settings. Window preferences are stored in the app-config directory.

Credential values in known configuration fields are masked in the UI, not encrypted on disk. Skills, configuration files, backups, and transcripts can contain private information. Review transcript exports before sharing them. Hook tests execute shell commands with your account's permissions. MCP tests start configured processes or contact configured endpoints and may send their credentials. These operations are not sandboxed.

## Checks

Run the frontend and Rust checks locally:

```sh
npm ci
npm run build
npm test
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

The OpenCode native archive test requires an installed v2 CLI. It uses temporary configuration and storage, without model calls:

```sh
cargo test --manifest-path src-tauri/Cargo.toml native_v2_cli_accepts_isolated_api_fixture_and_roundtrips_archive_collision --lib -- --ignored --nocapture
```

GitHub Actions runs these checks on Ubuntu, macOS, and Windows for pushes and pull requests. The Windows workflow also builds the NSIS installer with:

```sh
npm run tauri build
```

The installer is written under `src-tauri/target/release/bundle/nsis`.

## License

Orbit is licensed under the [Apache License 2.0](LICENSE).
