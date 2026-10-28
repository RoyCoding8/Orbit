import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { describe, expect, it, vi } from "vitest";
import { HarnessProvider } from "../HarnessContext";
import { HarnessTransfer } from "./HarnessTransfer";

const dialogs = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => dialogs);

const skill = { id: "skill", category: "skills", label: "example/SKILL.md", scope: "user", target: "C:/config/skills/example/SKILL.md", targetKey: "file", status: "add", expected: "before-skill" };
const settings = { id: "settings", category: "settings", label: "Settings", scope: "user", target: "C:/config/config.toml", targetKey: "settings", status: "conflict", expected: "before-settings" };
const mcp = { id: "mcp", category: "mcps", label: "docs", scope: "project", target: "C:/project/.codex/config.toml", targetKey: "mcp-docs", status: "identical", expected: "before-mcp" };
const preview = { checksum: "zip-checksum", projectRoot: "\\\\?\\D:\\project", entries: [skill, settings, mcp], warnings: [] };

function show() {
  render(<HarnessProvider client="codex"><HarnessTransfer /></HarnessProvider>);
}

function toggle(label: string) { fireEvent.change(screen.getByRole("checkbox", { name: label })); }

async function openImport() {
  dialogs.open.mockResolvedValue("C:/saved/orbit.zip");
  fireEvent.click(screen.getByRole("button", { name: "Import setup" }));
  fireEvent.click(await screen.findByRole("button", { name: "Current project" }));
  await screen.findByText("Found 3 items. Choose what to import.");
}

