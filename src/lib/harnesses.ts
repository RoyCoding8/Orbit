export const clients = ["claude-code", "codex", "kiro", "opencode", "pi", "antigravity"] as const;
export type Client = typeof clients[number];
export type SkillState = "on" | "off" | "name-only" | "user-invocable-only";

type HarnessProfile = {
  label: string;
  archiveLabel: string;
  settingsLayers: readonly ("user" | "project" | "local")[];
  projectSkills: boolean;
  skillModes: readonly SkillState[];
  skillsNote: string;
  memoriesNote: string;
  connectionsNote: string;
  configNote: string;
  modelExample: string;
  transcripts: { archive: "migration" | "backup"; scope: "all" | "project"; note: string };
  hooks: { kind: "commands"; events: readonly string[]; canToggle: boolean; note: string; userPath: string; projectPath: string }
    | { kind: "plugins"; reason: string };
};

const claudeEvents = ["PreToolUse", "PostToolUse", "Notification", "UserPromptSubmit", "Stop", "SubagentStop", "PreCompact", "SessionStart", "SessionEnd"];

export const harnesses: Record<Client, HarnessProfile> = {
  "claude-code": {
    label: "Claude Code", archiveLabel: "Claude", settingsLayers: ["user", "project", "local"], projectSkills: false,
    skillModes: ["on", "name-only", "user-invocable-only", "off"], skillsNote: "",
    memoriesNote: "Edit the project-scoped memory and instruction files Claude Code reads. Auto-memory files are project-specific even when stored under the Claude config directory.",
    connectionsNote: "Claude Code follows @file imports in instruction files up to four levels. Ordinary Markdown links do not import context.",
    configNote: "", modelExample: '"sonnet"', transcripts: { archive: "migration", scope: "all", note: "" },
    hooks: { kind: "commands", events: claudeEvents, canToggle: true, note: "", userPath: "User settings.json in the configured Claude Code directory", projectPath: ".claude/settings.json" },
  },
  codex: {
    label: "Codex", archiveLabel: "Codex", settingsLayers: ["user", "project"], projectSkills: true,
    skillModes: ["on", "off"], skillsNote: "Codex loads repository and user skills. Restart Codex after changing skill availability.",
    memoriesNote: "Edit Codex memory Markdown and AGENTS.md instructions. Memories are shared across projects; instructions keep their native scope. Each saved edit has a restorable version in History.",
    connectionsNote: "Codex combines AGENTS.md instructions from the repository root to the working directory. Ordinary Markdown links do not import context.",
    configNote: "Preview of user settings and trusted project files from the repository root to the selected folder. Profiles, command-line flags, environment and system overrides are not resolved here.",
    modelExample: '"gpt-5-codex"',
    transcripts: { archive: "migration", scope: "all", note: "" },
    hooks: { kind: "commands", events: ["PreToolUse", "PermissionRequest", "PostToolUse", "PreCompact", "PostCompact", "SessionStart", "SessionEnd", "SubagentStart", "SubagentStop", "UserPromptSubmit", "Stop", "Interrupt"], canToggle: false,
      note: "Codex reviews hook definitions before running them. Use /hooks in Codex to approve, enable or disable hooks. Editing a definition requires a new review.",
      userPath: "User hooks in CODEX_HOME/config.toml and hooks.json", projectPath: ".codex/config.toml and hooks.json" },
  },
  kiro: {
    label: "Kiro", archiveLabel: "Kiro", settingsLayers: ["user", "project"], projectSkills: true,
    skillModes: [], skillsNote: "Kiro discovers user and project skills from disk. It has no native skill availability switch. Uninstall removes a skill directory and saves a restorable version.",
    memoriesNote: "Edit Kiro steering files and AGENTS.md instructions in their user or project scope. Kiro's learned memories are managed by its account service and are not editable files.",
    connectionsNote: "Kiro steering uses inclusion rules and file references. Each file shows its native loading behavior; ordinary Markdown links remain navigation links.",
    configNote: "Preview of local user and selected-project settings. Agent profiles, account settings and command-line overrides can change the running session.", modelExample: '"auto"',
    transcripts: { archive: "backup", scope: "all", note: "Orbit indexes local Kiro V2 ACP session files and exports their native source files as backups. V3, terminal and cloud sessions are not indexed. These backups cannot be imported through Orbit; Kiro chat archive save/load requires its interactive /chat commands." },
    hooks: { kind: "commands", events: [], canToggle: false, note: "Kiro V2 hooks belong to agent profiles. V3 also supports standalone hooks. Choose a native hook source to use its supported events.", userPath: "Kiro agent profiles and standalone hooks", projectPath: ".kiro/agents and .kiro/hooks" },
  },
  opencode: {
    label: "OpenCode v2", archiveLabel: "OpenCode", settingsLayers: ["user", "project"], projectSkills: true,
    skillModes: [], skillsNote: "OpenCode v2 discovers native and shared skill folders. It has no per-skill availability switch; permissions and runtime plugins can affect access. Uninstall saves a restorable version.",
    memoriesNote: "Edit OpenCode v2 AGENTS.md and configured instruction files. OpenCode has no native generated-memory file catalog; plugin storage belongs to each plugin.",
    connectionsNote: "OpenCode v2 loads AGENTS.md and configured instructions. Ordinary Markdown links do not import context.",
    configNote: "Preview of local configuration files. Remote, system, environment and runtime plugin configuration can affect the running session. JSONC comments are preserved when saving.", modelExample: '"openai/gpt-5"',
    transcripts: { archive: "migration", scope: "all", note: "Chats are discovered across projects known to OpenCode v2, including child sessions, using its installed native CLI. Deleting a parent with child sessions is unavailable because native deletion would also remove the children." },
    hooks: { kind: "plugins", reason: "OpenCode v2 hooks are JavaScript plugins using @opencode/plugin. Orbit's command-hook builder cannot edit or test plugin code. Manage the native plugins setting on the Settings page." },
  },
  antigravity: {
    label: "Antigravity", archiveLabel: "Antigravity", settingsLayers: ["user"], projectSkills: true,
    skillModes: [], skillsNote: "Shared skills work in the Antigravity App and CLI. CLI-only skill folders are also discovered. Inherited skills are read-only. Uninstall saves a restorable version; there is no verified per-skill availability switch.",
    memoriesNote: "Edit AGENTS.md, GEMINI.md, and modular rules. Antigravity shares these instructions between the App and CLI. Internal knowledge storage is managed by Antigravity.",
    connectionsNote: "Rules show their native trigger. Markdown links provide navigation; they do not import context.",
    configNote: "CLI preferences are user-wide. App and project preferences remain managed in Antigravity. MCP servers, hooks, skills, and instructions use shared native files.",
    modelExample: '\"gemini-3.1-pro\"',
    transcripts: { archive: "migration", scope: "all", note: "Browse App and CLI chats. Archives preserve the native database, annotations, and chat artifacts. Close Antigravity before import, restore, conversion into Antigravity, or deletion; these actions require agy 1.5.0. Restore keeps the original App or CLI store; converted chats go to the CLI. An existing internal chat ID is never overwritten. External files and installation-wide runtime state are not bundled." },
    hooks: { kind: "commands", events: ["SessionStart", "PreToolUse", "PostToolUse", "PreInvocation", "PostInvocation", "Stop"], canToggle: true,
      note: "Hooks are shared between the App and CLI. Toggling a hook changes its entire native named group, including other commands in that group. Inherited and plugin hooks are managed in Antigravity.", userPath: "~/.gemini/config/hooks.json", projectPath: ".agents/hooks.json" },
  },
  pi: {
    label: "Pi", archiveLabel: "Pi", settingsLayers: ["user", "project"], projectSkills: true,
    skillModes: [], skillsNote: "Pi discovers skills from its native and shared folders. Resource filters, packages and project trust can change availability. Orbit edits local files; it does not change Pi's trust decisions.",
    memoriesNote: "Edit Pi's AGENTS and CLAUDE instructions, plus system prompt files. Pi has no built-in generated-memory file catalog. Ancestor instructions outside the selected project are read-only.",
    connectionsNote: "Pi loads one context file per directory, with AGENTS.override.md taking precedence. Ordinary Markdown links are navigation links.",
    configNote: "Preview of local user and project files. Pi merges nested settings and combines resource lists. Project trust, packages, extensions and command-line options can change runtime behavior. Orbit does not edit trust or authentication state.",
    modelExample: '"gpt-5"',
    transcripts: { archive: "migration", scope: "project", note: "Discover Pi version 3 JSONL chats across its default session folders. Choose a project to also include its custom session directory. Archives preserve native records and inline images. External files referenced by a chat are not bundled." },
    hooks: { kind: "plugins", reason: "Pi lifecycle hooks are TypeScript or JavaScript extensions. Orbit's command-hook builder cannot edit or test extension code. Manage the native extensions list on the Settings page." },
  },
};

export function isClient(value: string): value is Client {
  return clients.some(client => client === value);
}
