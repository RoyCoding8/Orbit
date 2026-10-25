import { beforeAll, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { Memories, type MemoryCatalog } from "./Memories";
import { HarnessProvider } from "../HarnessContext";

const catalog: MemoryCatalog = {
  projectRoot: "D:/workspace/orbit",
  groups: [
    { id: "auto:orbit-project", label: "orbit-project", role: "projectAutoMemory" },
    { id: "selected-project", label: "D:/workspace/orbit", role: "projectInstructions" },
    { id: "user-instructions", label: "User instructions", role: "userInstructions" },
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
    if (command === "memory_delete") {
      current = { ...current, files: current.files.filter((file) => file.id !== args?.id).map((file) => ({
        ...file,
        content: "Only remaining [outside](../private.md).",
        references: file.references.filter((reference) => reference.fileId !== args?.id),
      })) };
      return null;
    }
    if (command === "project_choose") return "D:/workspace/other-project";
    throw new Error("Unexpected command " + command);
  });
  mockIPC(ipc);
  return ipc;
}

describe("Memories", () => {
  beforeAll(async () => { await import("../components/MarkdownPreview"); });

  it("shows ordinary Windows paths while saving with the original canonical project identity", async () => {
    const projectRoot = "\\\\?\\D:\\AI\\Orbit";
    const ipc = installCatalog({
      ...catalog,
      projectRoot,
      groups: [{ id: "selected-project", label: projectRoot, role: "projectInstructions" }],
      files: catalog.files.filter((file) => file.id === "topic").map((file) => ({
        ...file, groupId: "selected-project", path: `${projectRoot}\\architecture.md`,
      })),
    });
    render(<Memories />);
    const editor = await screen.findByLabelText("Markdown content");
    expect(screen.getByRole("heading", { name: "D:\\AI\\Orbit" })).toBeInTheDocument();
    expect(screen.getByText("Selected project")).toBeInTheDocument();
    expect(screen.getByText("Selected project").closest("h3")).toBeNull();
    expect(screen.getByText("D:\\AI\\Orbit\\architecture.md", { exact: true })).toBeInTheDocument();
    fireEvent.change(editor, { target: { value: "Updated topic." } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc).toHaveBeenCalledWith("memory_edit", expect.objectContaining({
      projectRoot, id: "topic", expectedContent: "Original topic.", content: "Updated topic.",
    })));
  });

  it("edits project sections within one checked file while retaining other sections and CRLF", async () => {
    const first = "# Task Group: First\r\napplies_to: cwd=D:\\Projects\\One\r\n\r\nFirst stays.\r\n\r\n";
    const second = "# Task Group: Second\r\napplies_to: cwd=D:\\Projects\\Two\r\n\r\nSecond changes.\r\n\r\n";
    const shared = "# Shared preferences\r\n\r\nShared stays.\r\n";
    const original = first + second + shared;
    let persisted = original;
    const ipc = vi.fn((command: string, args?: unknown) => {
      if (command === "memory_catalog") return {
        ...catalog, groups: [{ id: "global", label: "Codex generated memory", role: "globalAutoMemory" }],
        files: [{ ...catalog.files[0], groupId: "global", source: "globalAutoMemory", content: persisted }],
      };
      if (command === "memory_edit" && args && typeof args === "object" && "content" in args && typeof args.content === "string") {
        persisted = args.content;
        return null;
      }
      throw new Error("Unexpected command " + command);
    });
    mockIPC(ipc);
    render(<HarnessProvider client="codex"><Memories /></HarnessProvider>);
    const navigation = await screen.findByRole("navigation", { name: "Memory sections" });
    expect(within(navigation).getByRole("heading", { name: "D:\\Projects\\One" })).toBeInTheDocument();
    const secondProjectGroup = within(navigation).getByText("D:\\Projects\\Two").closest("details");
    expect(secondProjectGroup).toHaveProperty("open", false);
    fireEvent.click(within(navigation).getByText("D:\\Projects\\Two"));
    expect(secondProjectGroup).toHaveProperty("open", true);
    fireEvent.click(within(navigation).getByText("D:\\Projects\\Two"));
    expect(secondProjectGroup).toHaveProperty("open", false);
    fireEvent.click(within(navigation).getByText("D:\\Projects\\Two"));
    expect(within(navigation).getByRole("heading", { name: "D:\\Projects\\Two" })).toBeInTheDocument();
    expect(within(navigation).getByRole("heading", { name: "Project not stated" })).toBeInTheDocument();
    fireEvent.click(within(navigation).getByRole("button", { name: "Task Group: Second" }));
    const editor = screen.getByLabelText(/Markdown content/);
    expect(editor).toHaveValue(second.replace(/\r\n/g, "\n"));
    fireEvent.change(editor, { target: { value: "# Renamed second\napplies_to: cwd=D:\\Projects\\Two\n\nRevised." } });
    fireEvent.change(editor, { target: { value: "# Renamed second\napplies_to: cwd=D:\\Projects\\Two\n\nRevised again.\n\n" } });
    fireEvent.click(within(navigation).getByRole("button", { name: "Task Group: First" }));
    expect(screen.getByLabelText(/Markdown content/)).toHaveValue(first.replace(/\r\n/g, "\n"));
    fireEvent.click(within(navigation).getByRole("button", { name: "Whole file" }));
    const expected = first + "# Renamed second\r\napplies_to: cwd=D:\\Projects\\Two\r\n\r\nRevised again.\r\n\r\n" + shared;
    expect(screen.getByLabelText(/Markdown content/)).toHaveValue(expected.replace(/\r\n/g, "\n"));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled());
    expect(screen.getByLabelText(/Markdown content/)).toHaveValue(expected.replace(/\r\n/g, "\n"));
    expect(persisted).toBe(expected);
    expect(ipc).toHaveBeenCalledWith("memory_edit", { client: "codex", projectRoot: catalog.projectRoot, id: "index", expectedContent: original, content: expected });
  });

  it("retains whole-file access after removing a section heading and rejecting a stale save", async () => {
    const content = "# One\nOriginal.\n\n# Two\nUnchanged.\n";
    mockIPC((command: string) => {
      if (command === "memory_catalog") return { ...catalog, files: [{ ...catalog.files[0], source: "globalAutoMemory", content }] };
      if (command === "memory_edit") throw new Error("Memory changed since this editor opened.");
      throw new Error("Unexpected command " + command);
    });
    render(<HarnessProvider client="codex"><Memories /></HarnessProvider>);
    const editor = await screen.findByLabelText(/Markdown content/);
    fireEvent.change(editor, { target: { value: "My draft survives without a heading.\n\n" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Memory changed since");
    expect(screen.getByLabelText(/Markdown content/)).toHaveValue("My draft survives without a heading.\n\n");
    fireEvent.click(screen.getByRole("button", { name: "Whole file" }));
    expect(screen.getByLabelText(/Markdown content/)).toHaveValue("My draft survives without a heading.\n\n# Two\nUnchanged.\n");
  });

  it("groups project-scoped auto memory, distinguishes load behavior, and navigates known links only", async () => {
    installCatalog();
    render(<Memories />);
    const groupSummary = await screen.findByRole("heading", { name: "orbit-project" });
    expect(screen.getByText("Auto memory")).toBeInTheDocument();
    expect(groupSummary).not.toHaveTextContent("Auto memory");
    const navigation = screen.getByRole("navigation", { name: "Memory files by source" });
    const headings = within(navigation).getAllByRole("heading");
    expect(headings[0]).toHaveTextContent("User instructions");
    expect(screen.getByText("User instructions")).toBe(headings[0]);
    expect(groupSummary.closest("details")).toHaveProperty("open", true);
    expect(screen.getByText(/First 200 lines or 25 KB/)).toBeInTheDocument();
    const fileGroup = groupSummary.closest("details");
    fireEvent.click(groupSummary);
    expect(fileGroup).toHaveProperty("open", false);
    fireEvent.click(groupSummary);
    expect(fileGroup).toHaveProperty("open", true);
    fireEvent.click(screen.getByText("Connections"));
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
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText("Updated topic.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc).toHaveBeenCalledWith("memory_edit", {
      projectRoot: "D:/workspace/orbit", id: "topic", expectedContent: "Original topic.", content: "Updated topic.",
    }));
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
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

  it("deletes with the original content and reloads the updated memory index", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const ipc = installCatalog();
    render(<Memories />);
    fireEvent.click(await screen.findByRole("button", { name: /architecture\.md Auto-memory topic/ }));
    fireEvent.change(screen.getByLabelText("Markdown content"), { target: { value: "Unsaved draft." } });
    fireEvent.click(screen.getByRole("button", { name: "Delete file" }));
    await screen.findByRole("heading", { name: "MEMORY.md" });
    expect(ipc).toHaveBeenCalledWith("memory_delete", {
      projectRoot: "D:/workspace/orbit", id: "topic", expectedContent: "Original topic.",
    });
    expect(screen.getByLabelText("Markdown content")).toHaveValue("Only remaining [outside](../private.md).");
    expect(screen.queryByRole("button", { name: /architecture\.md Auto-memory topic/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Connections"));
    expect(within(screen.getByRole("region", { name: "Memory connections" })).getByText("../private.md")).toBeInTheDocument();
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
    fireEvent.click(screen.getByRole("button", { name: "Delete file" }));
    expect(confirm).toHaveBeenCalledTimes(2);
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

  it("searches file and section content without replacing the selected draft or section boundaries", async () => {
    const first = "# Task Group: First\nFirst original.\n\n";
    const second = "# Task Group: Second\nSecond original.\n";
    const original = first + second;
    mockIPC((command: string) => {
      if (command === "memory_catalog") return {
        ...catalog,
        groups: [{ id: "global", label: "Codex generated memory", role: "globalAutoMemory" }],
        files: [{ ...catalog.files[0], id: "global-memory", groupId: "global", source: "globalAutoMemory", content: original }],
      };
      throw new Error("Unexpected command " + command);
    });
    render(<HarnessProvider client="codex"><Memories /></HarnessProvider>);

    const editor = await screen.findByLabelText("Markdown content");
    const search = screen.getByRole("searchbox", { name: "Search memories" });
    fireEvent.change(editor, { target: { value: "# Task Group: First\nFirst draft.\n\n" } });
    fireEvent.change(search, { target: { value: "Second original" } });

    const navigation = screen.getByRole("navigation", { name: "Memory sections" });
    expect(within(navigation).getByRole("button", { name: "Task Group: Second" })).toBeInTheDocument();
    expect(within(navigation).queryByRole("button", { name: "Task Group: First" })).not.toBeInTheDocument();
    expect(editor).toHaveValue("# Task Group: First\nFirst draft.\n\n");

    fireEvent.click(screen.getByRole("button", { name: "Whole file" }));
    expect(editor).toHaveValue("# Task Group: First\nFirst draft.\n\n# Task Group: Second\nSecond original.\n");
    fireEvent.change(search, { target: { value: "no such memory text" } });
    expect(screen.getByText('No memories match "no such memory text".')).toBeInTheDocument();
    expect(editor).toHaveValue("# Task Group: First\nFirst draft.\n\n# Task Group: Second\nSecond original.\n");

    fireEvent.click(screen.getByRole("button", { name: "Clear memory search" }));
    expect(await screen.findByRole("button", { name: "Task Group: First" })).toBeInTheDocument();
    expect(editor).toHaveValue("# Task Group: First\nFirst draft.\n\n# Task Group: Second\nSecond original.\n");
    fireEvent.change(search, { target: { value: "Codex generated memory" } });
    expect(screen.getByRole("button", { name: /MEMORY\.md Auto-memory index/ })).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "C:/Users/roy/.claude/projects/orbit-project/memory/MEMORY.md" } });
    expect(screen.getByRole("button", { name: /MEMORY\.md Auto-memory index/ })).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "First draft" } });
    expect(screen.getByRole("button", { name: /MEMORY\.md Auto-memory index/ })).toBeInTheDocument();
  });
});
