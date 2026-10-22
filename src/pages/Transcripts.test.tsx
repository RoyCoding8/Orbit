import { describe, expect, it, vi } from "vitest";
import { StrictMode } from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { InvokeArgs } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { open } from "@tauri-apps/plugin-dialog";
import { Transcripts } from "./Transcripts";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const SESSIONS = [
  { sessionId: "alpha/one", project: "alpha", updatedAt: "2026-10-20T10:00:00Z", messageCount: 3 },
  { sessionId: "beta/one", project: "beta", updatedAt: "2026-10-19T10:00:00Z", messageCount: 5 },
];

const message = (text: string) => ({ uuid: text, parentUuid: null, messageType: "user", text, position: 1, depth: 0, branchIndex: 0, attachment: null, toolResult: null, toolResultNote: null });

it.each(["success", "failure"])("ignores an obsolete session-list %s after a newer load", async outcome => {
  let release: () => void = () => { throw new Error("Expected a pending request"); };
  let loads = 0;
  mockIPC(command => {
    if (command !== "transcript_list_sessions") throw new Error(command);
    if (++loads > 1) return SESSIONS;
    return new Promise((resolve, reject) => { release = () => outcome === "success" ? resolve([]) : reject(new Error("Obsolete load failed")); });
  });
  render(<StrictMode><Transcripts /></StrictMode>);
  await screen.findByText("alpha/one");
  await act(async () => { release(); });
  expect(screen.getByText("alpha/one")).toBeInTheDocument();
  expect(screen.queryByText(/Obsolete load failed/)).not.toBeInTheDocument();
});

