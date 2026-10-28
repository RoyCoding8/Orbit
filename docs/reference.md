# Agent support and development reference

[Back to Orbit](../README.md)

## Agent support

Support follows each agent's native formats:

| Area | Claude Code | Codex | Kiro | OpenCode v2 | Pi |
|---|---|---|---|---|---|
| Skills | User skills and visibility controls | Native user/project/plugin discovery, enabled state | User/project skills | Native and shared user/project skills, including Markdown skills | Native user/project folders and Markdown skills; shared and configured paths read-only |
| Memories and instructions | Project auto-memory and Claude instructions | Shared memory Markdown and scoped AGENTS instructions | User/project steering and AGENTS instructions | AGENTS and configured local instruction files | AGENTS/CLAUDE instruction hierarchy and system prompt files |
| MCP | User servers, local and HTTP transports | User servers, local and HTTP transports | User servers, local and HTTP transports | User servers in the v2 `mcp.servers` format | User servers, local and streamable HTTP transports |
| Hooks | Command hooks, enable/disable, test | Command hooks and test; approval in Codex | V2 agent hooks and V3 hooks; Markdown profiles read-only | Native JavaScript plugins; configure their list in Settings | Native JavaScript/TypeScript extensions; configure their list in Settings |
| Settings | User/project/local | User/trusted project TOML | User/project CLI JSON | User/project JSON or JSONC, preserving comments | User/project JSON; nested values merge and resource lists combine |
| Chats | Project groups, search, native archives, recoverable deletion | Project groups, search, native archives, recoverable deletion | Project groups, V2 ACP search and source backup export | Discover known projects, search, native archives, childless-chat deletion | Discover default folders, include custom project directories, search branches, native archives, recoverable deletion |
| Chat conversion | Source and destination | Source and destination | V2 ACP source only | Source and destination through native v2 CLI | Source and destination for v3 JSONL sessions |
| History | Versions, restore, backup deletion, separate logs | Same | Same for supported local resources | Same for supported local resources | Same for supported local resources |

Kiro's learned memories belong to its account service. OpenCode and Pi have no built-in generated-memory file catalog. Kiro, OpenCode, and Pi have no native per-skill enable switch, so Orbit offers editing and removal with restorable backups. Inherited skills and instructions outside the selected project are read-only. MCP editing currently covers user configuration; project and agent overrides can change the running session.

Kiro V3, terminal, and cloud transcripts are not indexed. V2 ACP source backups preserve native files but cannot be imported through Orbit. Kiro's interactive `/chat save` and `/chat load` remain its supported archive workflow. OpenCode plugin code is managed by OpenCode rather than the command-hook builder.

The Chats page discovers native sessions when opened and groups them by their recorded working directory. Project groups collapse independently inside a bounded list. The page shows its configured source and reports unreadable files; **Refresh chats** scans it again. OpenCode v2 enumerates its known projects and follows native session-page cursors, including child chats. Imports use a chosen destination project and add a numeric suffix when a chat title already exists. Archives preserve the native session payload; external files referenced by a chat are not bundled.

Pi discovers chats across its default session folders and the selected project's configured session directory. Choose a project and refresh to include a local custom directory; unknown custom directories cannot be discovered automatically. Previously indexed custom projects remain visible. Archives preserve native tree records, unknown entry fields, and inline images. Import assigns a fresh session ID, changes the working directory, preserves entry links, and suffixes duplicate titles. Session paths follow Pi's environment override, project/user sessionDir, or default project directory. Explicit CLI session-directory overrides are outside Orbit's settings preview.

Deleting a supported chat first saves a native archive in a folder you choose. Restore it with **Import archive**, choosing its destination project. Close the native chat before deleting it. Kiro deletion is unavailable because its source backups cannot be imported. OpenCode parents with children are refused because native deletion cascades to children and a single-session archive cannot restore that hierarchy.

To convert a chat, select it in Chats and choose **Convert chat**. Choose the destination agent and optionally change the working directory. Leaving it unchanged uses the directory recorded in the source chat, rather than Orbit's launch folder. Conversion creates a separate native chat and leaves the source intact. Duplicate destination titles receive numeric suffixes.

