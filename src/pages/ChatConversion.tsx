import { useEffect, useRef, useState } from "react";
import { useHarness } from "../HarnessContext";
import { clients, harnesses, isClient, type Client } from "../harnesses";

type Preview = {
  workingDirectory: string;
  title: string;
  messages: number;
  notes: string[];
};
type ConversionReport = Pick<Preview, "workingDirectory" | "messages" | "notes"> & {
  sessionId: string;
};
type PreviewState =
  | { kind: "loading" }
  | { kind: "ready"; preview: Preview }
  | { kind: "error"; message: string };

export function ChatConversion({ sessionId, onClose, onConverted }: {
  sessionId: string;
  onClose: () => void;
  onConverted: (notice: string) => void;
}) {
  const { client, invoke } = useHarness();
  const destinations = clients.filter(candidate => candidate !== client && harnesses[candidate].transcripts.archive === "migration");
  const [target, setTarget] = useState<Client>(destinations[0] ?? "codex");
  const [previewState, setPreviewState] = useState<PreviewState>({ kind: "loading" });
  const [projectRoot, setProjectRoot] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<ConversionReport | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const current = dialog.current;
    current?.showModal();
    return () => { if (current?.open) current.close(); };
  }, []);

  useEffect(() => {
    let active = true;
    void invoke<Preview>("transcript_conversion_preview", { sessionId }).then(preview => {
      if (active) setPreviewState({ kind: "ready", preview });
    }).catch(failure => {
      if (active) setPreviewState({ kind: "error", message: String(failure) });
    });
    return () => { active = false; };
  }, [invoke, sessionId]);

  const chooseDirectory = async () => {
    setBusy(true);
    setError("");
    try {
      const selected = await invoke<string | null>("project_choose");
      if (selected) setProjectRoot(selected);
    } catch (failure) {
      setError(`Could not choose a working directory. ${String(failure)}`);
    } finally { setBusy(false); }
  };

  const convert = async () => {
    setBusy(true);
    setError("");
    try {
      const report = await invoke<ConversionReport>("transcript_convert", {
        sessionId, targetClient: target, projectRoot,
      });
      setResult(report);
      onConverted(`Converted chat to ${harnesses[target].label} as ${report.sessionId}. Select that agent to browse it. The source chat is unchanged.`);
    } catch (failure) {
      setError(`Chat conversion failed. ${String(failure)}`);
    } finally { setBusy(false); }
  };

  return <dialog ref={dialog} className="edit-dialog conversion-dialog" aria-labelledby="conversion-title"
    onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <div className="pane-heading">
      <h3 id="conversion-title">Convert chat</h3>
      <button type="button" className="text-button" disabled={busy} onClick={onClose}>Close</button>
    </div>
    <div className="conversion-body">
      {previewState.kind === "loading" && <p role="status">Reading the native conversation…</p>}
      {previewState.kind === "error" && <p role="alert" className="inline-error">{previewState.message}</p>}
      {previewState.kind === "ready" && <>
        <p><strong>{previewState.preview.title}</strong> · {previewState.preview.messages} messages</p>
        {result ? <div role="status">
          <p>Created {harnesses[target].label} chat <strong>{result.sessionId}</strong>.</p>
          <p>Working directory: {result.workingDirectory}</p>
          <p>Select {harnesses[target].label} to browse the converted chat.</p>
        </div> : <>
          <label htmlFor="conversion-target">Destination agent</label>
          <select id="conversion-target" value={target} disabled={busy} onChange={event => {
            if (isClient(event.target.value)) setTarget(event.target.value);
          }}>
            {destinations.map(candidate => <option key={candidate} value={candidate}>{harnesses[candidate].label}</option>)}
          </select>
          <p className="quiet-copy">Kiro cannot be a destination because its current interface does not support creating a chat from a transcript.</p>
          <p>Working directory: <span className="conversion-path">{projectRoot ?? previewState.preview.workingDirectory}</span></p>
          <div className="dialog-actions">
            <button type="button" className="secondary-action" disabled={busy} onClick={() => void chooseDirectory()}>Change working directory</button>
            {projectRoot && <button type="button" className="text-button" disabled={busy} onClick={() => setProjectRoot(null)}>Use original directory</button>}
          </div>
          <p className="supporting-copy">Creates a separate chat with the latest active branch, conversation text, and inline images. Tool history becomes text. The destination agent uses its own settings and instructions.</p>
        </>}
        <details>
          <summary>Conversion details</summary>
          <ul>{(result?.notes ?? previewState.preview.notes).map(note => <li key={note}>{note}</li>)}</ul>
        </details>
      </>}
      {error && <p role="alert" className="inline-error">{error}</p>}
    </div>
    <div className="dialog-actions">
      <button type="button" className="secondary-action" disabled={busy} onClick={onClose}>{result ? "Done" : "Cancel"}</button>
      {!result && <button type="button" className="primary-action" disabled={busy || previewState.kind !== "ready"} onClick={() => void convert()}>{busy ? "Working…" : "Convert chat"}</button>}
    </div>
  </dialog>;
}
