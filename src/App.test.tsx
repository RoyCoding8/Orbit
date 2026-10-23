import { describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { App } from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

describe("App shell", () => {
  it("renders the M3E shell with navigation, heading, and an empty pane", () => {
    render(<App />);

    expect(screen.getByRole("heading", { name: "Orbit" })).toBeTruthy();
    expect(screen.getByLabelText("Main navigation")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Your harness, in view." })).toBeTruthy();
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
    { nav: /Transcripts/, heading: "Transcripts", action: "Index transcripts", empty: "No sessions indexed yet.", input: "Search message text" },
    { nav: /^Skills$/, heading: "Skills", action: "Install skill", empty: "No skills found", input: null },
    { nav: /Servers/, heading: "MCP servers", action: "Add server", empty: "Connect a tool server", input: null },
    { nav: /Memories/, heading: "Memories & instructions", action: "Choose project", empty: "No memory files found", input: null },
    { nav: /History/, heading: "History", action: "Choose project", empty: "No saved versions match this scope.", input: null },
  ])("opens $heading from navigation", async ({ nav, heading, action, empty, input }) => {
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "memory_catalog") return { projectRoot: "D:/project", groups: [], files: [], excludedCustomMemoryDirectory: null };
      if (command === "history_load") return { versions: [], logs: [] };
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
    vi.mocked(invoke).mockImplementation(async (command) => command === "project_choose" ? "D:/demo-project" : []);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Hooks" }));
    await screen.findByRole("heading", { name: "Hooks" });

    fireEvent.change(screen.getByLabelText("Settings scope"), { target: { value: "project" } });
    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
    expect(await screen.findByText("D:/demo-project/.claude/settings.json")).toBeTruthy();

    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "echo checked" } });
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
});
