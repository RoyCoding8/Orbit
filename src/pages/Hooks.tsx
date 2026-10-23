import { useCallback, useEffect, useRef, useState } from "react";
import { useHarness } from "../HarnessContext";

type Layer = "user" | "project";
type Hook = {
  id: string;
  event: string;
  matcher: string | null;
  commands: string[];
  enabled: boolean;
  layer: Layer;
  source_path: string;
  editable: boolean;
};
type TestResult = {
  stdout: string;
  stderr: string;
  exit_code: number | null;
  timed_out: boolean;
};
const events = [
  "PreToolUse",
  "PostToolUse",
  "Notification",
  "UserPromptSubmit",
  "Stop",
  "SubagentStop",
  "PreCompact",
  "SessionStart",
  "SessionEnd",
];
const destructive =
  /(?:\brm\b|\brmdir\b|\bdel\b|\berase\b|\bformat(?:\.com)?\b|\bRemove-Item\b|\bshutdown\b|\bmkfs\b|\bdd\s+if=)/i;

export function Hooks() {
  const { invoke, client, label } = useHarness();
  const [hooks, setHooks] = useState<Hook[]>([]);
  const [loadState, setLoadState] = useState<"loading" | "ready" | "error">(
    "loading",
  );
  const [layer, setLayer] = useState<Layer>("user");
  const [projectRoot, setProjectRoot] = useState<string | null>(null);
  const [event, setEvent] = useState(events[0]);
  const [matcher, setMatcher] = useState("");
  const [command, setCommand] = useState("");
  const [editing, setEditing] = useState<Hook | null>(null);
  const [sample, setSample] = useState(
    '{"tool_name":"Read","tool_input":{"file_path":"example.txt"}}',
  );
  const [result, setResult] = useState<TestResult | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const loadSequence = useRef(0);

  const load = useCallback(async () => {
    const sequence = ++loadSequence.current;
    setLoadState("loading");
    try {
      const loaded = await invoke<Hook[]>("hooks_list", { projectRoot });
      if (sequence !== loadSequence.current) return;
      setHooks(loaded);
      setError("");
      setLoadState("ready");
    } catch (cause) {
      if (sequence !== loadSequence.current) return;
      setError(String(cause));
      setLoadState("error");
    }
  }, [projectRoot, invoke]);
  useEffect(() => {
    void load();
    return () => { loadSequence.current++; };
  }, [load]);

  const chooseProject = async () => {
    try {
      const selected = await invoke<string | null>("project_choose");
      if (selected !== null && selected !== projectRoot) {
        setEditing(null);
        setCommand("");
        setMatcher("");
        setResult(null);
        setProjectRoot(selected);
      }
    } catch (cause) {
      setError(String(cause));
    }
  };
  const save = async () => {
    if (
      destructive.test(command) &&
      !window.confirm(
        `This command looks destructive and will be saved to ${label} settings. Save it anyway?`,
      )
    )
      return;
    setBusy(true);
    setError("");
    try {
      await invoke("hooks_save", {
        draft: {
          layer,
          event,
          matcher: matcher.trim() || null,
          command,
          editId: editing?.id ?? null,
        },
        projectRoot,
      });
      setCommand("");
      setMatcher("");
      setEditing(null);
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const toggle = async (hook: Hook) => {
    setBusy(true);
    setError("");
    try {
      if (hook.enabled) {
        await invoke("hooks_disable", {
          layer: hook.layer,
          event: hook.event,
          id: hook.id,
          projectRoot,
        });
      } else await invoke("hooks_enable", { id: hook.id, projectRoot });
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const edit = (hook: Hook) => {
    if (!hook.enabled) return;
    setEditing(hook);
    setLayer(hook.layer);
    setEvent(hook.event);
    setMatcher(hook.matcher ?? "");
    setCommand(hook.commands[0] ?? "");
    setResult(null);
    setError("");
  };
  const remove = async (hook: Hook) => {
    if (!window.confirm(`Delete the ${hook.event} hook?`)) return;
    setBusy(true);
    setError("");
    try {
      await invoke("hooks_delete", {
        layer: hook.layer,
        event: hook.event,
        id: hook.id,
        projectRoot,
      });
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const test = async () => {
    let input: unknown;
    try {
      input = JSON.parse(sample);
    } catch {
      setError("Sample input must be valid JSON.");
      return;
    }
    setBusy(true);
    setError("");
    setResult(null);
    try {
      setResult(await invoke<TestResult>("hooks_test", { command, input }));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="hooks-page" aria-labelledby="hooks-title">
      <div className="pane-heading">
        <div>
          <p className="eyebrow">{label.toUpperCase()}</p>
          <h2 id="hooks-title">Hooks</h2>
          <p className="supporting-copy">
            Build event hooks, test them with sample JSON, and manage their
            settings scope.
          </p>
        </div>
        <div className="config-actions">
          <button
            className="action-button"
            type="button"
            disabled={busy || loadState === "loading"}
            onClick={() => void chooseProject()}
          >
            Choose project
          </button>
          <button
            className="action-button"
            type="button"
            disabled={busy || loadState === "loading"}
            onClick={() => void load()}
          >
            Refresh
          </button>
        </div>
      </div>
      {client === "codex" && <p className="supporting-copy">Codex reviews hook definitions before running them. Use /hooks in Codex to approve, enable or disable hooks. Editing a definition requires a new review.</p>}
      <div className="hooks-layout">
        <section className="config-editor" aria-label="Hook builder">
          <h3>{editing ? "Edit hook" : "Add hook"}</h3>
          <label className="field-label" htmlFor="hook-layer">
            Settings scope
          </label>
          <select
            id="hook-layer"
            value={layer}
            disabled={busy}
            onChange={(e) =>
              setLayer(e.target.value === "project" ? "project" : "user")
            }
          >
            <option value="user">User</option>
            <option value="project">Project</option>
          </select>
          <p className="layer-path">
            {client === "codex" ? (layer === "user" ? "User hooks in CODEX_HOME/config.toml and hooks.json" : projectRoot ? `${projectRoot}/.codex/config.toml and hooks.json` : "Choose a project to edit its .codex hooks") : layer === "user"
              ? "User settings.json in the configured Claude Code directory"
              : projectRoot
                ? `${projectRoot}/.claude/settings.json`
                : "Choose a project to edit its .claude/settings.json"}
          </p>
          <label className="field-label" htmlFor="hook-event">
            Event
          </label>
          <select
            id="hook-event"
            value={event}
            disabled={busy}
            onChange={(e) => setEvent(e.target.value)}
          >
            {[...new Set([...(client === "codex" ? ["PreToolUse", "PermissionRequest", "PostToolUse", "PreCompact", "PostCompact", "SessionStart", "SessionEnd", "SubagentStart", "SubagentStop", "UserPromptSubmit", "Stop", "Interrupt"] : events), ...hooks.map((hook) => hook.event)])].map(
              (name) => (
                <option key={name}>{name}</option>
              ),
            )}
          </select>
          <label className="field-label" htmlFor="hook-matcher">
            Matcher (optional)
          </label>
          <input
            id="hook-matcher"
            value={matcher}
            onChange={(e) => setMatcher(e.target.value)}
            placeholder="Read|Write"
          />
          <label className="field-label" htmlFor="hook-command">
            Command
          </label>
          <textarea
            id="hook-command"
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            rows={3}
            spellCheck={false}
            placeholder="node ./hooks/check.js"
          />
          {command && destructive.test(command) && (
            <p className="hook-warning" role="status">
              This command looks destructive. Saving requires confirmation.
            </p>
          )}
          <label className="field-label" htmlFor="hook-sample">
            Sample JSON input
          </label>
          <textarea
            id="hook-sample"
            value={sample}
            onChange={(e) => setSample(e.target.value)}
            rows={5}
            spellCheck={false}
          />
          <div className="hook-actions">
            <button
              className="action-button"
              type="button"
              onClick={() => void test()}
              disabled={busy || !command.trim()}
            >
              Test command
            </button>
            <button
              className="action-button primary"
              type="button"
              onClick={() => void save()}
              disabled={
                busy ||
                loadState !== "ready" ||
                !command.trim() ||
                (layer === "project" && !projectRoot)
              }
            >
              {editing ? "Update hook" : "Save hook"}
            </button>
            {editing && (
              <button
                className="action-button"
                type="button"
                onClick={() => {
                  setEditing(null);
                  setCommand("");
                  setMatcher("");
                }}
              >
                Cancel edit
              </button>
            )}
          </div>
          {result && (
            <div className="hook-result" aria-live="polite">
              <strong>
                {result.timed_out
                  ? "Timed out"
                  : `Exit code ${result.exit_code ?? "unknown"}`}
              </strong>
              <p>stdout</p>
              <pre>{result.stdout || "(empty)"}</pre>
              <p>stderr</p>
              <pre>{result.stderr || "(empty)"}</pre>
            </div>
          )}
        </section>
        <section className="hook-list-panel" aria-labelledby="hook-list-title">
          <div className="section-heading">
            <div>
              <p className="eyebrow">SETTINGS</p>
              <h3 id="hook-list-title">
                Configured hooks <span className="count">{hooks.length}</span>
              </h3>
            </div>
          </div>
          {error && (
            <p className="inline-error" role="alert">
              {error}
            </p>
          )}
          {loadState === "loading" ? (
            <p className="config-empty" role="status">
              Loading hooks…
            </p>
          ) : loadState === "error" ? null : hooks.length === 0 ? (
            <p className="config-empty">
              No hooks found in the user or selected project settings.
            </p>
          ) : (
            <div className="hook-list">
              {hooks.map((hook) => (
                <article className="hook-card" key={hook.id}>
                  <div className="hook-copy">
                    <div className="skill-title-row">
                      <h3>{hook.event}</h3>
                      <span
                        className={`state-badge${hook.enabled ? "" : " state-off"}`}
                      >
                        {client === "codex" ? "Configured" : hook.enabled ? "Enabled" : "Disabled"}
                      </span>
                    </div>
                    <p className="hook-meta">
                      {hook.layer} scope · matcher: {hook.matcher || "all"}
                    </p>
                    {hook.source_path && <p className="layer-path">{hook.source_path}{!hook.editable && " · Inherited, read-only"}</p>}
                    {hook.commands.map((value, i) => (
                      <code className="hook-command" key={`${hook.id}-${i}`}>
                        {value}
                      </code>
                    ))}
                  </div>
                  <div className="card-actions">
                    {hook.enabled && (
                      <button
                        className="action-button"
                        type="button"
                        disabled={busy || !hook.editable || hook.commands.length !== 1}
                        title={
                          hook.commands.length !== 1
                            ? "Use Settings to edit hooks with multiple commands or other hook types."
                            : undefined
                        }
                        onClick={() => edit(hook)}
                      >
                        Edit
                      </button>
                    )}
                    {client === "claude-code" && <button
                      className="action-button"
                      type="button"
                      disabled={busy}
                      onClick={() => void toggle(hook)}
                    >
                      {hook.enabled ? "Disable" : "Enable"}
                    </button>}
                    {hook.enabled && (
                      <button
                        className="action-button danger"
                        type="button"
                        disabled={busy || !hook.editable || (client === "codex" && hook.commands.length === 0)}
                        onClick={() => void remove(hook)}
                      >
                        Delete
                      </button>
                    )}
                  </div>
                </article>
              ))}
            </div>
          )}
        </section>
      </div>
      <footer className="pane-footer">
        <span>LOCAL FIRST</span>
        <span>Hook commands execute with a five-second test timeout.</span>
      </footer>
    </section>
  );
}
