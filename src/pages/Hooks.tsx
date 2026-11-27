import { useCallback, useEffect, useMemo, useState } from "react";
import { useHarness } from "../lib/HarnessContext";
import { displayPath } from "../lib/pathDisplay";
import { useSequencedLoad } from "../lib/useSequencedLoad";
import { JsonEditor } from "../components/JsonEditor";
import { SearchField } from "../components/SearchField";
import "../styles/ListControls.css";

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
type HookSource = { id: string; label: string; layer: Layer; events: string[]; canToggle: boolean };
type HookPolicy = Extract<ReturnType<typeof useHarness>["profile"]["hooks"], { kind: "commands" }>;
const destructive =
  /(?:\brm\b|\brmdir\b|\bdel\b|\berase\b|\bformat(?:\.com)?\b|\bRemove-Item\b|\bshutdown\b|\bmkfs\b|\bdd\s+if=)/i;

export function Hooks() {
  const { profile, label } = useHarness();
  if (profile.hooks.kind === "plugins") return (
    <section className="content-pane" aria-labelledby="hooks-title">
      <div className="pane-heading"><div><p className="eyebrow">{label.toUpperCase()}</p><h2 id="hooks-title">Hooks</h2></div></div>
      <p className="supporting-copy">{profile.hooks.reason}</p>
    </section>
  );
  return <CommandHooks policy={profile.hooks} />;
}

