import { useEffect, useMemo, useRef, useState } from "react";
import {
  ask,
  brief,
  classify,
  digest,
  getStatus,
  listRuns,
  listTickets,
  listTurns,
  listWaiting,
  probe,
  remember,
  resume,
} from "./api.js";

const TABS = [
  ["ask", "Ask"],
  ["tickets", "Tickets"],
  ["approvals", "Approvals"],
  ["file", "The file"],
];

const SAMPLE = "How long do I have to return unused headphones?";

export function App() {
  const [tab, setTab] = useState("ask");
  const [health, setHealth] = useState("down");
  const [agent, setAgent] = useState("maya");
  const [dept, setDept] = useState("");
  const [file, setFile] = useState(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const refreshing = useRef(false);

  async function refreshFile() {
    if (refreshing.current) {
      return;
    }
    refreshing.current = true;
    try {
      const next = await getStatus();
      setFile(next);
    } catch (err) {
      setFile(null);
      setError(err.message);
    } finally {
      refreshing.current = false;
    }
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
        refreshFile();
      } else {
        setFile(null);
      }
    }
    tick();
    const id = window.setInterval(tick, 4000);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, []);

  return (
    <div className="app">
      <aside className="rail">
        <p className="brand">Harbor</p>
        <p className="tag">Support desk on one AIDB file</p>
        <nav>
          {TABS.map(([id, label]) => (
            <button
              key={id}
              className={tab === id ? "active" : ""}
              onClick={() => setTab(id)}
            >
              {label}
              {id === "approvals" && file?.waiting > 0
                ? ` (${file.waiting})`
                : ""}
            </button>
          ))}
        </nav>
        <p className={`health ${health}`}>
          api <b>{health}</b>
          <br />
          file <b>{file ? "open" : "—"}</b>
        </p>
      </aside>

      <main className="main">
        {health !== "ok" ? (
          <p className="err">
            Harbor backend is not on :8091. Ctrl+C, then from the repo root run{" "}
            <code>pnpm example:support</code>.
          </p>
        ) : null}
        {error ? <p className="err">{error}</p> : null}
        {health === "ok" && tab === "ask" ? (
          <Ask
            agent={agent}
            setAgent={setAgent}
            dept={dept}
            setDept={setDept}
            busy={busy}
            setBusy={setBusy}
            onDone={refreshFile}
            setError={setError}
          />
        ) : null}
        {health === "ok" && tab === "tickets" ? (
          <Tickets
            busy={busy}
            setBusy={setBusy}
            onDone={refreshFile}
            setError={setError}
          />
        ) : null}
        {health === "ok" && tab === "approvals" ? (
          <Approvals onDone={refreshFile} setError={setError} />
        ) : null}
        {health === "ok" && tab === "file" ? <FileView file={file} /> : null}
      </main>

      <aside className="side">
        <h2>This file</h2>
        {file ? (
          <>
            <div className="stat">
              <span>schema</span>
              <b>{file.version}</b>
            </div>
            <div className="stat">
              <span>model</span>
              <b>
                {file.provider}/{file.model}
              </b>
            </div>
            <div className="stat">
              <span>policies</span>
              <b>{file.docs}</b>
            </div>
            <div className="stat">
              <span>tickets</span>
              <b>{file.tickets}</b>
            </div>
            <div className="stat">
              <span>spend</span>
              <b>${file.spend}</b>
            </div>
            <div className="stat">
              <span>waiting</span>
              <b>{file.waiting}</b>
            </div>
          </>
        ) : (
          <p className="tag">Waiting for Harbor API…</p>
        )}
        <h2 style={{ marginTop: 28 }}>Session</h2>
        <p className="tag">
          Agent runs bind <code>support:{agent || "maya"}</code>. Cited answers
          are generate-over-search. There is no conversations table.
        </p>
      </aside>
    </div>
  );
}

