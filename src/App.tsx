import { useState } from "react";
import { M3eButton } from "@m3e/react/button";
import { Skills } from "./pages/Skills";
import { Config } from "./pages/Config";
import { McpPage } from "./pages/Mcp";
import { Transcripts } from "./pages/Transcripts";
import { Hooks } from "./pages/Hooks";
import "@m3e/web/theme";
import "@m3e/web/button";
import "./app.css";

type Page = "overview" | "skills" | "mcp" | "config" | "hooks" | "transcripts";

export function App() {
  const [page, setPage] = useState<Page>("overview");

  return (
    <m3e-theme color="#6750a4" scheme="auto" density={-1} strong-focus>
      <div className="application-shell">
        <header className="topbar">
          <div className="brand-lockup">
            <span className="brand-mark" aria-hidden="true">H</span>
            <div>
              <p className="eyebrow">HARNESS MANAGER</p>
              <h1>Harness Agent</h1>
            </div>
          </div>
          <div className="connection-state">Claude Code</div>
        </header>
        <main className="workspace">
          <aside className="rail" aria-label="Main navigation">
            <button className={`rail-item${page === "overview" ? " active" : ""}`} onClick={() => setPage("overview")} type="button"><span aria-hidden="true">▦</span><span>Overview</span></button>
            <button className={`rail-item${page === "skills" ? " active" : ""}`} onClick={() => setPage("skills")} type="button"><span aria-hidden="true">◇</span><span>Skills</span></button>
            <button className={`rail-item${page === "mcp" ? " active" : ""}`} onClick={() => setPage("mcp")} type="button"><span aria-hidden="true">⌘</span><span>Servers</span></button>
            <button className={`rail-item${page === "config" ? " active" : ""}`} onClick={() => setPage("config")} type="button"><span aria-hidden="true">⚙</span><span>Settings</span></button>
            <button className={`rail-item${page === "hooks" ? " active" : ""}`} onClick={() => setPage("hooks")} type="button"><span aria-hidden="true">↪</span><span>Hooks</span></button>
            <button className={`rail-item${page === "transcripts" ? " active" : ""}`} onClick={() => setPage("transcripts")} type="button"><span aria-hidden="true">▤</span><span>Transcripts</span></button>
          </aside>
          {page === "skills" ? <Skills /> : page === "mcp" ? <McpPage /> : page === "config" ? <Config /> : page === "hooks" ? <Hooks /> : page === "transcripts" ? <Transcripts /> : (
            <section className="content-pane" aria-labelledby="welcome-title">
              <div className="pane-heading">
                <div>
                  <p className="eyebrow">WORKSPACE</p>
                  <h2 id="welcome-title">Your harness, in view.</h2>
                  <p className="supporting-copy">A clear home for the tools and context that shape your coding agent.</p>
                </div>
              </div>
              <div className="empty-panel">
                <p className="eyebrow">CLAUDE CODE</p>
                <h3 id="empty-title">Your Claude Code setup</h3>
                <p className="empty-copy">Skills, MCP servers, hooks, settings, and transcripts read from the Claude Code directories on this device.</p>
                <div className="card-actions">
                  <M3eButton variant="filled" onClick={() => setPage("skills")}>Open skills</M3eButton>
                  <M3eButton variant="filled-tonal" onClick={() => setPage("config")}>Open settings</M3eButton>
                </div>
              </div>
              <footer className="pane-footer"><span>LOCAL FIRST</span><span>Changes stay on this device</span></footer>
            </section>
          )}
        </main>
      </div>
    </m3e-theme>
  );
}