function CommandHooks({ policy }: { policy: HookPolicy }) {
  const { invoke, client, label, projectRoot, setProjectRoot } = useHarness();
  const [hooks, setHooks] = useState<Hook[]>([]);
  const [hookSearch, setHookSearch] = useState("");
  const [loadState, setLoadState] = useState<"loading" | "ready" | "error">(
    "loading",
  );
  const [layer, setLayer] = useState<Layer>("user");
  const [event, setEvent] = useState(policy.events[0] ?? "");
  const [sources, setSources] = useState<HookSource[]>([]);
  const [sourceId, setSourceId] = useState("");
  const [matcher, setMatcher] = useState("");
  const [command, setCommand] = useState("");
  const [editing, setEditing] = useState<Hook | null>(null);
  const [sample, setSample] = useState(
    '{"tool_name":"Read","tool_input":{"file_path":"example.txt"}}',
  );
  const [result, setResult] = useState<TestResult | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const { start: startLoad } = useSequencedLoad();
  const sourceById = useMemo(() => new Map(sources.map((item) => [item.id, item])), [sources]);
  const source = sourceById.get(sourceId);
  const effectiveLayer = source?.layer ?? layer;
  const events = source?.events ?? policy.events;
  const normalizedHookSearch = hookSearch.trim().toLocaleLowerCase();
  const visibleHooks = useMemo(
    () => normalizedHookSearch
      ? hooks.filter((hook) => [hook.id, hook.event, hook.matcher, hook.layer, hook.source_path, ...hook.commands]
          .some((value) => value?.toLocaleLowerCase().includes(normalizedHookSearch)))
      : hooks,
    [hooks, normalizedHookSearch],
  );
  useEffect(() => {
    if (!events.includes(event)) setEvent(events[0] ?? "");
  }, [event, events]);

  const load = useCallback(async () => {
    const request = startLoad();
    setLoadState("loading");
    try {
      const [loaded, nativeSources] = await Promise.all([
        invoke<Hook[]>("hooks_list", { projectRoot }),
        client === "kiro" ? invoke<HookSource[]>("hooks_sources", { projectRoot }) : Promise.resolve([]),
      ]);
      if (!request.isCurrent()) return;
      setHooks(loaded);
      setSources(nativeSources);
      setSourceId(current => nativeSources.some(item => item.id === current) ? current : nativeSources[0]?.id ?? "");
      setError("");
      setLoadState("ready");
    } catch (cause) {
      if (!request.isCurrent()) return;
      setError(String(cause));
      setLoadState("error");
    }
  }, [projectRoot, invoke, client, startLoad]);

  useEffect(() => {
    void load();
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
    await runBusy(async () => {
      await invoke("hooks_save", {
        draft: {
          layer: effectiveLayer,
          event,
          matcher: matcher.trim() || null,
          command,
          editId: editing?.id ?? null,
          ...(client === "kiro" ? { sourceId } : {}),
        },
        projectRoot,
      });
      setCommand("");
      setMatcher("");
      setEditing(null);
      await load();
    });
  };
  const runBusy = async (task: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await task();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };
  const toggle = (hook: Hook) =>
    runBusy(async () => {
      if (hook.enabled) {
        await invoke("hooks_disable", { layer: hook.layer, event: hook.event, id: hook.id, projectRoot });
      } else {
        await invoke("hooks_enable", { id: hook.id, layer: hook.layer, projectRoot });
      }
      await load();
    });
  const edit = (hook: Hook) => {
    if (!hook.enabled) return;
    setEditing(hook);
    setLayer(hook.layer);
    const owner = sourceById.get(hook.source_path);
    if (owner) setSourceId(owner.id);
    setEvent(hook.event);
    setMatcher(hook.matcher ?? "");
    setCommand(hook.commands[0] ?? "");
    setResult(null);
    setError("");
  };
  const remove = (hook: Hook) => {
    if (!window.confirm(`Delete the ${hook.event} hook?`)) return;
    return runBusy(async () => {
      await invoke("hooks_delete", { layer: hook.layer, event: hook.event, id: hook.id, projectRoot });
      await load();
    });
  };
  const test = async () => {
    let input: unknown;
    try {
      input = JSON.parse(sample);
    } catch {
      setError("Sample input must be valid JSON.");
      return;
    }
    setResult(null);
    await runBusy(async () => {
      setResult(await invoke<TestResult>("hooks_test", { command, input }));
    });
  };

  return (
    <section className="hooks-page" aria-labelledby="hooks-title" tabIndex={0}>
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
      {policy.note && <p className="supporting-copy">{policy.note}</p>}
      <div className="hooks-layout">
        <section className="config-editor" aria-label="Hook builder">
          <h3>{editing ? "Edit hook" : "Add hook"}</h3>
          {client === "kiro" && <>
            <label className="field-label" htmlFor="hook-source">Hook source</label>
            <select id="hook-source" value={sourceId} disabled={busy || editing !== null} onChange={e => {
              setSourceId(e.target.value);
              const owner = sourceById.get(e.target.value);
              if (owner) setLayer(owner.layer);
            }}>
              {!sources.length && <option value="">No native hook sources available</option>}
              {sources.map(item => <option key={item.id} value={item.id}>{item.label}</option>)}
            </select>
          </>}
          <label className="field-label" htmlFor="hook-layer">
            Settings scope
          </label>
          <select
            id="hook-layer"
            value={effectiveLayer}
            disabled={busy || client === "kiro" || policy.projectPath === null}
            onChange={(e) =>
              setLayer(e.target.value === "project" ? "project" : "user")
            }
          >
            <option value="user">User</option>
            {policy.projectPath !== null && <option value="project">Project</option>}
          </select>
          <p className="layer-path">
            {displayPath(source?.label ?? (effectiveLayer === "user" ? policy.userPath : projectRoot ? `${projectRoot}/${policy.projectPath}` : `Choose a project to edit ${policy.projectPath}`))}
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
            {[...new Set([...events, ...(editing ? [editing.event] : [])])].map(
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
          <JsonEditor id="hook-sample" label="Sample JSON input" value={sample} onChange={setSample} disabled={busy} />
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
                !event ||
                (client === "kiro" && !source) ||
                (effectiveLayer === "project" && !projectRoot)
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
              <details>
                <summary>stdout</summary>
                <pre tabIndex={0}>{result.stdout || "(empty)"}</pre>
              </details>
              <details>
                <summary>stderr</summary>
                <pre tabIndex={0}>{result.stderr || "(empty)"}</pre>
              </details>
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
          {displayPath(error)}
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
            <>
            <SearchField
              id="hooks-search"
              label="Search hooks"
              value={hookSearch}
              onChange={setHookSearch}
              status={`Showing ${visibleHooks.length} of ${hooks.length} hooks`}
            />
            {visibleHooks.length === 0 ? (
              <p className="config-empty">No hooks match "{hookSearch.trim()}".</p>
            ) : (
            <div className="hook-list" role="region" tabIndex={0} aria-label="Configured hooks">
              {visibleHooks.map((hook) => (
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
                    <details className="collection-card-details">
                      <summary>Hook details</summary>
                      <p className="hook-meta">
                        {hook.layer} scope · matcher: {hook.matcher || "all"}
                      </p>
                    {hook.source_path && <p className="layer-path">{displayPath(hook.source_path)}{!hook.editable && " · Inherited, read-only"}</p>}
                      {hook.commands.map((value, i) => (
                        <code className="hook-command" key={`${hook.id}-${i}`} tabIndex={0}>
                          {value}
                        </code>
                      ))}
                    </details>
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
                    {(sourceById.get(hook.source_path)?.canToggle ?? policy.canToggle) && <button
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
            </>
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
