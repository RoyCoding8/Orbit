import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { Transcripts } from "./Transcripts";

afterEach(() => {
  cleanup();
  clearMocks();
});

const SESSIONS = [
  { sessionId: "alpha/one", project: "alpha", updatedAt: "2026-10-20T10:00:00Z", messageCount: 3 },
  { sessionId: "beta/one", project: "beta", updatedAt: "2026-10-19T10:00:00Z", messageCount: 5 },
];

describe("Transcripts search results", () => {
  it("shows a no-matches section instead of unmounting the results", async () => {
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "transcript_list_sessions") return SESSIONS;
      if (command === "transcript_search") return [];
      throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
    });
    mockIPC(ipc);
    render(<Transcripts />);

    await screen.findByText("alpha/one");
    fireEvent.change(screen.getByLabelText("Search message text"), { target: { value: "needle" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));

    const results = await screen.findByRole("region", { name: "Search results" });
    expect(results).toHaveTextContent("No matches.");
    expect(ipc).toHaveBeenCalledWith("transcript_search", { query: "needle", limit: 50 });
  });

  it("does not send a blank search to the backend", async () => {
    const ipc = vi.fn((command: string) => {
      if (command === "transcript_list_sessions") return SESSIONS;
      throw new Error(`Unexpected command ${command}`);
    });
    mockIPC(ipc);
    render(<Transcripts />);

    await screen.findByText("alpha/one");
    fireEvent.change(screen.getByLabelText("Search message text"), { target: { value: "   " } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));

    expect(screen.queryByRole("region", { name: "Search results" })).not.toBeInTheDocument();
    expect(ipc.mock.calls).toEqual([["transcript_list_sessions", {}]]);
  });

  it("notes when the hit limit caps the result list", async () => {
    const hits = Array.from({ length: 50 }, (_, index) => ({
      sessionId: "alpha/one",
      project: "alpha",
      uuid: `hit-${index}`,
      position: index,
      messageType: "user",
      text: `wide match ${index}`,
      before: null,
      after: null,
    }));
    mockIPC((command) => {
      if (command === "transcript_list_sessions") return SESSIONS;
      if (command === "transcript_search") return hits;
      throw new Error(`Unexpected command ${command}`);
    });
    render(<Transcripts />);

    await screen.findByText("alpha/one");
    fireEvent.change(screen.getByLabelText("Search message text"), { target: { value: "wide" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));

    const results = await screen.findByRole("region", { name: "Search results" });
    expect(results).toHaveTextContent("Search results 50");
    expect(results).toHaveTextContent("Showing the first 50 matches");
  });

  it("reports a stale hit and clears the previously displayed session", async () => {
    const staleHit = {
      sessionId: "alpha/one",
      project: "alpha",
      uuid: "vanished",
      position: 4,
      messageType: "user",
      text: "gone message",
      before: null,
      after: null,
    };
    mockIPC((command, payload) => {
      if (command === "transcript_list_sessions") return SESSIONS;
      if (command === "transcript_search") return [staleHit];
      if (command === "transcript_message_page_offset") return null;
      if (command === "transcript_thread_page") {
        if (payload && "sessionId" in payload && payload.sessionId === "beta/one") {
          return [{ uuid: "beta-message", messageType: "user", text: "previous session content", position: 1, depth: 0, branchIndex: 0, parentUuid: null, attachment: null, toolResult: null, toolResultNote: null }];
        }
        return [];
      }
      throw new Error(`Unexpected command ${command}`);
    });
    render(<Transcripts />);

    await screen.findByText("beta/one");
    fireEvent.click(screen.getByRole("button", { name: /beta\/one/ }));
    await screen.findByText("previous session content");
    fireEvent.change(screen.getByLabelText("Search message text"), { target: { value: "gone" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    fireEvent.click(await screen.findByRole("button", { name: /alpha · alpha\/one · line 4/ }));

    expect(await screen.findByText(/no longer part of session/)).toBeInTheDocument();
    expect(screen.queryByText("previous session content")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Session alpha/one" })).toBeInTheDocument();
  });
});
