import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useHarness } from "../HarnessContext";
import { displayPath } from "../pathDisplay";
import "./History.css";

type Scope = "global" | "project" | "other";
type VersionKind = "file" | "directory";
type BackupStatus = "available" | "missing" | "checksumMismatch" | "unavailable";
const backupLabels: Record<BackupStatus, string> = {
  available: "Saved payload available",
  missing: "Saved backup is missing.",
  checksumMismatch: "Saved backup checksum does not match its recorded checksum.",
  unavailable: "Saved backup could not be read or verified.",
};
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
  backupStatus: BackupStatus;
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
  backupStatus: BackupStatus | null;
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

function matchesSearch(query: string, fields: readonly (string | null | undefined)[]): boolean {
  return !query || fields.some((field) => field && displayPath(field).toLocaleLowerCase().includes(query));
}

export function History() {
  const { invoke } = useHarness();
  const [snapshot, setSnapshot] = useState<HistorySnapshot>(emptySnapshot);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [tab, setTab] = useState<Tab>("versions");
  const [scopeFilter, setScopeFilter] = useState<ScopeFilter>("all");
  const [search, setSearch] = useState("");
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
  }, [projectRoot, invoke]);

  useEffect(() => {
    void load();
    return () => {
      requestNumber.current += 1;
    };
  }, [load]);

  const normalizedSearch = displayPath(search.trim()).toLocaleLowerCase();
  const scopedVersions = useMemo(
    () =>
      snapshot.versions.filter(
        (version) => scopeFilter === "all" || version.scope === scopeFilter,
      ),
    [scopeFilter, snapshot.versions],
  );
  const scopedLogs = useMemo(
    () =>
      snapshot.logs.filter(
        (entry) => scopeFilter === "all" || entry.scope === scopeFilter,
      ),
    [scopeFilter, snapshot.logs],
  );
  const visibleVersions = useMemo(
    () => scopedVersions.filter((version) => matchesSearch(normalizedSearch, [
      version.id,
      version.resourceKind,
      version.resourceId,
      version.target,
      version.scope,
      version.scopeId,
      version.scopeLabel,
      version.action,
      version.kind,
      formatTime(version.timestamp),
      backupLabels[version.backupStatus],
      targetState(version.currentChecksum),
    ])),
    [normalizedSearch, scopedVersions],
  );
  const visibleLogs = useMemo(
    () => scopedLogs.filter((entry) => matchesSearch(normalizedSearch, [
      entry.id,
      entry.resourceKind,
      entry.resourceId,
      entry.target ?? "Target not recorded",
      entry.scope,
      entry.scopeId,
      entry.scopeLabel,
      entry.action,
      entry.status,
      formatTime(entry.timestamp),
      entry.backupStatus === null ? "No saved payload" : backupLabels[entry.backupStatus],
    ])),
    [normalizedSearch, scopedLogs],
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
        ? `create the currently absent target at ${displayPath(version.target)}`
        : `replace the current target at ${displayPath(version.target)}`;
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
    <section className="content-pane history-page" aria-labelledby="history-title" tabIndex={0}>
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
        <p className="history-project" title={displayPath(projectRoot ?? "Current project")}>
          {displayPath(projectRoot ?? "Current project")}
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
        <label className="history-search" htmlFor="history-search">
          Search history
          <span>
            <input
              id="history-search"
              type="search"
              aria-label="Search history"
              value={search}
              onChange={(event) => setSearch(event.currentTarget.value)}
            />
            {search && (
              <button className="text-button" type="button" onClick={() => setSearch("")}>
                Clear search
              </button>
            )}
          </span>
        </label>
      </div>
      <p className="history-search-count" role="status">
        Showing {visibleVersions.length} of {scopedVersions.length} versions and {visibleLogs.length} of {scopedLogs.length} logs
      </p>

      {error && <p className="history-error" role="alert">{displayPath(error)}</p>}
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
          tabIndex={0}
        >
          {loadState === "ready" && visibleVersions.length === 0 && (
            <p className="history-empty">
              {snapshot.versions.length === 0
                ? "No saved versions found."
                : scopedVersions.length === 0
                  ? "No saved versions match this scope."
                  : `No versions match "${search.trim()}".`}
            </p>
          )}
          {versionsByGroup.map((group) => (
            <details className="history-group" key={`${group[0].scopeId}:${group[0].scopeLabel}`} open={versionsByGroup[0] === group}>
              <summary className="history-group-heading">
                <h3>{displayPath(group[0].scopeLabel)}</h3>
                <span>{group.length}</span>
              </summary>
              <div className="history-list">
                {group.map((version) => (
                  <article className="history-version" key={version.id}>
                    <div className="history-version-copy">
                      <div className="history-version-title">
                        <h4>{version.resourceKind} · {displayPath(version.resourceId)}</h4>
                        <span className="history-kind">{version.kind} version</span>
                      </div>
                      <p className="history-target" title={displayPath(version.target)}>{displayPath(version.target)}</p>
                      <p className="history-meta">
                        {formatTime(version.timestamp)} · {version.action} · {targetState(version.currentChecksum)}
                      </p>
                      {version.backupStatus !== "available" && (
                        <p className="history-unavailable">{backupLabels[version.backupStatus]}</p>
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
                        disabled={busy || loadState !== "ready" || version.backupStatus !== "available"}
                      >
                        Delete version
                      </button>
                    </div>
                  </article>
                ))}
              </div>
            </details>
          ))}
        </div>
      ) : (
        <div
          id="history-logs-panel"
          className="history-panel"
          role="tabpanel"
          aria-labelledby="history-logs-tab"
          tabIndex={0}
        >
          {loadState === "ready" && visibleLogs.length === 0 && (
            <p className="history-empty">
              {snapshot.logs.length === 0
                ? "No operations found."
                : scopedLogs.length === 0
                  ? "No operations match this scope."
                  : `No operations match "${search.trim()}".`}
            </p>
          )}
          <div className="history-list">
            {visibleLogs.map((entry) => (
              <article className="history-log" key={entry.id}>
                <div className="history-version-copy">
                  <div className="history-version-title">
                    <h4>{entry.resourceKind} · {displayPath(entry.resourceId)}</h4>
                    <span className={`history-status status-${entry.status}`}>{entry.status}</span>
                  </div>
                  <p className="history-target" title={entry.target ? displayPath(entry.target) : undefined}>
                    {displayPath(entry.target ?? "Target not recorded")}
                  </p>
                  <p className="history-meta">
                    {displayPath(entry.scopeLabel)} · {formatTime(entry.timestamp)} · {entry.action}
                  </p>
                  <p className="history-meta">
                    {entry.backupStatus === null ? "No saved payload" : backupLabels[entry.backupStatus]}
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