function Ask({
  agent,
  setAgent,
  dept,
  setDept,
  busy,
  setBusy,
  onDone,
  setError,
}) {
  const [question, setQuestion] = useState(SAMPLE);
  const [memory, setMemory] = useState("");
  const [turns, setTurns] = useState([]);
  const session = `support:${agent.trim() || "maya"}`;

  async function loadTurns() {
    const data = await listTurns(session);
    setTurns(data);
  }

  useEffect(() => {
    loadTurns().catch(() => {});
  }, [session]);

  async function onAsk(event) {
    event.preventDefault();
    const text = question.trim();
    if (!text) {
      return;
    }
    setError("");
    setBusy(true);
    try {
      const result = await ask({
        question: text,
        agent: agent.trim() || "maya",
        dept,
      });
      setTurns((prev) => [
        ...prev,
        {
          turn: "ask",
          kind: "generate",
          status: "succeeded",
          question: text,
          output: result.answer,
          sources: result.sources ?? [],
        },
      ]);
      setQuestion("");
      await onDone();
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  }

  async function onRemember(event) {
    event.preventDefault();
    const text = memory.trim();
    if (!text) {
      return;
    }
    setError("");
    try {
      await remember({ agent: agent.trim() || "maya", content: text });
      setMemory("");
      await onDone();
    } catch (err) {
      setError(err.message);
    }
  }

  async function onBrief() {
    setError("");
    setBusy(true);
    try {
      const result = await brief({
        agent: agent.trim() || "maya",
        goal: question.trim() || "Summarize the refund window for a customer.",
      });
      setTurns((prev) => [
        ...prev,
        {
          turn: "agent",
          kind: "agent",
          status: result.status,
          output: result.output,
          runId: result.run_id,
        },
      ]);
      await onDone();
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  }

  async function onDigest() {
    setError("");
    setBusy(true);
    try {
      const result = await digest({ agent: agent.trim() || "maya" });
      setTurns((prev) => [
        ...prev,
        {
          turn: "digest",
          kind: "agent",
          status: result.status,
          output: result.output,
          runId: result.run_id,
        },
      ]);
      await onDone();
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <h1>Ask the desk</h1>
      <p className="lede">
        The browser posts JSON. Fastify runs generate-over-search against the
        file and returns the cited answer.
      </p>
      <div className="row">
        <label className="field">
          agent
          <input value={agent} onChange={(e) => setAgent(e.target.value)} />
        </label>
        <label className="field">
          dept filter
          <select value={dept} onChange={(e) => setDept(e.target.value)}>
            <option value="">all</option>
            <option value="billing">billing</option>
            <option value="shipping">shipping</option>
            <option value="account">account</option>
          </select>
        </label>
      </div>
      <form className="composer" onSubmit={onAsk}>
        <textarea
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          placeholder="Ask from the policy corpus"
        />
        <div className="row">
          <button className="primary" disabled={busy}>
            {busy ? "Running in the file…" : "Ask with citations"}
          </button>
          <button type="button" className="ghost" disabled={busy} onClick={onBrief}>
            Agent brief
          </button>
          <button type="button" className="ghost" disabled={busy} onClick={onDigest}>
            Email digest
          </button>
        </div>
      </form>
      <div className="thread">
        {[...turns].reverse().map((turn, index) => (
          <div className="bubble" key={`${turn.turn}-${index}`}>
            <div className="who">
              {turn.kind} {turn.status}
              {turn.runId ? ` · ${turn.runId}` : ""}
            </div>
            {turn.question ? <p>{turn.question}</p> : null}
            <p>{turn.output}</p>
            {turn.sources?.length ? (
              <ul className="sources">
                {turn.sources.map((source) => (
                  <li key={`${source.document_id}-${source.chunk_id}`}>
                    {source.title || source.document_id}
                  </li>
                ))}
              </ul>
            ) : null}
          </div>
        ))}
      </div>
      <form className="composer" onSubmit={onRemember}>
        <label className="field">
          remember for this agent (memory is documents, not a chat store)
          <input
            value={memory}
            onChange={(e) => setMemory(e.target.value)}
            placeholder="Prefer two-sentence answers"
          />
        </label>
        <button className="ghost">Remember</button>
      </form>
    </>
  );
}

function Tickets({ busy, setBusy, onDone, setError }) {
  const [subject, setSubject] = useState("Need a refund");
  const [body, setBody] = useState(
    "The headphones are unused and still in the box. I bought them 10 days ago."
  );
  const [items, setItems] = useState([]);

  async function load() {
    setItems(await listTickets());
  }

  useEffect(() => {
    load().catch((err) => setError(err.message));
  }, []);

  async function onClassify(event) {
    event.preventDefault();
    setError("");
    setBusy(true);
    try {
      await classify({ subject, body });
      await load();
      await onDone();
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <h1>Tickets</h1>
      <p className="lede">
        Classify writes a generate run, then the app inserts its own row and
        keeps the run id. That join is ordinary SQL — on the server.
      </p>
      <form className="composer" onSubmit={onClassify}>
        <input value={subject} onChange={(e) => setSubject(e.target.value)} />
        <textarea value={body} onChange={(e) => setBody(e.target.value)} />
        <button className="primary" disabled={busy}>
          {busy ? "Classifying…" : "Classify into tickets"}
        </button>
      </form>
      <div className="list">
        {items.map((item) => (
          <div className="card" key={item.id}>
            <h3>
              {item.subject}
              {item.label ? <span className="badge">{item.label}</span> : null}
            </h3>
            <p>{item.body}</p>
            <div className="meta">run {item.runId || "—"}</div>
          </div>
        ))}
      </div>
    </>
  );
}

function Approvals({ onDone, setError }) {
  const [items, setItems] = useState([]);
  const [busy, setBusy] = useState("");

  async function load() {
    setItems(await listWaiting());
  }

  useEffect(() => {
    load().catch((err) => setError(err.message));
  }, []);

  async function decide(id, approved) {
    setError("");
    setBusy(id);
    try {
      await resume({ runId: id, approved });
      await load();
      await onDone();
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy("");
    }
  }

  return (
    <>
      <h1>Approvals</h1>
      <p className="lede">
        Irreversible tools park. The file keeps the draft until a human
        resumes it. Kill the process — the row is still here.
      </p>
      {items.length === 0 ? (
        <p className="tag">Nothing waiting. Run an email digest from Ask.</p>
      ) : (
        <div className="list">
          {items.map((item) => (
            <div className="card" key={item.id}>
              <h3>
                {item.kind}
                <span className="badge wait">{item.status}</span>
              </h3>
              <p>{item.message}</p>
              <div className="meta">{item.id}</div>
              <div className="row" style={{ marginTop: 10 }}>
                <button
                  className="primary"
                  disabled={Boolean(busy)}
                  onClick={() => decide(item.id, true)}
                >
                  Approve
                </button>
                <button
                  className="danger"
                  disabled={Boolean(busy)}
                  onClick={() => decide(item.id, false)}
                >
                  Reject
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

function FileView({ file }) {
  const [runs, setRuns] = useState([]);

  useEffect(() => {
    listRuns()
      .then(setRuns)
      .catch(() => {});
  }, [file?.spend, file?.waiting]);

  const snippet = useMemo(
    () =>
      `SELECT id, kind, status, cost_usd FROM runs ORDER BY created_at_ms DESC LIMIT 10;
SELECT w.label, COUNT(*) FROM tickets w GROUP BY w.label;
SELECT turn, kind FROM session_turns ORDER BY created_at_ms;`,
    []
  );

  return (
    <>
      <h1>The file is the product</h1>
      <p className="lede">
        Same SQLite file the CLI, bindings, and Studio open. Copy it and you
        copied the audit trail. The SQL below is what Fastify runs — the
        browser never sends it.
      </p>
      <pre className="sql">{snippet}</pre>
      <div className="list" style={{ marginTop: 20 }}>
        {runs.map((run) => (
          <div className="card" key={run.id}>
            <h3>
              {run.kind}
              <span className={`badge ${String(run.status).includes("await") ? "wait" : "ok"}`}>
                {run.status}
              </span>
            </h3>
            <div className="meta">
              {run.id} · ${Number(run.cost).toFixed(6)}
              {run.session ? ` · ${run.session}` : ""}
            </div>
          </div>
        ))}
      </div>
    </>
  );
}