Conversion preserves conversation text and inline images on the latest active branch. Historical tool calls and results become text; they are not replayed as tools. Signed reasoning, source system/developer instructions, and runtime state do not transfer. External attachments remain references, while available Codex image archive assets are embedded. Alternate branches stay in the source. The dialog reports conversion details, and unresolved partial OpenCode revert boundaries must be resolved in OpenCode first. Kiro has no verified transcript creation API and cannot be a destination. Use native archives when you need to preserve an agent's full original session structure.

Pi project trust, package expansion, and resource filters determine what Pi actually loads. Orbit discovers local skill files and literal configured paths; it does not evaluate package or glob rules. Extension code, trust decisions, and authentication state stay with Pi. MCP tests refuse OAuth, dynamic environment/command references, and relative working directories that need Pi's runtime context.

## Setup export and import

To save a setup, choose **Export setup** in the header. Choose a project, check the categories you want, then choose **Save backup**. **Select all** includes every available category.

To use a saved setup:

1. Select the same agent the backup came from, then choose **Import setup** and open the ZIP file. Importing into a different agent is planned for a future version.
2. Choose **Current project** or **User-wide**. Use **Change project** to choose another folder. You can also keep user-wide and project items separate using **Import destination**. Claude auto-memory stays attached to the chosen project; Codex generated memory stays user-wide.
3. Review the items, check what you want to import, and choose **Import selected**. Each category expands to show individual items and where they will be saved.

| Item | What happens on import |
|---|---|
| Already up to date | Skipped automatically. |
| Different version exists | Kept unless you choose **Replace** for that item. |
| Skills and memories | Selected files are added or replaced. Other destination files stay in place. |
| Hooks and extension lists | Missing entries are added without duplicates. |
| Settings | Preferences are replaced. Current hooks and MCP servers are preserved unless you select those too. |

Replaced files can be restored from **History**. After importing, refresh the open page when you are ready to load the changes; unsaved edits stay open.

Backups can contain passwords and API keys. Share them only with people you trust. Saved file paths and hook commands stay as written, so paths from another computer may need updating. Import does not install packages or run scripts.

<details>
<summary>Backup format and limits</summary>

The ZIP contains `metadata.json` identifying the agent and archive version, plus checksummed resource files. Login files, chats, caches, and previous Orbit backups are excluded.

Settings and hooks include user and selected-project sources; inherited parent-project configuration is excluded. Shared skills are copied into the destination agent's native skills folder. Codex skill exports cover native user/project folders, including the older user skill directory, rather than installed plugin packages. Kiro Markdown agent hook profiles are excluded. Memory exports include referenced local Markdown inside the same authorized memory or instruction root. External Markdown links and non-Markdown attachments are not bundled. Project trust and hook approval remain controlled by the agent.

Unrelated JSONC and TOML values and comments remain in place. The preview blocks selected items that would write the same destination file or setting. Import rechecks the backup and destination before writing and records each applied file in History. An interrupted import can leave earlier files applied and recorded there.

Backups are limited to 10,000 items, 16 MiB per file, and 128 MiB overall. Unsafe paths, links, missing files, and checksum mismatches are rejected.

</details>

## Configuration and memory files

Orbit reads Claude Code's normal configuration directories and honors `CLAUDE_CONFIG_DIR`. For Codex, it honors `CODEX_HOME`, which defaults to `~/.codex`. Select a project folder to inspect that project's instructions, skills, and settings. The launch directory is the initial project.

For Kiro, Orbit honors `KIRO_HOME`, defaulting to `~/.kiro`, and reads project files under `.kiro`. For OpenCode v2, user configuration lives under `XDG_CONFIG_HOME/opencode`, defaulting to `~/.config/opencode`; project configuration uses `opencode.json[c]` and `.opencode/opencode.json[c]`. The settings preview covers local files rather than remote or runtime plugin configuration. Configured instruction URLs and globs are not expanded by Orbit's file editor.

