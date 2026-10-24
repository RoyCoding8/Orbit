import { useState } from "react";
import "./content.css";

const PAGE_SIZE = 50;

export function JsonValue({ value }: { value: unknown }) {
  return <div className="json-tree"><JsonNode value={value} /></div>;
}

function JsonNode({ value, name }: { value: unknown; name?: string }) {
  const [expanded, setExpanded] = useState(false);
  const [visible, setVisible] = useState(PAGE_SIZE);
  if (value !== null && typeof value === "object") {
    const entries = Object.entries(value);
    const array = Array.isArray(value);
    return <div className="json-node">
      <button className="json-disclosure" type="button" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
        <span className="json-chevron" aria-hidden="true">›</span>
        <span className="json-key">{name ?? (array ? "Array" : "Object")}</span>
        <span className="json-count">{entries.length} {array ? "items" : "fields"}</span>
      </button>
      {expanded && <ul className="json-children">
        {entries.slice(0, visible).map(([key, child]) => <li key={key}><JsonNode name={array ? `[${key}]` : key} value={child} /></li>)}
        {visible < entries.length && <li><button className="json-more" type="button" onClick={() => setVisible(visible + PAGE_SIZE)}>Show next {Math.min(PAGE_SIZE, entries.length - visible)} of {entries.length - visible} remaining</button></li>}
      </ul>}
    </div>;
  }
  return <div className="json-leaf">
    {name !== undefined && <span className="json-key">{name}<span aria-hidden="true">: </span></span>}
    <code className={`json-${value === null ? "null" : typeof value}`}>{JSON.stringify(value) ?? "undefined"}</code>
  </div>;
}
