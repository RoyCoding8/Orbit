import { describe, expect, it, vi } from "vitest";
import { StrictMode } from "react";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { Channel, type InvokeArgs } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { open } from "@tauri-apps/plugin-dialog";
import { Transcripts } from "./Transcripts";
import { HarnessProvider } from "../HarnessContext";
import { clients } from "../harnesses";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const SESSIONS = [
  { sessionId: "alpha/one", project: "alpha", updatedAt: "2026-10-20T10:00:00Z", messageCount: 3, indexed: true },
  { sessionId: "beta/one", project: "beta", updatedAt: "2026-10-19T10:00:00Z", messageCount: 5, indexed: true },
];

const message = (text: string) => ({ uuid: text, parentUuid: null, messageType: "user", text, position: 1, depth: 0, branchIndex: 0, attachment: null, toolResult: null, toolResultNote: null });

it("lists unindexed chats and loads only the opened conversation", async () => {
  let loaded = false;
  mockIPC(command => {
    if (command === "transcript_list_sessions") return { source: "C:/native-chats", warnings: [], sessions: SESSIONS.map((session, index) => ({ ...session, indexed: loaded && index === 0, messageCount: loaded && index === 0 ? 1 : 0 })) };
    if (command === "transcript_thread_page") { loaded = true; return [message("loaded on demand")]; }
    throw new Error(`Unexpected operation ${command}`);
  });
  render(<Transcripts />);
  expect(await screen.findByText(/0 of 2 chats indexed/)).toBeInTheDocument();
  expect(screen.getAllByText("Not indexed")).toHaveLength(2);
  await vi.waitFor(() => expect(screen.getByRole("button", { name: "Refresh chats" })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: "Refresh chats" }));
  await vi.waitFor(() => expect(screen.getByRole("button", { name: "Refresh chats" })).toBeEnabled());
  expect(loaded).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: /alpha\/one/ }));
  expect(await screen.findByText("loaded on demand")).toBeInTheDocument();
  expect(await screen.findByText("1 message · Indexed")).toBeInTheDocument();
  expect(screen.getByText(/1 of 2 chats indexed/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /beta\/one/ })).toHaveTextContent("Not indexed");
});

