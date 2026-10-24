import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { InvokeArgs } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { Hooks } from "./Hooks";

const hook = {
  id: "user:PreToolUse:content-identity",
  event: "PreToolUse",
  matcher: "Read",
  commands: ["node check.js"],
  enabled: true,
  layer: "user",
  source_path: "settings.json",
  editable: true,
};

describe("hook mutation identity", () => {
  it("filters by hook metadata and commands and lets the user expand details", async () => {
    mockIPC(command => command === "hooks_list" ? [{ ...hook, matcher: "Write", commands: ["node verify-long-command.js"] }] : []);
    render(<Hooks />);
    const search = await screen.findByRole("searchbox", { name: "Search hooks" });
    fireEvent.change(search, { target: { value: "VERIFY-LONG" } });
    expect(screen.getByRole("heading", { name: "PreToolUse" })).toBeInTheDocument();
    expect(screen.getByText("Showing 1 of 1 hooks")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Search hooks" }), { target: { value: "does-not-match" } });
    expect(screen.getByText('No hooks match "does-not-match".')).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
    fireEvent.click(screen.getByText("Hook details"));
    expect(screen.getByText("node verify-long-command.js")).toBeInTheDocument();
    expect(screen.getByText("node verify-long-command.js").closest("details")).toHaveAttribute("open");
  });

  it("disables the displayed identity without converting it to an array index", async () => {
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "hooks_list") return [hook];
      if (command === "hooks_disable") return null;
      throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
    });
    mockIPC(ipc);
    render(<Hooks />);

    fireEvent.click(await screen.findByRole("button", { name: "Disable" }));

    await waitFor(() => expect(ipc).toHaveBeenCalledWith("hooks_disable", expect.objectContaining({
      id: hook.id,
      projectRoot: null,
    })));
    const calls = ipc.mock.calls.filter(([command]) => command === "hooks_disable");
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).not.toHaveProperty("index");
  });

  it("locks project and scope changes while a mutation is pending", async () => {
    let finish: () => void = () => {};
    const pending = new Promise<void>(resolve => { finish = resolve; });
    mockIPC((command: string) => {
      if (command === "hooks_list") return [hook];
      if (command === "hooks_disable") return pending;
      throw new Error(`Unexpected command ${command}`);
    });
    render(<Hooks />);

    fireEvent.click(await screen.findByRole("button", { name: "Disable" }));

    expect(screen.getByRole("button", { name: "Choose project" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Refresh" })).toBeDisabled();
    expect(screen.getByLabelText("Settings scope")).toBeDisabled();
    expect(screen.getByLabelText("Event")).toBeDisabled();
    finish();
    await waitFor(() => expect(screen.getByRole("button", { name: "Choose project" })).toBeEnabled());
  });

  it("deletes the displayed identity rather than a parsed position", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "hooks_list") return [hook];
      if (command === "hooks_delete") return null;
      throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
    });
    mockIPC(ipc);
    render(<Hooks />);

    fireEvent.click(await screen.findByRole("button", { name: "Delete" }));

    await waitFor(() => expect(ipc).toHaveBeenCalledWith("hooks_delete", expect.objectContaining({
      id: hook.id,
      projectRoot: null,
    })));
    const calls = ipc.mock.calls.filter(([command]) => command === "hooks_delete");
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).not.toHaveProperty("index");
  });

  it("does not offer a single-command editor for a multi-command hook", async () => {
    mockIPC(command => {
      if (command === "hooks_list") return [{ ...hook, commands: ["node first.js", "node second.js"] }];
      throw new Error(`Unexpected command ${command}`);
    });
    render(<Hooks />);
    expect(await screen.findByText("node first.js")).toBeInTheDocument();
    expect(screen.getByText("node second.js")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Disable" })).toBeEnabled();
  });
});
