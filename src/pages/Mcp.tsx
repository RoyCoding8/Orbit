import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { M3eButton } from "@m3e/react/button";
import "./Mcp.css";

const MASK = "••••••••";
type Transport = "stdio" | "http";
type Server = { name: string; kind: Transport; entry: Record<string, unknown>; enabled: boolean };
type Bench = { tools: string[]; latency_ms: number };
type EditorFields = { kind: Transport; command: string; args: string; env: string; url: string; headers: string; extra: Record<string, unknown> };
type Editor = ({ mode: "create"; name: string } | { mode: "edit"; name: string }) & EditorFields;
type LoadState = { kind: "loading" } | { kind: "ready" } | { kind: "error"; message: string };

const blankEditor = (kind: Transport = "stdio"): Editor => ({ mode: "create", name: "", kind, command: "", args: "", env: "", url: "", headers: "", extra: {} });

function parseMap(text: string): Record<string, string> {
  if (!text.trim()) return {};
  const parsed: unknown = JSON.parse(text);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed) || Object.values(parsed).some((value) => typeof value !== "string")) {
    throw new Error("Enter a JSON object with string values.");
  }
  return parsed as Record<string, string>;
}

function editorFor(server: Server): Editor {
  const entry = server.entry;
  return {
    mode: "edit",
    name: server.name,
    kind: server.kind,
    command: typeof entry.command === "string" ? entry.command : "",
    args: Array.isArray(entry.args) ? entry.args.filter((arg): arg is string => typeof arg === "string").join("\n") : "",
    env: JSON.stringify(entry.env ?? {}, null, 2),
    url: typeof entry.url === "string" ? entry.url : typeof entry.serverUrl === "string" ? entry.serverUrl : "",
    headers: JSON.stringify(entry.headers ?? {}, null, 2),
    extra: Object.fromEntries(Object.entries(entry).filter(([key]) => !["type", "command", "args", "env", "url", "serverUrl", "headers"].includes(key))),
  };
}

function payloadFor(editor: Editor): Record<string, unknown> {
  if (editor.kind === "stdio") {
    return { ...editor.extra, type: "stdio", command: editor.command, args: editor.args.split(/\r?\n/).filter(Boolean), env: parseMap(editor.env) };
  }
  return { ...editor.extra, type: "http", url: editor.url, headers: parseMap(editor.headers) };
}

