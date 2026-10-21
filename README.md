# Orbit

[![CI](https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml)

Orbit is a local desktop app for managing Claude Code configuration. It brings skills, project memories, MCP servers, hooks, settings, and local transcripts into one interface. The frontend is React and TypeScript; the desktop shell and local operations are implemented in Rust with Tauri.

Orbit currently supports Claude Code only. It does not sync configuration to a cloud service.

## Features

- **Skills and memories:** browse and edit skills, manage visibility, restore recent uninstalls, and read project memories.
- **MCP servers:** manage local stdio and remote HTTP servers, including enable, disable, delete, and connection tests.
- **Hooks:** inspect user and project hooks, edit them, and test commands with sample JSON input.
- **Settings:** inspect configuration layers, edit one key at a time, and undo recorded changes when the file has not changed since.
- **Transcripts:** index local Claude Code sessions, browse and search messages, follow conversation branches, and export Markdown.

## Requirements

- Node.js 22
- The stable Rust toolchain
- Platform prerequisites for [Tauri v2](https://v2.tauri.app/start/prerequisites/)

On Ubuntu, Tauri's native build dependencies include WebKitGTK 4.1, GTK 3, and Ayatana AppIndicator. Install the packages before building:

```sh
sudo apt-get update
sudo apt-get install --yes build-essential curl wget file \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev libxdo-dev \
  libssl-dev librsvg2-dev patchelf
```

Windows builds require the Microsoft C++ Build Tools and WebView2. macOS builds require Xcode Command Line Tools.

## Run locally

```sh
npm ci
npm run tauri dev
```

By default, Orbit reads and writes Claude Code configuration in the current user's Claude Code directories. To use a separate configuration directory, set `CLAUDE_CONFIG_DIR` before launching; Orbit then reads the settings and global state from that directory. The app treats its launch directory as the project until you select another folder.

Orbit writes configuration changes directly to disk. Backups and change history are kept under `.harness-agent` in the Claude configuration directory. Disabled MCP server and hook records and the transcript index are stored in the app-data directory; window preferences are stored in the app-config directory.

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