it.each(["selection", "pagination", "search jump"])("ignores an obsolete %s after selecting another session", async (scenario) => {
  let release: () => void = () => { throw new Error("Expected a pending request"); };
  let alphaPages = 0;
  const hit = { sessionId: "alpha/one", project: "alpha", uuid: "hit", position: 101, messageType: "user", text: "needle", before: null, after: null };
  mockIPC((command, payload) => {
    if (command === "transcript_list_sessions") return SESSIONS;
    if (command === "transcript_search") return [hit];
    if (command === "transcript_message_page_offset") return new Promise(resolve => { release = () => resolve(100); });
    if (command === "transcript_thread_page") {
      if (payload && "sessionId" in payload && payload.sessionId === "beta/one") return [message("beta content")];
      alphaPages += 1;
      if (scenario === "search jump") return [message("obsolete alpha content")];
      if (scenario === "pagination" && alphaPages === 1) return Array.from({ length: 100 }, (_, i) => message(`alpha ${i}`));
      return new Promise(resolve => { release = () => resolve([message("obsolete alpha content")]); });
    }
    throw new Error(command);
  });
  render(<Transcripts />);
  await screen.findByText("alpha/one");
  if (scenario === "search jump") {
    fireEvent.change(screen.getByLabelText("Search message text"), { target: { value: "needle" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    fireEvent.click(await screen.findByRole("button", { name: /alpha · alpha\/one · line 101/ }));
    await act(async () => {});
  } else {
    fireEvent.click(screen.getByRole("button", { name: /alpha\/one/ }));
    if (scenario === "pagination") fireEvent.click(await screen.findByRole("button", { name: "Load next 100 messages" }));
  }
  fireEvent.click(screen.getByRole("button", { name: /beta\/one/ }));
  await screen.findByText("beta content");
  await act(async () => { release(); });
  expect(screen.getByRole("heading", { name: "Session beta/one" })).toBeInTheDocument();
  expect(screen.getByText("beta content")).toBeInTheDocument();
  expect(screen.queryByText("obsolete alpha content")).not.toBeInTheDocument();
});

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

  it.each([null, 100])("opens a search hit at its resolved page or reports it stale (%s)", async (offset) => {
    vi.stubGlobal("CSS", { escape: (value: string) => value });
    vi.spyOn(document, "querySelector").mockReturnValue(null);
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
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "transcript_list_sessions") return SESSIONS;
      if (command === "transcript_search") return [staleHit];
      if (command === "transcript_message_page_offset") return offset;
      if (command === "transcript_thread_page") {
        if (payload && "sessionId" in payload && payload.sessionId === "beta/one") {
          return [{ uuid: "beta-message", messageType: "user", text: "previous session content", position: 1, depth: 0, branchIndex: 0, parentUuid: null, attachment: null, toolResult: null, toolResultNote: null }];
        }
        return [message("matched page content")];
      }
      throw new Error(`Unexpected command ${command}`);
    });
    mockIPC(ipc);
    render(<Transcripts />);

    await screen.findByText("beta/one");
    fireEvent.click(screen.getByRole("button", { name: /beta\/one/ }));
    await screen.findByText("previous session content");
    fireEvent.change(screen.getByLabelText("Search message text"), { target: { value: "gone" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    fireEvent.click(await screen.findByRole("button", { name: /alpha · alpha\/one · line 4/ }));

    if (offset === null) {
      expect(await screen.findByText(/no longer part of session/)).toBeInTheDocument();
      expect(ipc).not.toHaveBeenCalledWith("transcript_thread_page", { sessionId: "alpha/one", offset: 0, limit: 100 });
    } else {
      expect(await screen.findByText("matched page content")).toBeInTheDocument();
      expect(ipc).toHaveBeenCalledWith("transcript_thread_page", { sessionId: "alpha/one", offset: 100, limit: 100 });
    }
    expect(screen.queryByText("previous session content")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Session alpha/one" })).toBeInTheDocument();
  });
});

describe("native Claude archives", () => {
  it("exports the selected session to a user-chosen directory", async () => {
    vi.mocked(open).mockResolvedValue("C:/archive-destination");
    const report = {
      action: "exported",
      sourceSessionId: "alpha/one",
      sessionId: "alpha/one",
      files: 3,
      path: "C:/archive-destination/claude-session-one",
    };
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "transcript_list_sessions") return SESSIONS;
      if (command === "transcript_thread_page") return [message("selected content")];
      if (command === "transcript_export_directory") return report;
      throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
    });
    mockIPC(ipc);
    render(<Transcripts />);

    await screen.findByText("alpha/one");
    fireEvent.click(screen.getByRole("button", { name: /alpha\/one/ }));
    await screen.findByText("selected content");
    fireEvent.click(screen.getByRole("button", { name: "Export Claude archive" }));

    expect(await screen.findByText(/Archived 3 native files/)).toBeInTheDocument();
    expect(ipc).toHaveBeenCalledWith("transcript_export_directory", {
      sessionId: "alpha/one",
      destination: "C:/archive-destination",
    });
  });

  it("imports an archive only after both directory choices and refreshes sessions", async () => {
    vi.mocked(open).mockResolvedValue("C:/archive-source");
    const importedSession = {
      sessionId: "new-project/11111111-2222-4333-8444-555555555555.jsonl",
      project: "new-project",
      updatedAt: "2026-10-21T10:00:00Z",
      messageCount: 2,
    };
    let sessionLoads = 0;
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "transcript_list_sessions") {
        return ++sessionLoads === 1 ? SESSIONS : [...SESSIONS, importedSession];
      }
      if (command === "project_choose") return "C:/destination-project";
      if (command === "transcript_import_directory") {
        return {
          action: "imported",
          sourceSessionId: "alpha/one.jsonl",
          sessionId: importedSession.sessionId,
          files: 3,
          path: "C:/claude/projects/new-project/11111111-2222-4333-8444-555555555555.jsonl",
        };
      }
      throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
    });
    mockIPC(ipc);
    render(<Transcripts />);

    await screen.findByText("alpha/one");
    fireEvent.click(screen.getByRole("button", { name: "Import Claude archive" }));

    expect(await screen.findByText(/Imported alpha\/one.jsonl as new-project/)).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: /11111111-2222-4333-8444-555555555555.jsonl/ })).toBeInTheDocument();
    expect(ipc).toHaveBeenCalledWith("project_choose", {});
    expect(ipc).toHaveBeenCalledWith("transcript_import_directory", {
      source: "C:/archive-source",
      projectRoot: "C:/destination-project",
    });
  });

  it("does not invoke import when the archive picker is canceled", async () => {
    vi.mocked(open).mockResolvedValue(null);
    const ipc = vi.fn((command: string) => {
      if (command === "transcript_list_sessions") return SESSIONS;
      throw new Error(`Unexpected command ${command}`);
    });
    mockIPC(ipc);
    render(<Transcripts />);

    await screen.findByText("alpha/one");
    fireEvent.click(screen.getByRole("button", { name: "Import Claude archive" }));

    await act(async () => {});
    expect(ipc).not.toHaveBeenCalledWith("project_choose", {});
    expect(ipc).not.toHaveBeenCalledWith("transcript_import_directory", expect.anything());
  });
});