it.each(["success", "failure"])("shows optional indexing progress and keeps chat actions usable: %s", async outcome => {
  let send: (completed: number) => void = () => { throw new Error("Indexing has not started"); };
  let finish: () => void = () => { throw new Error("Indexing has not started"); };
  mockIPC((command, payload) => {
    if (command === "transcript_list_sessions") return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
    if (command === "transcript_thread_page") return [message("readable during indexing")];
    if (command === "transcript_ingest") {
      if (!payload || !("onProgress" in payload) || !(payload.onProgress instanceof Channel)) throw new Error("Missing progress channel");
      const channel = payload.onProgress;
      send = completed => channel.onmessage({ completed, total: 2, failed: 0, title: "Indexing beta" });
      return new Promise((resolve, reject) => { finish = () => outcome === "success" ? resolve({sessions: 2, skippedFiles: 0, malformedLines: 0}) : reject(new Error("Source unavailable")); });
    }
    throw new Error(command);
  });
  render(<Transcripts />);
  fireEvent.click(await screen.findByRole("button", { name: /alpha\/one/ }));
  await screen.findByText("readable during indexing");
  fireEvent.click(screen.getByRole("button", { name: "Index chats" }));
  expect(screen.getByRole("progressbar", { name: "Chat indexing" })).not.toHaveAttribute("value");
  await act(async () => { send(1); });
  expect(screen.getByRole("progressbar", { name: "Chat indexing" })).toHaveAttribute("value", "1");
  expect(screen.getByRole("progressbar", { name: "Chat indexing" })).toHaveAttribute("max", "2");
  expect(screen.getByText("1 of 2 chats processed")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Export Claude archive" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Convert chat" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Index chats" })).toBeDisabled();
  expect(screen.getByText("readable during indexing")).toBeInTheDocument();
  await act(async () => { send(2); finish(); });
  expect(await screen.findByText(outcome === "success" ? /Indexed 2 chats for message search/ : /Completed chats remain searchable.*Source unavailable/)).toBeInTheDocument();
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Index chats" })).toBeEnabled();
});

it("does not restart an opening conversation when metadata discovery finishes", async () => {
  let finishDiscovery: () => void = () => { throw new Error("Discovery did not start"); };
  let finishPage: () => void = () => { throw new Error("Chat did not start loading"); };
  const ipc = vi.fn((command, payload) => {
    if (command === "transcript_list_sessions") {
      const catalog = { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
      if (payload && "refresh" in payload && payload.refresh)
        return new Promise(resolve => { finishDiscovery = () => resolve(catalog); });
      return catalog;
    }
    if (command === "transcript_thread_page")
      return new Promise(resolve => { finishPage = () => resolve([message("opened once")]); });
    throw new Error(command);
  });
  mockIPC(ipc);
  render(<Transcripts />);
  fireEvent.click(await screen.findByRole("button", { name: /alpha\/one/ }));
  await screen.findByText("Loading chat…");
  await act(async () => { finishDiscovery(); });
  expect(ipc.mock.calls.filter(([command]) => command === "transcript_thread_page")).toHaveLength(1);
  await act(async () => { finishPage(); });
  expect(await screen.findByText("opened once")).toBeInTheDocument();
});

it.each(["success", "failure"])("keeps cached chats readable while native discovery ends in %s", async outcome => {
  let release: () => void = () => { throw new Error("Discovery did not start"); };
  let refreshed = false;
  mockIPC((command, payload) => {
    if (command === "transcript_list_sessions") {
      if (payload && "refresh" in payload && payload.refresh) return new Promise((resolve, reject) => {
        release = () => {
          refreshed = outcome === "success";
          if (refreshed) resolve({ sessions: SESSIONS, source: "C:/native-chats", warnings: [] });
          else reject(new Error("Native source is unavailable"));
        };
      });
      return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
    }
    if (command === "transcript_thread_page") return [message(refreshed ? "updated conversation" : "cached conversation")];
    throw new Error(command);
  });
  render(<Transcripts />);
  fireEvent.click(await screen.findByRole("button", { name: /alpha\/one/ }));
  expect(await screen.findByText("cached conversation")).toBeInTheDocument();
  expect(screen.queryByText("No chats found in this source.")).not.toBeInTheDocument();
  await act(async () => { release(); });
  expect(screen.getByRole("button", { name: /alpha\/one/ })).toBeInTheDocument();
  expect(await screen.findByText(outcome === "success" ? "updated conversation" : "cached conversation")).toBeInTheDocument();
  if (outcome === "failure") expect(screen.getByText(/Native source is unavailable/)).toBeInTheDocument();
});

it.each(clients)("discovers and groups %s chats by native project, keeping path aliases together", async client => {
  const ipc = vi.fn(command => {
    if (command === "transcript_list_sessions") return { source: "\\\\?\\C:\\native-chats", warnings: [], sessions: [
      { ...SESSIONS[0], project: "\\\\?\\D:\\Work\\Alpha", title: "First chat" },
      { ...SESSIONS[0], sessionId: "alpha/two", project: "d:/work/alpha/", title: "Second chat" },
      { ...SESSIONS[1], project: "/work/Beta", title: "Third chat" },
      { ...SESSIONS[1], sessionId: "lower/one", project: "/work/beta", title: "Fourth chat" },
    ] };
    throw new Error(command);
  });
  mockIPC(ipc);
  render(<HarnessProvider client={client}><Transcripts /></HarnessProvider>);
  const first = await screen.findByRole("button", {name: /First chat/});
  const group = first.closest("details");
  if (!group) throw new Error("Chat has no project group");
  expect(within(group).getByRole("button", {name: /Second chat/})).toBeInTheDocument();
  expect(within(group).queryByRole("button", {name: /Third chat/})).not.toBeInTheDocument();
  expect(screen.getByText("3 · 4 chats")).toBeInTheDocument();
  expect(screen.getByText("Chat source: C:\\native-chats")).toBeInTheDocument();
  expect(within(group).getByText("D:\\Work\\Alpha")).toBeInTheDocument();
  expect(ipc).toHaveBeenCalledWith("transcript_list_sessions", {client, refresh: true, projectRoot: null});
  fireEvent.change(screen.getByLabelText("Filter projects and chats"), {target: {value: "second"}});
  expect(screen.getByRole("button", {name: /Second chat/})).toBeInTheDocument();
  expect(screen.queryByRole("button", {name: /First chat/})).not.toBeInTheDocument();
});

it.each(["success", "cancel", "failure"])("backs up before native deletion: %s", async outcome => {
  vi.mocked(open).mockResolvedValue(outcome === "cancel" ? null : "C:/saved-chats");
  const ipc = vi.fn(command => {
    if (command === "transcript_list_sessions") return {sessions: SESSIONS, source: "C:/native-chats", warnings: []};
    if (command === "transcript_thread_page") return [message("chat content")];
    if (command === "transcript_delete") {
      if (outcome === "failure") throw new Error("Native chat changed; no files removed");
      return {path: "C:/saved-chats/archive", deletedSessions: ["alpha/one"], warning: null};
    }
    throw new Error(command);
  });
  mockIPC(ipc);
  render(<Transcripts />);
  fireEvent.click(await screen.findByRole("button", {name: /alpha\/one/}));
  fireEvent.click(await screen.findByRole("button", {name: "Delete chat"}));
  const dialog = within(screen.getByRole("dialog"));
  expect(ipc).not.toHaveBeenCalledWith("transcript_delete", expect.anything());
  fireEvent.click(dialog.getByRole("button", {name: "Save archive and delete"}));
  await act(async () => {});
  if (outcome === "success") {
    expect(screen.queryByRole("button", {name: /alpha\/one/})).not.toBeInTheDocument();
    expect(screen.getByRole("button", {name: /beta\/one/})).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Restore it with Import Claude archive from C:/saved-chats/archive");
  } else {
    expect(screen.getByRole("button", {name: /alpha\/one/})).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    if (outcome === "failure") expect(dialog.getByRole("alert")).toHaveTextContent("Native chat changed; no files removed");
  }
  if (outcome === "cancel") expect(ipc).not.toHaveBeenCalledWith("transcript_delete", expect.anything());
  else expect(ipc).toHaveBeenCalledWith("transcript_delete", {sessionId: "alpha/one", backupDirectory: "C:/saved-chats"});
});

it.each(["updated", "deleted", "empty"])("refreshes the open chat when its source is %s", async outcome => {
  let refreshed = false;
  let scans = 0;
  mockIPC((command, payload) => {
    if (command === "transcript_list_sessions") {
      if (payload && "refresh" in payload && payload.refresh && ++scans > 1) refreshed = true;
      return {
      sessions: !refreshed || outcome === "updated" ? SESSIONS : outcome === "deleted" ? [SESSIONS[1]] : [],
      source: "C:/native-chats", warnings: [],
      };
    }
    if (command === "transcript_thread_page") return [message(refreshed ? "fresh content" : "old content")];
    throw new Error(command);
  });
  render(<Transcripts />);
  fireEvent.click(await screen.findByRole("button", { name: /alpha\/one/ }));
  await screen.findByText("old content");
  fireEvent.click(screen.getByRole("button", { name: "Refresh chats" }));
  if (outcome === "updated") {
    await screen.findByText("fresh content");
    expect(screen.getByRole("heading", { name: "Session alpha/one" })).toBeInTheDocument();
  } else {
    if (outcome === "empty") await screen.findByText("No chats found in this source.");
    await vi.waitFor(() => expect(screen.queryByRole("heading", { name: /Session / })).not.toBeInTheDocument());
    expect(screen.queryByText("fresh content")).not.toBeInTheDocument();
  }
  expect(screen.queryByText("old content")).not.toBeInTheDocument();
});

it.each(["success", "failure"])("ignores an obsolete session-list %s after a newer load", async outcome => {
  let release: () => void = () => { throw new Error("Expected a pending request"); };
  let loads = 0;
  mockIPC(command => {
    if (command !== "transcript_list_sessions") throw new Error(command);
    if (++loads > 1) return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
    return new Promise((resolve, reject) => { release = () => outcome === "success" ? resolve({sessions: [], source: "C:/native-chats", warnings: []}) : reject(new Error("Obsolete load failed")); });
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
    if (command === "transcript_list_sessions") return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
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
      if (command === "transcript_list_sessions") return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
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
      if (command === "transcript_list_sessions") return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    mockIPC(ipc);
    render(<Transcripts />);

    await screen.findByText("alpha/one");
    fireEvent.change(screen.getByLabelText("Search message text"), { target: { value: "   " } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));

    expect(screen.queryByRole("region", { name: "Search results" })).not.toBeInTheDocument();
    expect(ipc.mock.calls).toEqual([
      ["transcript_list_sessions", { refresh: false, projectRoot: null }],
      ["transcript_list_sessions", { refresh: true, projectRoot: null }],
    ]);
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
      if (command === "transcript_list_sessions") return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
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
      if (command === "transcript_list_sessions") return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
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
      if (command === "transcript_list_sessions") return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
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
        return { sessions: ++sessionLoads === 1 ? SESSIONS : [...SESSIONS, importedSession], source: "C:/native-chats", warnings: [] };
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
      if (command === "transcript_list_sessions") return { sessions: SESSIONS, source: "C:/native-chats", warnings: [] };
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
