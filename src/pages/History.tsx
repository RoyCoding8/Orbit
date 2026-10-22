import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./History.css";

type Scope = "global" | "project" | "other";
type VersionKind = "file" | "directory";
type OperationStatus =
  | "applied"
  | "pending"
  | "notapplied"
  | "drift"
  | "legacy"
  | "unavailable";
type HistoryVersion = {
  id: string;
  timestamp: number;
  resourceKind: string;
  resourceId: string;
  action: string;
  scope: Scope;
  scopeId: string;
  scopeLabel: string;
  target: string;
  kind: VersionKind;
  checksum: string;
  currentChecksum: string | null;
  available: boolean;
  restorable: boolean;
};
type HistoryLog = {
  id: string;
  timestamp: number;
  resourceKind: string;
  resourceId: string;
  action: string;
  scope: Scope;
  scopeId: string;
  scopeLabel: string;
  target: string | null;
  status: OperationStatus;
  backupAvailable: boolean;
  hasBackup: boolean;
};
type HistorySnapshot = { versions: HistoryVersion[]; logs: HistoryLog[] };
type LoadState = "loading" | "ready" | "error";
type Tab = "versions" | "logs";
type ScopeFilter = "all" | "global" | "project";

const emptySnapshot: HistorySnapshot = { versions: [], logs: [] };

function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleString();
}

function targetState(checksum: string | null): string {
  return checksum === null ? "Target absent or unavailable" : "Target exists";
}

function parseScopeFilter(value: string): ScopeFilter {
  if (value === "global" || value === "project") return value;
  return "all";
}

