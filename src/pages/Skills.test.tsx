import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { Skills } from "./Skills";

const SKILL = { id: "alpha", name: "Alpha", description: "Fixture skill", path: "D:/fixture/skills/alpha/SKILL.md", content: "Original skill content.", state: "on" };

afterEach(() => {
  cleanup();
  clearMocks();
});

function inventory(command: string) {
  if (command === "skills_inventory") return [[SKILL], []];
  if (command === "skills_ledger") return [];
  throw new Error(`Unexpected command ${command}`);
}

async function edit() {
  render(<Skills />);
  fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
  return screen.getByRole("dialog", { name: "Edit alpha" });
}

describe("Skills loading", () => {
  it("waits for a successful inventory before offering an empty state", async () => {
    let finish: (value: unknown) => void = () => {};
    mockIPC(command => command === "skills_inventory" ? new Promise(resolve => { finish = resolve; }) : []);
    render(<Skills />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading skills and memories");
    expect(screen.getByRole("button", { name: "Install skill" })).toBeDisabled();
    expect(screen.queryByText("No skills found")).not.toBeInTheDocument();
    await act(async () => { finish([[], []]); });
    expect(await screen.findByText("No skills found")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Install skill" })).toBeEnabled();
  });

  it("offers refresh instead of a false empty state after a load failure", async () => {
    let failed = true;
    mockIPC(command => {
      if (command === "skills_inventory" && failed) throw new Error("Inventory unavailable");
      return inventory(command);
    });
    render(<Skills />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Inventory unavailable");
    expect(screen.queryByText("No skills found")).not.toBeInTheDocument();
    failed = false;
    fireEvent.click(screen.getByRole("button", { name: "Refresh skills" }));
    expect(await screen.findByRole("heading", { name: "Alpha" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

describe("Skills editor", () => {
  it("sends the original content as the edit precondition and displays the saved inventory", async () => {
    let saved = false;
    const ipc = vi.fn((command: string) => {
      if (command === "skill_edit") { saved = true; return null; }
      if (command === "skills_inventory" && saved) return [[{ ...SKILL, content: "Updated content." }], []];
      return inventory(command);
    });
    mockIPC(ipc);
    const dialog = await edit();
    fireEvent.change(within(dialog).getByLabelText("Proposed SKILL.md"), { target: { value: "Updated content." } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply edit" }));
    await waitFor(() => expect(ipc).toHaveBeenCalledWith("skill_edit", { id: "alpha", expectedContent: "Original skill content.", content: "Updated content." }));
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("Proposed SKILL.md")).toHaveValue("Updated content.");
  });

  it("keeps a refused draft and its error inside the modal", async () => {
    mockIPC(command => {
      if (command === "skill_edit") throw new Error("Skill alpha changed. Refresh skills and try again.");
      return inventory(command);
    });
    const dialog = await edit();
    fireEvent.change(within(dialog).getByLabelText("Proposed SKILL.md"), { target: { value: "Unsaved draft." } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply edit" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Skill alpha changed");
    expect(within(dialog).getByLabelText("Proposed SKILL.md")).toHaveValue("Unsaved draft.");
    expect(dialog).toHaveAttribute("open");
  });

  it("prevents dismissal while saving and accepts Escape after a rejected save", async () => {
    let reject: (error: unknown) => void = () => {};
    mockIPC(command => command === "skill_edit" ? new Promise((_, rejectSave) => { reject = rejectSave; }) : inventory(command));
    const dialog = await edit();
    fireEvent.change(within(dialog).getByLabelText("Proposed SKILL.md"), { target: { value: "Draft." } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply edit" }));
    fireEvent(dialog, new Event("cancel", { bubbles: false, cancelable: true }));
    expect(within(dialog).getByRole("button", { name: "Close" })).toBeDisabled();
    expect(dialog).toHaveAttribute("open");
    await act(async () => { reject(new Error("Write refused")); });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Write refused");
    fireEvent(dialog, new Event("cancel", { bubbles: false, cancelable: true }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { name: "Alpha" })).toBeInTheDocument();
  });
});
