import { useCallback, useEffect, useRef, useState } from "react";
import { useHarness } from "../HarnessContext";
import { open } from "@tauri-apps/plugin-dialog";
import { ChatConversion } from "./ChatConversion";

type Session = {
  sessionId: string;
  title: string | null;
  project: string;
  updatedAt: string | null;
  messageCount: number;
};
type SessionCatalog = { sessions: Session[]; source: string; warnings: string[] };
type Message = {
  uuid: string;
  parentUuid: string | null;
  messageType: string;
  text: string;
  position: number;
  depth: number;
  branchIndex: number;
  attachment: string | null;
  toolResult: string | null;
  toolResultNote: string | null;
};
type SearchHit = {
  sessionId: string;
  project: string;
  uuid: string;
  position: number;
  messageType: string;
  text: string;
  before: string | null;
  after: string | null;
};
type IngestReport = {
  sessions: number;
  linesRead: number;
  messagesIndexed: number;
  malformedLines: number;
  skippedFiles: number;
  skippedStateLines: number;
};
type ArchiveReport = {
  action: "exported" | "imported";
  sourceSessionId: string;
  sessionId: string;
  files: number;
  path: string;
};
type Thread = {
  sessionId: string;
  messages: Message[];
  offset: number;
  hasMore: boolean;
  loading: boolean;
  focusUuid: string | null;
};
const PAGE_SIZE = 100;
const SEARCH_LIMIT = 50;

function groupSessions(sessions: Session[]) {
  const projects = new Map<string, { key: string; project: string; sessions: Session[] }>();
  for (const session of sessions) {
    const project = session.project.trim() || "Unknown project";
    const path = project.replace(/^\\\\\?\\UNC\\/i, "//").replace(/^\\\\\?\\/, "")
      .replaceAll("\\", "/").replace(/\/+$/, "") || "/";
    const key = /^[a-z]:\//i.test(path) || path.startsWith("//") ? path.toLowerCase() : path;
    const group = projects.get(key);
    if (group) group.sessions.push(session);
    else projects.set(key, { key, project, sessions: [session] });
  }
  return [...projects.values()];
}

