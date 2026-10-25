import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useHarness } from "../HarnessContext";
import { displayPath } from "../pathDisplay";
import { M3eButton } from "@m3e/react/button";
import "@m3e/web/button";
import { JsonValue } from "../components/JsonValue";
import { JsonEditor } from "../components/JsonEditor";
import "./ConfigSearch.css";

type LayerName = "user" | "project" | "local";
type Layer = { name: LayerName | "project_parent"; path: string; values: Record<string, unknown> };
type EffectiveSetting = { value: unknown; source: string };
type Snapshot = {
  layers: Layer[];
  effective: Record<string, EffectiveSetting>;
};

const layerNames: LayerName[] = ["user", "project", "local"];
const emptySnapshot: Snapshot = { layers: [], effective: {} };

export function Config() {
  const { invoke, label, profile } = useHarness();
  const [snapshot, setSnapshot] = useState<Snapshot>(emptySnapshot);
  const [activeLayer, setActiveLayer] = useState<LayerName>("user");
  const [projectRoot, setProjectRoot] = useState<string | null>(null);
  const [key, setKey] = useState("");
  const [valueText, setValueText] = useState("");
  const [effectiveSearch, setEffectiveSearch] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  const loadSequenceRef = useRef(0);

  const load = useCallback(async () => {
    const requestId = ++loadSequenceRef.current;
    setLoading(true);
    setSnapshot(emptySnapshot);
    setKey("");
    setValueText("");
    try {
      const loaded = await invoke<Snapshot>("config_load", { projectRoot });
      if (requestId !== loadSequenceRef.current) return;
      setSnapshot(loaded);
      setError("");
    } catch (cause) {
      if (requestId !== loadSequenceRef.current) return;
      setError(String(cause));
    } finally {
      if (requestId === loadSequenceRef.current) setLoading(false);
    }
  }, [projectRoot, invoke]);

  useEffect(() => {
    void load();
  }, [load]);

  const selectedLayer = useMemo(
    () => snapshot.layers.find((layer) => layer.name === activeLayer),
    [activeLayer, snapshot.layers],
  );
  const matchingSettings = Object.entries(snapshot.effective).filter(([settingKey, entry]) => {
    const query = effectiveSearch.trim().toLocaleLowerCase();
    return !query || [settingKey, entry.source, JSON.stringify(entry.value) ?? ""].some((value) => value.toLocaleLowerCase().includes(query));
  });

  const chooseProject = async () => {
    try {
      const selected = await invoke<string | null>("project_choose");
      if (selected !== null) setProjectRoot(selected);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const selectSetting = (settingKey: string) => {
    setKey(settingKey);
    const layerValue = selectedLayer?.values[settingKey];
    const hasLayerValue =
      selectedLayer !== undefined &&
      Object.prototype.hasOwnProperty.call(selectedLayer.values, settingKey);
    const displayedValue = hasLayerValue
      ? layerValue
      : snapshot.effective[settingKey]?.value;
    setValueText(
      displayedValue === undefined
        ? ""
        : JSON.stringify(displayedValue, null, 2),
    );
    setError("");
  };

  const apply = async () => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(valueText);
    } catch {
      setError("Value must be valid JSON.");
      return;
    }
    setSaving(true);
    setError("");
    try {
      await invoke("config_apply", {
        layer: activeLayer,
        key,
        value: parsed,
        projectRoot,
      });
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSaving(false);
    }
  };

  const undo = async () => {
    setSaving(true);
    setError("");
    try {
      await invoke("config_undo", { layer: activeLayer, key, projectRoot });
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="config-page" aria-labelledby="config-title" tabIndex={0}>
      <div className="pane-heading">
        <div>
          <p className="eyebrow">{label.toUpperCase()}</p>
          <h2 id="config-title">Configuration</h2>
          <p className="supporting-copy">
            Review each settings layer and edit one value at a time.
          </p>
        </div>
        <div className="config-actions">
          <M3eButton
            variant="outlined"
            onClick={() => void chooseProject()}
            disabled={saving}
          >
            Choose project
          </M3eButton>
          <M3eButton
            variant="outlined"
            onClick={() => void load()}
            disabled={loading || saving}
          >
            Refresh
          </M3eButton>
        </div>
      </div>

      <div className="config-layout">
        <section className="config-editor" aria-label="Settings layer editor">
          <div
            className="layer-tabs"
            role="tablist"
            aria-label="Settings layers"
          >
            {layerNames.filter(layer => profile.localSettings || layer !== "local").map((layer) => (
              <button
                key={layer}
                type="button"
                role="tab"
                aria-selected={activeLayer === layer}
                disabled={saving}
                className={
                  activeLayer === layer ? "layer-tab selected" : "layer-tab"
                }
                onClick={() => {
                  setActiveLayer(layer);
                  setKey("");
                  setValueText("");
                  setError("");
                }}
              >
                {layer[0].toUpperCase() + layer.slice(1)}
              </button>
            ))}
          </div>
          <p className="layer-path">{selectedLayer && displayPath(selectedLayer.path)}</p>
          <label className="field-label" htmlFor="setting-key">
            Setting key
          </label>
          <input
            id="setting-key"
            value={key}
            onChange={(event) => setKey(event.target.value)}
            placeholder="e.g. model"
          />
          <JsonEditor id="setting-value" label="JSON value" value={valueText} onChange={setValueText} disabled={saving || loading} placeholder={profile.modelExample} />
          <M3eButton
            variant="filled"
            onClick={() => void apply()}
            disabled={
              saving ||
              loading ||
              !selectedLayer ||
              !key.trim() ||
              !valueText.trim()
            }
          >
            {saving ? "Saving…" : "Apply change"}
          </M3eButton>
          <M3eButton
            variant="outlined"
            onClick={() => void undo()}
            disabled={saving || loading || !selectedLayer || !key.trim()}
          >
            Undo last change to this key
          </M3eButton>
        </section>

        <section className="effective-panel" aria-labelledby="effective-title">
          <div className="effective-heading">
            <div>
              <p className="eyebrow">RESOLVED SETTINGS</p>
              <h3 id="effective-title">Effective configuration</h3>
              {profile.configNote && <p className="supporting-copy">{profile.configNote}</p>}
            </div>
            <span className="setting-count">
              {effectiveSearch.trim() ? `${matchingSettings.length} of ${Object.keys(snapshot.effective).length} keys` : `${Object.keys(snapshot.effective).length} keys`}
            </span>
          </div>
          {snapshot.layers.some(layer => layer.name === "project_parent") && (
            <details>
              <summary>Inherited project files</summary>
              {snapshot.layers.filter(layer => layer.name === "project_parent").map(layer => <p key={layer.path} className="layer-path">{displayPath(layer.path)}</p>)}
            </details>
          )}
          {error && (
            <p className="config-error" role="alert">
              {displayPath(error)}
            </p>
          )}
          <div className="effective-search">
            <label htmlFor="effective-search">Search effective settings</label>
            <div className="effective-search-controls">
              <input id="effective-search" type="search" value={effectiveSearch} onChange={(event) => setEffectiveSearch(event.target.value)} placeholder="Key, source, or value" />
              <button type="button" className="text-button" onClick={() => setEffectiveSearch("")} disabled={!effectiveSearch} aria-label="Clear settings search">Clear</button>
            </div>
          </div>
          {loading ? (
            <p className="config-empty" role="status">
              Loading settings…
            </p>
          ) : snapshot.layers.length === 0 ? null : Object.keys(
              snapshot.effective,
            ).length === 0 ? (
            <p className="config-empty">
              No settings found in the available layers.
            </p>
          ) : matchingSettings.length === 0 ? (
            <p className="config-empty" role="status">No effective settings match "{effectiveSearch.trim()}".</p>
          ) : (
            <div className="effective-table-wrap" role="region" tabIndex={0} aria-label="Effective settings table">
              <table className="effective-table">
                <thead>
                  <tr>
                    <th scope="col">Key</th>
                    <th scope="col">Value</th>
                    <th scope="col">Source</th>
                  </tr>
                </thead>
                <tbody>
                  {matchingSettings.map(
                    ([settingKey, entry]) => (
                      <tr key={settingKey}>
                        <th scope="row">
                          <button
                            className="setting-link"
                            type="button"
                            onClick={() => selectSetting(settingKey)}
                          >
                            {settingKey}
                          </button>
                        </th>
                        <td>
                          <JsonValue value={entry.value} />
                        </td>
                        <td>
                          <span
                            className={`source-badge source-${entry.source}`}
                          >
                            {entry.source.replace("_", " ")}
                          </span>
                        </td>
                      </tr>
                    ),
                  )}
                </tbody>
              </table>
            </div>
          )}
        </section>
      </div>
      <footer className="pane-footer">
        <span>LOCAL FIRST</span>
        <span>Settings are validated before they are written.</span>
      </footer>
    </section>
  );
}
