import { useEffect, useRef, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { M3eButton } from "@m3e/react/button";
import { M3eCheckbox } from "@m3e/react/checkbox";
import { M3eFormField } from "@m3e/react/form-field";
import { M3eOption } from "@m3e/react/option";
import { M3eSelect, type M3eSelectElement } from "@m3e/react/select";
import { useHarness } from "../HarnessContext";
import { displayPath } from "../pathDisplay";
import "./HarnessTransfer.css";

const categories = [
  { id: "skills", label: "Skills", detail: "Skills and their supporting files" },
  { id: "hooks", label: "Hooks", detail: "Automation rules, plugins, and extensions" },
  { id: "mcps", label: "MCP servers", detail: "Tool connections, including saved credentials" },
  { id: "memories", label: "Memories", detail: "Saved memories and project instructions" },
  { id: "settings", label: "Settings", detail: "Agent preferences and configuration" },
] as const;
type Category = typeof categories[number]["id"];
type Destination = "original" | "project" | "user";
type Item = {
  id: string; category: Category; label: string; scope: "user" | "project" | "local";
  target: string; targetKey: string; status: "add" | "identical" | "conflict" | "merge"; expected: string;
};

const scopeLabels = { user: "User-wide", project: "Project", local: "Local project" } satisfies Record<Item["scope"], string>;
const statusLabels = { add: "New", identical: "Already up to date", conflict: "Different version exists", merge: "Adds missing entries" } satisfies Record<Item["status"], string>;
type Preview = { checksum: string; projectRoot: string; entries: Item[]; warnings: string[] };
type Report = { files: number; skipped: number; path: string };
type Mode = { kind: "export" } | { kind: "import"; path: string };
type Loaded = { kind: "loading" } | { kind: "error"; message: string } | { kind: "ready"; preview: Preview };

function quantity(count: number, noun: string) { return `${count} ${noun}${count === 1 ? "" : "s"}`; }

function Check({ label, checked, mixed = false, disabled = false, onChange }: {
  label: string; checked: boolean; mixed?: boolean; disabled?: boolean; onChange: () => void;
}) {
  return <M3eCheckbox aria-label={label} role="checkbox" aria-checked={mixed ? "mixed" : checked}
    checked={checked} indeterminate={mixed} disabled={disabled} onChange={onChange} />;
}

function ChoiceSelect<T extends string>({ label, name = label, value, options, disabled, onChange }: {
  label: string; name?: string; value: T; options: readonly { value: T; label: string }[]; disabled: boolean; onChange: (value: T) => void;
}) {
  const select = useRef<M3eSelectElement>(null);
  return <M3eFormField variant="outlined">
    <span slot="label">{label}</span>
    <M3eSelect ref={select} aria-label={name} value={value} disabled={disabled} onChange={() => {
      const option = options.find(option => option.value === select.current?.value);
      if (option) onChange(option.value);
    }}>{options.map(option => <M3eOption key={option.value} value={option.value}>{option.label}</M3eOption>)}</M3eSelect>
  </M3eFormField>;
}

export function HarnessTransfer() {
  const [mode, setMode] = useState<Mode | null>(null);
  const [error, setError] = useState("");
  const chooseArchive = async () => {
    setError("");
    try {
      const path = await open({ directory: false, multiple: false, filters: [{ name: "Orbit setup backup", extensions: ["zip"] }] });
      if (typeof path === "string") setMode({ kind: "import", path });
    } catch (failure) { setError(`Could not open backup. ${String(failure)}`); }
  };
  return <div className="harness-transfer-actions">
    <M3eButton type="button" variant="outlined" onClick={() => setMode({ kind: "export" })}>Export setup</M3eButton>
    <M3eButton type="button" variant="text" onClick={() => void chooseArchive()}>Import setup</M3eButton>
    {error && <span role="alert">{displayPath(error)}</span>}
    {mode && <TransferDialog mode={mode} onClose={() => setMode(null)} />}
  </div>;
}

function TransferDialog({ mode, onClose }: { mode: Mode; onClose: () => void }) {
  const { invoke, label, client } = useHarness();
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });
  const [projectRoot, setProjectRoot] = useState<string | null>(null);
  const [destination, setDestination] = useState<Destination | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [replacements, setReplacements] = useState<Set<string>>(new Set());
  const [exportCategories, setExportCategories] = useState<Set<Category>>(new Set(categories.map(category => category.id)));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<Report | null>(null);
  const [revision, setRevision] = useState(0);
  const dialog = useRef<HTMLDialogElement>(null);
  const isExport = mode.kind === "export";

  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => { if (element?.open) element.close(); };
  }, []);
  useEffect(() => {
    if (mode.kind === "import" && destination === null) return;
    let active = true;
    setLoaded({ kind: "loading" });
    setError("");
    setSelected(new Set());
    setReplacements(new Set());
    const request = mode.kind === "export"
      ? invoke<Preview>("harness_archive_inventory", { projectRoot })
      : invoke<Preview>("harness_archive_preview", { path: mode.path, projectRoot, destination });
    void request.then(preview => {
      if (active) setLoaded({ kind: "ready", preview });
    }).catch(failure => {
      if (active) setLoaded({ kind: "error", message: String(failure) });
    });
    return () => { active = false; };
  }, [invoke, mode, projectRoot, destination, revision]);

  const changeProject = async () => {
    setBusy(true);
    try {
      const path = await invoke<string | null>("project_choose");
      if (path) setProjectRoot(path);
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  };
  const transfer = async () => {
    if (loaded.kind !== "ready") return;
    setBusy(true);
    setError("");
    try {
      if (mode.kind === "export") {
        const path = await save({ defaultPath: `orbit-${client}.zip`, filters: [{ name: "Orbit setup backup", extensions: ["zip"] }] });
        if (!path) return;
        const report = await invoke<Report>("harness_archive_export", { projectRoot, categories: [...exportCategories], path });
        setResult(report);
      } else {
        const report = await invoke<Report>("harness_archive_import", {
          path: mode.path, projectRoot, destination, checksum: loaded.preview.checksum,
          choices: loaded.preview.entries.filter(entry => selected.has(entry.id) && entry.status !== "identical")
            .map(entry => ({ id: entry.id, replace: replacements.has(entry.id), expected: entry.expected })),
        });
        setResult(report);
      }
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  };

  const preview = loaded.kind === "ready" ? loaded.preview : null;
  const selectable = preview?.entries.filter(entry => entry.status !== "identical") ?? [];
  const availableCategories = categories.filter(category => preview?.entries.some(entry => entry.category === category.id));
  const all = isExport ? availableCategories.length > 0 && availableCategories.every(category => exportCategories.has(category.id))
    : selectable.length > 0 && selectable.every(entry => selected.has(entry.id));
  const any = isExport ? availableCategories.some(category => exportCategories.has(category.id)) : selected.size > 0;
  const destinations = new Set<string>();
  const collisions = selectable.filter(entry => selected.has(entry.id) && (entry.status !== "conflict" || replacements.has(entry.id)))
    .some(entry => {
      const path = displayPath(entry.target);
      const identity = /^[A-Za-z]:[\\/]|^\\\\/.test(path) ? path.replaceAll("\\", "/").toLowerCase() : path;
      const key = `${identity}\0${entry.targetKey}`;
      if (destinations.has(key)) return true;
      destinations.add(key);
      return false;
    });
  const toggleItems = (items: Item[], checked: boolean) => setSelected(previous => {
    const next = new Set(previous);
    for (const item of items) { if (checked) next.add(item.id); else next.delete(item.id); }
    return next;
  });

  return <dialog ref={dialog} className="edit-dialog harness-transfer-dialog" aria-labelledby="harness-transfer-title"
    onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <div className="pane-heading">
      <h3 id="harness-transfer-title">{isExport ? "Export" : "Import"} {label} setup</h3>
      <M3eButton type="button" variant="text" disabled={busy} onClick={onClose}>{result ? "Done" : "Close"}</M3eButton>
    </div>
    <div className="harness-transfer-body">
      {result ? <div role="status">
        <p>{isExport ? `Saved a backup with ${quantity(result.files, "item")}.` : result.files ? `Imported ${quantity(result.files, "file")}.` : "No files were changed."}</p>
        {!isExport && result.skipped > 0 && <p>Kept {quantity(result.skipped, "existing item")} unchanged.</p>}
        <p className="transfer-path">{displayPath(result.path)}</p>
        {!isExport && result.files > 0 && <p>You can restore replaced files from History.</p>}
        {!isExport && result.files > 0 && <p>Refresh the open page when you are ready to load these changes. Your unsaved edits are still open.</p>}
      </div> : <>
        <div className="transfer-destination">
          <span>Project: <span className="transfer-path">{displayPath(preview?.projectRoot ?? projectRoot ?? "Folder Orbit opened in")}</span></span>
          <M3eButton type="button" variant="text" disabled={busy} onClick={() => void changeProject()}>Change project</M3eButton>
        </div>
        {!isExport && destination === null && <div className="transfer-destination-choice">
          <h4>Where do you want to import this setup?</h4>
          <p>Use it in the current project, or make it available to {label} across your projects.</p>
          <div className="dialog-actions">
            <M3eButton type="button" variant="filled" disabled={busy} onClick={() => setDestination("project")}>Current project</M3eButton>
            <M3eButton type="button" variant="outlined" disabled={busy} onClick={() => setDestination("user")}>User-wide</M3eButton>
          </div>
          {client === "claude-code" && <p className="quiet-copy">Claude auto-memory stays attached to the chosen project.</p>}
          {client === "codex" && <p className="quiet-copy">Codex generated memory remains user-wide.</p>}
        </div>}
        {!isExport && destination !== null && <ChoiceSelect<Destination> label="Import destination" disabled={busy} value={destination} onChange={setDestination}
          options={[{ value: "original", label: "Keep user-wide and project items separate" }, { value: "project", label: "Current project" }, { value: "user", label: "User-wide" }]} />}
        {loaded.kind === "loading" && (isExport || destination !== null) && <p role="status">{isExport ? "Finding your setup…" : "Checking the backup…"}</p>}
        {loaded.kind === "error" && <p role="alert" className="inline-error">{displayPath(loaded.message)}</p>}
        {preview && <>
          <p>{isExport ? "Choose what to save in your backup." : `Found ${quantity(preview.entries.length, "item")}. Choose what to import.`}</p>
          {preview.entries.length === 0 && <p className="quiet-copy">{isExport ? "Nothing to export was found for this agent and project. Try another project." : "This backup has no items to import."}</p>}
          {!isExport && preview.entries.length > 0 && selectable.length === 0 && <p className="quiet-copy">Everything in this backup is already up to date.</p>}
          {!isExport && <p className="quiet-copy">Existing items stay unchanged unless you choose Replace. Hooks and extension lists add missing entries.</p>}
          <p className="quiet-copy">{isExport ? "Importing Settings replaces preferences. Hooks and MCP servers are handled separately." : "Replacing Settings keeps your current hooks and MCP servers unless you select those too."}</p>
          {preview.entries.some(entry => entry.category === "mcps" || entry.category === "settings") && <p className="transfer-privacy">Backups can contain passwords and API keys. Share them only with people you trust.</p>}
          <p className="transfer-selection-summary" aria-live="polite">{isExport ? `Selected categories: ${availableCategories.filter(category => exportCategories.has(category.id)).length}` : `${quantity(selected.size, "item")} selected`}</p>
          <div className="transfer-select-all">
            <Check label="Select all items" checked={all} mixed={any && !all} disabled={busy || availableCategories.length === 0 || (!isExport && selectable.length === 0)} onChange={() => {
              if (isExport) setExportCategories(new Set(all ? [] : availableCategories.map(category => category.id)));
              else toggleItems(selectable, !all);
            }} />
            <span>Select all</span>
            {!isExport && <M3eButton type="button" variant="text" disabled={busy} onClick={() => setRevision(current => current + 1)}>Refresh preview</M3eButton>}
          </div>
          <div className="transfer-categories">
            {categories.map(category => {
              const entries = preview.entries.filter(entry => entry.category === category.id);
              const eligible = entries.filter(entry => entry.status !== "identical");
              const checked = isExport ? exportCategories.has(category.id) && entries.length > 0 : eligible.length > 0 && eligible.every(entry => selected.has(entry.id));
              const partial = !isExport && !checked && eligible.some(entry => selected.has(entry.id));
              return <div className="transfer-category" key={category.id}>
                <div className="transfer-category-heading">
                  <Check label={`Select ${category.label}`} checked={checked} mixed={partial} disabled={busy || entries.length === 0 || (!isExport && eligible.length === 0)} onChange={() => {
                    if (isExport) setExportCategories(previous => { const next = new Set(previous); if (checked) next.delete(category.id); else next.add(category.id); return next; });
                    else toggleItems(eligible, !checked);
                  }} />
                  <span><strong>{category.label}</strong><small>{category.detail}</small></span>
                  <span className="transfer-count">{entries.length}</span>
                </div>
                {entries.length > 0 && <details>
                  <summary>View {quantity(entries.length, "item")}</summary>
                  <div className="transfer-items">
                    {entries.map(entry => <div key={entry.id} className="transfer-item">
                      {!isExport && <Check label={`Import ${entry.label} (${scopeLabels[entry.scope]})`} checked={selected.has(entry.id)} disabled={busy || entry.status === "identical"} onChange={() => toggleItems([entry], !selected.has(entry.id))} />}
                      <div><span className="transfer-resource-name">{entry.label}</span><div className="transfer-item-meta"><span className="source-badge">{scopeLabels[entry.scope]}</span>{!isExport && <small>{statusLabels[entry.status]}</small>}</div>
                        {entry.target && <small className="transfer-path">{isExport ? "From" : "Save to"}: {displayPath(entry.target)}</small>}
                      </div>
                      {!isExport && entry.status === "conflict" && <div className="transfer-conflict">
                        <ChoiceSelect<"skip" | "replace"> label="Existing version" name={`Action for ${entry.label} (${scopeLabels[entry.scope]})`} disabled={busy} value={replacements.has(entry.id) ? "replace" : "skip"}
                          options={[{ value: "skip", label: "Skip" }, { value: "replace", label: "Replace" }]} onChange={value => {
                            setReplacements(previous => { const next = new Set(previous); if (value === "replace") next.add(entry.id); else next.delete(entry.id); return next; });
                          }} />
                      </div>}
                    </div>)}
                  </div>
                </details>}
              </div>;
            })}
          </div>
          {preview.warnings.length > 0 && <details className="transfer-notes"><summary>What is included?</summary><ul>{[...new Set(preview.warnings)].map(note => <li key={note}>{note}</li>)}</ul></details>}
        </>}
      </>}
      {error && <p role="alert" className="inline-error">{displayPath(error)}</p>}
      {collisions && !result && <p role="alert" className="inline-error">Some selected items would replace the same file or setting. Select one version of each, or keep user-wide and project items separate.</p>}
    </div>
    {!result && <div className="dialog-actions">
      <M3eButton type="button" variant="outlined" disabled={busy} onClick={onClose}>Cancel</M3eButton>
      <M3eButton type="button" variant="filled" disabled={busy || !preview || !any || collisions} onClick={() => void transfer()}>{busy ? isExport ? "Saving backup…" : "Importing…" : isExport ? "Save backup" : "Import selected"}</M3eButton>
    </div>}
  </dialog>;
}
