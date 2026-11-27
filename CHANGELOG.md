# Changelog

## 0.6.1

### Added

- Added `CODE_SIGNING_POLICY.md` documenting build provenance, network behavior, and SignPath Foundation open-source code signing status.
- Added a TLA+ / TLC journal crash-recovery model (`formal/tla/Ledger.tla`) and wired both Kani and TLC formal checks into GitHub Actions CI.

### Changed

- Rebuilt the Kani verification suite around 12 harnesses that directly check `orbit-core` production code and restored runtime coverage at filesystem and storage boundaries.
- Shared the selected project directory (`projectRoot`) across navigation tabs and Setup Export/Import so switching pages preserves your active project.

### Fixed

- Fixed ledger recovery (`reconcile_pending_for_target`) so retrying a write with repeated file bytes reconciles only the latest pending row instead of crediting an older abandoned attempt.
- Fixed Antigravity skills discovery so user manifests under `~/.gemini` accept sibling `~/.gemini/antigravity/skills` entries while still rejecting `..` traversal and outside-root paths.
- Fixed Antigravity settings credential masking so top-level `apiKey` and flat dotted secret keys are masked on read and preserved on write.
- Fixed Muse Code setup imports with destination set to **User-wide** so project hooks map cleanly to `~/.config/muse/settings.json` with `schema_version: 1` preserved.
- Fixed History file restoration to safely recreate missing parent directories (resolving `os error 3` when restoring an edit version of a deleted skill) and verified Junie settings version snapshots.

## 0.6.0

### Added

- In-app automatic updates powered by `tauri-plugin-updater` and `tauri-plugin-process`, with a topbar **Check for updates** control, silent startup checks, and Ed25519 signature verification against GitHub Releases (`latest.json`).
- Release workflow support for signed updater artifacts (`.sig`, `latest.json`), `SHA256SUMS.txt` checksums, architecture-disambiguated macOS updater archives, and a `prerelease` workflow toggle.
- Expanded offline test coverage across all 9 harnesses, including Pi and Codex chat deletion, `simple.rs` negative paths, Muse MCP credential preservation, and end-to-end History and Setup Transfer round-trips for Muse Code, Junie CLI, and Cursor CLI.

### Changed

- Updated `README.md`, `docs/reference.md`, `CONTRIBUTING.md`, and `formal/README.md` for all 9 native agents and the in-app updater, with a Table of Contents and collapsible per-agent sections in `docs/reference.md`.

### Fixed

- Fixed a trailing-slash bug in `history::canonical_location` that caused `ENOTDIR` on existing files and made Muse Code, Junie CLI, and Cursor CLI memory and skill history entries non-restorable.
- Fixed setup transfer validation to accept Muse Code `.agents/memory/*.md` bundles, Cursor `.cursor/rules/*.mdc` files, and hook exports for Muse Code, Junie CLI, and Cursor CLI.
- Fixed setup transfer imports into fresh Muse Code `settings.json` and Cursor `hooks.json` files to preserve `schema_version: 1` and `version: 1`.

## 0.5.1

### Added

- Native support for **Muse Code** (`~/.config/muse/settings.json` with `schema_version: 1`, `mcp_servers`, skills, `.agents/memory/`, and 17 hook events), **Junie CLI** (`config.json`, `mcp/mcp.json`, skills, `.junie/rules/`, and user hooks), and **Cursor CLI** (`cli-config.json` with `version: 1`, project permissions `cli.json`, `mcp.json`, skills, `.cursor/rules/*.{md,mdc}`, and 21 hook events).
- Shared pure-`std` `orbit-core` crate and standalone Kani 0.68.0 formal verification suite (`formal/`, 20 verified proofs) covering path safety, ledger/history state machines, credential masking, and transcript/frontmatter parsing.

### Changed

- Reorganized frontend source into `src/{components,lib,pages,styles}/`, moved frontend tests to `tests/frontend/`, moved Claude settings schema to `src-tauri/schemas/claude-settings.json`, and removed unused SVG assets and dead batch transcript ingestors.
- Eliminated O(n²)/O(n³) scans and redundant allocations/disk reads across history loading, memory link cleanup, setup transfer previews, transcript spine ordering, and frontend page filters.
- Overhauled `README.md` and `CONTRIBUTING.md` with architecture diagram, agent support matrix, and formal verification documentation.

### Fixed

- **Frontend (8 fixes):** Preserved `projectRoot` across transcript select/index/import and project switching; added request-sequence guards in MCP and Memories; fixed Kiro hook layer checks, unhandled skill dialog rejections, `displayPath` error formatting, and inline triple-backtick handling in `memorySections`.
- **Harnesses (19 fixes):** Hardened path containment and symlink checks across Antigravity, Codex, Kiro, OpenCode, and Pi; fixed Codex `headers` corruption, Claude sibling-hook re-enable and sandbox cleanup, `credential_flag` false positives, case-insensitive HTTPS/SSE checks, BOM/CRLF/unclosed frontmatter parsing, `~/` and leading `..` memory links, no-op backups, reaped-child process cleanup, and `Harness::list` error classification.
- **Core, Transcripts & Transfer (15 fixes):** Fixed UTF-8 `parse_checksum` panic, `history::delete` TOCTOU lock order, superseded pending row classification, atomic `restore_directory` rollback, monotonic ledger `op_id`/backup sequencing, symlink-safe undo, iterative spine traversal, 256 MiB Codex zstd cap, archive `local_

## 0.4.1

### Changed

- Kiro V3 refresh: new hook triggers SessionEnd, PreCompact, PostCompact, and Manual, with V3 trigger spelling compatibility. `chat.agentEngine` now defaults to V3 semantics with legacy V2 profile detection. Documentation refreshed for local Kiro Memory and V3 session boundaries.

## 0.4.0

### Added

- Antigravity App and CLI support: skills, instructions, modular rules, hooks, MCP servers, and CLI settings.
- Antigravity chats grouped by project, with on-demand conversation loading and optional indexing for message search.
- Antigravity native chat export, import, restoration, and deletion. Archives include the session database, annotations, and chat-owned artifacts.
- Chat conversion to and from Antigravity, with an optional destination working directory. Incoming conversions create CLI chats and transfer conversation text and supported inline images.
- Selective Antigravity setup export and import, with saved versions in History for replaced resources.

### Compatibility

Antigravity chat import, restoration, deletion, and incoming conversion require `agy` 1.5.0 with the Antigravity App and CLI sessions closed. Restoration preserves the original App or CLI store and chat ID; an existing ID cannot be overwritten. Conversion creates a new chat and leaves the source unchanged.

See [Antigravity support](docs/reference.md#antigravity) for details.
