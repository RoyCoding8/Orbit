import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

type SkillState = "on" | "name-only" | "user-invocable-only" | "off";
type Skill = {
  id: string;
  name: string;
  description: string | null;
  path: string;
  content: string;
  state: SkillState;
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
  const [skills, setSkills] = useState<Skill[]>([]);
  const [ledger, setLedger] = useState<LedgerRow[]>([]);
  const [loadState, setLoadState] = useState<LoadState>({ kind: "loading" });
  const [editing, setEditing] = useState<Skill | null>(null);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState("");
  const [editError, setEditError] = useState("");
  const [busy, setBusy] = useState(false);
  const editDialog = useRef<HTMLDialogElement>(null);
  const draftInput = useRef<HTMLTextAreaElement>(null);
  const isEditing = editing !== null;

  const load = async () => {
    setLoadState({ kind: "loading" });
    try {
      const [inventory, rows] = await Promise.all([
        invoke<Skill[]>("skills_inventory"),
        invoke<LedgerRow[]>("skills_ledger"),
      ]);
      setSkills(inventory);
      setLedger(rows);
      setError("");
      setLoadState({ kind: "ready" });
    } catch (err) {
      setLoadState({ kind: "error", message: String(err) });
    }
  };
  useEffect(() => {
    void load();
  }, []);
  useEffect(() => {
    const dialog = editDialog.current;
    if (!isEditing || !dialog) return;
    dialog.showModal();
    draftInput.current?.focus();
    return () => {
      if (dialog.open) dialog.close();
    };
  }, [isEditing]);

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
    const source = await open({
      directory: true,
      multiple: false,
      title: "Choose a skill folder",
    });
    if (typeof source === "string")
      await update(() => invoke("skill_install", { source }));
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
    >
      <div className="pane-heading">
        <div>
          <p className="eyebrow">CLAUDE CODE</p>
          <h2 id="skills-title">Skills</h2>
          <p className="supporting-copy">
            Skills are read from folders on disk, including folders copied
            outside Orbit. Refresh to pick up changes.
          </p>
        </div>
        <div className="skills-header-actions">
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
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      {loadState.kind === "loading" ? (
        <p role="status">Loading skills…</p>
      ) : loadState.kind === "error" ? (
        <p className="inline-error" role="alert">
          Could not load skills. {loadState.message}
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
            <div className="skill-list">
              {skills.map((skill) => (
                <article className="skill-card" key={skill.id}>
                  <div className="skill-copy">
                    <div className="skill-title-row">
                      <h3>{skill.name || skill.id}</h3>
                      <span className={`state-badge state-${skill.state}`}>
                        {skill.state}
                      </span>
                    </div>
                    <p className="skill-id">{skill.id}</p>
                    <p className="supporting-copy">
                      {skill.description || "No description in frontmatter."}
                    </p>
                  </div>
                  <label className="state-toggle">
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
                      <option value="on">On</option>
                      <option value="name-only">Name only</option>
                      <option value="user-invocable-only">
                        User invocable only
                      </option>
                      <option value="off">Off</option>
                    </select>
                  </label>
                  <div className="card-actions">
                    <button
                      className="action-button"
                      disabled={!canMutate}
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
                      disabled={!canMutate}
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
          {restorable.length > 0 && (
            <div className="restore-section">
              <p className="eyebrow">BACKUPS</p>
              <h3>Recently uninstalled</h3>
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
            </div>
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
                {editError}
              </p>
            )}
            <div className="diff-columns">
              <label>
                Current SKILL.md<pre>{editing.content}</pre>
              </label>
              <label>
                Proposed SKILL.md
                <textarea
                  ref={draftInput}
                  disabled={busy}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                />
              </label>
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
