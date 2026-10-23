import { useState } from "react";
import { M3eButton } from "@m3e/react/button";
import { Skills } from "./pages/Skills";
import { Config } from "./pages/Config";
import { McpPage } from "./pages/Mcp";
import { Transcripts } from "./pages/Transcripts";
import { Hooks } from "./pages/Hooks";
import { Memories } from "./pages/Memories";
import { History } from "./pages/History";
import { HarnessProvider, type Client } from "./HarnessContext";
import "@m3e/web/theme";
import "@m3e/web/button";
import "./app.css";

const pages = [
  { id: "overview", label: "Overview", icon: "▦" },
  { id: "skills", label: "Skills", icon: "◇" },
  { id: "memories", label: "Memories", icon: "◈" },
  { id: "mcp", label: "Servers", icon: "⌘" },
  { id: "config", label: "Settings", icon: "⚙" },
  { id: "hooks", label: "Hooks", icon: "↪" },
  { id: "transcripts", label: "Transcripts", icon: "▤" },
  { id: "history", label: "History", icon: "↶" },
] as const;

type Page = typeof pages[number]["id"];

const views = {
  skills: Skills,
  memories: Memories,
  mcp: McpPage,
  config: Config,
  hooks: Hooks,
  transcripts: Transcripts,
  history: History,
} satisfies Record<Exclude<Page, "overview">, typeof Skills>;

export function App() {
  const [page, setPage] = useState<Page>("overview");
  const [client, setClient] = useState<Client>("claude-code");
  const clientLabel = client === "codex" ? "Codex" : "Claude Code";
  const SelectedPage = page === "overview" ? null : views[page];

  return (
    <m3e-theme color="#6750a4" scheme="auto" density={-1} strong-focus>
      <div className="application-shell">
        <header className="topbar">
          <div className="brand-lockup">
            <span className="brand-mark" aria-hidden="true">O</span>
            <div>
              <p className="eyebrow">CODING AGENT WORKSPACE</p>
              <h1>Orbit</h1>
            </div>
          </div>
          <label className="connection-state">
            Coding agent
            <select aria-label="Coding agent" value={client} onChange={event => {
              const value = event.target.value;
              if (value === "claude-code" || value === "codex") setClient(value);
            }}>
              <option value="claude-code">Claude Code</option>
              <option value="codex">Codex</option>
            </select>
          </label>
        </header>
        <main className="workspace">
          <aside className="rail" aria-label="Main navigation">
            {pages.map(item => (
              <button key={item.id} className={`rail-item${page === item.id ? " active" : ""}`} aria-current={page === item.id ? "page" : undefined} onClick={() => setPage(item.id)} type="button">
                <span aria-hidden="true">{item.icon}</span><span>{item.label}</span>
              </button>
            ))}
          </aside>
          <HarnessProvider client={client} key={client}>
          {SelectedPage ? <SelectedPage /> : (
            <section className="content-pane" aria-labelledby="welcome-title">
              <div className="pane-heading">
                <div>
                  <p className="eyebrow">WORKSPACE</p>
                  <h2 id="welcome-title">Your harness, in view.</h2>
                  <p className="supporting-copy">A clear home for the tools and context that shape your coding agent.</p>
                </div>
              </div>
              <div className="empty-panel">
                <p className="eyebrow">{clientLabel.toUpperCase()}</p>
                <h3 id="empty-title">Your {clientLabel} setup</h3>
                <p className="empty-copy">Manage skills, project memories, MCP servers, hooks, settings, and chats. Review changes and restore saved versions in History.</p>
                <div className="card-actions">
                  <M3eButton variant="filled" onClick={() => setPage("skills")}>Open skills</M3eButton>
                  <M3eButton variant="filled-tonal" onClick={() => setPage("config")}>Open settings</M3eButton>
                </div>
              </div>
              <footer className="pane-footer"><span>LOCAL FIRST</span><span>Changes stay on this device</span></footer>
            </section>
          )}
          </HarnessProvider>
        </main>
      </div>
    </m3e-theme>
  );
}
