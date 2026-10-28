import { useState } from "react";
import { Skills } from "./pages/Skills";
import { Config } from "./pages/Config";
import { McpPage } from "./pages/Mcp";
import { Transcripts } from "./pages/Transcripts";
import { Hooks } from "./pages/Hooks";
import { Memories } from "./pages/Memories";
import { History } from "./pages/History";
import { HarnessTransfer } from "./components/HarnessTransfer";
import { HarnessProvider, type Client } from "./HarnessContext";
import { clients, harnesses, isClient } from "./harnesses";
import "@m3e/web/theme";
import "./app.css";
import "./theme.css";

const pages = [
  { id: "overview", label: "Overview", icon: "▦", description: "Your coding workspace" },
  { id: "skills", label: "Skills", icon: "◇", description: "Browse, edit, and install SKILL.md files." },
  { id: "memories", label: "Memories", icon: "◈", description: "Edit the memory and instruction files this agent reads." },
  { id: "mcp", label: "Servers", icon: "⌘", description: "Configure MCP servers and test their connections." },
  { id: "config", label: "Settings", icon: "⚙", description: "Inspect effective settings and where each value comes from." },
  { id: "hooks", label: "Hooks", icon: "↪", description: "Configure hook commands and test them with sample input." },
  { id: "transcripts", label: "Chats", icon: "▤", description: "Browse chats by project, then export, import, or convert them." },
  { id: "history", label: "History", icon: "↶", description: "Restore saved versions and browse operation logs." },
] as const;

type Page = typeof pages[number]["id"];
type Theme = "system" | "light" | "dark";

function readTheme(): Theme {
  try {
    const stored = localStorage.getItem("orbit.theme");
    if (stored === "light" || stored === "dark") return stored;
  } catch {}
  return "system";
}

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
  const [theme, setTheme] = useState<Theme>(readTheme);
  const clientLabel = harnesses[client].label;
  const SelectedPage = page === "overview" ? null : views[page];

  return (
    <m3e-theme color="#2563eb" scheme={theme === "system" ? "auto" : theme} density={-1} strong-focus>
      <HarnessProvider client={client} key={client}>
        <div className="application-shell" data-theme={theme}>
          <header className="topbar">
            <div className="brand-lockup">
              <span className="brand-mark" aria-hidden="true">
                <svg viewBox="0 0 24 24" width="24" height="24" focusable="false">
                  <path d="M16.91 12.98A9.5 3.2-24 1 0 9.45 16.3" fill="none" stroke="currentColor" strokeWidth="1.4" opacity=".45" />
                  <circle cx="12" cy="12" r="5" fill="currentColor" />
                  <circle cx="10.2" cy="10.2" r="1.2" fill="#ffffff" opacity=".85" />
                  <path d="M16.91 12.98A9.5 3.2-24 0 1 9.45 16.3" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
                </svg>
              </span>
              <h1>Orbit</h1>
            </div>
            <div className="workspace-controls">
              <HarnessTransfer />
              <label className="connection-state">
                Agent
                <select aria-label="Coding agent" value={client} onChange={event => {
                  const value = event.target.value;
                  if (isClient(value)) setClient(value);
                }}>
                  {clients.map(id => <option key={id} value={id}>{harnesses[id].label}</option>)}
                </select>
              </label>
              <button
                className="theme-toggle"
                type="button"
                aria-label="Toggle light and dark theme"
                title="Toggle light and dark theme"
                onClick={() => {
                  const isDark = theme === "system"
                    ? window.matchMedia("(prefers-color-scheme: dark)").matches
                    : theme === "dark";
                  const nextTheme = isDark ? "light" : "dark";
                  setTheme(nextTheme);
                  try { localStorage.setItem("orbit.theme", nextTheme); } catch {}
                }}
              >
                <svg className="theme-icon theme-icon-sun" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                  <circle cx="12" cy="12" r="4" />
                  <path d="M12 2v2m0 16v2M4.93 4.93l1.42 1.42m11.3 11.3 1.42 1.42M2 12h2m16 0h2M4.93 19.07l1.42-1.42m11.3-11.3 1.42-1.42" />
                </svg>
                <svg className="theme-icon theme-icon-moon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                  <path d="M20.1 15.3A8.4 8.4 0 0 1 8.7 3.9 8.5 8.5 0 1 0 20.1 15.3Z" />
                </svg>
              </button>
            </div>
          </header>
          <main className="workspace">
            <aside className="rail" aria-label="Main navigation">
              {pages.map(item => (
                <button key={item.id} data-section={item.id} className={`rail-item${page === item.id ? " active" : ""}`} aria-current={page === item.id ? "page" : undefined} onClick={() => setPage(item.id)} type="button">
                  <span aria-hidden="true">{item.icon}</span><span>{item.label}</span>
                </button>
              ))}
            </aside>
            {SelectedPage ? <SelectedPage /> : (
              <section className="content-pane" aria-labelledby="welcome-title">
                <div className="pane-heading">
                  <div>
                    <h2 id="welcome-title">{clientLabel}</h2>
                    <p className="supporting-copy">Manage the files, tools, settings, and chats used by this coding agent.</p>
                  </div>
                </div>
                <div className="overview-grid">
                  {pages.filter(item => item.id !== "overview").map(item => <button type="button" key={item.id} data-section={item.id}
                    className="overview-link" aria-label={`Open ${item.label.toLowerCase()}`} onClick={() => setPage(item.id)}>
                    <span className="feature-icon" aria-hidden="true">{item.icon}</span>
                    <span><strong>{item.label}</strong><span>{item.description}</span></span>
                    <span className="shortcut-arrow" aria-hidden="true">↗</span>
                  </button>)}
                </div>
              </section>
            )}
          </main>
        </div>
      </HarnessProvider>
    </m3e-theme>
  );
}
