import { lazy, Suspense, useState, type Ref } from "react";
import "./content.css";

const MarkdownPreview = lazy(() => import("./MarkdownPreview"));

type Props = {
  id: string;
  label: string;
  value: string;
  disabled?: boolean;
  inputRef?: Ref<HTMLTextAreaElement>;
} & ({ readOnly: true; onChange?: never } | { readOnly?: false; onChange: (value: string) => void });

export function MarkdownEditor(props: Props) {
  const [preview, setPreview] = useState(props.readOnly ?? false);
  return <div className="markdown-editor">
    <div className="content-toolbar">
      <label htmlFor={props.id}>{props.label}</label>
      <div className="view-switch" role="group" aria-label="Markdown display">
        <button type="button" aria-pressed={!preview} onClick={() => setPreview(false)}>{props.readOnly ? "Source" : "Edit"}</button>
        <button type="button" aria-pressed={preview} onClick={() => setPreview(true)}>Preview</button>
      </div>
    </div>
    {preview ? <Suspense fallback={<p role="status">Loading preview…</p>}>
      <MarkdownPreview value={props.value} />
    </Suspense> : <textarea id={props.id} ref={props.inputRef} className="markdown-source" spellCheck={false}
      value={props.value} readOnly={props.readOnly} disabled={props.disabled}
      onChange={event => props.onChange?.(event.target.value)} />}
  </div>;
}
