<div align="center">
  <img src="assets/orbit-readme.svg" alt="Orbit. Your coding agent setup, in one place." width="900" />
  <br />
  <a href="https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml"><img src="https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-6750A4.svg" alt="Apache License 2.0" /></a>
</div>

<br />

Orbit puts Claude Code and Codex tools and context in one desktop app. Manage skills, memories, MCP servers, hooks, settings, and transcripts without hunting through configuration files.

Choose the coding agent in the header. Claude Code is selected initially. Orbit is a local desktop app and keeps each agent's configuration, backups, and transcript index separate.

## What you can manage

| | Area | What you can do |
|:--:|---|---|
| <img src="assets/icons/skills.svg" alt="" width="24" /> | **Skills** | Browse and edit skills, control visibility, install or remove folders, and discover skills copied in outside Orbit. |
| | **Memories** | Edit or delete owned memories and instructions, follow links, and find incoming references. See Claude project memory groups or Codex's instruction hierarchy and generated memory. |
| <img src="assets/icons/mcp.svg" alt="" width="24" /> | **MCP servers** | Manage local stdio and remote HTTP servers, then test their connections. |
| <img src="assets/icons/hooks.svg" alt="" width="24" /> | **Hooks** | Inspect user, project, and inherited hooks. Edit supported command hooks and test commands with sample JSON input. |
| <img src="assets/icons/settings.svg" alt="" width="24" /> | **Settings** | Inspect configuration layers, edit one key at a time, and undo a recorded change when the file has not changed since. |
| <img src="assets/icons/transcripts.svg" alt="" width="24" /> | **Transcripts** | Index sessions, search messages, follow conversation branches, and export or import native chat archives to a chosen project. Markdown export is also available. |
| | **History** | Browse saved versions separately from operation logs, restore a version, or delete a saved backup. |

## Requirements

- Node.js 22
- The stable Rust toolchain
- Platform prerequisites for [Tauri v2](https://v2.tauri.app/start/prerequisites/)
- Codex CLI on `PATH` for Codex skill discovery. Native skill discovery and imported chat resume are verified against Codex 0.163.0.

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

Codex settings remain TOML, and Orbit preserves unrelated keys and comments. The effective settings view previews user and trusted project files. Profiles, command-line options, environment variables, and system policy can change Codex's runtime settings. Codex skill discovery uses the installed CLI's native inventory, including plugin skills and folders copied outside Orbit. Plugin and system skill files are read-only.

Codex instructions follow `AGENTS.override.md`, `AGENTS.md`, and configured fallback precedence through the repository hierarchy. Ancestor instructions are read-only until their project is selected. Generated files under `CODEX_HOME/memories` are read-only because Codex owns their lifecycle. Codex hook trust stays with its native `/hooks` review flow.

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

GitHub Actions runs these checks on Ubuntu, macOS, and Windows for pushes and pull requests. The Windows workflow also builds the NSIS installer with:

```sh
npm run tauri build
```

The installer is written under `src-tauri/target/release/bundle/nsis`.

## License

Orbit is licensed under the [Apache License 2.0](LICENSE).
