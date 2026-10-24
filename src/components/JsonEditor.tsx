import { useState } from "react";
import { JsonValue } from "./JsonValue";

export function JsonEditor({ id, label, value, onChange, disabled, placeholder }: {
  id: string; label: string; value: string; onChange: (value: string) => void; disabled?: boolean; placeholder?: string;
}) {
  const [structured, setStructured] = useState(false);
  let parsed: unknown;
  let invalid = false;
  if (structured) {
    try { parsed = JSON.parse(value); } catch { invalid = true; }
  }
  return <div className="json-editor">
    <div className="content-toolbar">
      <label htmlFor={id}>{label}</label>
      <div className="view-switch" role="group" aria-label="JSON display">
        <button type="button" aria-pressed={!structured} onClick={() => setStructured(false)}>JSON</button>
        <button type="button" aria-pressed={structured} onClick={() => setStructured(true)}>Explore</button>
      </div>
    </div>
    {structured ? invalid ? <p className="quiet-copy" role="status">Enter valid JSON to explore its fields.</p> : <JsonValue value={parsed} />
      : <textarea id={id} rows={6} spellCheck={false} value={value} onChange={event => onChange(event.target.value)} disabled={disabled} placeholder={placeholder} />}
  </div>;
}
