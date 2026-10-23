import { useEffect, useMemo, useState } from "react";
import { useHarness } from "../HarnessContext";
import { parseMemorySections, type MemorySection } from "./memorySections";
import "./Memories.css";

export type MemorySource = "projectAutoMemory" | "globalAutoMemory" | "projectInstructions" | "userInstructions";
export type MemoryRole = "index" | "topic" | "instruction" | "rule" | "summary" | "raw" | "evidence";
export type MemoryReference = {
  target: string;
  kind: "markdownLink" | "import";
  status: "known" | "external" | "unresolved";
  fileId: string | null;
};
export type MemoryFile = {
  id: string;
  groupId: string;
  name: string;
  path: string;
  source: MemorySource;
  role: MemoryRole;
  loadBehavior: string;
  editable: boolean;
  content: string;
  references: MemoryReference[];
};
export type MemoryGroup = { id: string; label: string; role: MemorySource };
export type MemoryCatalog = {
  projectRoot: string;
  groups: MemoryGroup[];
  files: MemoryFile[];
  excludedCustomMemoryDirectory: string | null;
};

type LoadState = { kind: "loading" } | { kind: "ready" } | { kind: "error"; message: string };
type EditorView = { kind: "file" } | { kind: "section"; start: number; trailingLength: number };

function sectionView(section: MemorySection, content: string): EditorView {
  return { kind: "section", start: section.start, trailingLength: content.length - section.end };
}

function initialView(file: MemoryFile | null): EditorView {
  const sections = file?.source === "globalAutoMemory" ? parseMemorySections(file.content) : [];
  return file && sections.length > 1 ? sectionView(sections[0], file.content) : { kind: "file" };
}