describe("harness archives", () => {
  it("exports only checked categories for the selected harness and project", async () => {
    const ipc = vi.fn(command => {
      if (command === "harness_archive_inventory") return preview;
      if (command === "project_choose") return "D:/other-project";
      if (command === "harness_archive_export") return { files: 1, skipped: 0, path: "C:/saved/export.zip" };
      throw new Error(command);
    });
    mockIPC(ipc);
    dialogs.save.mockResolvedValue("C:/saved/export.zip");
    show();
    fireEvent.click(screen.getByRole("button", { name: "Export setup" }));
    await screen.findByText("Choose what to save in your backup.");
    expect(screen.getByText("Backups can contain passwords and API keys. Share them only with people you trust.")).toBeInTheDocument();
    expect(screen.getByText("D:\\project")).toBeInTheDocument();
    toggle("Select all items");
    await waitFor(() => expect(screen.getByRole("button", { name: "Save backup" })).toBeDisabled());
    toggle("Select Skills");
    fireEvent.click(screen.getByRole("button", { name: "Change project" }));
    await screen.findByText("Choose what to save in your backup.");
    await waitFor(() => expect(ipc).toHaveBeenCalledWith("harness_archive_inventory", { client: "codex", projectRoot: "D:/other-project" }));
    fireEvent.click(screen.getByRole("button", { name: "Save backup" }));
    await screen.findByText("Saved a backup with 1 item.");
    expect(ipc).toHaveBeenCalledWith("harness_archive_export", { client: "codex", projectRoot: "D:/other-project", categories: ["skills"], path: "C:/saved/export.zip" });
  });

  it("imports only selected items with explicit conflict choices and skips identical resources", async () => {
    const ipc = vi.fn(command => {
      if (command === "harness_archive_preview") return preview;
      if (command === "harness_archive_import") return { files: 1, skipped: 0, path: "C:/saved/orbit.zip" };
      throw new Error(command);
    });
    mockIPC(ipc);
    show();
    await openImport();
    expect(screen.getByRole("button", { name: "Import selected" })).toBeDisabled();
    for (const summary of screen.getAllByText("View 1 item")) fireEvent.click(summary);
    expect(screen.getByRole("checkbox", { name: "Import docs (Project)" })).toHaveAttribute("aria-disabled", "true");
    toggle("Select all items");
    toggle("Import example/SKILL.md (User-wide)");
    expect(screen.getByText("1 item selected")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Action for Settings (User-wide)"), { target: { value: "replace" } });
    fireEvent.click(screen.getByRole("button", { name: "Import selected" }));
    await screen.findByText("Imported 1 file.");
    expect(ipc).toHaveBeenCalledWith("harness_archive_import", {
      client: "codex", path: "C:/saved/orbit.zip", projectRoot: null, destination: "project", checksum: "zip-checksum",
      choices: [{ id: "settings", replace: true, expected: "before-settings" }],
    });
    expect(screen.getByText("You can restore replaced files from History.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("requires a fresh selection after changing the destination and prevents duplicate destinations", async () => {
    const ipc = vi.fn((command, args) => {
      if (command === "harness_archive_preview") return args.destination === "user"
        ? { ...preview, entries: [skill, { ...skill, id: "second", scope: "project" }, mcp] }
        : preview;
      throw new Error(command);
    });
    mockIPC(ipc);
    show();
    await openImport();
    toggle("Select all items");
    fireEvent.change(screen.getByLabelText("Import destination"), { target: { value: "user" } });
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "Select all items" })).toHaveAttribute("aria-checked", "false"));
    expect(screen.getByRole("button", { name: "Import selected" })).toBeDisabled();
    toggle("Select Skills");
    expect(screen.getByRole("alert")).toHaveTextContent("Some selected items would replace the same file or setting");
    expect(screen.getByRole("button", { name: "Import selected" })).toBeDisabled();
    expect(ipc).toHaveBeenCalledWith("harness_archive_preview", { client: "codex", path: "C:/saved/orbit.zip", projectRoot: null, destination: "user" });
  });

  it.each([
    { entries: [], message: "This backup has no items to import." },
    { entries: [mcp], message: "Everything in this backup is already up to date." },
  ])("explains why nothing can be imported: $message", async ({ entries, message }) => {
    const ipc = vi.fn(() => ({ ...preview, entries }));
    mockIPC(ipc);
    dialogs.open.mockResolvedValue("C:/saved/orbit.zip");
    show();
    fireEvent.click(screen.getByRole("button", { name: "Import setup" }));
    await screen.findByRole("button", { name: "Current project" });
    expect(screen.getByRole("button", { name: "User-wide" })).toBeInTheDocument();
    expect(ipc).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "User-wide" }));
    await screen.findByText(message);
    expect(screen.getByRole("checkbox", { name: "Select all items" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Import selected" })).toBeDisabled();
    expect(ipc).toHaveBeenCalledWith("harness_archive_preview", { client: "codex", path: "C:/saved/orbit.zip", projectRoot: null, destination: "user" });
  });

  it("reports unchanged files when an existing version is skipped", async () => {
    mockIPC(command => {
      if (command === "harness_archive_preview") return preview;
      if (command === "harness_archive_import") return { files: 0, skipped: 1, path: "C:/saved/orbit.zip" };
      throw new Error(command);
    });
    show();
    await openImport();
    toggle("Select Settings");
    fireEvent.click(screen.getByRole("button", { name: "Import selected" }));
    await screen.findByText("No files were changed.");
    expect(screen.getByText("Kept 1 existing item unchanged.")).toBeInTheDocument();
    expect(screen.queryByText("You can restore replaced files from History.")).not.toBeInTheDocument();
  });

  it.each(["preview", "apply"])("keeps a %s failure visible without reporting success", async stage => {
    mockIPC(command => {
      if (command === "harness_archive_preview") {
        if (stage === "preview") throw new Error("Cross-harness import is planned for a future version.");
        return preview;
      }
      if (command === "harness_archive_import") throw new Error("Resource changed after preview");
      throw new Error(command);
    });
    show();
    dialogs.open.mockResolvedValue("C:/saved/orbit.zip");
    fireEvent.click(screen.getByRole("button", { name: "Import setup" }));
    fireEvent.click(await screen.findByRole("button", { name: "Current project" }));
    if (stage === "apply") {
      await screen.findByText("Found 3 items. Choose what to import.");
      toggle("Select Skills");
      fireEvent.click(screen.getByRole("button", { name: "Import selected" }));
    }
    const dialog = within(await screen.findByRole("dialog"));
    expect(await dialog.findByRole("alert")).toHaveTextContent(stage === "preview" ? "Cross-harness import" : "changed after preview");
    expect(dialog.queryByText(/Imported \d/)).not.toBeInTheDocument();
  });
});
