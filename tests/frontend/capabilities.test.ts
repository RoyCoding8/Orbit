import { expect, it } from "vitest";
import capability from "../../src-tauri/capabilities/default.json";
import { clients, harnesses } from "../../src/lib/harnesses";
import config from "../../src-tauri/tauri.conf.json";

it("does not grant unused filesystem and opener commands to the webview", () => {
  expect(capability).toMatchObject({
    permissions: [
      "core:default",
      "dialog:default",
      "window-state:default",
    ],
  });
});

it("restricts packaged webview scripts and connections without opening remote sources", () => {
  expect(config.app.security.csp).toBe("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src ipc: http://ipc.localhost; img-src 'self' data:; object-src 'none'; base-uri 'self'; frame-src 'none'; form-action 'none'");
  expect(config.app.security.devCsp).toBe("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src ipc: http://ipc.localhost http://127.0.0.1:1420 ws://127.0.0.1:1420; img-src 'self' data:; object-src 'none'; base-uri 'self'; frame-src 'none'; form-action 'none'");
});

it("exposes Muse Code, Junie CLI, and Cursor CLI with native capability flags", () => {
  expect(clients).toEqual(expect.arrayContaining(["muse", "junie", "cursor"]));
  expect(harnesses.muse.label).toBe("Muse Code");
  expect(harnesses.muse.settingsLayers).toEqual(["user"]);
  expect(harnesses.muse.transcripts.archive).toBe("none");
  expect(harnesses.muse.hooks.kind).toBe("commands");
  if (harnesses.muse.hooks.kind === "commands") {
    expect(harnesses.muse.hooks.events).toHaveLength(17);
  }

  expect(harnesses.junie.label).toBe("Junie CLI");
  expect(harnesses.junie.settingsLayers).toEqual(["user", "project"]);
  expect(harnesses.junie.transcripts.archive).toBe("none");
  expect(harnesses.junie.hooks.kind).toBe("commands");
  if (harnesses.junie.hooks.kind === "commands") {
    expect(harnesses.junie.hooks.projectPath).toBeNull();
  }

  expect(harnesses.cursor.label).toBe("Cursor CLI");
  expect(harnesses.cursor.settingsLayers).toEqual(["user", "project"]);
  expect(harnesses.cursor.transcripts.archive).toBe("none");
  expect(harnesses.cursor.hooks.kind).toBe("commands");
  if (harnesses.cursor.hooks.kind === "commands") {
    expect(harnesses.cursor.hooks.events).toHaveLength(21);
  }
});
