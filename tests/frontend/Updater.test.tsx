import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { UpdateChecker } from "../../src/components/UpdateChecker";

vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

function mockUpdate(overrides: Partial<Update> = {}): Update {
  return {
    available: true,
    currentVersion: "0.5.1",
    version: "0.6.0",
    body: "In-app updates and bug fixes.",
    rawJson: {},
    rid: 1,
    download: vi.fn(),
    install: vi.fn(),
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  } as unknown as Update;
}

describe("UpdateChecker", () => {
  it("shows update dialog on startup check and downloads, installs, and relaunches", async () => {
    const update = mockUpdate();
    vi.mocked(check).mockResolvedValue(update);
    vi.mocked(relaunch).mockResolvedValue(undefined);

    render(<UpdateChecker autoCheck />);

    expect(await screen.findByRole("dialog", { name: "Update available: v0.6.0" })).toBeInTheDocument();
    expect(screen.getByText("In-app updates and bug fixes.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Update & Restart" }));

    await waitFor(() => {
      expect(update.downloadAndInstall).toHaveBeenCalledTimes(1);
      expect(relaunch).toHaveBeenCalledTimes(1);
    });
  });

  it("shows up-to-date status on manual check when no update is available", async () => {
    vi.mocked(check).mockResolvedValue(null);

    render(<UpdateChecker autoCheck={false} />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Up to date");
  });

  it("surfaces errors on manual check and install failure", async () => {
    vi.mocked(check).mockRejectedValueOnce(new Error("Network offline"));

    render(<UpdateChecker autoCheck={false} />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Update check failed. Network offline");

    const failingUpdate = mockUpdate({
      downloadAndInstall: vi.fn().mockRejectedValue(new Error("Signature mismatch")),
    });
    vi.mocked(check).mockResolvedValueOnce(failingUpdate);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByRole("dialog", { name: "Update available: v0.6.0" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Update & Restart" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not install update. Signature mismatch");
    expect(relaunch).not.toHaveBeenCalled();
  });

  it("silently ignores network failures during background startup check", async () => {
    vi.mocked(check).mockRejectedValue(new Error("Offline"));

    render(<UpdateChecker autoCheck />);

    await waitFor(() => expect(check).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