export function McpPage() {
  const [servers, setServers] = useState<Server[]>([]);
  const [loadState, setLoadState] = useState<LoadState>({ kind: "loading" });
  const [editor, setEditor] = useState<Editor | null>(null);
  const [error, setError] = useState("");
  const [editorError, setEditorError] = useState("");
  const [bench, setBench] = useState<{ name: string; result?: Bench; error?: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const firstInput = useRef<HTMLInputElement>(null);
  const isEditing = editor !== null;
  const canMutate = loadState.kind === "ready" && !busy;

  const refresh = useCallback(async () => {
    setLoadState({ kind: "loading" });
    try {
      setServers(await invoke<Server[]>("mcp_list"));
      setError("");
      setLoadState({ kind: "ready" });
    } catch (caught) {
      setLoadState({ kind: "error", message: String(caught) });
    }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    const element = dialog.current;
    if (!isEditing || !element) return;
    element.showModal();
    firstInput.current?.focus();
    return () => { if (element.open) element.close(); };
  }, [isEditing]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!editor || !canMutate) return;
    setBusy(true);
    setEditorError("");
    try {
      await invoke("mcp_save", { name: editor.name, entry: payloadFor(editor) });
      setEditor(null);
      await refresh();
    } catch (caught) {
      setEditorError(String(caught));
    } finally { setBusy(false); }
  };

  const toggle = async (server: Server) => {
    if (!canMutate) return;
    setBusy(true);
    try {
      await invoke(server.enabled ? "mcp_disable" : "mcp_enable", { name: server.name });
      await refresh();
    } catch (caught) { setError(String(caught)); }
    finally { setBusy(false); }
  };

  const remove = async (server: Server) => {
    if (!canMutate || !window.confirm(`Delete MCP server "${server.name}"?`)) return;
    setBusy(true);
    try {
      await invoke("mcp_delete", { name: server.name });
      await refresh();
    } catch (caught) { setError(String(caught)); }
    finally { setBusy(false); }
  };

  const test = async (server: Server) => {
    if (!canMutate) return;
    setBusy(true);
    setBench({ name: server.name });
    try {
      const result = await invoke<Bench>("mcp_test", { name: server.name });
      setBench({ name: server.name, result });
    } catch (caught) {
      setBench({ name: server.name, error: String(caught) });
    } finally { setBusy(false); }
  };

  const openEditor = (next: Editor) => {
    setEditorError("");
    setEditor(next);
  };

  return <section className="content-pane mcp-page" aria-labelledby="mcp-title">
    <div className="pane-heading">
      <div><p className="eyebrow">HARNESS CONFIGURATION</p><h2 id="mcp-title">MCP servers</h2><p className="supporting-copy">Manage tool servers available to Claude Code. Known credential fields are masked in this view.</p></div>
      <div className="config-actions"><M3eButton variant="outlined" disabled={busy || loadState.kind === "loading"} onClick={() => void refresh()}>Refresh servers</M3eButton><M3eButton variant="filled-tonal" disabled={!canMutate} onClick={() => openEditor(blankEditor())}>Add server</M3eButton></div>
    </div>
    {error && <p className="mcp-error" role="alert">{error}</p>}
    {loadState.kind === "loading" ? <p role="status">Loading MCP servers…</p> : loadState.kind === "error" ? <p className="mcp-error" role="alert">Could not load MCP servers. {loadState.message}</p> : servers.length === 0 ? <div className="empty-panel"><p className="eyebrow">NO SERVERS YET</p><h3>Connect a tool server</h3><p className="empty-copy">Add a local stdio process or a remote HTTP endpoint.</p></div> : <div className="mcp-list">
      {servers.map((server) => <article className="mcp-card" key={server.name}>
        <div className="mcp-card-heading"><div><h3>{server.name}</h3><span className="mcp-badge">{server.kind}</span><span className={`mcp-state ${server.enabled ? "enabled" : "disabled"}`}>{server.enabled ? "Enabled" : "Disabled"}</span></div><div className="mcp-actions">
          <button type="button" disabled={!canMutate} onClick={() => void toggle(server)}>{server.enabled ? "Disable" : "Enable"}</button>
          {server.enabled && <button type="button" disabled={!canMutate} onClick={() => void test(server)}>Test</button>}
          {server.enabled && <button type="button" disabled={!canMutate} onClick={() => openEditor(editorFor(server))}>Edit</button>}
          <button type="button" disabled={!canMutate} onClick={() => void remove(server)}>Delete</button>
        </div></div>
        <p className="mcp-summary">{server.kind === "stdio" ? `${String(server.entry.command ?? "")} ${Array.isArray(server.entry.args) ? server.entry.args.join(" ") : ""}` : String(server.entry.url ?? server.entry.serverUrl ?? "")}</p>
        {(server.entry.env || server.entry.headers) ? <pre className="mcp-secrets">{JSON.stringify(server.entry.env ?? server.entry.headers, null, 2)}</pre> : null}
        {bench?.name === server.name && <div className="mcp-bench" aria-live="polite">{bench.result ? <><strong>{bench.result.tools.length} tools · {bench.result.latency_ms} ms</strong><ul>{bench.result.tools.map((tool) => <li key={tool}>{tool}</li>)}</ul></> : bench.error ? <p className="mcp-error">{bench.error}</p> : <p>Testing server…</p>}</div>}
      </article>)}
    </div>}
    <footer className="pane-footer"><span>LOCAL FIRST</span><span>Testing starts local server commands or contacts remote servers.</span></footer>
    <dialog ref={dialog} className="mcp-editor" aria-labelledby="mcp-editor-title" onCancel={event => { event.preventDefault(); if (!busy) setEditor(null); }}>
      {editor && <form onSubmit={(event) => void save(event)} aria-label="MCP server editor">
        <div className="mcp-editor-heading"><div><p className="eyebrow">SERVER DETAILS</p><h3 id="mcp-editor-title">{editor.mode === "edit" ? "Edit server" : "Add server"}</h3></div><button type="button" aria-label="Close editor" disabled={busy} onClick={() => setEditor(null)}>×</button></div>
        {editorError && <p className="mcp-error" role="alert">{editorError}</p>}
        <label>Server name<input ref={editor.mode === "create" ? firstInput : undefined} required disabled={editor.mode === "edit" || busy} value={editor.name} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></label>
        <label>Transport<select disabled={busy} value={editor.kind} onChange={(event) => setEditor({ ...editor, kind: event.target.value === "http" ? "http" : "stdio" })}><option value="stdio">Local stdio</option><option value="http">Remote HTTP</option></select></label>
        {editor.kind === "stdio" ? <>
          <label>Command<input ref={editor.mode === "edit" ? firstInput : undefined} required disabled={busy} value={editor.command} onChange={(event) => setEditor({ ...editor, command: event.target.value })} /></label>
          <label>Arguments, one per line<textarea disabled={busy} value={editor.args} onChange={(event) => setEditor({ ...editor, args: event.target.value })} /></label>
          <label>Environment variables, JSON<textarea disabled={busy} value={editor.env} onChange={(event) => setEditor({ ...editor, env: event.target.value })} placeholder='{"MODE":"safe"}' /></label>
        </> : <>
          <label>Server URL<input ref={editor.mode === "edit" ? firstInput : undefined} required type="url" disabled={busy} value={editor.url} onChange={(event) => setEditor({ ...editor, url: event.target.value })} /></label>
          <label>Headers, JSON<textarea disabled={busy} value={editor.headers} onChange={(event) => setEditor({ ...editor, headers: event.target.value })} placeholder='{"Authorization":"Bearer …"}' /></label>
        </>}
        <p className="mcp-hint">Secret-like values display as {MASK}. Unchanged masked values in object fields are restored when saved. Masked values in arrays cannot be restored safely, so replace them before saving.</p>
        <div className="mcp-editor-actions"><button type="button" disabled={busy} onClick={() => setEditor(null)}>Cancel</button><M3eButton variant="filled" type="submit" disabled={!canMutate}>{busy ? "Saving…" : "Save server"}</M3eButton></div>
      </form>}
    </dialog>
  </section>;
}
