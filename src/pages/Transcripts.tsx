import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Session = { sessionId: string; project: string; updatedAt: string | null; messageCount: number };
type Message = { uuid: string; parentUuid: string | null; messageType: string; text: string; position: number; depth: number; branchIndex: number; attachment: string | null; toolResult: string | null; toolResultNote: string | null };
type SearchHit = { sessionId: string; project: string; uuid: string; position: number; messageType: string; text: string; before: string | null; after: string | null };
type IngestReport = { sessions: number; linesRead: number; messagesIndexed: number; malformedLines: number; skippedFiles: number; skippedStateLines: number };
const PAGE_SIZE = 100;
const SEARCH_LIMIT = 50;

export function Transcripts() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionState, setSessionState] = useState<"loading" | "ready" | "error">("loading");
  const [selectedSession, setSelectedSession] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [offset, setOffset] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [searched, setSearched] = useState(false);
  const [focusUuid, setFocusUuid] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("Index the configured Claude Code projects directory to browse sessions.");

  const refreshSessions = useCallback(async () => {
    setSessionState("loading");
    try {
      const result = await invoke<Session[]>("transcript_list_sessions");
      setSessions(result);
      setSessionState("ready");
      return result;
    } catch (error) {
      setSessionState("error");
      throw error;
    }
  }, []);

  useEffect(() => { void refreshSessions().catch(error => setNotice(`Could not load indexed sessions. ${String(error)}`)); }, [refreshSessions]);
  useEffect(() => {
    if (focusUuid) document.querySelector(`[data-message-uuid="${CSS.escape(focusUuid)}"]`)?.scrollIntoView({ block: "center" });
  }, [focusUuid, messages]);

  const indexSource = async () => {
    setBusy(true);
    setNotice("Reading transcript files into the local index…");
    try {
      const report = await invoke<IngestReport>("transcript_ingest");
      const nextSessions = await refreshSessions();
      const skipped = [
        report.malformedLines > 0 ? `${report.malformedLines} malformed lines` : null,
        report.skippedStateLines > 0 ? `${report.skippedStateLines} state lines` : null,
        report.skippedFiles > 0 ? `${report.skippedFiles} unreadable files` : null,
      ].filter(Boolean).join(", ");
      setNotice(`Indexed ${report.messagesIndexed.toLocaleString()} messages from ${report.sessions} sessions.${skipped ? ` Skipped: ${skipped}.` : ""}`);
      if (!selectedSession && nextSessions[0]) await selectSession(nextSessions[0].sessionId);
    } catch (error) {
      setNotice(`Could not index transcripts. ${String(error)}`);
    } finally { setBusy(false); }
  };

  const selectSession = async (sessionId: string) => {
    setSelectedSession(sessionId);
    setOffset(0);
    setMessages([]);
    setBusy(true);
    try {
      const page = await invoke<Message[]>("transcript_thread_page", { sessionId, offset: 0, limit: PAGE_SIZE });
      setMessages(page);
      setHasMore(page.length === PAGE_SIZE);
    } catch (error) { setNotice(`Could not open this session. ${String(error)}`); }
    finally { setBusy(false); }
  };

  const loadMore = async () => {
    if (!selectedSession || busy) return;
    setBusy(true);
    try {
      const page = await invoke<Message[]>("transcript_thread_page", { sessionId: selectedSession, offset: offset + messages.length, limit: PAGE_SIZE });
      setMessages((current) => [...current, ...page]);
      setHasMore(page.length === PAGE_SIZE);
    } catch (error) { setNotice(`Could not load more messages. ${String(error)}`); }
    finally { setBusy(false); }
  };

  const search = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!query.trim()) { setHits([]); setSearched(false); return; }
    setBusy(true);
    try { setHits(await invoke<SearchHit[]>("transcript_search", { query, limit: SEARCH_LIMIT })); setSearched(true); }
    catch (error) { setNotice(`Search failed. ${String(error)}`); }
    finally { setBusy(false); }
  };

  const jumpToHit = async (hit: SearchHit) => {
    await selectSession(hit.sessionId);
    setBusy(true);
    try {
      const pageOffset = await invoke<number | null>("transcript_message_page_offset", { sessionId: hit.sessionId, uuid: hit.uuid, pageSize: PAGE_SIZE });
      if (pageOffset === null) {
        setNotice(`The matched message is no longer part of session ${hit.sessionId}. The index may be stale for this session; re-index to refresh it.`);
      } else if (pageOffset !== 0) {
        const page = await invoke<Message[]>("transcript_thread_page", { sessionId: hit.sessionId, offset: pageOffset, limit: PAGE_SIZE });
        setMessages(page);
        setOffset(pageOffset);
        setHasMore(page.length === PAGE_SIZE);
      }
      setFocusUuid(hit.uuid);
    } catch (error) { setNotice(`Could not jump to this message. ${String(error)}`); }
    finally { setBusy(false); }
  };

  const exportSession = async () => {
    if (!selectedSession) return;
    try {
      const markdown = await invoke<string>("transcript_export_markdown", { sessionId: selectedSession });
      const url = URL.createObjectURL(new Blob([markdown], { type: "text/markdown;charset=utf-8" }));
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `${selectedSession.split("/").pop() ?? selectedSession}.md`;
      anchor.click();
      URL.revokeObjectURL(url);
      setNotice("Markdown export downloaded.");
    } catch (error) { setNotice(`Export failed. ${String(error)}`); }
  };

  return <section className="content-pane transcript-pane" aria-labelledby="transcripts-title">
    <div className="pane-heading">
      <div><p className="eyebrow">LOCAL HISTORY</p><h2 id="transcripts-title">Transcripts</h2><p className="supporting-copy">Browse Claude Code sessions, follow branches, and export a readable record.</p></div>
      <button className="primary-action" type="button" onClick={() => void indexSource()} disabled={busy}>{busy ? "Working…" : "Index transcripts"}</button>
    </div>
    <p className="transcript-notice" role="status">{notice}</p>
    <form className="transcript-search" onSubmit={(event) => void search(event)}>
      <label htmlFor="transcript-query">Search message text</label>
      <div className="search-row"><input id="transcript-query" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search across indexed sessions" /><button type="submit" disabled={busy}>Search</button></div>
    </form>
    <div className="transcript-layout">
      <aside className="session-list" aria-label="Transcript sessions">
        <h3>Sessions <span>{sessions.length}</span></h3>
        {sessionState === "loading" ? <p className="quiet-copy" role="status">Loading sessions…</p> : sessionState === "error" ? null : sessions.length === 0 ? <p className="quiet-copy">No sessions indexed yet.</p> : sessions.map((session) => <button key={session.sessionId} className={`session-card ${selectedSession === session.sessionId ? "selected" : ""}`} onClick={() => void selectSession(session.sessionId)}>
          <strong>{session.sessionId}</strong><span>{session.project} · {session.messageCount.toLocaleString()} messages</span>
        </button>)}
      </aside>
      <div className="transcript-results">
        {searched && <section className="search-results" aria-label="Search results">
          <h3>Search results <span>{hits.length}</span></h3>
          {hits.length === 0 && <p className="quiet-copy">No matches.</p>}
          {hits.length === SEARCH_LIMIT && <p className="quiet-copy">Showing the first {SEARCH_LIMIT} matches; narrow the query to see more.</p>}
          {hits.map((hit) => <button className="search-hit" key={`${hit.sessionId}:${hit.uuid}`} onClick={() => void jumpToHit(hit)}><strong>{hit.project} · {hit.sessionId} · line {hit.position}</strong><span>{hit.before && `${hit.before.slice(0, 140)} … `}<mark>{hit.text.slice(0, 260)}</mark>{hit.after && ` … ${hit.after.slice(0, 140)}`}</span></button>)}
        </section>}
        <div className="thread-heading"><h3>{selectedSession ? `Session ${selectedSession}` : "Thread view"}</h3>{selectedSession && <button type="button" className="secondary-action" onClick={() => void exportSession()}>Export Markdown</button>}</div>
        {!selectedSession && <p className="quiet-copy">Choose a session or search the indexed history.</p>}
        <div className="thread-list">{messages.map((message) => <article className="message-card" data-message-uuid={message.uuid} key={message.uuid} style={{ marginLeft: `${Math.min(message.depth, 5) * 12}px` }}>
          <header><span className="message-role">{message.messageType}</span><span>#{message.position}{message.branchIndex > 0 ? ` · branch ${message.branchIndex + 1}` : ""}</span></header>
          {message.text && <pre>{message.text}</pre>}
          {message.attachment && <details><summary>Attachment</summary><pre>{message.attachment}</pre></details>}
          {message.toolResult && <details open><summary>Tool result</summary><pre>{message.toolResult}</pre></details>}
          {message.toolResultNote && <p className="inline-note">{message.toolResultNote}</p>}
        </article>)}</div>
        {hasMore && <button className="load-more" type="button" onClick={() => void loadMore()} disabled={busy}>Load next 100 messages</button>}
      </div>
    </div>
  </section>;
}
