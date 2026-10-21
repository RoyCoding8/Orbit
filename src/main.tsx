import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("root element missing from index.html");
}

createRoot(rootElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

declare global {
  interface Window {
    shellReady?: boolean;
  }
}
window.shellReady = true;
console.log("shell-ready");