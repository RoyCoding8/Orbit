import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { it, expect, vi } from "vitest";
import { History } from "./History";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const version = {
  id: "operation-1", timestamp: 1, resourceKind: "memory", resourceId: "notes",
  action: "edit", scope: "project", scopeId: "project", scopeLabel: "Project memory",
  target: "D:/project/notes.md", kind: "file", checksum: "saved", currentChecksum: "current",
  backupStatus: "available", restorable: true,
};
const log = { ...version, status: "applied" };

it("displays ordinary Windows paths in versions, logs and restore prompts while preserving project identity", async () => {
  const projectRoot = "\\\\?\\D:\\AI\\Orbit";
  const target = `${projectRoot}\\AGENTS.md`;
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "history_load") return {
      versions: [{ ...version, scopeLabel: projectRoot, target }],
      logs: [{ ...log, scopeLabel: projectRoot, target }],
    };
    if (command === "project_choose") return projectRoot;
    if (command === "history_restore") return null;
    throw new Error(command);
  });
  render(<History />);
  expect(await screen.findByRole("heading", { name: "D:\\AI\\Orbit" })).toBeInTheDocument();
  expect(screen.getByText("D:\\AI\\Orbit\\AGENTS.md")).toHaveAttribute("title", "D:\\AI\\Orbit\\AGENTS.md");
  fireEvent.change(screen.getByRole("searchbox", { name: "Search history" }), { target: { value: target } });
  expect(screen.getByText("Showing 1 of 1 versions and 1 of 1 logs")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("history_load", { projectRoot }));
  fireEvent.click(screen.getByRole("button", { name: "Restore" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("history_restore", { projectRoot, id: "operation-1", expectedCurrentChecksum: "current" }));
  expect(confirm).toHaveBeenCalledWith(expect.stringContaining("at D:\\AI\\Orbit\\AGENTS.md"));
  fireEvent.click(screen.getByRole("tab", { name: /Logs/ }));
  expect(screen.getByText("D:\\AI\\Orbit\\AGENTS.md")).toBeInTheDocument();
  expect(document.body.textContent).not.toContain("\\\\?\\");
});

it("keeps a failed restore visible and sends the chosen project and current checksum", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "history_load") return { versions: [version], logs: [log] };
    if (command === "project_choose") return "D:/project";
    if (command === "history_restore") throw new Error("Target changed; refresh first");
    throw new Error(command);
  });
  render(<History />);
  await screen.findByRole("button", { name: "Restore" });
  fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("history_load", { projectRoot: "D:/project" }));
  fireEvent.click(await screen.findByRole("button", { name: "Restore" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Target changed; refresh first");
  expect(invoke).toHaveBeenCalledWith("history_restore", { projectRoot: "D:/project", id: "operation-1", expectedCurrentChecksum: "current" });
});

it("combines scope and text search across tabs while keeping version actions bound to ids", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const globalVersion = {
    ...version,
    id: "operation-2",
    resourceId: "settings",
    scope: "global" as const,
    scopeId: "global",
    scopeLabel: "Global settings",
    target: "C:/users/config.json",
  };
  const globalLog = {
    ...log,
    id: "log-2",
    resourceId: "settings",
    scope: "global" as const,
    scopeId: "global",
    scopeLabel: "Global settings",
    target: "C:/users/config.json",
    status: "pending" as const,
  };
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "history_load") return { versions: [version, globalVersion], logs: [log, globalLog] };
    if (command === "history_restore") return null;
    throw new Error(command);
  });
  render(<History />);

  const projectGroup = (await screen.findByText("Project memory")).closest("details");
  const globalGroup = screen.getByText("Global settings").closest("details");
  expect(projectGroup).toHaveAttribute("open");
  expect(globalGroup).not.toHaveAttribute("open");

  fireEvent.change(screen.getByRole("searchbox", { name: "Search history" }), { target: { value: "notes.md" } });
  expect(screen.getByText("Showing 1 of 2 versions and 1 of 2 logs")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("tab", { name: /Logs/ }));
  expect(screen.getByText("memory · notes")).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Scope"), { target: { value: "global" } });
  expect(screen.getByText('No operations match "notes.md".')).toBeInTheDocument();
  fireEvent.click(screen.getByRole("tab", { name: /Versions/ }));
  expect(screen.getByText('No versions match "notes.md".')).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
  expect(screen.getByText("Global settings").closest("details")).toHaveAttribute("open");
  expect(screen.getByRole("button", { name: "Restore" })).toBeEnabled();
  fireEvent.click(screen.getByRole("button", { name: "Restore" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("history_restore", {
    projectRoot: null,
    id: "operation-2",
    expectedCurrentChecksum: "current",
  }));

  fireEvent.click(screen.getByRole("tab", { name: /Logs/ }));
  fireEvent.change(screen.getByRole("searchbox", { name: "Search history" }), { target: { value: "pending" } });
  expect(screen.getByText(/Global settings/)).toBeInTheDocument();
  expect(screen.getByText("memory · settings")).toBeInTheDocument();
  fireEvent.change(screen.getByRole("searchbox", { name: "Search history" }), { target: { value: "log-2" } });
  expect(screen.getByText("Showing 0 of 1 versions and 1 of 1 logs")).toBeInTheDocument();
  expect(screen.getByText("memory · settings")).toBeInTheDocument();
});

it("deletes the saved payload while retaining the separate operation log", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  let deleted = false;
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "history_load") return {
      versions: deleted ? [] : [version],
      logs: [{ ...log, backupStatus: deleted ? "missing" : "available" }],
    };
    if (command === "history_delete") { deleted = true; return null; }
    throw new Error(command);
  });
  render(<History />);
  fireEvent.click(await screen.findByRole("button", { name: "Delete version" }));
  await screen.findByText("No saved versions found.");
  expect(invoke).toHaveBeenCalledWith("history_delete", { id: "operation-1" });
  fireEvent.click(screen.getByRole("tab", { name: /Logs/ }));
  expect(screen.getByText("memory · notes")).toBeInTheDocument();
  expect(screen.getByText("Saved backup is missing.")).toBeInTheDocument();
});

it("reports a checksum mismatch separately and prevents restoring the changed backup", async () => {
  vi.mocked(invoke).mockResolvedValue({
    versions: [{ ...version, backupStatus: "checksumMismatch", restorable: false }],
    logs: [{ ...log, backupStatus: "checksumMismatch" }],
  });
  render(<History />);
  expect(await screen.findByText("Saved backup checksum does not match its recorded checksum.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Restore" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Delete version" })).toBeDisabled();
  expect(screen.queryByText(/missing/i)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("tab", { name: /Logs/ }));
  expect(screen.getByText("Saved backup checksum does not match its recorded checksum.")).toBeInTheDocument();
});
