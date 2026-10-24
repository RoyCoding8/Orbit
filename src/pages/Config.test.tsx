import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { InvokeArgs } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { Config } from "./Config";

const SNAPSHOT = {
  layers: [
    { name: "user", path: "D:/fixture-config/settings.json", values: { model: "sonnet" } },
    { name: "project", path: "D:/launch/.claude/settings.json", values: {} },
    { name: "local", path: "D:/launch/.claude/settings.local.json", values: {} },
  ],
  effective: { model: { value: "sonnet", source: "user" } },
};

const SNAPSHOT_B = {
  layers: [
    { name: "user", path: "D:/fixture-config/settings.json", values: { model: "haiku" } },
    { name: "project", path: "D:/project-b/.claude/settings.json", values: { model: "haiku" } },
    { name: "local", path: "D:/project-b/.claude/settings.local.json", values: {} },
  ],
  effective: { model: { value: "haiku", source: "project" } },
};

function ipcFor(load: (payload?: InvokeArgs) => unknown, chooseProject: () => unknown = () => null) {
  return vi.fn((command: string, payload?: InvokeArgs) => {
    if (command === "config_load") return load(payload);
    if (command === "project_choose") return chooseProject();
    throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
  });
}

describe("Config project selection", () => {
  it("displays the resolved native settings path instead of guessing the user directory", async () => {
    mockIPC(ipcFor(() => SNAPSHOT));
    render(<Config />);

    expect(await screen.findByText("D:/fixture-config/settings.json")).toBeInTheDocument();
    expect(screen.queryByText("~/.claude/settings.json")).not.toBeInTheDocument();
  });

  it("preserves the launch directory when the native picker is cancelled", async () => {
    const ipc = ipcFor(() => SNAPSHOT);
    mockIPC(ipc);
    render(<Config />);

    await screen.findByText('"sonnet"');
    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));

    await waitFor(() => expect(ipc).toHaveBeenCalledWith("project_choose", {}));
    expect(ipc.mock.calls.filter(([command]) => command === "config_load")).toHaveLength(1);
    fireEvent.click(screen.getByRole("tab", { name: "Project" }));
    expect(screen.getByText("D:/launch/.claude/settings.json")).toBeInTheDocument();
  });
});

describe("effective settings search", () => {
  it("matches keys, sources, and nested JSON values while leaving the editor draft intact", async () => {
    const snapshot = {
      ...SNAPSHOT,
      effective: {
        model: { value: "sonnet", source: "user" },
        permissions: { value: { allow: ["Read", "Write"] }, source: "project" },
      },
    };
    mockIPC(ipcFor(() => snapshot));
    render(<Config />);

    const search = await screen.findByRole("searchbox", { name: "Search effective settings" });
    fireEvent.click(screen.getByRole("button", { name: "permissions" }));
    fireEvent.change(screen.getByLabelText("JSON value"), { target: { value: '{"allow":["Read"]}' } });
    fireEvent.change(search, { target: { value: "Write" } });

    expect(screen.getByRole("button", { name: "permissions" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "model" })).not.toBeInTheDocument();
    expect(screen.getByText("1 of 2 keys")).toBeInTheDocument();
    expect(screen.getByLabelText("JSON value")).toHaveValue('{"allow":["Read"]}');

    fireEvent.change(search, { target: { value: "project" } });
    expect(screen.getByRole("button", { name: "permissions" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "model" })).not.toBeInTheDocument();
    fireEvent.change(search, { target: { value: "no-such-setting" } });
    expect(screen.getByRole("status")).toHaveTextContent('No effective settings match "no-such-setting".');
    expect(screen.getByLabelText("JSON value")).toHaveValue('{"allow":["Read"]}');

    fireEvent.click(screen.getByRole("button", { name: "Clear settings search" }));
    expect(screen.getByRole("button", { name: "model" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "permissions" })).toBeInTheDocument();
    expect(screen.getByLabelText("JSON value")).toHaveValue('{"allow":["Read"]}');
    fireEvent.change(search, { target: { value: "model" } });
    expect(screen.getByRole("button", { name: "model" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "permissions" })).not.toBeInTheDocument();
  });
});

describe("Config undo", () => {
  it("restores the selected key and layer through the native command", async () => {
    let undone = false;
    const ipc = vi.fn((command: string) => {
      if (command === "config_load") return undone ? SNAPSHOT_B : SNAPSHOT;
      if (command === "config_undo") { undone = true; return null; }
      throw new Error(`Unexpected command ${command}`);
    });
    mockIPC(ipc);
    render(<Config />);
    fireEvent.click(await screen.findByRole("button", { name: "model" }));
    fireEvent.click(screen.getByRole("button", { name: "Undo last change to this key" }));
    await waitFor(() => expect(ipc).toHaveBeenCalledWith("config_undo", { layer: "user", key: "model", projectRoot: null }));
    expect(await screen.findByText('"haiku"')).toBeInTheDocument();
  });
});