export function Transcripts() {
  const { invoke, label, profile } = useHarness();
  const [sessions, setSessions] = useState<Session[]>([]);
  const [source, setSource] = useState("");
  const [filter, setFilter] = useState("");
  const [projectRoot, setProjectRoot] = useState<string | null>(null);
  const [sessionState, setSessionState] = useState<
    "loading" | "ready" | "error"
  >("loading");
  const [thread, setThread] = useState<Thread | null>(null);
  const sessionRequest = useRef(0);
  const threadRequest = useRef(0);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [searched, setSearched] = useState(false);
  const [pending, setPending] = useState(false);
  const [converting, setConverting] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState("");
  const deleteDialog = useRef<HTMLDialogElement>(null);
  const [notice, setNotice] = useState(`Finding ${label} chats…`);
  const selectedSession = thread?.sessionId ?? null;
  const messages = thread?.messages ?? [];
  const focusUuid = thread?.focusUuid;
  const busy = pending || sessionState === "loading" || thread?.loading === true;

  const groups = groupSessions(sessions.filter(session =>
    `${session.project} ${session.title ?? ""} ${session.sessionId}`.toLowerCase().includes(filter.toLowerCase().trim())));

  const refreshSessions = useCallback(async (refresh = false, projectRoot: string | null = null) => {
    const request = ++sessionRequest.current;
    setSessionState("loading");
    try {
      const result = await invoke<SessionCatalog>("transcript_list_sessions", { refresh, projectRoot });
      if (request !== sessionRequest.current) return;
      setSessions(result.sessions);
      setSource(result.source);
      setSessionState("ready");
      if (refresh) setNotice(`Found ${result.sessions.length} ${label} chats.${result.warnings.length ? ` Skipped: ${result.warnings.join(", ")}.` : " Choose a project below."}`);
      return result.sessions;
    } catch (error) {
      if (request !== sessionRequest.current) return;
      setSessionState("error");
      throw error;
    }
  }, [invoke, label]);

  useEffect(() => {
    void refreshSessions(true).catch(error => setNotice(`Could not find chats. ${String(error)}`));
    return () => {
      sessionRequest.current += 1;
      threadRequest.current += 1;
    };
  }, [refreshSessions, label]);
  useEffect(() => {
    setDeleteError("");
    if (deleting) deleteDialog.current?.showModal();
    else deleteDialog.current?.close();
  }, [deleting]);
  useEffect(() => {
    if (focusUuid)
      document
        .querySelector(`[data-message-uuid="${CSS.escape(focusUuid)}"]`)
        ?.scrollIntoView({ block: "center" });
  }, [focusUuid, messages]);

  const indexSource = async () => {
    const selection = threadRequest.current;
    setPending(true);
    setNotice("Reading transcript files into the local index…");
    try {
      const report = await invoke<IngestReport>("transcript_ingest", profile.transcripts.scope === "project" ? { projectRoot } : undefined);
      const nextSessions = await refreshSessions();
      const skipped = [
        report.malformedLines > 0
          ? `${report.malformedLines} malformed lines`
          : null,
        report.skippedStateLines > 0
          ? `${report.skippedStateLines} state lines`
          : null,
        report.skippedFiles > 0
          ? `${report.skippedFiles} unreadable files`
          : null,
      ]
        .filter(Boolean)
        .join(", ");
      setNotice(
        `Refreshed ${report.sessions} chats. Re-indexed ${report.messagesIndexed.toLocaleString()} messages from changed files.${skipped ? ` Skipped: ${skipped}.` : ""}`,
      );
      setHits([]);
      setSearched(false);
      if (selection === threadRequest.current && nextSessions) {
        const next = nextSessions.find(session => session.sessionId === selectedSession) ?? nextSessions[0];
        if (next) await selectSession(next.sessionId);
        else {
          threadRequest.current += 1;
          setThread(null);
        }
      }
    } catch (error) {
      setNotice(`Could not index transcripts. ${String(error)}`);
    } finally {
      setPending(false);
    }
  };

  const selectSession = async (sessionId: string, uuid?: string) => {
    const request = ++threadRequest.current;
    setThread({
      sessionId,
      messages: [],
      offset: 0,
      hasMore: false,
      loading: true,
      focusUuid: null,
    });
    try {
      const offset = uuid
        ? await invoke<number | null>("transcript_message_page_offset", {
            sessionId,
            uuid,
            pageSize: PAGE_SIZE,
          })
        : 0;
      if (request !== threadRequest.current) return;
      if (offset === null) {
        setNotice(
          `The matched message is no longer part of session ${sessionId}. The index may be stale for this session; re-index to refresh it.`,
        );
        return;
      }
      const page = await invoke<Message[]>("transcript_thread_page", {
        sessionId,
        offset,
        limit: PAGE_SIZE,
      });
      if (request !== threadRequest.current) return;
      setThread({
        sessionId,
        messages: page,
        offset,
        hasMore: page.length === PAGE_SIZE,
        loading: false,
        focusUuid: uuid ?? null,
      });
    } catch (error) {
      if (request === threadRequest.current)
        setNotice(`Could not open this session. ${String(error)}`);
    } finally {
      if (request === threadRequest.current)
        setThread((current) => current && { ...current, loading: false });
    }
  };

  const loadMore = async () => {
    if (!thread || busy) return;
    const request = ++threadRequest.current;
    setThread({ ...thread, loading: true });
    try {
      const page = await invoke<Message[]>("transcript_thread_page", {
        sessionId: thread.sessionId,
        offset: thread.offset + thread.messages.length,
        limit: PAGE_SIZE,
      });
      if (request === threadRequest.current)
        setThread({
          ...thread,
          messages: [...thread.messages, ...page],
          hasMore: page.length === PAGE_SIZE,
          loading: false,
        });
    } catch (error) {
      if (request === threadRequest.current)
        setNotice(`Could not load more messages. ${String(error)}`);
    } finally {
      if (request === threadRequest.current)
        setThread((current) => current && { ...current, loading: false });
    }
  };

  const search = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy) return;
    if (!query.trim()) {
      setHits([]);
      setSearched(false);
      return;
    }
    setPending(true);
    try {
      setHits(
        await invoke<SearchHit[]>("transcript_search", {
          query,
          limit: SEARCH_LIMIT,
        }),
      );
      setSearched(true);
    } catch (error) {
      setNotice(`Search failed. ${String(error)}`);
    } finally {
      setPending(false);
    }
  };

  const exportSession = async () => {
    if (!selectedSession) return;
    try {
      const markdown = await invoke<string>("transcript_export_markdown", {
        sessionId: selectedSession,
      });
      const url = URL.createObjectURL(
        new Blob([markdown], { type: "text/markdown;charset=utf-8" }),
      );
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `${selectedSession.split("/").pop() ?? selectedSession}.md`;
      anchor.click();
      URL.revokeObjectURL(url);
      setNotice("Markdown export downloaded.");
    } catch (error) {
      setNotice(`Export failed. ${String(error)}`);
    }
  };

  const exportArchive = async () => {
    if (!selectedSession) return;
    setPending(true);
    try {
      const destination = await open({
        directory: true,
        multiple: false,
        title: `Choose where to save the ${label} session archive`,
      });
      if (typeof destination !== "string") return;
      const report = await invoke<ArchiveReport>("transcript_export_directory", {
        sessionId: selectedSession,
        destination,
      });
      setNotice(`Archived ${report.files} native files to ${report.path}.`);
    } catch (error) {
      setNotice(`Archive export failed. ${String(error)}`);
    } finally {
      setPending(false);
    }
  };

  const importArchive = async () => {
    setPending(true);
    try {
      const source = await open({
        directory: true,
        multiple: false,
        title: `Choose a ${label} session archive`,
      });
      if (typeof source !== "string") return;
      const projectRoot = await invoke<string | null>("project_choose");
      if (!projectRoot) return;
      const report = await invoke<ArchiveReport>("transcript_import_directory", {
        source,
        projectRoot,
      });
      try {
        await refreshSessions();
        setNotice(`Imported ${report.sourceSessionId} as ${report.sessionId}.`);
      } catch (error) {
        setNotice(
          `Imported ${report.sessionId}, but the transcript list could not refresh. ${String(error)}`,
        );
      }
    } catch (error) {
      setNotice(`Archive import failed. ${String(error)}`);
    } finally {
      setPending(false);
    }
  };

  const deleteSession = async () => {
    if (!deleting) return;
    setDeleteError("");
    setPending(true);
    try {
      const backupDirectory = await open({ directory: true, multiple: false,
        title: "Choose where to save the chat before deleting it" });
      if (typeof backupDirectory !== "string") return;
      const report = await invoke<{ path: string; deletedSessions: string[]; warning: string | null }>("transcript_delete", {
        sessionId: deleting, backupDirectory,
      });
      threadRequest.current += 1;
      setThread(null);
      setHits(current => current.filter(hit => !report.deletedSessions.includes(hit.sessionId)));
      setSessions(current => current.filter(session => !report.deletedSessions.includes(session.sessionId)));
      setDeleting(null);
      setNotice(`Deleted chat. Restore it with Import ${profile.archiveLabel} archive from ${report.path}.${report.warning ? ` ${report.warning}` : ""}`);
    } catch (error) {
      setDeleteError(`Could not delete chat. ${String(error)}`);
    } finally { setPending(false); }
  };

  return (
    <section
      className="content-pane transcript-pane"
      aria-labelledby="transcripts-title"
      tabIndex={0}
    >
      <div className="pane-heading">
        <div>
          <p className="eyebrow">LOCAL HISTORY</p>
          <h2 id="transcripts-title">Chats</h2>
          <p className="supporting-copy">
            Browse {label} chats by project and search messages.{profile.transcripts.archive === "migration" && " Export, import or convert native chats."}
          </p>
        </div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          {profile.transcripts.scope === "project" && <button
            className="secondary-action" type="button" disabled={busy}
            onClick={() => void invoke<string | null>("project_choose").then(selected => {
              if (selected) setProjectRoot(selected);
            }).catch(error => setNotice(`Could not choose a project. ${String(error)}`))}
          >Choose project</button>}
          <button
            className="secondary-action"
            type="button"
            onClick={() => void importArchive()}
            disabled={busy || profile.transcripts.archive !== "migration"}
            title={profile.transcripts.archive === "migration" ? undefined : profile.transcripts.note}
          >
            Import {profile.archiveLabel} archive
          </button>
          <button
            className="primary-action"
            type="button"
            onClick={() => void indexSource()}
            disabled={busy}
          >
            {pending ? "Working…" : "Refresh chats"}
          </button>
        </div>
      </div>
      {profile.transcripts.note && <p className="supporting-copy">{profile.transcripts.note}</p>}
      {source && <p className="quiet-copy">Chat source: {source}</p>}
      {profile.transcripts.scope === "project" && <p className="quiet-copy">Indexing project: {projectRoot ?? "Launch folder"}</p>}
      <p className="transcript-notice" role="status">
        {notice}
      </p>
      <form
        className="transcript-search"
        onSubmit={(event) => void search(event)}
      >
        <label htmlFor="transcript-query">Search message text</label>
        <div className="search-row">
          <input
            id="transcript-query"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search across indexed sessions"
          />
          <button type="submit" disabled={busy}>
            Search
          </button>
        </div>
      </form>
      <div className="transcript-layout">
        <aside className="session-list" aria-label="Transcript sessions" tabIndex={0}>
          <h3>Projects {sessionState === "ready" && <span>{groups.length} · {sessions.length} chats</span>}</h3>
          <label className="chat-filter">Filter projects and chats
            <input value={filter} onChange={event => setFilter(event.target.value)} placeholder="Project, chat name or ID" />
          </label>
          {sessionState === "loading" ? (
            <p className="quiet-copy" role="status">
              Finding chats…
            </p>
          ) : sessionState === "error" ? null : sessions.length === 0 ? (
            <p className="quiet-copy">No chats found in this source.</p>
          ) : (
            groups.map(group => <details className="chat-project" key={group.key} open>
              <summary><strong>{group.project.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || group.project}</strong><span>{group.sessions.length} chats</span></summary>
              <p className="chat-project-path">{group.project}</p>
              {group.sessions.map((session) => (
              <button
                key={session.sessionId}
                className={`session-card ${selectedSession === session.sessionId ? "selected" : ""}`}
                onClick={() => void selectSession(session.sessionId)}
              >
                <strong>{session.title ?? session.sessionId}</strong>
                <span>
                  {session.messageCount.toLocaleString()}{" "}
                  messages
                </span>
              </button>
              ))}
            </details>)
          )}
          {sessionState === "ready" && sessions.length > 0 && groups.length === 0 && <p className="quiet-copy">No chats match this filter.</p>}
        </aside>
        <div className="transcript-results">
          {searched && (
            <section className="search-results" aria-label="Search results">
              <h3>
                Search results <span>{hits.length}</span>
              </h3>
              {hits.length === 0 && <p className="quiet-copy">No matches.</p>}
              {hits.length === SEARCH_LIMIT && (
                <p className="quiet-copy">
                  Showing the first {SEARCH_LIMIT} matches; narrow the query to
                  see more.
                </p>
              )}
              {hits.map((hit) => (
                <button
                  className="search-hit"
                  key={`${hit.sessionId}:${hit.uuid}`}
                  onClick={() => void selectSession(hit.sessionId, hit.uuid)}
                >
                  <strong>
                    {hit.project} · {hit.sessionId} · line {hit.position}
                  </strong>
                  <span>
                    {hit.before && `${hit.before.slice(0, 140)} … `}
                    <mark>{hit.text.slice(0, 260)}</mark>
                    {hit.after && ` … ${hit.after.slice(0, 140)}`}
                  </span>
                </button>
              ))}
            </section>
          )}
          <div className="thread-heading">
            <h3>
              {selectedSession ? `Session ${selectedSession}` : "Thread view"}
            </h3>
            {selectedSession && (
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                <button type="button" className="secondary-action" disabled={busy}
                  onClick={() => setConverting(selectedSession)}>Convert chat</button>
                <button type="button" className="secondary-action" disabled={busy || profile.transcripts.archive !== "migration"}
                  title={profile.transcripts.archive === "migration" ? undefined : "Deletion requires a chat archive that Orbit can restore."}
                  onClick={() => setDeleting(selectedSession)}>Delete chat</button>
                <button
                  type="button"
                  className="secondary-action"
                  onClick={() => void exportArchive()}
                  disabled={busy}
                  title={profile.transcripts.archive === "backup" ? profile.transcripts.note : undefined}
                >
                  Export {profile.archiveLabel} {profile.transcripts.archive === "backup" ? "source backup" : "archive"}
                </button>
                <button
                  type="button"
                  className="secondary-action"
                  onClick={() => void exportSession()}
                  disabled={busy}
                >
                  Export Markdown
                </button>
              </div>
            )}
          </div>
          {!selectedSession && (
            <p className="quiet-copy">
              Choose a session or search the indexed history.
            </p>
          )}
          <div className="thread-list" role="region" tabIndex={0} aria-label="Transcript messages">
            {messages.map((message) => (
              <article
                className="message-card"
                data-message-uuid={message.uuid}
                key={message.uuid}
                style={{ marginLeft: `${Math.min(message.depth, 5) * 12}px` }}
              >
                <header>
                  <span className="message-role">{message.messageType}</span>
                  <span>
                    #{message.position}
                    {message.branchIndex > 0
                      ? ` · branch ${message.branchIndex + 1}`
                      : ""}
                  </span>
                </header>
                {message.text && <pre tabIndex={0}>{message.text}</pre>}
                {message.attachment && (
                  <details>
                    <summary>Attachment</summary>
                    <pre tabIndex={0}>{message.attachment}</pre>
                  </details>
                )}
                {message.toolResult && (
                  <details>
                    <summary>Tool result</summary>
                    <pre tabIndex={0}>{message.toolResult}</pre>
                  </details>
                )}
                {message.toolResultNote && (
                  <p className="inline-note">{message.toolResultNote}</p>
                )}
              </article>
            ))}
          </div>
          {thread?.hasMore && (
            <button
              className="load-more"
              type="button"
              onClick={() => void loadMore()}
              disabled={busy}
            >
              Load next 100 messages
            </button>
          )}
        </div>
      </div>
      {converting && <ChatConversion sessionId={converting} onClose={() => setConverting(null)} onConverted={setNotice} />}
      <dialog ref={deleteDialog} className="edit-dialog" aria-labelledby="delete-chat-title"
        onCancel={event => { event.preventDefault(); if (!pending) setDeleting(null); }}>
        <h3 id="delete-chat-title">Delete this chat?</h3>
        <p>{sessions.find(session => session.sessionId === deleting)?.title ?? deleting}</p>
        <p>Save a native archive first, then remove the chat from {label}. To restore it later, import that archive and choose its project.</p>
        <p>Close this chat in {label} before deleting it.</p>
        {deleteError && <p className="inline-error" role="alert">{deleteError}</p>}
        <div className="dialog-actions">
          <button type="button" disabled={pending} onClick={() => setDeleting(null)}>Cancel</button>
          <button type="button" disabled={pending} onClick={() => void deleteSession()}>Save archive and delete</button>
        </div>
      </dialog>
    </section>
  );
}
