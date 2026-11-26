# Orbit Reference

[Back to Orbit](../README.md)

## Contents

- [Agent support](#agent-support)
- [Configuration and memory files](#configuration-and-memory-files)
- [Per-agent notes (expandable)](#per-agent-notes)
- [Chats](#chats)
- [Setup export and import](#setup-export-and-import)
- [Stored data and command execution](#stored-data-and-command-execution)

## Agent support

Orbit works with each agent's native files and installed CLI. Features vary by agent:

| Area | Claude Code | Codex | Kiro | OpenCode v2 | Pi | Antigravity | Muse Code | Junie CLI | Cursor CLI |
|---|---|---|---|---|---|---|---|---|---|
| **Skills** | User + visibility toggles | User, project, plugin + toggles | User and project | Native and shared | Folders and standalone `.md` | Shared, CLI-only, manifests | User, shared (RO), project | User and project | User, built-in (RO), project |
| **Memories & instructions** | Auto-memory + `CLAUDE.md` | Shared memory + `AGENTS.md` | Steering + `AGENTS.md` | `AGENTS.md` + configured local | `AGENTS`/`CLAUDE` + `SYSTEM.md` | `AGENTS`/`GEMINI` + rules | `AGENTS`/`CLAUDE` + `.agents/memory` | `AGENTS.md`, guidelines, `.junie/rules` | `AGENTS`/`CLAUDE` + `.cursor/rules/*.{md,mdc}` |
| **MCP** | Local and HTTP | Local and HTTP | Local and HTTP | Native v2 `mcp.servers` | Local and streamable HTTP | Shared local and HTTP | `mcp_servers` in `settings.json` | `mcpServers` in `mcp/mcp.json` | `mcpServers` in `mcp.json` |
| **Hooks** | Edit, toggle, test | Edit, test (native approval) | V2/V3 profiles + `.kiro/hooks/*.json` | Plugin list in Settings | Extension list in Settings | Named groups, toggle, test | 17 events (user and project) | 10 events (user only) | 21 events (user and project) |
| **Settings** | User, project, local JSON | User and trusted project TOML | User and project JSON | User and project JSON/JSONC | User and project JSON | User CLI JSON | User JSON (`schema_version: 1`) | User and project JSON | User JSON (`version: 1`) + project `permissions` |
| **Chat archives** | Export and import | Export and import | V2 ACP export only | Native v2 CLI | V3 JSONL sessions | App and CLI (`agy 1.5.0`) | Unavailable | Unavailable | Unavailable |
| **Chat conversion** | Source and dest | Source and dest | V2 ACP source only | Source and dest | Source and dest | Source and CLI dest | Unavailable | Unavailable | Unavailable |
| **Chat deletion** | With backup | With backup | Unavailable | Leaf chats only | With backup | Inactive, unlinked chats | Unavailable | Unavailable | Unavailable |

<details>
<summary><strong>General capability notes</strong></summary>

- **History:** Provides saved versions, restoration, backup deletion, and separate operation logs for all supported local resources.
- **Skill toggles:** Only Claude Code and Codex support individual skill enable/disable toggles; for all other agents, uninstalling saves a restorable version in History.
- **Read-only items:** Inherited resources outside the selected project and built-in/shared skill folders are read-only.
- **MCP scope:** MCP editing targets user configuration; project and runtime overrides can still affect the running agent.
- **Generated memory:** Kiro's learned memory lives in a local DB under `KIRO_HOME` that Orbit does not edit. OpenCode, Pi, Junie CLI, and Cursor CLI have no built-in generated-memory file catalog. OpenCode plugins and Pi extensions are managed in Settings rather than the command-hook editor.

</details>

## Configuration and memory files

Select a project folder to inspect its instructions, skills, and settings (Orbit starts in its launch directory). Skills copied into native folders are discovered automatically.

| Agent | User configuration | Project configuration |
|---|---|---|
| **Claude Code** | Native Claude dir (`CLAUDE_CONFIG_DIR`) | `CLAUDE.md` and `.claude/` files |
| **Codex** | `CODEX_HOME` (default `~/.codex`) | `AGENTS.md` and `.codex/` settings |
| **Kiro** | `KIRO_HOME` (default `~/.kiro`) | `.kiro/` and `AGENTS.md` files |
| **OpenCode v2** | `XDG_CONFIG_HOME/opencode` (default `~/.config/opencode`) | `opencode.json[c]` and `.opencode/opencode.json[c]` |
| **Pi** | `PI_CODING_AGENT_DIR` (default `~/.pi/agent`) | `.pi/` settings and instruction files |
| **Antigravity** | `~/.gemini/config` (shared) and `~/.gemini/antigravity-cli` (CLI) | `.agents/` customizations and native aliases |
| **Muse Code** | `MUSE_CONFIG_DIR` or `XDG_CONFIG_HOME/muse` (default `~/.config/muse`), shared RO `~/.agents/skills` | `AGENTS.md`, `CLAUDE.md`, `.agents/AGENTS.md`, `.claude/CLAUDE.md`, `.agents/skills`, `.agents/memory/*.md`, `.muse/hooks.json` |
| **Junie CLI** | `JUNIE_HOME` (default `~/.junie`: `config.json`, `mcp/mcp.json`, `skills/`, `AGENTS.md`) | `.junie/config.json`, `.junie/skills/`, `AGENTS.md`, `.junie/guidelines.md`, `.junie/rules/*.md` |
| **Cursor CLI** | `CURSOR_CONFIG_DIR` or `XDG_CONFIG_HOME/cursor` on Linux/BSD (default `~/.cursor`), built-in RO `skills-cursor/` | `.cursor/cli.json`, `.cursor/hooks.json`, `.cursor/skills/`, `.cursor/rules/*.{md,mdc}`, `AGENTS.md`, `CLAUDE.md` |

## Per-agent notes

Expand any agent below for native format rules, limits, and edge cases.

<details>
<summary><strong>Claude Code</strong></summary>

- Auto-memory is project-scoped, including files stored under the user configuration directory.
- Linked files and `@` instruction imports appear alongside their source files.
- Disabled MCP servers and disabled hook records are stored in Orbit's app-data directory so native settings stay clean.

</details>

<details>
<summary><strong>OpenAI Codex</strong></summary>

- Instructions follow `AGENTS.override.md`, `AGENTS.md`, and configured fallback precedence through the repository hierarchy.
- Orbit edits Markdown under `CODEX_HOME/memories` (indexes, summaries, raw inputs, and supporting notes). Section edits share one draft with the whole-file editor, and saved edits have restorable versions in History. Git metadata and linked paths are excluded. Codex can regenerate memory files and replace manual edits; Orbit does not reset memory generation state.
- Settings preserve TOML keys and comments; previews include user and trusted project sources. Plugin and system skills remain read-only. Approve Codex hook changes through its native `/hooks` flow.

</details>

<details>
<summary><strong>Kiro</strong></summary>

- Steering and `AGENTS.md` instructions are editable; Kiro's local learned memory database under `KIRO_HOME` is managed by Kiro and not edited by Orbit.
- Hooks support both legacy V2 agent profiles and V3 profiles plus standalone `.kiro/hooks/*.json` files. Markdown agent hook profiles are listed read-only.
- Chat support covers local V2 ACP sessions under `KIRO_HOME/sessions/cli` (export only; V3, terminal, and cloud sessions use separate stores with no documented client layout).

</details>

<details>
<summary><strong>OpenCode v2</strong></summary>

- Settings preserve JSONC comments. Remote configuration, instruction URLs, globs, and runtime plugin configuration are not expanded by Orbit's file editor.
- Plugins are configured in Settings rather than the command-hook editor.
- Chat listing discovers projects known to the native v2 CLI, including child sessions.

</details>

<details>
<summary><strong>Pi</strong></summary>

- Follows one instruction file per directory through the ancestor hierarchy. Project `SYSTEM.md` replaces the global system prompt; `APPEND_SYSTEM.md` adds instructions.
- Local skill folders and standalone Markdown skills are editable. Package expansion, glob filters, project trust, and authentication remain managed by Pi.
- Settings previews merge nested settings and combine resource lists; CLI flags, environment variables, profiles, and native policy can still override runtime behavior.
- MCP supports local stdio and streamable HTTP servers (SSE is rejected).

</details>

<details id="antigravity">
<summary><strong>Antigravity</strong></summary>

- **Customizations:** Shared files under `~/.gemini/config/` serve the App and CLI. Orbit also discovers CLI-only skill folders and project `.agents/`, `.agent/`, `_agents/`, and `_agent/` locations following native manifests and inclusion/exclusion lists. Duplicate skill names stay visible; Antigravity decides which version loads.
- **Instructions & rules:** Includes user `AGENTS.md` and `GEMINI.md`, shared-config equivalents, and project ancestors. Modular rules show their native trigger. Built-in resources, plugins, repository personal folders, and internal generated knowledge remain managed by Antigravity.
- **Settings & MCP:** Settings edit `~/.gemini/antigravity-cli/settings.json` (App preferences stay in Antigravity). User MCP edits `~/.gemini/config/mcp_config.json` (project MCP files can be included in setup backups). Remote servers accept `serverUrl` or `url`. OAuth and runtime-dependent authentication are handled in Antigravity.
- **Hooks:** Uses named groups in `hooks.json` without a top-level `hooks` wrapper. Toggling one command changes its whole group; commands sharing a matcher also share matcher edits. Native hook payloads use camelCase fields.
- **Chats (`agy 1.5.0`):** Chat import, restoration, deletion, and incoming conversion require **agy 1.5.0** with App and CLI sessions closed (browsing and export work for compatible local formats on other versions). Archives bundle a consistent `session.db` snapshot (including WAL), checksummed `manifest.json` + annotations, and chat-owned files under `brain/<id>`. Restoration preserves the original App/CLI store and internal ID; deletion refuses changed, active, parent/child, or battle-linked chats.
- **Upstream docs:** [customization discovery](https://antigravity.google/docs/customization-discovery), [rules](https://antigravity.google/docs/rules), [hooks](https://antigravity.google/docs/hooks), [MCP](https://antigravity.google/docs/mcp), [CLI reference](https://antigravity.google/docs/cli/reference).

</details>

<details>
<summary><strong>Muse Code</strong></summary>

- **Settings:** User-scoped in `settings.json` (under `MUSE_CONFIG_DIR`, `XDG_CONFIG_HOME/muse`, or `~/.config/muse`). Orbit always preserves `schema_version: 1`.
- **MCP:** Stored under `mcp_servers` in `settings.json` (`stdio` or `streamable_http`), with per-server `enabled` toggles and optional `mode` (`required` or `optional`).
- **Skills:** Discovered from `<config_root>/skills` (editable), `~/.agents/skills` (read-only), and project `.agents/skills` (editable).
- **Instructions & memory:** Covers `AGENTS.md`, `CLAUDE.md`, `.agents/AGENTS.md`, `.claude/CLAUDE.md`, and `.agents/memory/*.md` (`MEMORY.md` as index, sibling `.md` files as topics).
- **Hooks:** Matcher groups across 17 lifecycle events (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`, `PreLLMCall`, `PostLLMCall`, `PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`, `Notification`, `Stop`, `StopFailure`, `SessionEnd`). User hooks live in `settings.json` (preserving `schema_version: 1`); project hooks live in `.muse/hooks.json`.

</details>

<details>
<summary><strong>Junie CLI</strong></summary>

- **Settings:** Resolves user dir from `JUNIE_HOME` (default `~/.junie`). Merges user `~/.junie/config.json` with project `.junie/config.json` (project overrides user).
- **MCP:** Stored under `mcpServers` in `~/.junie/mcp/mcp.json` (`stdio` or `http`).
- **Skills:** Discovered from `~/.junie/skills` and project `.junie/skills`.
- **Instructions:** Covers user `~/.junie/AGENTS.md` plus project `AGENTS.md`, `.junie/guidelines.md`, and `.junie/rules/*.md`.
- **Hooks:** Matcher groups across 10 events (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `SubagentStart`, `SubagentStop`, `PreCompact`, `Notification`, `Stop`, `SessionEnd`) in `~/.junie/config.json`. Junie CLI ignores project hooks for safety, so Orbit restricts hook edits to user scope.

</details>

<details>
<summary><strong>Cursor CLI</strong></summary>

- **Settings:** Resolves user dir from `CURSOR_CONFIG_DIR`, then `XDG_CONFIG_HOME/cursor` on Linux/BSD, defaulting to `~/.cursor`. User settings live in `cli-config.json` (preserving `version: 1`). Project `.cursor/cli.json` supports only the `permissions` key.
- **MCP:** Stored under `mcpServers` in `~/.cursor/mcp.json` (`stdio` or `http`).
- **Skills:** Discovered from `~/.cursor/skills` (editable), `~/.cursor/skills-cursor` (read-only built-in), and project `.cursor/skills` (editable).
- **Instructions & rules:** Covers `AGENTS.md`, `CLAUDE.md`, and `.cursor/rules/*.{md,mdc}`.
- **Hooks:** Flat command list across 21 camelCase events (`sessionStart`, `sessionEnd`, `preToolUse`, `postToolUse`, `postToolUseFailure`, `subagentStart`, `subagentStop`, `beforeShellExecution`, `afterShellExecution`, `beforeMCPExecution`, `afterMCPExecution`, `beforeReadFile`, `afterFileEdit`, `beforeSubmitPrompt`, `preCompact`, `stop`, `afterAgentResponse`, `afterAgentThought`, `beforeTabFileRead`, `afterTabFileEdit`, `workspaceOpen`) in user `~/.cursor/hooks.json` and project `.cursor/hooks.json`, preserving `version: 1`.

</details>

## Chats

Open **Chats** to browse sessions grouped by working directory. Orbit reads metadata first and lazy-loads each conversation on demand. Use **Index chats** to build SQLite FTS5 message search, **Search message text** to query indexed chats, and **Refresh chats** to discover new sessions.

*(Muse Code, Junie CLI, and Cursor CLI do not have a supported local transcript format in Orbit yet.)*

<details>
<summary><strong>Browse, index, and agent-specific discovery rules</strong></summary>

- Project groups collapse inside the chat list. Opening a conversation also indexes its messages for search.
- **OpenCode v2** lists projects known to its native CLI, including child sessions.
- **Pi** lists its default session folders plus the selected project's configured session directory. Select a project and refresh to include its custom directory; previously indexed custom projects stay visible.
- **Kiro** reads local V2 ACP sessions from `KIRO_HOME/sessions/cli`. Kiro CLI 3.x defaults to V3, whose sessions live in a separate undocumented store, so V3, terminal, and cloud sessions are excluded.

</details>

<details>
<summary><strong>Export and import chat archives</strong></summary>

A **chat archive is a folder** containing native session files and migration metadata (distinct from a [setup backup](#setup-export-and-import), which is a ZIP).

1. Open a chat and choose **Export [agent] archive**, then pick a destination folder.
2. To import, select the matching agent, choose **Import [agent] archive**, and pick the archive folder and destination project.

Duplicate chat titles receive numeric suffixes. External files referenced by a chat are not bundled. **Export Markdown** writes a separate readable copy. Kiro offers source backup export only (use Kiro's `/chat save` and `/chat load` for native archives). Antigravity import preserves the original App or CLI store and internal ID and refuses duplicate IDs.

</details>

<details>
<summary><strong>Convert chats between agents</strong></summary>

Open a chat, choose **Convert chat**, pick the destination agent, and optionally change the working directory (defaults to the source chat's directory; required if the source has none). The original chat stays unchanged.

- Transfers conversation text and supported inline images from the latest active branch (other branches stay in the source).
- Historical tool activity is represented as text where supported (omitted for Antigravity). Signed reasoning, system/developer instructions, and runtime state do not transfer.
- External attachments stay as references; available Codex image archive assets can be embedded.
- Resolve partial OpenCode revert boundaries in OpenCode before converting. Kiro can be a source but not a destination.

</details>

<details>
<summary><strong>Delete and restore chats</strong></summary>

Deleting a supported chat saves a native archive backup first in a folder you choose. Restore it later with **Import [agent] archive**. Close the native chat before deleting.

- **Kiro:** Deletion is unavailable (its backups cannot be imported through Orbit).
- **OpenCode:** Chats with children cannot be deleted individually.
- **Antigravity:** Requires App and CLI sessions to be closed and refuses changed, active, parent/child, or battle-linked chats.

</details>

## Setup export and import

Choose **Export setup** in the header, select a project, pick categories (skills, hooks, MCP servers, memories, settings), and save a checksummed ZIP backup. To import, select the **same agent**, choose **Import setup**, pick **Current project** or **User-wide**, review conflicts, and click **Import selected**.

| Item | Import behavior |
|---|---|
| Already up to date | Skipped automatically. |
| Different version exists | Kept unless you choose **Replace**. |
| Skills and memories | Selected files are added or replaced; other files stay in place. |
| Hooks and extension lists | Missing entries are added without duplicates. |
| Settings | Preferences are replaced. Current hooks and MCP servers stay unless selected too. |

Replaced resources are saved as restorable versions in **History**. Import does not install packages or run scripts; paths saved on another computer may need updating.

<details>
<summary><strong>Archive contents, scope rules, and limits</strong></summary>

- **Scope rules:** Claude auto-memory and Muse Code `.agents/memory` files stay attached to the destination project; Codex generated memory stays user-wide. Antigravity CLI preferences and Muse Code settings remain user-wide; shared and user-root instructions keep their native locations.
- **Included vs. excluded:** The ZIP holds `metadata.json` and checksummed resource files. Login files, chats, caches, previous Orbit backups, installed plugin packages, Kiro Markdown hook profiles, and Antigravity App preferences / internal knowledge are excluded. Memory exports include local Markdown links within the same root (plus Cursor `.cursor/rules/*.mdc` files).
- **Preservation:** Unrelated JSONC and TOML values and comments stay intact. Items targeting the same file or setting cannot be imported together. Unsaved editor drafts stay open until you refresh.
- **Limits:** Maximum 10,000 items, 16 MiB per file, and 128 MiB overall. Unsafe paths, symlinks, missing files, and checksum mismatches are rejected.

</details>

## Stored data and command execution

- **Native files & backups:** Orbit writes directly to each agent's native files. Backups and change history live under `.orbit` inside each agent's config directory. Claude disabled MCP/hook records and transcript indexes live in Orbit's app-data directory; window preferences live in app-config. Bundle ID: `com.roycoding8.orbit`.
- **In-app updates:** Uses `tauri-plugin-updater` and `tauri-plugin-process`. On launch and via **Check for updates**, Orbit fetches `latest.json` from GitHub Releases and verifies each archive's Ed25519 signature against the public key in `src-tauri/tauri.conf.json` before downloading, installing, and relaunching.
- **Credential masking:** Conceals known secret values in the UI only; files on disk are not encrypted.
- **Unsandboxed tests:** Hook tests run shell commands with your user account's permissions, and MCP tests launch configured processes or contact endpoints (potentially sending configured credentials). Pi tests skip OAuth, dynamic command/env references, and relative working directories that require Pi's runtime context.