export function History() {
  const [snapshot, setSnapshot] = useState<HistorySnapshot>(emptySnapshot);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [tab, setTab] = useState<Tab>("versions");
  const [scopeFilter, setScopeFilter] = useState<ScopeFilter>("all");
  const [projectRoot, setProjectRoot] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const requestNumber = useRef(0);

  const load = useCallback(async () => {
    const request = ++requestNumber.current;
    setLoadState("loading");
    try {
      const result = await invoke<HistorySnapshot>("history_load", {
        projectRoot,
      });
      if (request !== requestNumber.current) return;
      setSnapshot(result);
      setError("");
      setLoadState("ready");
    } catch (cause) {
      if (request !== requestNumber.current) return;
      setError(`Could not load history. ${String(cause)}`);
      setLoadState("error");
    }
  }, [projectRoot]);

  useEffect(() => {
    void load();
    return () => {
      requestNumber.current += 1;
    };
  }, [load]);

  const visibleVersions = useMemo(
    () =>
      snapshot.versions.filter(
        (version) => scopeFilter === "all" || version.scope === scopeFilter,
      ),
    [scopeFilter, snapshot.versions],
  );
  const visibleLogs = useMemo(
    () =>
      snapshot.logs.filter(
        (entry) => scopeFilter === "all" || entry.scope === scopeFilter,
      ),
    [scopeFilter, snapshot.logs],
  );

  const chooseProject = async () => {
    try {
      const selected = await invoke<string | null>("project_choose");
      if (selected !== null) setProjectRoot(selected);
    } catch (cause) {
      setError(`Could not choose a project. ${String(cause)}`);
    }
  };

  const runAction = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await action();
      await load();
    } catch (cause) {
      await load();
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };

  const restore = (version: HistoryVersion) => {
    const targetDescription =
      version.currentChecksum === null
        ? `create the currently absent target at ${version.target}`
        : `replace the current target at ${version.target}`;
    if (
      !window.confirm(
        `Restore this ${version.kind} version and ${targetDescription}? The state being replaced will be saved as a new version.`,
      )
    )
      return;
    void runAction(() =>
      invoke("history_restore", {
        projectRoot,
        id: version.id,
        expectedCurrentChecksum: version.currentChecksum,
      }),
    );
  };

  const removeVersion = (version: HistoryVersion) => {
    if (
      !window.confirm(
        `Delete the saved payload for this ${version.kind} version? Its operation log will remain.`,
      )
    )
      return;
    void runAction(() => invoke("history_delete", { id: version.id }));
  };

  const versionsByGroup = useMemo(() => {
    const groups = new Map<string, HistoryVersion[]>();
    for (const version of visibleVersions) {
      const key = `${version.scopeId}\u0000${version.scopeLabel}`;
      const group = groups.get(key) ?? [];
      group.push(version);
      groups.set(key, group);
    }
    return [...groups.values()];
  }, [visibleVersions]);

  return (
    <section className="content-pane history-page" aria-labelledby="history-title">
      <div className="pane-heading">
        <div>
          <p className="eyebrow">LOCAL CHANGE RECORD</p>
          <h2 id="history-title">History</h2>
          <p className="supporting-copy">
            Review saved file and skill versions separately from the operation log.
          </p>
        </div>
        <div className="history-header-actions">
          <button
            className="secondary-action"
            type="button"
            onClick={() => void chooseProject()}
            disabled={busy || loadState === "loading"}
          >
            Choose project
          </button>
          <button
            className="primary-action"
            type="button"
            onClick={() => void load()}
            disabled={busy || loadState === "loading"}
          >
            Refresh
          </button>
        </div>
      </div>

      <div className="history-toolbar">
        <p className="history-project" title={projectRoot ?? "Current project"}>
          {projectRoot ?? "Current project"}
        </p>
        <label className="history-scope-filter">
          Scope
          <select
            value={scopeFilter}
            onChange={(event) => setScopeFilter(parseScopeFilter(event.currentTarget.value))}
          >
            <option value="all">All scopes</option>
            <option value="global">Global</option>
            <option value="project">Project</option>
          </select>
        </label>
      </div>

      {error && <p className="history-error" role="alert">{error}</p>}
      {loadState === "loading" && (
        <p className="history-quiet" role="status">Loading history…</p>
      )}

      <div className="history-tabs" role="tablist" aria-label="History views">
        <button
          id="history-versions-tab"
          type="button"
          role="tab"
          aria-selected={tab === "versions"}
          aria-controls="history-versions-panel"
          className={tab === "versions" ? "selected" : ""}
          onClick={() => setTab("versions")}
        >
          Versions <span>{visibleVersions.length}</span>
        </button>
        <button
          id="history-logs-tab"
          type="button"
          role="tab"
          aria-selected={tab === "logs"}
          aria-controls="history-logs-panel"
          className={tab === "logs" ? "selected" : ""}
          onClick={() => setTab("logs")}
        >
          Logs <span>{visibleLogs.length}</span>
        </button>
      </div>

      {tab === "versions" ? (
        <div
          id="history-versions-panel"
          className="history-panel"
          role="tabpanel"
          aria-labelledby="history-versions-tab"
        >
          {loadState === "ready" && visibleVersions.length === 0 && (
            <p className="history-empty">No saved versions match this scope.</p>
          )}
          {versionsByGroup.map((group) => (
            <section className="history-group" key={`${group[0].scopeId}:${group[0].scopeLabel}`}>
              <header className="history-group-heading">
                <h3>{group[0].scopeLabel}</h3>
                <span>{group.length}</span>
              </header>
              <div className="history-list">
                {group.map((version) => (
                  <article className="history-version" key={version.id}>
                    <div className="history-version-copy">
                      <div className="history-version-title">
                        <h4>{version.resourceKind} · {version.resourceId}</h4>
                        <span className="history-kind">{version.kind} version</span>
                      </div>
                      <p className="history-target" title={version.target}>{version.target}</p>
                      <p className="history-meta">
                        {formatTime(version.timestamp)} · {version.action} · {targetState(version.currentChecksum)}
                      </p>
                      {!version.available && (
                        <p className="history-unavailable">Saved payload is missing or does not match its checksum.</p>
                      )}
                    </div>
                    <div className="history-actions">
                      <button
                        className="action-button primary"
                        type="button"
                        onClick={() => restore(version)}
                        disabled={busy || loadState !== "ready" || !version.restorable}
                      >
                        Restore
                      </button>
                      <button
                        className="action-button danger"
                        type="button"
                        onClick={() => removeVersion(version)}
                        disabled={busy || loadState !== "ready" || !version.available}
                      >
                        Delete version
                      </button>
                    </div>
                  </article>
                ))}
              </div>
            </section>
          ))}
        </div>
      ) : (
        <div
          id="history-logs-panel"
          className="history-panel"
          role="tabpanel"
          aria-labelledby="history-logs-tab"
        >
          {loadState === "ready" && visibleLogs.length === 0 && (
            <p className="history-empty">No operations match this scope.</p>
          )}
          <div className="history-list">
            {visibleLogs.map((entry) => (
              <article className="history-log" key={entry.id}>
                <div className="history-version-copy">
                  <div className="history-version-title">
                    <h4>{entry.resourceKind} · {entry.resourceId}</h4>
                    <span className={`history-status status-${entry.status}`}>{entry.status}</span>
                  </div>
                  <p className="history-target" title={entry.target ?? undefined}>
                    {entry.target ?? "Target not recorded"}
                  </p>
                  <p className="history-meta">
                    {entry.scopeLabel} · {formatTime(entry.timestamp)} · {entry.action}
                  </p>
                  <p className="history-meta">
                    {entry.hasBackup
                      ? entry.backupAvailable
                        ? "Saved payload available"
                        : "Saved payload removed or unavailable"
                      : "No saved payload"}
                  </p>
                </div>
              </article>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