For Pi, Orbit honors `PI_CODING_AGENT_DIR`, defaulting to `~/.pi/agent`, and reads project settings under `.pi`. Instruction discovery follows Pi's one-file-per-directory precedence from filesystem ancestors to the selected project. Project `SYSTEM.md` replaces the global system prompt; `APPEND_SYSTEM.md` adds instructions. The skill installer accepts folders; standalone skills copied into native skill directories are also discovered and editable.

Codex settings remain TOML, and Orbit preserves unrelated keys and comments. The effective settings view previews user and trusted project files. Profiles, command-line options, environment variables, and system policy can change Codex's runtime settings. Codex skill discovery uses the installed CLI's native inventory, including plugin skills and folders copied outside Orbit. Plugin and system skill files are read-only.

Codex instructions follow `AGENTS.override.md`, `AGENTS.md`, and configured fallback precedence through the repository hierarchy. Ancestor instructions are read-only until their project is selected. Orbit edits Markdown under `CODEX_HOME/memories`, including the memory index, summary, raw inputs, and supporting notes, with stale-content checks and restorable versions in History. Structured files offer section navigation grouped by explicit project details; shared or unclear sections stay visible. Section edits share one file draft, and the whole-file editor remains available. These groups help navigation without changing Codex's shared memory scope. Git metadata and linked paths are excluded. Codex can regenerate these files and replace manual edits; editing a file does not reset its memory generation state. Codex hook trust stays with its native `/hooks` review flow.

## Stored data and command execution

Orbit writes configuration changes directly to disk. Backups and change history are kept under `.orbit` in each agent's configuration directory. Claude's disabled MCP and hook records and the transcript indexes are stored in the app-data directory. Codex uses native enablement settings. Window preferences are stored in the app-config directory.

Credential values in known configuration fields are masked in the UI, not encrypted on disk. Skills, configuration files, backups, and transcripts can contain private information. Review transcript exports before sharing them. Hook tests execute shell commands with your account's permissions. MCP tests start configured processes or contact configured endpoints and may send their credentials. These operations are not sandboxed.

Orbit's bundle identifier is `com.roycoding8.orbit`. Tauri uses its standard Orbit application directories. Older storage locations are not read or migrated.

## Requirements

- Node.js 22
- The stable Rust toolchain
- Platform prerequisites for [Tauri v2](https://v2.tauri.app/start/prerequisites/)
- Codex CLI on `PATH` for Codex skill discovery. Native skill discovery and imported chat resume are verified against Codex 0.163.0.
- OpenCode v2 CLI on `PATH` for its transcript features. Native format checks target OpenCode v2.1.3; v1 is not supported.
- Kiro native file adapters target CLI 2.32.0, including V2 and opt-in V3 hook formats.
- Pi adapters target the [v1.4.0 native formats](https://github.com/earendil-works/pi/tree/v1.4.0/packages/coding-agent/docs), including v3 sessions. Native Pi 1.4.0 cold-loads converted chats through RPC and preserves text, inline images, titles, and original or overridden working directories. Checks run offline without a model prompt.

On Ubuntu, Tauri's native build dependencies include WebKitGTK 4.1, GTK 3, and Ayatana AppIndicator. Install the packages before building:

```sh
sudo apt-get update
sudo apt-get install --yes build-essential curl wget file \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev libxdo-dev \
  libssl-dev librsvg2-dev patchelf
```

Windows builds use WebView2 and a native compiler matching your default Rust toolchain. For GNU, use `stable-x86_64-pc-windows-gnu` with MinGW-w64 GCC and G++ on `PATH`. For MSVC, use Microsoft C++ Build Tools. Windows CI uses MSVC.

macOS builds require Xcode Command Line Tools.

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

The optional conversion checks cold-read/resume converted chats in Codex and Pi and import/re-export one in OpenCode v2. They use disposable homes and storage and send no model prompt:

```sh
cargo test --manifest-path src-tauri/Cargo.toml transcript::conversion::tests::native_ --lib -- --ignored --nocapture
```

GitHub Actions runs these checks on Ubuntu, macOS, and Windows for pushes and pull requests. The Windows workflow also builds the NSIS installer with:

```sh
npm run tauri build
```

The installer is written under `src-tauri/target/release/bundle/nsis`.
