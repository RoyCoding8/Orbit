import { expect, it } from "vitest";
import capability from "../src-tauri/capabilities/default.json";
import config from "../src-tauri/tauri.conf.json";

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
