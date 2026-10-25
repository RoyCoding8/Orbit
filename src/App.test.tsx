import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { App } from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
});

function useSystemTheme(isDark: boolean) {
  vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({
    matches: isDark,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }));
}

describe("App shell", () => {
  it.each([
    { systemDark: false, next: "dark" },
    { systemDark: true, next: "light" },
  ])("follows the system theme and toggles from its current appearance", ({ systemDark, next }) => {
    useSystemTheme(systemDark);
    const view = render(<App />);
    expect(view.container.querySelector("m3e-theme")).toHaveProperty("scheme", "auto");
    expect(view.container.querySelector(".application-shell")).toHaveAttribute("data-theme", "system");

    fireEvent.click(screen.getByRole("button", { name: "Toggle light and dark theme" }));

    expect(view.container.querySelector("m3e-theme")).toHaveProperty("scheme", next);
    expect(view.container.querySelector(".application-shell")).toHaveAttribute("data-theme", next);
    expect(localStorage.getItem("orbit.theme")).toBe(next);
  });

  it("loads and persists a valid preference across remounts", () => {
    localStorage.setItem("orbit.theme", "dark");
    const first = render(<App />);
    expect(first.container.querySelector("m3e-theme")).toHaveProperty("scheme", "dark");
    fireEvent.click(screen.getByRole("button", { name: "Toggle light and dark theme" }));
    expect(first.container.querySelector("m3e-theme")).toHaveProperty("scheme", "light");
    first.unmount();

    const second = render(<App />);
    expect(second.container.querySelector("m3e-theme")).toHaveProperty("scheme", "light");
    fireEvent.click(screen.getByRole("button", { name: "Toggle light and dark theme" }));
    expect(second.container.querySelector("m3e-theme")).toHaveProperty("scheme", "dark");
    expect(localStorage.getItem("orbit.theme")).toBe("dark");
  });

  it("falls back to system theme for an invalid preference", () => {
    localStorage.setItem("orbit.theme", "invalid");
    useSystemTheme(true);
    const view = render(<App />);
    expect(view.container.querySelector("m3e-theme")).toHaveProperty("scheme", "auto");
    fireEvent.click(screen.getByRole("button", { name: "Toggle light and dark theme" }));
    expect(view.container.querySelector("m3e-theme")).toHaveProperty("scheme", "light");
    expect(localStorage.getItem("orbit.theme")).toBe("light");
  });

  it("stays usable when storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("Storage unavailable"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("Storage unavailable"); });
    useSystemTheme(false);
    const view = render(<App />);
    expect(view.container.querySelector("m3e-theme")).toHaveProperty("scheme", "auto");
    fireEvent.click(screen.getByRole("button", { name: "Toggle light and dark theme" }));
    expect(view.container.querySelector("m3e-theme")).toHaveProperty("scheme", "dark");
    expect(screen.getByRole("heading", { name: "Orbit" })).toBeInTheDocument();
  });

  it("renders the M3E shell with navigation, heading, and an empty pane", () => {
    render(<App />);

    expect(screen.getByRole("heading", { name: "Orbit" })).toBeTruthy();
    expect(screen.getByLabelText("Main navigation")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Claude Code" })).toBeTruthy();
    expect(screen.getByText("Manage the files, tools, settings, and chats used by this coding agent.")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Coding agent"), { target: { value: "codex" } });
    expect(screen.getByRole("heading", { name: "Codex" })).toBeTruthy();
    expect(screen.getByText("Overview")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Connect Claude Code" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add harness" })).toBeNull();
    expect(screen.getByRole("button", { name: "Open skills" })).toBeTruthy();
  });

  it("opens skills and settings from the overview actions", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      switch (command) {
        case "skills_inventory":
          return [] as never;
        case "skills_ledger":
          return [] as never;
        case "config_load":
          return {
            layers: [
              { name: "user", path: "D:/fixture-config/settings.json", values: { model: "sonnet" } },
              { name: "project", path: "D:/launch/.claude/settings.json", values: {} },
              { name: "local", path: "D:/launch/.claude/settings.local.json", values: {} },
            ],
            effective: { model: { value: "sonnet", source: "user" } },
          } as never;
        default:
          return null as never;
      }
    });
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: "Open skills" }));
    expect(screen.getByRole("heading", { name: "Skills" })).toBeTruthy();

    cleanup();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    expect(await screen.findByRole("heading", { name: "Configuration" })).toBeTruthy();
    expect(await screen.findByText('"sonnet"')).toBeTruthy();
  });

  it.each([
    { nav: /Chats/, heading: "Chats", action: "Refresh chats", empty: "No chats found in this source.", input: "Search message text" },
    { nav: /^Skills$/, heading: "Skills", action: "Install skill", empty: "No skills found", input: null },
    { nav: /Servers/, heading: "MCP servers", action: "Add server", empty: "Connect a tool server", input: null },
    { nav: /Memories/, heading: "Memories & instructions", action: "Choose project", empty: "No memory files found", input: null },
    { nav: /History/, heading: "History", action: "Choose project", empty: "No saved versions found.", input: null },
  ])("opens $heading from navigation", async ({ nav, heading, action, empty, input }) => {
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "memory_catalog") return { projectRoot: "D:/project", groups: [], files: [], excludedCustomMemoryDirectory: null };
      if (command === "history_load") return { versions: [], logs: [] };
      if (command === "transcript_list_sessions") return {sessions: [], source: "D:/native-chats", warnings: []};
      return [];
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: nav }));
    expect(await screen.findByText(empty)).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: heading })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: action })).toBeInTheDocument();
    if (input) expect(screen.getByLabelText(input)).toBeInTheDocument();
  });

  it("keeps a destructive hook unsaved when confirmation is declined", async () => {
    vi.mocked(invoke).mockResolvedValue([] as never);
    vi.spyOn(window, "confirm").mockReturnValue(false);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    await screen.findByRole("heading", { name: "Hooks" });
    vi.mocked(invoke).mockClear();

    const command = "rm -rf ./build";
    fireEvent.change(screen.getByLabelText("Command"), { target: { value: command } });
    fireEvent.click(screen.getByRole("button", { name: "Save hook" }));

    expect(window.confirm).toHaveBeenCalled();
    expect(screen.getByLabelText("Command")).toHaveValue(command);
    expect(vi.mocked(invoke).mock.calls.some(([name]) => name === "hooks_save")).toBe(false);
  });

  it("saves a hook to the selected project scope", async () => {
    let finishProjectLoad: (hooks: never[]) => void = () => {};
    const projectHooks = new Promise<never[]>(resolve => { finishProjectLoad = resolve; });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "project_choose") return "D:/demo-project";
      if (command === "hooks_list" && args && "projectRoot" in args && args.projectRoot === "D:/demo-project") return projectHooks;
      return [];
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    await screen.findByRole("heading", { name: "Hooks" });

    fireEvent.change(screen.getByLabelText("Settings scope"), { target: { value: "project" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Choose project" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
    expect(await screen.findByText("D:/demo-project/.claude/settings.json")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Save hook" })).toBeDisabled();
    await act(async () => { finishProjectLoad([]); });

    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "echo checked" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Save hook" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Save hook" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("hooks_save", {
      client: "claude-code",
      draft: {
        layer: "project",
        event: "PreToolUse",
        matcher: null,
        command: "echo checked",
        editId: null,
      },
      projectRoot: "D:/demo-project",
    }));
  });

  it("opens the effective configuration view with per-key source badges", async () => {
    vi.mocked(invoke).mockResolvedValue({
      layers: [
        { name: "user", path: "D:/fixture-config/settings.json", values: { model: "sonnet" } },
        { name: "project", path: "D:/launch/.claude/settings.json", values: {} },
        { name: "local", path: "D:/launch/.claude/settings.local.json", values: {} },
      ],
      effective: { model: { value: "sonnet", source: "user" } },
    } as never);
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));

    expect(await screen.findByRole("heading", { name: "Configuration" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "User" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Project" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Local" })).toBeTruthy();
    expect(await screen.findByText('"sonnet"')).toBeTruthy();
    expect(screen.getByText("user")).toBeTruthy();
  });

  it("switches clients without retaining settings or an unfinished hook edit", async () => {
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "config_load") {
        const model = args && "client" in args && args.client === "codex" ? "gpt-5-codex" : "sonnet";
        return { layers: [{ name: "user", path: "config", values: { model } }], effective: { model: { value: model, source: "user" } } };
      }
      return [];
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    expect(await screen.findByText('"sonnet"')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Coding agent"), { target: { value: "codex" } });
    expect(await screen.findByText('"gpt-5-codex"')).toBeInTheDocument();
    expect(screen.queryByText('"sonnet"')).toBeNull();
    expect(screen.queryByRole("tab", { name: "Local" })).toBeNull();
    expect(invoke).toHaveBeenCalledWith("config_load", { client: "codex", projectRoot: null });

    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "echo draft" } });
    fireEvent.change(screen.getByLabelText("Coding agent"), { target: { value: "claude-code" } });
    expect(screen.getByLabelText("Command")).toHaveValue("");
  });

  it("leaves Codex hook trust native and saves generated memory through the Codex client", async () => {
    let content = "Generated summary.";
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "hooks_list") return [{ id: "hook", event: "Stop", matcher: null, commands: ["echo done"], enabled: true, layer: "user", source_path: "D:/codex/hooks.json", editable: true }];
      if (command === "memory_catalog") return {
        projectRoot: "D:/project", excludedCustomMemoryDirectory: null,
        groups: [{ id: "generated", label: "Generated memory", role: "globalAutoMemory" }],
        files: [{ id: "memory", groupId: "generated", name: "memory_summary.md", path: "D:/codex/memories/memory_summary.md", source: "globalAutoMemory", role: "summary", loadBehavior: "High-level overview of saved memories.", editable: true, content, references: [] }],
      };
      if (command === "memory_edit") content = "Corrected summary.";
      return [];
    });
    render(<App />);
    fireEvent.change(screen.getByLabelText("Coding agent"), { target: { value: "codex" } });
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    expect(await screen.findByText("echo done")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Disable" })).toBeNull();
    expect(screen.getByRole("option", { name: "Interrupt" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Memories" }));
    expect(await screen.findByLabelText("Markdown content")).toBeEnabled();
    expect(screen.getByLabelText("Markdown content")).toHaveValue("Generated summary.");
    expect(screen.getByRole("button", { name: "Delete file" })).toBeEnabled();
    fireEvent.click(screen.getByText("File details"));
    expect(screen.getByRole("note")).toHaveTextContent("Codex may regenerate this file");
    expect(invoke).toHaveBeenCalledWith("memory_catalog", { client: "codex", projectRoot: null });
    fireEvent.change(screen.getByLabelText("Markdown content"), { target: { value: "Corrected summary." } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("memory_edit", {
      client: "codex", projectRoot: "D:/project", id: "memory", expectedContent: "Generated summary.", content: "Corrected summary.",
    }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled());
    expect(screen.getByLabelText("Markdown content")).toHaveValue("Corrected summary.");
  });

  it("uses Kiro's native hook owner and hides unavailable skill and archive controls", async () => {
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "skills_inventory") return [{ id: "kiro-skill", name: "Kiro skill", description: "Native skill", path: "D:/kiro/skills/review/SKILL.md", content: "Review", state: "on", editable: true }];
      if (command === "hooks_sources") return [{ id: "D:/kiro/agents/reviewer.json", label: "Reviewer agent", layer: "user", events: ["userPromptSubmit"], canToggle: false }];
      return [];
    });
    render(<App />);
    fireEvent.change(screen.getByLabelText("Coding agent"), { target: { value: "kiro" } });
    fireEvent.click(screen.getByRole("button", { name: /^Skills$/ }));
    expect(await screen.findByText("Kiro skill")).toBeInTheDocument();
    expect(screen.queryByLabelText("Enable kiro-skill")).toBeNull();
    expect(screen.queryByLabelText("Visibility for kiro-skill")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    expect(await screen.findByRole("option", { name: "Reviewer agent" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "echo checked" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Save hook" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Save hook" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("hooks_save", {
      client: "kiro", projectRoot: null,
      draft: { layer: "user", event: "userPromptSubmit", matcher: null, command: "echo checked", editId: null, sourceId: "D:/kiro/agents/reviewer.json" },
    }));
    fireEvent.click(screen.getByRole("button", { name: "Chats" }));
    expect(screen.getByRole("button", { name: "Import Kiro archive" })).toBeDisabled();
    expect(screen.getByText(/V3, terminal and cloud sessions are not indexed/)).toBeInTheDocument();
  });

  it.each([
    { client: "opencode", label: "OpenCode", hookText: /@opencode\/plugin/ },
    { client: "pi", label: "Pi", hookText: /TypeScript or JavaScript extensions/ },
  ])("routes $label settings, extensions and project archives", async ({ client, label, hookText }) => {
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "config_load") return { layers: [{ name: "user", path: "D:/config/opencode.jsonc", values: { model: "openai/example" } }], effective: { model: { value: "openai/example", source: "user" } } };
      if (command === "project_choose") return "D:/chosen-project";
      if (command === "transcript_list_sessions") return {sessions: [], source: "D:/native-chats", warnings: []};
      return [];
    });
    render(<App />);
    fireEvent.change(screen.getByLabelText("Coding agent"), { target: { value: client } });
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    expect(await screen.findByText('"openai/example"')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("config_load", { client, projectRoot: null });
    expect(screen.queryByRole("tab", { name: "Local" })).toBeNull();
    vi.mocked(invoke).mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    expect(screen.getByText(hookText)).toBeInTheDocument();
    expect(screen.queryByLabelText("Command")).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Chats" }));
    await waitFor(() => expect(screen.getByRole("button", { name: `Import ${label} archive` })).toBeEnabled());
    if (client === "pi") {
      fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
      expect(await screen.findByText("Additional project: D:/chosen-project")).toBeInTheDocument();
    } else expect(screen.queryByRole("button", { name: "Choose project" })).toBeNull();
    await waitFor(() => expect(screen.getByRole("button", { name: "Refresh chats" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Refresh chats" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("transcript_list_sessions", { client, refresh: true, projectRoot: client === "pi" ? "D:/chosen-project" : null }));
    expect(await screen.findByText(/Found 0 .* chats/)).toBeInTheDocument();
  });
});
