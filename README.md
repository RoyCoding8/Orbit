<div align="center">
  <img src="assets/orbit-readme.svg" alt="Orbit" width="84" height="84" />
  <h1>Orbit</h1>
  <p><strong>A local-first desktop control plane for your AI coding agents.</strong></p>
  <p>Manage configuration, skills, memories, MCP servers, hooks, setup backups, and chat archives across 9 native CLI agents, with atomic writes, full version history, and signed in-app updates.</p>

  <p>
    <a href="https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml"><img src="https://github.com/RoyCoding8/Orbit/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI Status" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-2563eb.svg" alt="Apache License 2.0" /></a>
    <a href="https://v2.tauri.app/"><img src="https://img.shields.io/badge/Tauri-v2-24c8db.svg?logo=tauri&logoColor=white" alt="Tauri v2" /></a>
    <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.82%2B-dea584.svg?logo=rust&logoColor=black" alt="Rust 1.82+" /></a>
  </p>

  <p>
    <a href="#features">Features</a> |
    <a href="#agent-support">Agent Support</a> |
    <a href="#architecture">Architecture</a> |
    <a href="#formal-verification">Formal Verification</a> |
    <a href="#quick-start">Quick Start</a> |
    <a href="docs/reference.md">Reference Docs</a>
  </p>
</div>

## Supported Agents

**Claude Code**, **OpenAI Codex**, **Kiro**, **OpenCode v2**, **Pi**, **Antigravity**, **Muse Code**, **Junie CLI**, and **Cursor CLI**.

Orbit reads and writes each agent's native on-disk formats directly (JSON, JSONC, TOML, Markdown, SQLite, JSONL) without locking you into a proprietary wrapper.

## Features

| Capability | What Orbit Does |
|---|---|
| **Skills** | Discover local, project, and shared skills; edit `SKILL.md` with live Markdown preview; install, uninstall, and restore previous versions. |
| **Memories & Instructions** | Inspect hierarchical `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, rules, and auto-memory catalogs; collapse project groups, edit sections, and follow linked files. |
| **MCP Servers** | Configure stdio and HTTP/SSE MCP servers, preserve unknown native fields, mask credentials in the UI, and run live connection benches. |
| **Settings & Hooks** | Layer-aware inspection (user / project / local) preserving JSONC & TOML comments, nested JSON expansion, and interactive command-hook testing. |
| **Chats & Conversion** | Browse sessions by project, lazy-load threads on demand, build SQLite FTS5 message search, export/import native archives, convert across agents, and delete with mandatory backup. |
| **Setup Backups** | Selectively export skills, hooks, MCP servers, memories, and settings to checksummed ZIP archives with conflict-aware import previews. |
| **Atomic History & Undo** | Every write uses temp-file staging, checksum pre-images, and a durable ledger so you can restore prior file or directory versions at any time. |
| **In-App Updates** | Checks GitHub Releases (`latest.json`) automatically and on demand via `tauri-plugin-updater` and `tauri-plugin-process`, verifying Ed25519 signatures before installing and relaunching. |

## Agent Support

| Area | Claude Code | Codex | Kiro | OpenCode v2 | Pi | Antigravity | Muse Code | Junie CLI | Cursor CLI |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| **Settings** | JSON layers | TOML layers | JSON layers | JSON / JSONC | JSON layers | CLI JSON | User JSON (`v1`) | User/project JSON | User (`v1`) / project perms |
| **Skills** | Full + toggle | Full + toggle | Full | Full | Full | Full + manifests | Full + shared | Full | Full + built-in |
| **Memories & Instructions** | Auto-memory + `CLAUDE.md` | Shared + `AGENTS.md` | Steering + `AGENTS.md` | `AGENTS.md` + local | Hierarchy + `SYSTEM.md` | Rules + `AGENTS`/`GEMINI` | `.agents/memory` + instructions | Guidelines, rules + `AGENTS.md` | `.cursor/rules` + instructions |
| **MCP Servers** | Local & HTTP | Local & HTTP | Local & HTTP | Native v2 | Local & HTTP | Shared local & HTTP | `mcp_servers` (stdio / HTTP) | `mcp/mcp.json` | `mcp.json` |
| **Hooks** | Edit, toggle, test | Edit, test | V2/V3 + `.kiro/hooks` | Via Settings | Via Settings | Named groups + test | 17 events (user & project) | 10 events (user only) | 21 events (user & project) |
| **Chat Archives** | Export / Import | Export / Import | V2 export only | Native v2 CLI | JSONL v3 | App & CLI (`agy 1.5`) | Unavailable | Unavailable | Unavailable |
| **Chat Conversion** | Source & Dest | Source & Dest | Source only | Source & Dest | Source & Dest | Source & CLI Dest | Unavailable | Unavailable | Unavailable |
| **Chat Deletion** | With backup | With backup | Unavailable | Leaf chats | With backup | Inactive unlinked | Unavailable | Unavailable | Unavailable |

See the full [Agent Support & Format Reference](docs/reference.md#agent-support) for per-agent nuances, paths, and limits.

## Architecture

The `orbit-core` crate makes deterministic decisions about paths, credentials, journal state, and transcript data. The desktop backend handles filesystem access, process coordination, SQLite, and provider formats.

```mermaid
flowchart LR
  subgraph UI["Frontend: src/ (React 19 + TypeScript + M3E)"]
    Pages["Pages\nConfig, Skills, Memories\nMCP, Hooks, Chats, History"]
    Lib["Shared Lib & Components\nHarnessContext, UpdateChecker\nuseSequencedLoad, useModalDialog"]
    Pages --> Lib
  end

  subgraph Backend["Desktop Backend: src-tauri/ (Rust + Tauri v2)"]
    Commands["Tauri IPC Commands & Plugins\nupdater, process, dialog, window-state"]
    Harnesses["9 Harness Adapters\nClaude, Codex, Kiro, OpenCode, Pi\nAntigravity, Muse, Junie, Cursor"]
    Engine["Ledger & History\nSQLite FTS5 Transcripts, Transfer"]
    Commands --> Harnesses
    Commands --> Engine
  end

  subgraph Core["Production rules: orbit-core/ (std-only)"]
    Rules["Paths, journal, credentials, transcripts"]
  end

  subgraph Formal["Formal Verification: formal/"]
    Kani["Kani bounded safety checks"]
    TLC["TLC journal progress model"]
  end

  Lib -- "Tauri invoke()" --> Commands
  Harnesses --> Rules
  Engine --> Rules
  Kani -. "checks production functions" .-> Rules
  TLC -. "models journal operations" .-> Engine