describe("Config snapshot after failed reload", () => {
  it("clears the stale table and keeps the error beside it when the new project fails to load", async () => {
    let loads = 0;
    mockIPC(ipcFor(() => {
      loads += 1;
      if (loads >= 2) return Promise.reject(new Error("settings.local.json is not valid JSON"));
      return SNAPSHOT;
    }, () => "D:/project-b"));
    render(<Config />);

    await screen.findByText('"sonnet"');
    expect(screen.getByText("model")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
    await screen.findByText(/not valid JSON/);

    expect(screen.queryByText("model")).not.toBeInTheDocument();
    expect(screen.queryByText("No settings found in the available layers.")).not.toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("settings.local.json is not valid JSON");

    fireEvent.click(screen.getByRole("tab", { name: "Project" }));
    expect(screen.queryByText("D:/launch/.claude/settings.json")).not.toBeInTheDocument();
    expect(screen.queryByText("model")).not.toBeInTheDocument();
  });

  it("clears the pending key and value draft when the snapshot reloads", async () => {
    const ipc = ipcFor(() => SNAPSHOT);
    mockIPC((command, payload) => ipc(command, payload));
    render(<Config />);

    await screen.findByText('"sonnet"');
    fireEvent.click(screen.getByRole("button", { name: "model" }));
    expect(screen.getByLabelText("Setting key")).toHaveValue("model");
    expect(screen.getByLabelText("JSON value")).toHaveValue('"sonnet"');

    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(ipc.mock.calls.filter(([command]) => command === "config_load").length).toBe(2));

    expect(screen.getByLabelText("Setting key")).toHaveValue("");
    expect(screen.getByLabelText("JSON value")).toHaveValue("");
    expect(screen.getByText('"sonnet"')).toBeInTheDocument();
  });

  it("keeps project and refresh controls disabled until an apply finishes", async () => {
    let finishApply: () => void = () => {};
    mockIPC((command) => {
      if (command === "config_load") return SNAPSHOT;
      if (command === "config_apply") return new Promise<void>((resolve) => { finishApply = resolve; });
      throw new Error(`Unexpected command ${command}`);
    });
    render(<Config />);

    await screen.findByText('"sonnet"');
    fireEvent.click(screen.getByRole("button", { name: "model" }));
    fireEvent.click(screen.getByRole("button", { name: "Apply change" }));

    await waitFor(() => expect(screen.getByRole("button", { name: "Choose project" })).toBeDisabled());
    expect(screen.getByRole("button", { name: "Refresh" })).toBeDisabled();
    await act(async () => { finishApply(); });
    await waitFor(() => expect(screen.getByRole("button", { name: "Choose project" })).not.toBeDisabled());
    expect(screen.getByRole("button", { name: "Refresh" })).not.toBeDisabled();
  });

  it("ignores a slow A snapshot that resolves after B already succeeded", async () => {
    let resolveA: (snapshot: typeof SNAPSHOT) => void = () => {};
    mockIPC(ipcFor((payload) => {
      if (payload && "projectRoot" in payload && payload.projectRoot === "D:/project-b") {
        return SNAPSHOT_B;
      }
      return new Promise((resolve) => { resolveA = resolve; });
    }, () => "D:/project-b"));
    render(<Config />);

    await screen.findByRole("heading", { name: "Configuration" });
    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));

    expect(await screen.findByText('"haiku"')).toBeInTheDocument();

    await act(async () => {
      resolveA(SNAPSHOT);
    });

    expect(screen.queryByText('"sonnet"')).not.toBeInTheDocument();
    expect(screen.getByText('"haiku"')).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps B's error visible when a slow A load rejects after B failed", async () => {
    let rejectA: (cause: unknown) => void = () => {};
    mockIPC(ipcFor((payload) => {
      if (payload && "projectRoot" in payload && payload.projectRoot === "D:/project-b") {
        return Promise.reject(new Error("settings.local.json is not valid JSON"));
      }
      return new Promise((_, reject) => { rejectA = reject; });
    }, () => "D:/project-b"));
    render(<Config />);

    await screen.findByRole("heading", { name: "Configuration" });
    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
    await screen.findByText(/not valid JSON/);

    await act(async () => {
      rejectA(new Error("stale A failure"));
    });

    expect(screen.getByRole("alert")).toHaveTextContent("settings.local.json is not valid JSON");
    expect(screen.queryByText(/stale A failure/)).not.toBeInTheDocument();
  });

  it("does not surface a stale A rejection after B succeeded", async () => {
    let rejectA: (cause: unknown) => void = () => {};
    mockIPC(ipcFor((payload) => {
      if (payload && "projectRoot" in payload && payload.projectRoot === "D:/project-b") {
        return SNAPSHOT_B;
      }
      return new Promise((_, reject) => { rejectA = reject; });
    }, () => "D:/project-b"));
    render(<Config />);

    await screen.findByRole("heading", { name: "Configuration" });
    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));

    expect(await screen.findByText('"haiku"')).toBeInTheDocument();

    await act(async () => {
      rejectA(new Error("stale A failure"));
    });

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText('"haiku"')).toBeInTheDocument();
  });
});
