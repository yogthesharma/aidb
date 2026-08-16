import { useEffect, useRef, useState } from "react";
import {
  addDocument,
  chatStream,
  getStatus,
  listDocuments,
  listSessions,
  listTurns,
  probe,
} from "./api.js";

function newSession() {
  return `chat:${crypto.randomUUID().slice(0, 8)}`;
}

function titleOf(session, sessions) {
  const known = sessions.find((item) => item.id === session);
  if (known?.turns) {
    return session.replace(/^chat:/, "");
  }
  return session.replace(/^chat:/, "");
}

export function App() {
  const [health, setHealth] = useState("down");
  const [session, setSession] = useState(() => newSession());
  const [sessions, setSessions] = useState([]);
  const [file, setFile] = useState(null);
  const [docs, setDocs] = useState([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState("");
  const [messages, setMessages] = useState([]);
  const [knowledge, setKnowledge] = useState(false);
  const [docTitle, setDocTitle] = useState("");
  const [docBody, setDocBody] = useState("");
  const thread = useRef(null);
  const box = useRef(null);

  async function refresh() {
    try {
      const [nextFile, nextSessions, nextDocs] = await Promise.all([
        getStatus(),
        listSessions(),
        listDocuments(),
      ]);
      setFile(nextFile);
      setSessions(nextSessions);
      setDocs(nextDocs);
    } catch (err) {
      setFile(null);
      setError(err.message);
    }
  }

  async function loadThread(id) {
    const turns = await listTurns(id);
    const next = [];
    for (const turn of turns) {
      if (turn.user) {
        next.push({ role: "user", text: turn.user });
      }
      next.push({
        role: "assistant",
        text: turn.assistant,
        sources: turn.sources,
        runId: turn.runId,
      });
    }
    setMessages(next);
  }

  useEffect(() => {
    let stop = false;
    async function tick() {
      const next = await probe();
      if (stop) {
        return;
      }
      setHealth(next);
      if (next === "ok") {
        refresh();
      } else {
        setFile(null);
      }
    }
    tick();
    const id = window.setInterval(tick, 5000);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, []);

  useEffect(() => {
    if (health !== "ok") {
      return;
    }
    loadThread(session).catch(() => setMessages([]));
  }, [session, health]);

  useEffect(() => {
    thread.current?.scrollTo(0, thread.current.scrollHeight);
  }, [messages, busy]);

  async function onSend(event) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || busy) {
      return;
    }
    setError("");
    setBusy(true);
    setDraft("");
    setMessages((prev) => [
      ...prev,
      { role: "user", text },
      { role: "assistant", text: "", streaming: true },
    ]);
    try {
      const result = await chatStream(
        { session, text },
        {
          onToken: (event) => {
            const chunk = event.text || "";
            if (!chunk) {
              return;
            }
            setMessages((prev) => {
              const next = [...prev];
              const last = next[next.length - 1];
              if (last?.role !== "assistant") {
                return prev;
              }
              next[next.length - 1] = { ...last, text: `${last.text}${chunk}` };
              return next;
            });
          },
        }
      );
      setMessages((prev) => {
        const next = [...prev];
        const last = next[next.length - 1];
        if (last?.role !== "assistant") {
          return prev;
        }
        next[next.length - 1] = {
          role: "assistant",
          text: result.answer || last.text,
          sources: result.sources,
          runId: result.runId,
        };
        return next;
      });
      await refresh();
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
      box.current?.focus();
    }
  }

  async function onAddDoc(event) {
    event.preventDefault();
    const content = docBody.trim();
    if (!content) {
      return;
    }
    setError("");
    try {
      await addDocument({ title: docTitle.trim() || "Untitled", content });
      setDocTitle("");
      setDocBody("");
      setKnowledge(false);
      await refresh();
    } catch (err) {
      setError(err.message);
    }
  }

  function onKeyDown(event) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      event.currentTarget.form?.requestSubmit();
    }
  }

  const empty = messages.length === 0 && !busy;

  return (
    <div className="app">
      <aside className="rail">
        <button className="new" onClick={() => setSession(newSession())}>
          New chat
        </button>
        <div className="chats">
          {sessions.map((item) => (
            <button
              key={item.id}
              className={session === item.id ? "active" : ""}
              onClick={() => setSession(item.id)}
            >
              {titleOf(item.id, sessions)}
            </button>
          ))}
        </div>
        <div className="rail-foot">
          <button className="ghost" onClick={() => setKnowledge((v) => !v)}>
            Knowledge {file?.docs ? `(${file.docs})` : ""}
          </button>
          {file ? (
            <div className="usage">
              <div className="stat">
                <span>llm</span>
                <b>
                  {file.provider}/{file.model}
                </b>
              </div>
              <div className="stat">
                <span>embed</span>
                <b>
                  {file.embedProvider || "—"}/{file.embedDims || "—"}d
                </b>
              </div>
              <div className="stat">
                <span>index</span>
                <b>
                  {file.docs} docs · {file.chunks} chunks
                </b>
              </div>
              <div className="stat">
                <span>vectors</span>
                <b>{file.vectors}</b>
              </div>
              <div className="stat">
                <span>tokens</span>
                <b>
                  {file.promptTokens} in / {file.completionTokens} out
                </b>
              </div>
              <div className="stat">
                <span>runs</span>
                <b>
                  {file.generates} gen · {file.embeds} embed
                </b>
              </div>
              <div className="stat">
                <span>spend</span>
                <b>${file.spend}</b>
              </div>
              {file.process ? (
                <>
                  <div className="stat">
                    <span>rss</span>
                    <b>
                      {file.process.rssMb} MB · cpu {file.process.cpuPct}%
                    </b>
                  </div>
                  <div className="stat">
                    <span>file</span>
                    <b>
                      {file.process.fileMb} MB · {file.process.uptimeSec}s
                    </b>
                  </div>
                </>
              ) : null}
            </div>
          ) : null}
          <p className={`health ${health}`}>
            {health === "ok" ? `${file?.name || "Ada"} · live` : "backend down"}
          </p>
        </div>
      </aside>

      <main className="main">
        {health !== "ok" ? (
          <p className="err">
            Backend is not on :8092. From the repo root run{" "}
            <code>pnpm example:chat</code>.
          </p>
        ) : null}
        {error ? <p className="err">{error}</p> : null}

        {knowledge ? (
          <div className="panel">
            <p className="lede">
              Optional. Paste your own text into the file. Later questions will
              search it. The file starts empty.
            </p>
            <form onSubmit={onAddDoc}>
              <input
                value={docTitle}
                onChange={(e) => setDocTitle(e.target.value)}
                placeholder="Title"
              />
              <textarea
                value={docBody}
                onChange={(e) => setDocBody(e.target.value)}
                placeholder="Paste anything you want the chat to know"
              />
              <button className="primary" type="submit">
                Add to file
              </button>
            </form>
            {docs.length ? (
              <ul className="docs">
                {docs.map((doc) => (
                  <li key={doc.id}>
                    {doc.title} <span>{doc.bytes}b</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="muted">No documents yet.</p>
            )}
          </div>
        ) : null}

        <div className="thread" ref={thread}>
          {empty ? (
            <div className="hero">
              <h1>Ada</h1>
              <p>Lives in this file. Type anything.</p>
            </div>
          ) : (
            messages.map((message, index) => (
              <div className={`row ${message.role}`} key={`${message.runId || "m"}-${index}`}>
                <div className="bubble">
                  <p>
                    {message.text}
                    {message.streaming ? <span className="cursor" /> : null}
                  </p>
                  {message.sources?.length ? (
                    <ul className="sources">
                      {message.sources.map((source) => (
                        <li key={`${source.document_id}-${source.chunk_id}`}>
                          {source.title || source.document_id}
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </div>
              </div>
            ))
          )}
        </div>

        <form className="composer" onSubmit={onSend}>
          <textarea
            ref={box}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Message"
            rows={1}
            disabled={health !== "ok"}
          />
          <button className="send" disabled={busy || !draft.trim()}>
            Send
          </button>
        </form>
      </main>
    </div>
  );
}
