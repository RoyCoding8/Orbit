import { useCallback, useEffect, useRef, useState } from "react";
import { useHarness } from "../HarnessContext";
import { open } from "@tauri-apps/plugin-dialog";
import { Channel } from "@tauri-apps/api/core";
import { displayPath } from "../pathDisplay";
import { ChatConversion } from "./ChatConversion";

type Session = {
  sessionId: string;
  title: string | null;
  project: string;
  updatedAt: string | null;
  messageCount: number;
  indexed: boolean;
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
type IndexProgress = { completed: number; total: number; failed: number; title: string | null };
type Indexing = { kind: "idle" } | { kind: "running"; progress: IndexProgress | null };

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
    const path = displayPath(project).replaceAll("\\", "/").replace(/\/+$/, "") || "/";
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
  const threadRequest = useRef<{ sessionId: string; loading: boolean } | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [searched, setSearched] = useState(false);
  const [pending, setPending] = useState(false);
  const [discovering, setDiscovering] = useState(true);
  const [indexing, setIndexing] = useState<Indexing>({ kind: "idle" });
  const indexRequest = useRef(0);
  const [converting, setConverting] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState("");
  const deleteDialog = useRef<HTMLDialogElement>(null);
  const [notice, setNotice] = useState(`Finding ${label} chats…`);
  const selectedSession = thread?.sessionId ?? null;
  const messages = thread?.messages ?? [];
  const focusUuid = thread?.focusUuid;
  const busy = pending || sessionState === "loading" || thread?.loading === true;

  const normalizedFilter = displayPath(filter.trim()).toLowerCase();
  const groups = groupSessions(sessions.filter(session =>
    displayPath(`${session.project} ${session.title ?? ""} ${session.sessionId}`).toLowerCase().includes(normalizedFilter)));

  const refreshSessions = useCallback(async (refresh = false, projectRoot: string | null = null) => {
    const request = ++sessionRequest.current;
    setSessionState(current => current === "ready" ? current : "loading");
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

  const selectSession = useCallback(async (sessionId: string, uuid?: string) => {
    const request = { sessionId, loading: true };
    threadRequest.current = request;
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
      const catalogRequest = sessionRequest.current;
      const catalog = await invoke<SessionCatalog>("transcript_list_sessions", { refresh: false, projectRoot: null });
      if (catalogRequest === sessionRequest.current) setSessions(catalog.sessions);
    } catch (error) {
      if (request === threadRequest.current)
        setNotice(`Could not open this session. ${String(error)}`);
    } finally {
      request.loading = false;
      if (request === threadRequest.current)
        setThread((current) => current && { ...current, loading: false });
    }
  }, [invoke]);

  const refreshThread = useCallback(async (nextSessions: Session[], selection = threadRequest.current) => {
    if (!selection || selection !== threadRequest.current) return;
    const next = nextSessions.find(session => session.sessionId === selection.sessionId);
    if (next) {
      if (!selection.loading) await selectSession(next.sessionId);
    }
    else {
      threadRequest.current = null;
      setThread(null);
    }
  }, [selectSession]);

  useEffect(() => {
    let active = true;
    setDiscovering(true);
    void (async () => {
      try {
        const cached = await refreshSessions();
        if (!active || !cached) return;
        setNotice(cached.length ? `Updating ${label} chats…` : `Finding ${label} chats…`);
        const nextSessions = await refreshSessions(true);
        if (active && nextSessions && threadRequest.current) await refreshThread(nextSessions);
      } catch (error) {
        if (active) setNotice(`Could not find chats. ${String(error)}`);
      } finally {
        if (active) setDiscovering(false);
      }
    })();
    return () => {
      active = false;
      sessionRequest.current += 1;
      threadRequest.current = null;
      indexRequest.current += 1;
    };
  }, [refreshSessions, refreshThread, label]);
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

  const discover = async () => {
    const selection = threadRequest.current;
    setDiscovering(true);
    try {
      const nextSessions = await refreshSessions(true, projectRoot);
      setHits([]);
      setSearched(false);
      if (nextSessions) await refreshThread(nextSessions, selection);
    } catch (error) {
      setNotice(`Could not refresh chats. ${String(error)}`);
    } finally { setDiscovering(false); }
  };

  const indexSource = async () => {
    const request = ++indexRequest.current;
    setIndexing({ kind: "running", progress: null });
    const onProgress = new Channel<IndexProgress>(progress => {
      if (request === indexRequest.current) setIndexing({ kind: "running", progress });
    });
    try {
      const report = await invoke<IngestReport>("transcript_ingest", { projectRoot, onProgress });
      if (request !== indexRequest.current) return;
      await refreshSessions();
      setHits([]);
      setSearched(false);
      setNotice(`Indexed ${report.sessions} chats for message search.${report.skippedFiles ? ` Could not index ${report.skippedFiles} sources; retry indexing to try them again.` : ""}${report.malformedLines ? ` Skipped ${report.malformedLines} malformed records.` : ""}`);
    } catch (error) {
      if (request === indexRequest.current) setNotice(`Indexing stopped. Completed chats remain searchable. ${String(error)}`);
    } finally {
      onProgress.onmessage = () => {};
      if (request === indexRequest.current) setIndexing({ kind: "idle" });
    }
  };

  const loadMore = async () => {
    if (!thread || busy) return;
    const request = { sessionId: thread.sessionId, loading: true };
    threadRequest.current = request;
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
      request.loading = false;
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
      threadRequest.current = null;
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
            onClick={() => void discover()}
            disabled={busy || discovering || indexing.kind === "running"}
            aria-busy={discovering}
          >
            Refresh chats
          </button>
          <button className="secondary-action" type="button" onClick={() => void indexSource()}
            disabled={busy || discovering || indexing.kind === "running" || sessions.length === 0}>
            Index chats
          </button>
        </div>
      </div>
      {profile.transcripts.note && <p className="supporting-copy">{profile.transcripts.note}</p>}
      {source && <p className="quiet-copy">Chat source: {displayPath(source)}</p>}
      {profile.transcripts.scope === "project" && <p className="quiet-copy">Additional project: {displayPath(projectRoot ?? "Launch folder")}</p>}
      <p className="transcript-notice" role="status">
        {displayPath(notice)}
      </p>
      {indexing.kind === "running" && <div className="chat-index-progress" role="status">
        <progress aria-label="Chat indexing" max={indexing.progress?.total || 1} value={indexing.progress?.total ? indexing.progress.completed : undefined} />
        <span>{indexing.progress ? `${indexing.progress.completed} of ${indexing.progress.total} chats processed${indexing.progress.failed ? ` · ${indexing.progress.failed} failed` : ""}` : "Preparing chat index…"}</span>
        {indexing.progress?.title && <small>{indexing.progress.title}</small>}
      </div>}
      <p className="quiet-copy">{sessions.filter(session => session.indexed).length} of {sessions.length} chats indexed. Message search covers indexed chats. Open a chat to load it, or index all chats for search.</p>
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
          {sessionState === "loading" || (discovering && sessions.length === 0) ? (
            <p className="quiet-copy" role="status">
              Finding chats…
            </p>
          ) : sessionState === "error" && sessions.length === 0 ? null : sessions.length === 0 ? (
            <p className="quiet-copy">No chats found in this source.</p>
          ) : (
            groups.map(group => <details className="chat-project" key={group.key} open>
              <summary><strong>{group.project.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || group.project}</strong><span>{group.sessions.length} chats</span></summary>
              <p className="chat-project-path">{displayPath(group.project)}</p>
              {group.sessions.map((session) => (
              <button
                key={session.sessionId}
                className={`session-card ${selectedSession === session.sessionId ? "selected" : ""}`}
                onClick={() => void selectSession(session.sessionId)}
              >
                <strong>{session.title ?? session.sessionId}</strong>
                <span>
                  {session.indexed ? `${session.messageCount.toLocaleString()} ${session.messageCount === 1 ? "message" : "messages"} · Indexed` : "Not indexed"}
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
                    {displayPath(hit.project)} · {hit.sessionId} · line {hit.position}
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
                <button type="button" className="secondary-action" disabled={busy || indexing.kind === "running" || profile.transcripts.archive !== "migration"}
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
            {thread?.loading && <p role="status" className="quiet-copy">Loading chat…</p>}
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
                  <p className="inline-note">{displayPath(message.toolResultNote)}</p>
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
