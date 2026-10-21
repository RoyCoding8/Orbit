import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { InvokeArgs } from "@tauri-apps/api/core";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { McpPage } from "./Mcp";

afterEach(() => {
  cleanup();
  clearMocks();
});

const HTTP_SERVER = {
  name: "remote",
  kind: "http",
  entry: { type: "http", url: "https://example.test/mcp", timeout: 5, headers: {} },
  enabled: true,
};

const STDIO_SERVER = {
  name: "local",
  kind: "stdio",
  entry: { type: "stdio", command: "npx", args: ["-y", "server"], env: { MODE: "safe" } },
  enabled: true,
};

async function openEditor(name: string) {
  render(<McpPage />);
  await screen.findByRole("heading", { name });
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  await screen.findByRole("heading", { name: "Edit server" });
}

describe("MCP transport switching", () => {
  it("writes a clean stdio entry when a http server is switched to stdio", async () => {
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "mcp_list") return [HTTP_SERVER];
      if (command === "mcp_save") return null;
      throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
    });
    mockIPC(ipc);

    await openEditor("remote");
    fireEvent.change(screen.getByLabelText("Transport"), { target: { value: "stdio" } });
    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "bun" } });
    fireEvent.submit(screen.getByRole("form", { name: "MCP server editor" }));

    await waitFor(() => expect(ipc).toHaveBeenCalledWith("mcp_save", {
      name: "remote",
      entry: { type: "stdio", command: "bun", args: [], env: {}, timeout: 5 },
    }));
  });

  it("writes a clean http entry when a stdio server is switched to http", async () => {
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "mcp_list") return [STDIO_SERVER];
      if (command === "mcp_save") return null;
      throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
    });
    mockIPC(ipc);

    await openEditor("local");
    fireEvent.change(screen.getByLabelText("Transport"), { target: { value: "http" } });
    fireEvent.change(screen.getByLabelText("Server URL"), { target: { value: "https://example.test/mcp" } });
    fireEvent.submit(screen.getByRole("form", { name: "MCP server editor" }));

    await waitFor(() => expect(ipc).toHaveBeenCalledWith("mcp_save", {
      name: "local",
      entry: { type: "http", url: "https://example.test/mcp", headers: {} },
    }));
  });

  it("keeps unknown entry fields on an unchanged round-trip", async () => {
    const ipc = vi.fn((command: string, payload?: InvokeArgs) => {
      if (command === "mcp_list") return [HTTP_SERVER];
      if (command === "mcp_save") return null;
      throw new Error(`Unexpected command ${command} with ${JSON.stringify(payload)}`);
    });
    mockIPC(ipc);

    await openEditor("remote");
    fireEvent.submit(screen.getByRole("form", { name: "MCP server editor" }));

    await waitFor(() => expect(ipc).toHaveBeenCalledWith("mcp_save", {
      name: "remote",
      entry: { type: "http", url: "https://example.test/mcp", timeout: 5, headers: {} },
    }));
  });

  it("keeps a new server name editable and saves the completed name", async () => {
    const ipc = vi.fn((command: string) => {
      if (command === "mcp_list") return [];
      if (command === "mcp_save") return null;
      throw new Error(`Unexpected command ${command}`);
    });
    mockIPC(ipc);
    render(<McpPage />);
    await screen.findByText("Connect a tool server");
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));
    const name = screen.getByLabelText("Server name");
    fireEvent.change(name, { target: { value: "f" } });
    expect(name).toBeEnabled();
    fireEvent.change(name, { target: { value: "fixture-server" } });
    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "fixture-command" } });
    fireEvent.submit(screen.getByRole("form", { name: "MCP server editor" }));
    await waitFor(() => expect(ipc).toHaveBeenCalledWith("mcp_save", {
      name: "fixture-server", entry: { type: "stdio", command: "fixture-command", args: [], env: {} },
    }));
    expect(await screen.findByText("Connect a tool server")).toBeInTheDocument();
  });

  it("keeps save errors and draft fields in the open dialog", async () => {
    mockIPC(command => {
      if (command === "mcp_list") return [HTTP_SERVER];
      if (command === "mcp_save") throw new Error("Server changed before save");
      throw new Error(`Unexpected command ${command}`);
    });
    await openEditor("remote");
    fireEvent.change(screen.getByLabelText("Server URL"), { target: { value: "https://changed.test/mcp" } });
    fireEvent.submit(screen.getByRole("form", { name: "MCP server editor" }));
    const error = await screen.findByRole("alert");
    expect(error).toHaveTextContent("Server changed before save");
    expect(screen.getByRole("dialog", { name: "Edit server" })).toContainElement(error);
    expect(screen.getByLabelText("Server URL")).toHaveValue("https://changed.test/mcp");
  });
});
