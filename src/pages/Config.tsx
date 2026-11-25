import { useCallback, useEffect, useMemo, useState } from "react";
import { M3eButton } from "@m3e/react/button";
import "@m3e/web/button";
import { useHarness } from "../lib/HarnessContext";
import { displayPath } from "../lib/pathDisplay";
import { useSequencedLoad } from "../lib/useSequencedLoad";
import { JsonValue } from "../components/JsonValue";
import { JsonEditor } from "../components/JsonEditor";
import { SearchField } from "../components/SearchField";
import "../styles/ConfigSearch.css";

type LayerName = "user" | "project" | "local";
type Layer = { name: LayerName | "project_parent"; path: string; values: Record<string, unknown> };
type EffectiveSetting = { value: unknown; source: string };
type Snapshot = {
  layers: Layer[];
  effective: Record<string, EffectiveSetting>;
};

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

  const { start: startLoad } = useSequencedLoad();

  const load = useCallback(async () => {
    const request = startLoad();
    setLoading(true);
    setSnapshot(emptySnapshot);
    setKey("");
    setValueText("");
    try {
      const loaded = await invoke<Snapshot>("config_load", { projectRoot });
      if (!request.isCurrent()) return;
      setSnapshot(loaded);
      setActiveLayer(current => loaded.layers.some(layer => layer.name === current) ? current : "user");
      setError("");
    } catch (cause) {
      if (!request.isCurrent()) return;
      setError(String(cause));
    } finally {
      if (request.isCurrent()) setLoading(false);
    }
  }, [projectRoot, invoke, startLoad]);

  useEffect(() => {
    void load();
  }, [load]);

  const selectedLayer = useMemo(
    () => snapshot.layers.find((layer) => layer.name === activeLayer),
    [activeLayer, snapshot.layers],
  );
  const matchingSettings = useMemo(() => {
    const entries = Object.entries(snapshot.effective);
    const query = effectiveSearch.trim().toLocaleLowerCase();
    if (!query) return entries;
    return entries.filter(([settingKey, entry]) =>
      [settingKey, entry.source, JSON.stringify(entry.value) ?? ""].some((value) => value.toLocaleLowerCase().includes(query)),
    );
  }, [snapshot.effective, effectiveSearch]);
  const parentLayers = useMemo(
    () => snapshot.layers.filter((layer) => layer.name === "project_parent"),
    [snapshot.layers],
  );

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
            {profile.settingsLayers.map((layer) => (
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
          {parentLayers.length > 0 && (
            <details>
              <summary>Inherited project files</summary>
              {parentLayers.map(layer => <p key={layer.path} className="layer-path">{displayPath(layer.path)}</p>)}
            </details>
          )}
          {error && (
            <p className="config-error" role="alert">
              {displayPath(error)}
            </p>
          )}
          <SearchField
            id="effective-search"
            label="Search effective settings"
            value={effectiveSearch}
            onChange={setEffectiveSearch}
            placeholder="Key, source, or value"
            className="effective-search"
            controlsClassName="effective-search-controls"
            clearLabel="Clear"
            clearAriaLabel="Clear settings search"
            alwaysShowClear
          />
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