```

### Repository Layout

```text
Orbit/
├── src/                  # React 19 + TypeScript UI (components/, lib/, pages/, styles/)
├── tests/frontend/       # Vitest + Testing Library frontend test suites
├── src-tauri/            # Tauri v2 Rust backend (harness/, transcript/, transfer/, schemas/)
├── orbit-core/           # Pure-std shared crate (zero Tauri/SQLite/OS FFI)
├── formal/               # Kani safety checks and the TLA+ journal model
├── docs/                 # Deep reference documentation
└── assets/               # README icon artwork
```

## Formal Verification

[Kani](https://model-checking.github.io/kani/) checks safety properties of production Rust functions within the bounds of each harness. TLC checks conditional progress in a finite TLA+ model of the journal. Rust and frontend tests exercise the platform adapters, storage, provider formats, and UI.

The [formal verification guide](formal/README.md) gives the commands, properties, and assumptions for each check.

## Quick Start

**Prerequisites:** Node.js 22+, stable Rust (1.82+), and the [Tauri v2 platform prerequisites](https://v2.tauri.app/start/prerequisites/). Install whichever native agent CLIs you use (OpenCode requires v2).

```sh
npm ci
npm run tauri dev
```

On Windows, use either Rust GNU with MinGW-w64 GCC/G++ or Rust MSVC with the Microsoft C++ Build Tools. See [platform setup](CONTRIBUTING.md#platform-setup).

## Building Release Packages

| Platform | Command | Output Package |
|---|---|---|
| **Windows** | `npm run tauri build` | NSIS installer (`.exe`) |
| **Linux** | `npm run tauri build -- --bundles deb,rpm,appimage` | `.deb`, `.rpm`, `.AppImage` |
| **macOS** | `npm run tauri build -- --bundles app,dmg` | `.app` bundle, `.dmg` |

Packages are written to `src-tauri/target/release/bundle/`. GitHub Releases built by the Release workflow include signed updater manifests (`latest.json`), `SHA256SUMS.txt` checksums, and an optional `prerelease` toggle. See [Contributing](CONTRIBUTING.md#checks) for signing and release workflow details.

## Verification & Checks

```sh
# Frontend build & tests
npm run build
npm test

# Rust formatting, lints, & unit/integration tests
cargo fmt --manifest-path orbit-core/Cargo.toml --check
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path orbit-core/Cargo.toml -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml

# Bounded core safety (requires Kani 0.68.0)
bash formal/run.sh
```

## Documentation

- **[Agent Support & Reference](docs/reference.md)**: per-agent configuration paths, chat archive/conversion rules, setup backup limits, and execution security model
- **[Formal Verification Guide](formal/README.md)**: Safety bounds, journal progress assumptions, and boundary tests
- **[Contributing Guide](CONTRIBUTING.md)**: platform toolchains, CI checks, release workflow, and optional live-CLI integration tests
- **[Changelog](CHANGELOG.md)**: release history

## License

Licensed under the [Apache License 2.0](LICENSE).

## Code signing policy

Free code signing provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/). See [CODE_SIGNING_POLICY.md](CODE_SIGNING_POLICY.md) for build provenance, roles, and privacy details.
