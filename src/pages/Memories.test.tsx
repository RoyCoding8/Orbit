import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { Memories, type MemoryCatalog } from "./Memories";

const catalog: MemoryCatalog = {
  projectRoot: "D:/workspace/orbit",
  groups: [
    { id: "auto:orbit-project", label: "Project auto memory · orbit-project", role: "projectAutoMemory" },
    { id: "selected-project", label: "Selected project · D:/workspace/orbit", role: "projectInstructions" },
  ],
  files: [
    {
      id: "index", groupId: "auto:orbit-project", name: "MEMORY.md", path: "C:/Users/roy/.claude/projects/orbit-project/memory/MEMORY.md",
      source: "projectAutoMemory", role: "index", loadBehavior: "First 200 lines or 25 KB at startup.", editable: true,
      content: "See [architecture](architecture.md) and [outside](../private.md) and @architecture.md", references: [
        { target: "architecture.md", kind: "markdownLink", status: "known", fileId: "topic" },
        { target: "../private.md", kind: "markdownLink", status: "external", fileId: null },
        { target: "architecture.md", kind: "import", status: "known", fileId: "topic" },
      ],
    },
    {
      id: "topic", groupId: "auto:orbit-project", name: "architecture.md", path: "C:/Users/roy/.claude/projects/orbit-project/memory/architecture.md",
      source: "projectAutoMemory", role: "topic", loadBehavior: "Read on demand.", editable: true,
      content: "Original topic.", references: [],
    },
  ],
  excludedCustomMemoryDirectory: null,
};

function installCatalog(initial = catalog) {
  let current = initial;
  const ipc = vi.fn((command: string, payload?: unknown) => {
    const args = payload && typeof payload === "object" ? payload as Record<string, unknown> : undefined;
    if (command === "memory_catalog") return { ...current, projectRoot: typeof args?.projectRoot === "string" ? args.projectRoot : current.projectRoot };
    if (command === "memory_edit") {
      if (args?.expectedContent !== "Original topic.") throw new Error("Memory file changed since this editor opened.");
      current = { ...current, files: current.files.map((file) => file.id === args.id ? { ...file, content: String(args.content) } : file) };
      return null;
    }
    if (command === "project_choose") return "D:/workspace/other-project";
    throw new Error("Unexpected command " + command);
  });
  mockIPC(ipc);
  return ipc;
}

describe("Memories", () => {
  it("groups project-scoped auto memory, distinguishes load behavior, and navigates known links only", async () => {
    installCatalog();
    render(<Memories />);
    expect(await screen.findByText("Project auto memory · orbit-project")).toBeInTheDocument();
    expect(screen.getByText(/First 200 lines or 25 KB/)).toBeInTheDocument();
    expect(screen.getByText(/Context import/)).toBeInTheDocument();
    expect(screen.getAllByText(/^Markdown link ·/)).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: /architecture\.md Auto-memory topic/ }));
    expect(await screen.findByLabelText("Markdown content")).toHaveValue("Original topic.");
    const connections = screen.getByRole("region", { name: "Memory connections" });
    expect(within(connections).getAllByRole("button", { name: "MEMORY.md" })).toHaveLength(2);
    fireEvent.click(within(connections).getAllByRole("button", { name: "MEMORY.md" })[0]);
    expect(await screen.findByRole("heading", { name: "MEMORY.md" })).toBeInTheDocument();
    expect(screen.getByText("../private.md")).toHaveAttribute("class", "memory-link unavailable");
    expect(screen.queryByRole("button", { name: "../private.md" })).not.toBeInTheDocument();
  });

  it("sends the file ID and original content as the edit precondition", async () => {
    const ipc = installCatalog();
    render(<Memories />);
    fireEvent.click(await screen.findByRole("button", { name: /architecture\.md Auto-memory topic/ }));
    const editor = await screen.findByLabelText("Markdown content");
    fireEvent.change(editor, { target: { value: "Updated topic." } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc).toHaveBeenCalledWith("memory_edit", {
      projectRoot: "D:/workspace/orbit", id: "topic", expectedContent: "Original topic.", content: "Updated topic.",
    }));
    await waitFor(() => expect(screen.getByLabelText("Markdown content")).toHaveValue("Updated topic."));
  });

  it("preserves the selected project after the picker grants another root", async () => {
    const ipc = installCatalog();
    render(<Memories />);
    await screen.findByText("Authorized project");
    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
    await waitFor(() => expect(ipc).toHaveBeenCalledWith("memory_catalog", { projectRoot: "D:/workspace/other-project" }));
    expect(await screen.findByText("D:/workspace/other-project")).toBeInTheDocument();
  });

  it("keeps a draft when the backend rejects a stale save", async () => {
    mockIPC((command: string) => {
      if (command === "memory_catalog") return catalog;
      if (command === "memory_edit") throw new Error("Memory file changed since this editor opened.");
      throw new Error("Unexpected command " + command);
    });
    render(<Memories />);
    fireEvent.click(await screen.findByRole("button", { name: /architecture\.md Auto-memory topic/ }));
    const editor = await screen.findByLabelText("Markdown content");
    fireEvent.change(editor, { target: { value: "Draft survives." } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Memory file changed");
    expect(screen.getByLabelText("Markdown content")).toHaveValue("Draft survives.");
  });

  it("keeps an unsaved draft when the shared discard confirmation is declined", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    installCatalog();
    render(<Memories />);
    fireEvent.click(await screen.findByRole("button", { name: /architecture\.md Auto-memory topic/ }));
    const editor = await screen.findByLabelText("Markdown content");
    fireEvent.change(editor, { target: { value: "Keep this draft." } });
    fireEvent.click(screen.getByRole("button", { name: /MEMORY\.md Auto-memory index/ }));
    expect(confirm).toHaveBeenCalledOnce();
    expect(screen.getByLabelText("Markdown content")).toHaveValue("Keep this draft.");
    expect(screen.getByRole("heading", { name: "architecture.md" })).toBeInTheDocument();
    confirm.mockRestore();
  });

  it("shows an excluded custom memory path as a boundary notice", async () => {
    installCatalog({ ...catalog, excludedCustomMemoryDirectory: "D:/private/memory" });
    render(<Memories />);
    expect(await screen.findByRole("note")).toHaveTextContent("outside this authorized project");
    expect(screen.getByRole("note")).toHaveTextContent("D:/private/memory");
  });
});
