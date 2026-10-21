import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";

afterEach(() => {
  cleanup();
  clearMocks();
  vi.restoreAllMocks();
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

class TestStyleSheet {
  replaceSync(_text: string) {}
}

Object.defineProperty(globalThis, "CSSStyleSheet", {
  configurable: true,
  value: TestStyleSheet,
});

Object.defineProperty(globalThis, "matchMedia", {
  configurable: true,
  value: (_query: string) => ({
    matches: false,
    media: _query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent() { return false; },
  }),
});

for (const prototype of [Document.prototype, ShadowRoot.prototype]) {
  Object.defineProperty(prototype, "adoptedStyleSheets", {
    configurable: true,
    get() {
      return this.__adoptedStyleSheets ?? [];
    },
    set(sheets: TestStyleSheet[]) {
      this.__adoptedStyleSheets = sheets;
    },
  });
}

Object.defineProperties(HTMLDialogElement.prototype, {
  showModal: { configurable: true, value(this: HTMLDialogElement) { this.setAttribute("open", ""); } },
  close: { configurable: true, value(this: HTMLDialogElement) { this.removeAttribute("open"); } },
});

const attachInternals = HTMLElement.prototype.attachInternals;
if (attachInternals) {
  Object.defineProperty(HTMLElement.prototype, "attachInternals", {
    configurable: true,
    value(this: HTMLElement) {
      const internals = attachInternals.call(this);
      Object.defineProperty(internals, "states", {
        configurable: true,
        value: new Set<string>(),
      });
      return internals;
    },
  });
}

declare global {
  interface Document {
    __adoptedStyleSheets?: TestStyleSheet[];
  }
  interface ShadowRoot {
    __adoptedStyleSheets?: TestStyleSheet[];
  }
}
