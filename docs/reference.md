# Orbit reference

[Back to Orbit](../README.md)

## Agent support

Orbit works with each agent's native files and installed CLI. Features vary by agent:

| Area | Claude Code | Codex | Kiro | OpenCode v2 | Pi | Antigravity |
|---|---|---|---|---|---|---|
| Skills | User skills and visibility controls | User, project, and plugin discovery; enable/disable | User/project skills | Native and shared skills | Folders and standalone Markdown skills | Shared and CLI-only skills; discovery manifests |
| Memories and instructions | Project auto-memory and Claude instructions | Shared memory Markdown and scoped AGENTS instructions | Steering and AGENTS instructions | AGENTS and configured local instructions | AGENTS/CLAUDE hierarchy and system prompts | AGENTS, GEMINI, and modular rules |
| MCP | Local and HTTP servers | Local and HTTP servers | Local and HTTP servers | Native v2 `mcp.servers` | Local and streamable HTTP servers | Shared local and streamable HTTP servers |
| Hooks | Command editing, toggles, tests | Command editing and tests; native approval | V2 agent hooks and V3 hooks | Plugin list in Settings | Extension list in Settings | Named command groups, toggles, tests |
| Settings | User/project/local JSON | User/trusted project TOML | User/project CLI JSON | User/project JSON or JSONC | User/project JSON | User-wide CLI preferences |
| Chat archives | Export/import | Export/import | V2 ACP source backup only | Export/import through native v2 CLI | Export/import of v3 JSONL sessions | App/CLI export/import through agy 1.5.0 |
| Chat conversion | Source and destination | Source and destination | V2 ACP source only | Source and destination | Source and destination | Source and CLI destination |
| Chat deletion | With backup | With backup | Unavailable | Chats without children | With backup | With backup; unlinked, inactive chats |

History provides saved versions, restoration, backup deletion, and separate operation logs for supported local resources.

Orbit does not offer individual skill availability controls for Kiro, OpenCode, Pi, or Antigravity; removal saves a restorable version. Inherited resources outside the selected project are read-only. MCP editing covers user configuration; project and runtime overrides can affect the running agent.

Kiro's learned memories remain in its account service. OpenCode and Pi have no built-in generated-memory file catalog. OpenCode plugins and Pi extensions are configured in Settings; their code is managed outside the command-hook editor.

## Chats

### Browse and search

Open **Chats** to list sessions grouped by their recorded working directory. Orbit reads chat metadata first and loads a conversation when opened. Project groups collapse within the chat list.

Choose **Index chats** to build message search across the catalog. A progress bar shows indexing progress, and indexed chats are marked in the list. Opening a conversation also makes its messages searchable. **Search message text** searches indexed content. Choose **Refresh chats** to discover newly created sessions.

OpenCode v2 lists projects known to its native CLI, including child sessions. Pi lists its default session folders and the selected project's configured session directory. Select a project and refresh to include its custom directory; previously indexed custom projects remain visible. Unknown custom directories and explicit CLI session-directory overrides are not discovered automatically.

Kiro supports local V2 ACP sessions. V3, terminal, and cloud sessions are not included.

### Export and import