export function Memories() {
  const { invoke, label, profile } = useHarness();
  const [catalog, setCatalog] = useState<MemoryCatalog | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [view, setView] = useState<EditorView>({ kind: "file" });
  const [loadState, setLoadState] = useState<LoadState>({ kind: "loading" });
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const selected = catalog?.files.find((file) => file.id === selectedId) ?? null;
  const dirty = !!selected && draft !== selected.content;
  const sections = useMemo(() => selected?.source === "globalAutoMemory" ? parseMemorySections(draft) : [], [selected?.source, draft]);
  const sectionGroups = useMemo(() => {
    const groups = new Map<string, MemorySection[]>();
    for (const section of sections) {
      const group = section.projects.length === 1 ? section.projects[0] : section.projects.length > 1 ? "Multiple projects" : "Project not stated";
      const entries = groups.get(group) ?? [];
      entries.push(section);
      groups.set(group, entries);
    }
    return [...groups];
  }, [sections]);
  const editorStart = view.kind === "section" ? view.start : 0;
  const editorEnd = view.kind === "section" ? draft.length - view.trailingLength : draft.length;
  const showSections = sections.length > 1 || view.kind === "section";
  const editContent = (value: string) => {
    const newline = draft.slice(editorStart, editorEnd).includes("\r\n") ? "\r\n" : "\n";
    setDraft(draft.slice(0, editorStart) + value.replace(/\r?\n/g, newline) + draft.slice(editorEnd));
  };

  const confirmDiscard = () => !dirty || window.confirm("Discard your unsaved memory changes?");

  const load = async (projectRoot: string | null = catalog?.projectRoot ?? null, confirmed = false) => {
    if (!confirmed && !confirmDiscard()) return;
    setLoadState({ kind: "loading" });
    try {
      const next = await invoke<MemoryCatalog>("memory_catalog", { projectRoot });
      setCatalog(next);
      const active = next.files.find((file) => file.id === selectedId) ?? next.files[0] ?? null;
      setSelectedId(active?.id ?? null);
      setDraft(active?.content ?? "");
      setView(initialView(active));
      setError("");
      setLoadState({ kind: "ready" });
      return active;
    } catch (reason) {
      setLoadState({ kind: "error", message: String(reason) });
    }
  };

  useEffect(() => { void load(null); }, []);

  const incoming = useMemo(() => catalog?.files.flatMap((file) =>
    file.references.filter((reference) => reference.status === "known" && reference.fileId === selected?.id)
      .map((reference) => ({ file, reference })),
  ) ?? [], [catalog, selected?.id]);
  const unlinkedTopics = useMemo(() => catalog?.files.filter((file) =>
    file.groupId === selected?.groupId && file.role === "topic" && !selected.references.some((reference) => reference.status === "known" && reference.fileId === file.id),
  ) ?? [], [catalog, selected]);

  const chooseProject = async () => {
    if (!confirmDiscard()) return;
    setBusy(true);
    try {
      const projectRoot = await invoke<string | null>("project_choose");
      if (projectRoot) await load(projectRoot, true);
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };

  const save = async () => {
    if (!selected || !catalog || draft === selected.content) return;
    setBusy(true);
    setError("");
    try {
      await invoke("memory_edit", { projectRoot: catalog.projectRoot, id: selected.id, expectedContent: selected.content, content: draft });
      const saved = await load(catalog.projectRoot, true);
      if (saved?.id === selected.id && saved.content === draft) setView(view);
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };

  const selectFile = (file: MemoryFile) => {
    if (!busy && file.id !== selectedId && confirmDiscard()) {
      setSelectedId(file.id);
      setDraft(file.content);
      setView(initialView(file));
    }
  };
  const removeFile = async () => {
    if (!selected || !catalog || !window.confirm(
      `Delete ${selected.name} and remove references to it from editable memory and instruction files? The file and changed references will be backed up in History.${dirty ? " Unsaved edits will be discarded." : ""}`,
    )) return;
    setBusy(true);
    setError("");
    try {
      await invoke("memory_delete", { projectRoot: catalog.projectRoot, id: selected.id, expectedContent: selected.content });
      await load(catalog.projectRoot, true);
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };
  const openReference = (reference: MemoryReference) => {
    if (reference.status !== "known" || !reference.fileId) return;
    const file = catalog?.files.find((entry) => entry.id === reference.fileId);
    if (file) selectFile(file);
  };

  return (
    <section className="content-pane memories-pane" aria-labelledby="memories-title">
      <header className="pane-heading memories-heading">
        <div>
          <p className="eyebrow">{label.toUpperCase()}</p>
          <h2 id="memories-title">Memories & instructions</h2>
          <p className="supporting-copy">{profile.memoriesNote}</p>
        </div>
        <div className="memory-header-actions">
          <button className="action-button" onClick={() => void load()} disabled={busy || loadState.kind === "loading"}>Refresh</button>
          <button className="action-button" onClick={() => void chooseProject()} disabled={busy || loadState.kind === "loading"}>Choose project</button>
        </div>
      </header>

      {error && <p className="inline-error" role="alert">{error}</p>}
      {loadState.kind === "loading" ? <p role="status">Loading project memories…</p> : null}
      {loadState.kind === "error" ? <div className="empty-panel"><h3>Could not load memories</h3><p className="inline-error">{loadState.message}</p><button className="action-button" onClick={() => void load()}>Retry</button></div> : null}
      {loadState.kind === "ready" && catalog ? (
        <>
          <div className="memory-project-banner"><span>Authorized project</span><code>{catalog.projectRoot}</code></div>
          {catalog.excludedCustomMemoryDirectory && <p className="memory-boundary-note" role="note">Custom auto-memory directory is outside this authorized project and was excluded: <code>{catalog.excludedCustomMemoryDirectory}</code></p>}
          <div className="memory-browser">
            <nav className="memory-groups" aria-label="Memory files by source">
              {catalog.groups.map((group) => {
                const files = catalog.files.filter((file) => file.groupId === group.id);
                return <section className="memory-group" key={group.id}>
                  <div className="memory-group-heading"><h3>{group.label}</h3><span className="count">{files.length}</span></div>
                  <p className="memory-source-hint">{sourceHint(group.role)}</p>
                  {files.length ? <ul>{files.map((file) => <li key={file.id}>
                    <button className={file.id === selectedId ? "memory-file active" : "memory-file"} aria-current={file.id === selectedId ? "page" : undefined} onClick={() => selectFile(file)} disabled={busy}>
                      <span>{file.name}</span><small>{roleLabel(file.role)}</small>
                    </button>
                  </li>)}</ul> : <p className="memory-empty-group">No files found.</p>}
                </section>;
              })}
            </nav>

            <main className="memory-editor-area">
              {selected ? <>
                <div className="memory-file-heading">
                  <div><p className="eyebrow">{roleLabel(selected.role)}</p><h3>{selected.name}</h3><code>{selected.path}</code></div>
                  <div className="memory-header-actions">
                    <button className="action-button danger" onClick={() => void removeFile()} disabled={busy || !selected.editable}>Delete file</button>
                    <button className="action-button primary" onClick={() => void save()} disabled={busy || !selected.editable || draft === selected.content}>Save changes</button>
                  </div>
                </div>
                <details className="memory-file-details">
                  <summary>File details</summary>
                  <p className="memory-load-behavior"><strong>{selected.source === "globalAutoMemory" ? "About this file:" : `${label} loading:`}</strong> {selected.loadBehavior}</p>
                  {selected.source === "globalAutoMemory" && <p className="memory-boundary-note" role="note">Codex may regenerate this file and replace your edits. Use /memories in Codex to control memory use and generation.</p>}
                </details>
                <div className={showSections ? "memory-content-layout sectioned" : "memory-content-layout"}>
                  {showSections && <nav className="memory-section-nav" aria-label="Memory sections">
                    <div className="memory-section-heading"><h4>Sections</h4><button className="text-button" aria-pressed={view.kind === "file"} onClick={() => setView({ kind: "file" })} disabled={busy}>Whole file</button></div>
                    <p className="memory-source-hint">Grouped by project details stated in the text. Codex memories remain shared.</p>
                    <div className="memory-section-list" tabIndex={0}>
                      {sectionGroups.map(([group, entries]) => <section className="memory-section-group" key={group}>
                        <h5>{group}</h5>
                        {entries.map((entry) => <button key={entry.start} className="memory-section-button" aria-current={view.kind === "section" && view.start === entry.start ? "true" : undefined} onClick={() => setView(sectionView(entry, draft))} disabled={busy}>
                          <span>{entry.title}</span>
                          {entry.projects.length > 1 && <small>{entry.projects.join(" · ")}</small>}
                        </button>)}
                      </section>)}
                    </div>
                  </nav>}
                  <div className="memory-draft-editor">
                    <label className="memory-content-label" htmlFor="memory-content">Markdown content{view.kind === "section" && <span className="memory-source-hint"> · Selected section</span>}</label>
                    <textarea id="memory-content" className="memory-content" spellCheck={false} value={draft.slice(editorStart, editorEnd)} onChange={(event) => editContent(event.target.value)} disabled={busy || !selected.editable} />
                    {dirty && <p className="memory-source-hint" role="status">Unsaved changes. Save changes applies your edits to {selected.name}.</p>}
                  </div>
                </div>
                <details className="memory-connection-details">
                  <summary>Connections</summary>
                  <section className="memory-connections" aria-label="Memory connections">
                  <p>{profile.connectionsNote}</p>
                  <div className="memory-connection-column">
                    <h4>Outgoing links <span className="count">{selected.references.length}</span></h4>
                    {selected.references.length ? <ul>{selected.references.map((reference, index) => <li key={reference.kind + reference.target + index}>
                      {reference.status === "known" && reference.fileId ? <button className="memory-link known" onClick={() => openReference(reference)}>{reference.target}</button> : <span className="memory-link unavailable">{reference.target}</span>}
                      <small>{reference.kind === "import" ? "Context import" : "Markdown link"} · {reference.status === "known" ? "in catalog" : reference.status === "external" ? "outside authorized roots" : "not in catalog"}</small>
                    </li>)}</ul> : <p>No recognized links or imports.</p>}
                  </div>
                  <div className="memory-connection-column">
                    <h4>Linked from <span className="count">{incoming.length}</span></h4>
                    {incoming.length ? <ul>{incoming.map(({ file, reference }) => <li key={file.id + reference.kind + reference.target}><button className="memory-link known" onClick={() => selectFile(file)}>{file.name}</button><small>{reference.kind === "import" ? "Context import" : "Markdown link"}</small></li>)}</ul> : <p>No known backlinks.</p>}
                  </div>
                  </section>
                {selected.role === "index" && <section className="memory-unlinked" aria-label="Unlinked topic files"><h4>Topic files not linked from this index <span className="count">{unlinkedTopics.length}</span></h4>{unlinkedTopics.length ? <ul>{unlinkedTopics.map((file) => <li key={file.id}><button className="memory-link known" onClick={() => selectFile(file)}>{file.name}</button><small>Same memory group</small></li>)}</ul> : <p>Every topic file in this group is linked from the index.</p>}</section>}
                </details>
              </> : <div className="empty-panel"><h3>No memory files found</h3><p className="empty-copy">Choose a project or add a {label} instruction file to see it here.</p></div>}
            </main>
          </div>
        </>
      ) : null}
    </section>
  );
}

function roleLabel(role: MemoryRole): string {
  switch (role) {
    case "index": return "Auto-memory index";
    case "topic": return "Auto-memory topic";
    case "instruction": return "Instruction";
    case "rule": return "Rule";
    case "summary": return "Memory summary";
    case "raw": return "Raw memories";
    case "evidence": return "Chat evidence";
  }
}

function sourceHint(source: MemorySource): string {
  switch (source) {
    case "projectAutoMemory": return "Project-scoped learned context; startup loading is limited to the index.";
    case "globalAutoMemory": return "Shared Codex memories: index, summary, raw inputs, and supporting notes. Markdown files can be edited here.";
    case "projectInstructions": return "Project instructions. Loading order and scope are shown for each file.";
    case "userInstructions": return "User-level instructions shared across projects.";
  }
}
