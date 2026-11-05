<div align="center">
  <img src="assets/orbit-readme.svg" alt="Orbit" width="72" height="72" />
  <h1>Orbit</h1>
  <p>A local desktop app for managing your AI coding agents: their configuration, skills, memories, and chat archives.</p>
  <p><strong>Claude Code · Codex · Kiro · OpenCode v2 · Pi · Antigravity</strong></p>
  <a href="https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml"><img src="https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-2563eb.svg" alt="Apache License 2.0" /></a>
</div>

## Features

| Area | What you can do |
|---|---|
| **Skills** | Search local skills, edit Markdown with preview, install, and restore. |
| **Memories** | Search and collapse project groups, edit sections, and follow linked files. |
| **MCP servers** | Configure local or HTTP servers and test connections. |
| **Settings & hooks** | Inspect configuration sources, expand nested JSON, and test command hooks. |
| **Chats** | Browse by project, load conversations on demand, index for message search, export, import, convert, and delete with a backup. |
| **Setup backups** | Choose skills, hooks, MCPs, memories, and settings to save; review and import them into the same agent. |
| **History** | Restore saved versions and browse operation logs separately. |

Support varies by agent. See the [support matrix](docs/reference.md#agent-support) for available features and chat compatibility.

New in **0.4.0**: Antigravity App and CLI support, including native chat archives and conversion. See the [release notes](CHANGELOG.md#040).

## Run

Requires Node.js 22, stable Rust, and the [Tauri v2 platform prerequisites](https://v2.tauri.app/start/prerequisites/). Install the native agent CLIs you use. OpenCode requires v2.

```sh
npm ci
npm run tauri dev
```

On Windows, use either Rust GNU with MinGW-w64 GCC/G++, or Rust MSVC with the Microsoft C++ Build Tools. See [platform setup](CONTRIBUTING.md#platform-setup).

## Build

| Platform | Command | Package |
|---|---|---|
| Windows | `npm run tauri build` | NSIS installer (`.exe`) |
| Linux | `npm run tauri build -- --bundles deb,rpm,appimage` | DEB, RPM, AppImage |
| macOS | `npm run tauri build -- --bundles app,dmg` | App bundle, DMG |

Packages are written under `src-tauri/target/release/bundle/`.

## Checks

```sh
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

## Reference

- [Configuration and memory files](docs/reference.md#configuration-and-memory-files)
- [Setup export and import](docs/reference.md#setup-export-and-import)
- [Chat archives and conversion](docs/reference.md#chats)
- [Contributing and checks](CONTRIBUTING.md)
- [Stored data and command execution](docs/reference.md#stored-data-and-command-execution)

## License
[Apache License 2.0](LICENSE)
