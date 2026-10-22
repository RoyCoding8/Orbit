import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

type Session = {
  sessionId: string;
  title: string | null;
  project: string;
  updatedAt: string | null;
  messageCount: number;
};
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

export function Transcripts() {
  const [sessions, setSessions] = useState<Session[]>([]);
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
  const [notice, setNotice] = useState(
    "Index the configured Claude Code projects directory to browse sessions.",
  );
  const selectedSession = thread?.sessionId ?? null;
  const messages = thread?.messages ?? [];
  const focusUuid = thread?.focusUuid;
  const busy = pending || sessionState === "loading" || thread?.loading === true;

  const refreshSessions = useCallback(async () => {
    const request = ++sessionRequest.current;
    setSessionState("loading");
    try {
      const result = await invoke<Session[]>("transcript_list_sessions");
      if (request !== sessionRequest.current) return [];
      setSessions(result);
      setSessionState("ready");
      return result;
    } catch (error) {
      if (request !== sessionRequest.current) return [];
      setSessionState("error");
      throw error;
    }
  }, []);

  useEffect(() => {
    void refreshSessions().catch((error) =>
      setNotice(`Could not load indexed sessions. ${String(error)}`),
    );
    return () => {
      sessionRequest.current += 1;
      threadRequest.current += 1;
    };
  }, [refreshSessions]);
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
      const report = await invoke<IngestReport>("transcript_ingest");
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
        `Indexed ${report.messagesIndexed.toLocaleString()} messages from ${report.sessions} sessions.${skipped ? ` Skipped: ${skipped}.` : ""}`,
      );
      if (
        !selectedSession &&
        selection === threadRequest.current &&
        nextSessions[0]
      )
        await selectSession(nextSessions[0].sessionId);
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
        title: "Choose where to save the Claude session archive",
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
        title: "Choose a Claude session archive",
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

  return (
    <section
      className="content-pane transcript-pane"
      aria-labelledby="transcripts-title"
    >
      <div className="pane-heading">
        <div>
          <p className="eyebrow">LOCAL HISTORY</p>
          <h2 id="transcripts-title">Transcripts</h2>
          <p className="supporting-copy">
            Browse Claude Code sessions, follow branches, and export a readable
            record.
          </p>
        </div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button
            className="secondary-action"
            type="button"
            onClick={() => void importArchive()}
            disabled={busy}
          >
            Import Claude archive
          </button>
          <button
            className="primary-action"
            type="button"
            onClick={() => void indexSource()}
            disabled={busy}
          >
            {pending ? "Working…" : "Index transcripts"}
          </button>
        </div>
      </div>
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
        <aside className="session-list" aria-label="Transcript sessions">
          <h3>
            Sessions <span>{sessions.length}</span>
          </h3>
          {sessionState === "loading" ? (
            <p className="quiet-copy" role="status">
              Loading sessions…
            </p>
          ) : sessionState === "error" ? null : sessions.length === 0 ? (
            <p className="quiet-copy">No sessions indexed yet.</p>
          ) : (
            sessions.map((session) => (
              <button
                key={session.sessionId}
                className={`session-card ${selectedSession === session.sessionId ? "selected" : ""}`}
                onClick={() => void selectSession(session.sessionId)}
              >
                <strong>{session.title ?? session.sessionId}</strong>
                <span>
                  {session.project} · {session.messageCount.toLocaleString()}{" "}
                  messages
                </span>
              </button>
            ))
          )}
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
                <button
                  type="button"
                  className="secondary-action"
                  onClick={() => void exportArchive()}
                  disabled={busy}
                >
                  Export Claude archive
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
          <div className="thread-list">
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
                {message.text && <pre>{message.text}</pre>}
                {message.attachment && (
                  <details>
                    <summary>Attachment</summary>
                    <pre>{message.attachment}</pre>
                  </details>
                )}
                {message.toolResult && (
                  <details open>
                    <summary>Tool result</summary>
                    <pre>{message.toolResult}</pre>
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
    </section>
  );
}
