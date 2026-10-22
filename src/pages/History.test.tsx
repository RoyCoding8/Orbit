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
  await screen.findByText("No saved versions match this scope.");
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
