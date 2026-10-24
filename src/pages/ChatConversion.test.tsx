import { fireEvent, render, screen, within } from "@testing-library/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { describe, expect, it, vi } from "vitest";
import { Transcripts } from "./Transcripts";

const preview = { title: "Source chat", workingDirectory: "D:/old-project", messages: 2, notes: ["Tool history becomes text."] };

describe("chat conversion", () => {
  it.each(["original", "override", "reset"])("converts using the %s working directory", async mode => {
    const ipc = vi.fn((command, payload) => {
      if (command === "transcript_list_sessions") return { sessions: [{ sessionId: "source/one", title: "Source chat", project: "encoded-key", messageCount: 2 }], source: "C:/native-chats", warnings: [] };
      if (command === "transcript_thread_page") return [];
      if (command === "transcript_conversion_preview") return preview;
      if (command === "project_choose") return "D:/new-project";
      if (command === "transcript_convert") return { sessionId: "converted-id", workingDirectory: payload.projectRoot ?? preview.workingDirectory, messages: 2, notes: preview.notes };
      throw new Error(command);
    });
    mockIPC(ipc);
    render(<Transcripts />);
    fireEvent.click(await screen.findByRole("button", { name: /Source chat/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Convert chat" }));
    const dialog = within(screen.getByRole("dialog", { name: "Convert chat" }));
    expect(await dialog.findByText("D:/old-project")).toBeInTheDocument();
    expect(dialog.queryByRole("option", { name: "Claude Code" })).not.toBeInTheDocument();
    expect(dialog.queryByRole("option", { name: "Kiro" })).not.toBeInTheDocument();
    if (mode !== "original") {
      fireEvent.click(dialog.getByRole("button", { name: "Change working directory" }));
      await dialog.findByText("D:/new-project");
      if (mode === "reset") fireEvent.click(dialog.getByRole("button", { name: "Use original directory" }));
    }
    fireEvent.change(dialog.getByLabelText("Destination agent"), { target: { value: "pi" } });
    fireEvent.click(dialog.getByRole("button", { name: "Convert chat" }));
    await dialog.findByText("converted-id");
    expect(ipc).toHaveBeenCalledWith("transcript_convert", {
      sessionId: "source/one", targetClient: "pi", projectRoot: mode === "override" ? "D:/new-project" : null,
    });
    fireEvent.click(dialog.getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Converted chat to Pi as converted-id");
    expect(screen.getByRole("heading", { name: "Session source/one" })).toBeInTheDocument();
  });

  it.each(["preview", "convert"])("keeps a %s failure visible without reporting success", async step => {
    const ipc = vi.fn(command => {
      if (command === "transcript_list_sessions") return { sessions: [{ sessionId: "source/one", project: "key", messageCount: 2 }], source: "C:/native-chats", warnings: [] };
      if (command === "transcript_thread_page") return [];
      if (command === "transcript_conversion_preview") {
        if (step === "preview") throw new Error("Native session is unavailable");
        return preview;
      }
      if (command === "transcript_convert") throw new Error("Destination import rejected");
      throw new Error(command);
    });
    mockIPC(ipc);
    render(<Transcripts />);
    fireEvent.click(await screen.findByRole("button", { name: /source\/one/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Convert chat" }));
    const dialog = within(screen.getByRole("dialog"));
    if (step === "convert") {
      await dialog.findByText("D:/old-project");
      fireEvent.click(dialog.getByRole("button", { name: "Convert chat" }));
    }
    expect(await dialog.findByRole("alert")).toHaveTextContent(step === "preview" ? "Native session is unavailable" : "Destination import rejected");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.queryByText(/Converted chat to/)).not.toBeInTheDocument();
    if (step === "preview") expect(dialog.getByRole("button", { name: "Convert chat" })).toBeDisabled();
  });
});