A **chat archive is a folder** containing the native session files and migration metadata. It differs from a [setup backup](#setup-export-and-import), which is a ZIP of configuration resources.

1. Open a chat and choose **Export [agent] archive**, then choose where to save it.
2. To import, select the matching agent and choose **Import [agent] archive**.
3. Choose the archive folder and the destination project.

Duplicate chat titles receive numeric suffixes. External files referenced by a chat are not bundled. Native archives preserve the source agent's session structure; **Export Markdown** produces a separate readable copy.

Kiro offers source backup export only. Use its interactive `/chat save` and `/chat load` commands for its native archive workflow. Antigravity import preserves the source App or CLI store and internal ID; an existing ID cannot be imported again.

### Convert between agents

Open a chat and choose **Convert chat**. Select the destination agent and optionally change the working directory. By default, conversion uses the source chat's recorded directory. A source without a working directory requires an explicit destination. The new chat is separate from the source, which stays unchanged.

Conversion transfers conversation text and supported inline images from the latest active branch. Other branches remain in the source. Historical tool activity is represented as text where supported; Antigravity tool steps are omitted. Signed reasoning, source system/developer instructions, and runtime state do not transfer. Conversion creates conversation history, not an executable copy of the source agent's tool state.

External attachments remain references; available Codex image archive assets can be embedded. The conversion dialog describes the content being transferred. Resolve partial OpenCode revert boundaries in OpenCode before converting. Kiro can be a source but not a destination.

### Delete and restore

Deleting a supported chat first saves a native archive in a folder you choose. Restore it using **Import [agent] archive**. Close the native chat before deleting it.

Kiro deletion is unavailable because its backups cannot be imported through Orbit. OpenCode chats with children cannot be deleted individually: native deletion would also remove the children. Antigravity requires the App and CLI sessions to be closed and refuses active or linked chats.

### Antigravity

Antigravity chat import, restoration, deletion, and incoming conversion require **agy 1.5.0**, with the App and CLI sessions closed. Other CLI versions can still use browsing and export for compatible local formats.

Orbit lists App and CLI chats from their local catalogs and reads conversation content on demand. Legacy IDE and cloud stores are excluded. An archive contains:

- A consistent `session.db` snapshot, including committed SQLite WAL changes.
- A checksummed `manifest.json` and native annotations.
- Files owned by the chat under its `brain/<id>` folder.

External files and installation-wide runtime state are excluded. Older database-only archives can be imported, but cannot restore artifacts they never contained.

Restoration preserves the original App or CLI store and internal chat ID. An already-present ID is refused; different chats with the same title receive numeric suffixes. Incoming conversion creates a new CLI chat and transfers user/assistant text and supported inline images. Tool steps, private reasoning, and external artifacts are omitted. Deletion requires a complete backup and refuses changed, active, parent/child, or battle-linked chats.

## Setup export and import

Choose **Export setup** in the header, select a project, and check the categories to save: skills, hooks, MCP servers, memories, or settings. **Select all** includes every available category. Choose **Save backup** to create a ZIP.

To import a setup:

1. Select the same agent the backup came from, choose **Import setup**, and open the ZIP. Cross-agent setup imports are not supported.
2. Choose **Current project** or **User-wide**, where available. **Change project** selects another folder; **Import destination** can keep user-wide and project items separate.
3. Expand the categories, check individual items, review their destinations, and choose **Import selected**.

| Item | Import behavior |
|---|---|
| Already up to date | Skipped automatically. |
| Different version exists | Kept unless you choose **Replace**. |
| Skills and memories | Selected files are added or replaced; other files stay in place. |
| Hooks and extension lists | Missing entries are added without duplicates. |
| Settings | Preferences are replaced. Current hooks and MCP servers stay unless selected too. |

Claude auto-memory stays attached to the destination project; Codex generated memory stays user-wide. Antigravity CLI preferences always remain user-wide, and shared and user-root instructions keep their native locations.

Replaced resources have restorable versions in **History**. Unsaved editor drafts remain open after import; refresh the page to load the imported files when ready. Import does not install packages or run scripts. Saved paths and commands are unchanged, so paths from another computer may need updating.

<details>
<summary>Archive contents and limits</summary>

The ZIP contains `metadata.json` identifying the agent and archive version, plus checksummed resource files. Login files, chats, caches, previous Orbit backups, and installed plugin packages are excluded.

Settings and hooks include user and selected-project sources, excluding inherited parent-project configuration. Shared skills are copied into the destination agent's native skills folder. Codex skill exports cover native user/project folders, including the older user skill directory. Kiro Markdown agent hook profiles are excluded. Memory exports include referenced local Markdown within the same memory or instruction root, excluding external links and non-Markdown attachments. Antigravity App preferences and internal knowledge are excluded. Agent trust and hook approvals remain unchanged.

Unrelated JSONC and TOML values and comments remain in place. Items targeting the same file or setting cannot be imported together. Each applied file is recorded in History; an interrupted import may have already applied earlier items.

Backups are limited to 10,000 items, 16 MiB per file, and 128 MiB overall. Unsafe paths, links, missing files, and checksum mismatches are rejected.

</details>

## Configuration and memory files

Select a project folder to inspect its instructions, skills, and settings. Orbit initially selects its launch directory. Skills copied into supported native folders are discovered without installation through Orbit.

| Agent | User configuration | Project configuration |
|---|---|---|
| Claude Code | Native Claude directory; honors `CLAUDE_CONFIG_DIR` | Claude instruction and `.claude` files |
| Codex | `CODEX_HOME`, default `~/.codex` | AGENTS instructions and `.codex` settings |
| Kiro | `KIRO_HOME`, default `~/.kiro` | `.kiro` and AGENTS files |
| OpenCode v2 | `XDG_CONFIG_HOME/opencode`, default `~/.config/opencode` | `opencode.json[c]` and `.opencode/opencode.json[c]` |
| Pi | `PI_CODING_AGENT_DIR`, default `~/.pi/agent` | `.pi` settings and instruction files |
| Antigravity | Shared `~/.gemini/config`; CLI preferences under `~/.gemini/antigravity-cli` | `.agents` customizations and native aliases |

### Instructions and generated memories

Claude auto-memory is project-scoped, including files stored under its user configuration directory. Linked files and instruction imports are shown alongside their source files.

Codex instructions follow `AGENTS.override.md`, `AGENTS.md`, and configured fallback precedence through the repository hierarchy. Orbit edits Markdown under `CODEX_HOME/memories`, including indexes, summaries, raw inputs, and supporting notes. Explicit project details organize sections in the UI; the underlying memory remains shared. Section edits share one draft with the whole-file editor, and saved edits have restorable versions in History. Git metadata and linked paths are excluded. Codex can regenerate memory files and replace manual edits; Orbit does not reset memory generation state.

Pi follows one instruction file per directory through the ancestor hierarchy. Project `SYSTEM.md` replaces the global system prompt; `APPEND_SYSTEM.md` adds instructions. Local skill folders and standalone Markdown skills are editable. Package expansion, glob filters, project trust, and authentication remain managed by Pi.

### Settings and hooks

Codex settings preserve TOML keys and comments; the preview includes user and trusted project sources. Plugin and system skills remain read-only. Approve Codex hook changes through its native `/hooks` flow.

OpenCode settings preserve JSONC comments. Remote configuration, instruction URLs, globs, and runtime plugin configuration are not expanded by Orbit's file editor. Pi's preview merges nested settings and combines resource lists. Command-line options, environment settings, profiles, and native policy can change runtime behavior beyond these file previews.

### Antigravity customizations

Shared files under `~/.gemini/config/` serve the App and CLI. Orbit also discovers CLI-only skill folders and project `.agents/`, `.agent/`, `_agents/`, and `_agent/` locations. Skill and rule discovery follows native manifests and inclusion/exclusion lists. Duplicate skill names stay visible; Antigravity determines which version loads.

Instructions include user `AGENTS.md` and `GEMINI.md`, shared-config equivalents, and project ancestors. Modular rules show their native trigger. Built-in resources, plugins, repository personal folders, and internal generated knowledge remain managed by Antigravity.

Settings edits `~/.gemini/antigravity-cli/settings.json`; App preferences stay in Antigravity. User MCP editing uses `~/.gemini/config/mcp_config.json`; project MCP files can be included in setup backups. Remote servers accept `serverUrl` or `url`. OAuth and runtime-dependent authentication are handled in Antigravity.

Hooks use named groups in `hooks.json`, without a top-level `hooks` wrapper. Toggling one command changes its whole group; commands sharing a matcher also share matcher edits. Tests run commands with the supplied JSON. Native hook payloads use camelCase fields.

See Antigravity's [customization discovery](https://antigravity.google/docs/customization-discovery), [rules](https://antigravity.google/docs/rules), [hooks](https://antigravity.google/docs/hooks), [MCP](https://antigravity.google/docs/mcp), and [CLI reference](https://antigravity.google/docs/cli/reference).

## Stored data and command execution

Orbit writes configuration changes directly to native files. Backups and change history live under `.orbit` in each agent's configuration directory. Claude's disabled MCP and hook records and transcript indexes use Orbit's app-data directory. Codex uses native enablement settings. Window preferences use the app-config directory. Orbit's bundle identifier is `com.roycoding8.orbit`.

Credential masking conceals known values in the interface; it does not encrypt files. Configuration, backups, and transcripts can contain credentials or private conversation content.

Hook tests execute shell commands with your account's permissions. MCP tests start configured processes or contact endpoints and may send configured credentials. These operations are not sandboxed. Pi tests exclude OAuth, dynamic command/environment references, and relative working directories that require Pi's runtime context.
