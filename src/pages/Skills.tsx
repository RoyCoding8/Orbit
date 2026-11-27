import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useHarness } from "../lib/HarnessContext";
import type { SkillState } from "../lib/harnesses";
import { displayPath } from "../lib/pathDisplay";
import { useSequencedLoad } from "../lib/useSequencedLoad";
import { useModalDialog } from "../lib/useModalDialog";
import { MarkdownEditor } from "../components/MarkdownEditor";
import { SearchField } from "../components/SearchField";
import "../styles/ListControls.css";

type Skill = {
  id: string;
  name: string;
  description: string | null;
  path: string;
  content: string;
  state: SkillState;
  editable: boolean;
};
type LedgerRow = {
  timestamp: number;
  resourceKind: string;
  id: string;
  action: string;
  backupPath: string | null;
  beforeChecksum: string | null;
  afterChecksum: string | null;
};
type LoadState =
  | { kind: "loading" }
  | { kind: "ready" }
  | { kind: "error"; message: string };

export function Skills() {
  const { invoke: request, label, profile, projectRoot, setProjectRoot } = useHarness();
  const invoke = useCallback(<T,>(command: string, args?: Record<string, unknown>) =>
    request<T>(command, profile.projectSkills ? { ...args, projectRoot } : args), [request, profile.projectSkills, projectRoot]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [skillSearch, setSkillSearch] = useState("");
  const [ledger, setLedger] = useState<LedgerRow[]>([]);
  const [loadState, setLoadState] = useState<LoadState>({ kind: "loading" });
  const [editing, setEditing] = useState<Skill | null>(null);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState("");
  const [editError, setEditError] = useState("");
  const [busy, setBusy] = useState(false);
  const draftInput = useRef<HTMLTextAreaElement>(null);
  const isEditing = editing !== null;
  const editDialog = useModalDialog(isEditing, draftInput);
  const { start: startLoad } = useSequencedLoad();

  const load = useCallback(async () => {
    const req = startLoad();
    setLoadState({ kind: "loading" });
    try {
      const [inventory, rows] = await Promise.all([
        invoke<Skill[]>("skills_inventory"),
        invoke<LedgerRow[]>("skills_ledger"),
      ]);
      if (!req.isCurrent()) return;
      setSkills(inventory);
      setLedger(rows);
      setError("");
      setLoadState({ kind: "ready" });
    } catch (err) {
      if (!req.isCurrent()) return;
      setLoadState({ kind: "error", message: String(err) });
    }
  }, [invoke, startLoad]);

  useEffect(() => {
    void load();
  }, [load]);

  const restorable = useMemo(() => {
    const restored = new Set(
      ledger
        .filter((row) => row.action === "restore")
        .map((row) => row.backupPath),
    );
    return ledger.filter(
      (row) =>
        row.action === "uninstall" &&
        row.backupPath &&
        !restored.has(row.backupPath),
    );
  }, [ledger]);
  const canMutate = loadState.kind === "ready" && !busy;
  const normalizedSkillSearch = skillSearch.trim().toLocaleLowerCase();
  const visibleSkills = useMemo(
    () => normalizedSkillSearch
      ? skills.filter((skill) => [skill.id, skill.name, skill.description, skill.path, skill.content]
          .some((value) => value?.toLocaleLowerCase().includes(normalizedSkillSearch)))
      : skills,
    [normalizedSkillSearch, skills],
  );
  const update = async (action: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await action();
      await load();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };
  const install = async () => {
    try {
      const source = await open({
        directory: true,
        multiple: false,
        title: "Choose a skill folder",
      });
      if (typeof source === "string")
        await update(() => invoke("skill_install", { source }));
    } catch (err) {
      setError(String(err));
    }
  };
  const saveEdit = async () => {
    if (!editing) return;
    setBusy(true);
    setEditError("");
    try {
      await invoke("skill_edit", {
        id: editing.id,
        expectedContent: editing.content,
        content: draft,
      });
      setEditing(null);
      await load();
    } catch (err) {
      setEditError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section
      className="content-pane skills-pane"
      aria-labelledby="skills-title"
      tabIndex={0}
    >
      <div className="pane-heading">
        <div>
          <p className="eyebrow">{label.toUpperCase()}</p>
          <h2 id="skills-title">Skills</h2>
          <p className="supporting-copy">
            Skills are read from folders on disk, including folders copied
            outside Orbit. Refresh to pick up changes.
          </p>
        </div>
        <div className="skills-header-actions">
          {profile.projectSkills && <button className="action-button" disabled={busy} onClick={() => {
            void request<string | null>("project_choose").then(selected => {
              if (selected) setProjectRoot(selected);
            }).catch(reason => setError(String(reason)));
          }}>Choose project</button>}
          <button
            className="action-button"
            onClick={() => void load()}
            disabled={busy || loadState.kind === "loading"}
          >
            Refresh skills
          </button>
          <button
            className="action-button primary"
            onClick={() => void install()}
            disabled={!canMutate}
          >
            Install skill
          </button>
        </div>
      </div>
      {profile.projectSkills && <p className="supporting-copy">Project: {displayPath(projectRoot ?? "Current project")}. {profile.skillsNote}</p>}
      {error && (
        <p className="inline-error" role="alert">
          {displayPath(error)}
        </p>
      )}
      {loadState.kind === "loading" ? (
        <p role="status">Loading skills…</p>
      ) : loadState.kind === "error" ? (
        <p className="inline-error" role="alert">
          Could not load skills. {displayPath(loadState.message)}
        </p>
      ) : (
        <>
          <div className="section-heading">
            <div>
              <p className="eyebrow">SKILLS</p>
              <h3>
                Available skills <span className="count">{skills.length}</span>
              </h3>
            </div>
          </div>
          {skills.length === 0 ? (
            <div className="empty-panel">
              <h3>No skills found</h3>
              <p className="empty-copy">
                Install a skill folder containing a SKILL.md file to get
                started.
              </p>
              <button
                className="action-button primary"
                onClick={() => void install()}
                disabled={!canMutate}
              >
                Choose skill folder
              </button>
            </div>
          ) : (
            <>
              <SearchField
                id="skills-search"
                label="Search skills"
                value={skillSearch}
                onChange={setSkillSearch}
                status={`Showing ${visibleSkills.length} of ${skills.length} skills`}
              />
              {visibleSkills.length === 0 ? (
                <p className="empty-copy">No skills match "{skillSearch.trim()}".</p>
              ) : (
            <div className="skill-list" role="region" tabIndex={0} aria-label="Available skills">
              {visibleSkills.map((skill) => (
                <article className="skill-card" key={skill.id}>
                  <div className="skill-copy">
                    <div className="skill-title-row">
                      <h3>{skill.name || skill.id}</h3>
                      <span className={`state-badge state-${skill.state}`}>
                        {skill.state}
                      </span>
                    </div>
                    <p className="skill-id">{skill.id}</p>
                    <details className="collection-card-details">
                      <summary>Skill details</summary>
                      <p className="supporting-copy">{skill.description || "No description in frontmatter."}</p>
                      <p className="skill-id">Path: {displayPath(skill.path)}</p>
                    </details>
                  </div>
                  {profile.skillModes.length > 0 && <><label className="state-toggle">
                    <input
                      type="checkbox"
                      aria-label={`Enable ${skill.id}`}
                      checked={skill.state !== "off"}
                      disabled={!canMutate}
                      onChange={(event) =>
                        void update(() =>
                          invoke("skill_set_override", {
                            id: skill.id,
                            state: event.target.checked ? "on" : "off",
                          }),
                        )
                      }
                    />
                    Enabled
                  </label>
                  <label className="state-control">
                    Visibility
                    <select
                      aria-label={`Visibility for ${skill.id}`}
                      value={skill.state}
                      disabled={!canMutate}
                      onChange={(event) =>
                        void update(() =>
                          invoke("skill_set_override", {
                            id: skill.id,
                            state: event.target.value,
                          }),
                        )
                      }
                    >
                      {profile.skillModes.map(mode => <option key={mode} value={mode}>{({ on: "On", off: "Off", "name-only": "Name only", "user-invocable-only": "User invocable only" })[mode]}</option>)}
                    </select>
                  </label></>}
                  <div className="card-actions">
                    <button
                      className="action-button"
                        disabled={!canMutate || !skill.editable}
                      onClick={() => {
                        setEditing(skill);
                        setDraft(skill.content);
                        setEditError("");
                      }}
                    >
                      Edit
                    </button>
                    <button
                      className="action-button danger"
                      disabled={!canMutate || !skill.editable}
                      onClick={() =>
                        void update(() =>
                          invoke("skill_uninstall", { id: skill.id }),
                        )
                      }
                    >
                      Uninstall
                    </button>
                  </div>
                </article>
              ))}
            </div>
              )}
            </>
          )}
          {restorable.length > 0 && (
            <details className="restore-section">
              <summary>Recently uninstalled ({restorable.length})</summary>
              <p className="eyebrow">BACKUPS</p>
              {restorable.map((row) => (
                <div className="restore-row" key={`${row.timestamp}-${row.id}`}>
                  <span>{row.id}</span>
                  <button
                    className="action-button"
                    disabled={!canMutate}
                    onClick={() =>
                      void update(() =>
                        invoke("skill_restore", {
                          id: row.id,
                          backup: row.backupPath,
                        }),
                      )
                    }
                  >
                    Restore
                  </button>
                </div>
              ))}
            </details>
          )}
        </>
      )}
      <dialog
        ref={editDialog}
        className="edit-dialog"
        aria-labelledby="edit-title"
        onCancel={(event) => {
          event.preventDefault();
          if (!busy) setEditing(null);
        }}
      >
        {editing && (
          <>
            <div className="pane-heading">
              <div>
                <p className="eyebrow">REVIEW CHANGES</p>
                <h3 id="edit-title">Edit {editing.id}</h3>
              </div>
              <button
                className="text-button"
                disabled={busy}
                onClick={() => setEditing(null)}
              >
                Close
              </button>
            </div>
            <p className="supporting-copy">
              Compare the original SKILL.md with your draft. Saving also records
              a backup and change history. If the file changed elsewhere,
              refresh skills before editing again.
            </p>
            {editError && (
              <p className="inline-error" role="alert">
                {displayPath(editError)}
              </p>
            )}
            <div className="diff-columns">
              <MarkdownEditor key={`${editing.id}-original`} id="skill-original" label="Current SKILL.md" value={editing.content} readOnly />
              <MarkdownEditor key={editing.id} id="skill-draft" label="Proposed SKILL.md" value={draft} onChange={setDraft} disabled={busy} inputRef={draftInput} />
            </div>
            <div className="dialog-actions">
              <button
                className="action-button"
                disabled={busy}
                onClick={() => setEditing(null)}
              >
                Cancel
              </button>
              <button
                className="action-button primary"
                disabled={!canMutate || draft === editing.content}
                onClick={() => void saveEdit()}
              >
                {busy ? "Saving…" : "Apply edit"}
              </button>
            </div>
          </>
        )}
      </dialog>
    </section>
  );
}
