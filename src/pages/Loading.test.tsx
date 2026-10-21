import { afterEach, describe, expect, it } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { Hooks } from "./Hooks";
import { McpPage } from "./Mcp";
import { Transcripts } from "./Transcripts";

afterEach(() => { cleanup(); clearMocks(); });

const pages = [
  { name: "hooks", Component: Hooks, command: "hooks_list", loading: "Loading hooks…", empty: "No hooks found in the user or selected project settings." },
  { name: "MCP servers", Component: McpPage, command: "mcp_list", loading: "Loading MCP servers…", empty: "Connect a tool server" },
  { name: "transcripts", Component: Transcripts, command: "transcript_list_sessions", loading: "Loading sessions…", empty: "No sessions indexed yet." },
];

describe.each(pages)("$name load feedback", ({ Component, command, loading, empty }) => {
  it("shows an empty state only after the load succeeds", async () => {
    let finish: (value: unknown) => void = () => {};
    mockIPC(name => {
      if (name !== command) throw new Error(`Unexpected command ${name}`);
      return new Promise(resolve => { finish = resolve; });
    });
    render(<Component />);
    expect(screen.getByText(loading)).toBeInTheDocument();
    expect(screen.queryByText(empty)).not.toBeInTheDocument();
    await act(async () => { finish([]); });
    expect(await screen.findByText(empty)).toBeInTheDocument();
    expect(screen.queryByText(loading)).not.toBeInTheDocument();
  });

  it("reports a rejected load instead of claiming the source is empty", async () => {
    mockIPC(() => Promise.reject(new Error("Fixture read failed")));
    render(<Component />);
    expect(await screen.findByText(/Fixture read failed/)).toBeInTheDocument();
    expect(screen.queryByText(empty)).not.toBeInTheDocument();
    expect(screen.queryByText(loading)).not.toBeInTheDocument();
  });
});
