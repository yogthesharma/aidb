// Harbor desk operations. SQL lives here, not in the browser.
import { CORPUS } from "./corpus.mjs";
import {
  json,
  kimiKeyName,
  kimiModel,
  pauseMessage,
  scalar,
  sql,
} from "./load.mjs";

export const APP_SCHEMA = [
  `CREATE TABLE IF NOT EXISTS tickets (
     id            INTEGER PRIMARY KEY AUTOINCREMENT,
     subject       TEXT NOT NULL,
     body          TEXT NOT NULL,
     label         TEXT,
     run_id        TEXT,
     created_at_ms INTEGER NOT NULL
   )`,
];

const TOOLS = {
  tools: [
    {
      name: "send.email",
      inputs: { to: "string", subject: "string", body: "string" },
      side_effect: "irreversible",
      retry: "forbidden",
    },
  ],
};

const POLICY = {
  name: "harbor",
  allow: ["search", "generate", "send.email"],
  max_usd: 0.05,
  max_llm_calls: 16,
  require_approval: ["send.email"],
};

export async function init(db, { live }) {
  for (const statement of APP_SCHEMA) {
    await db.execute(statement);
  }
  if (live) {
    const model = kimiModel();
    const keyName = kimiKeyName();
    // Upsert, then UPDATE, so a leftover kimi-k2-turbo-preview row cannot win.
    await db.execute(
      `CREATE MODEL desk PROVIDER kimi KIND llm MODEL ${sql(model)} KEY_NAME ${sql(keyName)}`
    );
    await db.execute(
      `UPDATE models SET provider = 'kimi', provider_model = ${sql(model)}, key_name = ${sql(keyName)}
       WHERE kind = 'llm'`
    );
  } else {
    await db.execute("CREATE MODEL desk PROVIDER fake KIND llm");
  }
  await db.query(`SELECT aidb_mcp_register(${json(TOOLS)})`);
  await db.query(`SELECT aidb_set_policy(${json(POLICY)})`);
  return { provider: live ? "kimi" : "fake", model: live ? kimiModel() : "aidb-fake" };
}

export async function waitIndexed(db, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const pending = Number(
      await scalar(
        db,
        "SELECT COUNT(*) FROM documents WHERE index_status != 'ready'"
      )
    );
    if (pending === 0) {
      return;
    }
    if (Date.now() > deadline) {
      throw new Error(`${pending} document(s) never finished indexing`);
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

async function ingested(db) {
  const { rows } = await db.query(
    `SELECT DISTINCT json_extract(metadata_json, '$.source_id') FROM documents
     WHERE json_extract(metadata_json, '$.source_id') IS NOT NULL`
  );
  return new Set(rows.map((row) => String(row[0])));
}

export async function ingest(db) {
  const known = await ingested(db);
  const ids = [];
  let skipped = 0;
  for (const doc of CORPUS) {
    if (known.has(doc.sourceId)) {
      skipped += 1;
      continue;
    }
    const { id } = await db.documents.insert({
      title: doc.title,
      content: doc.content,
      metadata: { source_id: doc.sourceId, dept: doc.dept, kind: "policy" },
    });
    ids.push(id);
  }
  await waitIndexed(db);
  return { ids, skipped };
}

async function titleOf(db, documentId) {
  return scalar(db, `SELECT title FROM documents WHERE id = ${sql(documentId)}`);
}

export async function ask(db, question, { dept, agent, k = 4 } = {}) {
  const filter = {};
  if (dept) {
    filter.dept = dept;
  }
  const prefs = [];
  if (agent) {
    try {
      const found = await db.memory.search({
        query: question,
        scope: `user:${agent}`,
        limit: 3,
      });
      for (const row of found.rows ?? []) {
        const text = String(row[1] ?? row[0] ?? "");
        if (text) {
          prefs.push(text);
        }
      }
    } catch {
      // no memory yet
    }
  }
  const instructions = [
    "You are Harbor support. Answer only from the sources. Be concise.",
    ...prefs.map((p) => `Agent preference: ${p}`),
    `Question: ${question}`,
  ].join(" ");
  const answer = await scalar(
    db,
    `SELECT aidb_generate(${sql(instructions)}, content)
     FROM aidb_search(${sql(question)}, ${k}, ${json(filter)})`
  );
  let parsed;
  try {
    parsed = JSON.parse(answer);
  } catch {
    parsed = { answer, sources: [] };
  }
  const sources = [];
  for (const source of parsed.sources ?? []) {
    sources.push({ ...source, title: await titleOf(db, source.document_id) });
  }
  return { answer: parsed.answer ?? answer, sources };
}

export async function remember(db, agent, content) {
  const inserted = await db.memory.insert({
    scope: `user:${agent}`,
    content,
  });
  await waitIndexed(db);
  return inserted;
}

export async function brief(db, { goal, agent }) {
  return db.agent.run({
    instructions:
      "You are Harbor support. Use the policy documents. Write two short sentences. End with DONE.",
    goal: goal || "Summarize the refund window for a customer.",
    tools: ["search", "generate"],
    maxSteps: 3,
    k: 4,
    decide: true,
    session: `support:${agent || "maya"}`,
  });
}

export async function digest(db, { agent } = {}) {
  return db.agent.run({
    instructions:
      "You are Harbor support. Draft a short customer email from the policy documents, then send it. End with DONE.",
    goal: "Email the customer a two-sentence summary of the refund window.",
    tools: ["search", "generate", "send.email"],
    maxSteps: 4,
    k: 4,
    decide: true,
    session: `support:${agent || "maya"}`,
  });
}

export async function classifyTicket(db, { subject, body }) {
  const label = await scalar(
    db,
    `SELECT aidb_classify('billing or shipping or account or other', ${sql(body)})`
  );
  const runId = await scalar(db, "SELECT aidb_last_run_id()");
  await db.execute(
    `INSERT INTO tickets (subject, body, label, run_id, created_at_ms)
     VALUES (${sql(subject)}, ${sql(body)}, ${sql(label)}, ${sql(runId)}, ${Date.now()})`
  );
  return { label, runId };
}

export async function tickets(db) {
  const { rows } = await db.query(
    `SELECT id, subject, body, COALESCE(label, ''), COALESCE(run_id, ''), created_at_ms
     FROM tickets ORDER BY id DESC LIMIT 20`
  );
  return rows.map(([id, subject, body, label, runId, created]) => ({
    id,
    subject,
    body,
    label,
    runId,
    created,
  }));
}

export async function waiting(db) {
  const { rows } = await db.query(
    `SELECT id, kind, status, COALESCE(output_json, '') FROM runs
     WHERE status IN ('awaiting_approval', 'suspended')
     ORDER BY created_at_ms`
  );
  return rows.map(([id, kind, status, output]) => ({
    id,
    kind,
    status,
    message: pauseMessage(String(output)),
  }));
}

export async function resume(db, runId, approved) {
  return db.runs.resume(runId, { approved });
}

export async function turns(db, session) {
  const { rows } = await db.query(
    `SELECT turn, kind, status, COALESCE(output_json, '') FROM session_turns
     WHERE session_id = ${sql(session)} ORDER BY turn`
  );
  return rows.map(([turn, kind, status, output]) => ({
    turn,
    kind,
    status,
    output,
  }));
}

export async function recentRuns(db) {
  const { rows } = await db.query(
    `SELECT id, kind, status, COALESCE(cost_usd, 0), COALESCE(session_id, '')
     FROM runs ORDER BY created_at_ms DESC, rowid DESC LIMIT 16`
  );
  return rows.map(([id, kind, status, cost, session]) => ({
    id,
    kind,
    status,
    cost,
    session,
  }));
}

export async function status(db) {
  const version = await scalar(
    db,
    "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
  );
  const docs = await scalar(
    db,
    `SELECT COUNT(*) FROM documents WHERE index_status = 'ready'
     AND COALESCE(json_extract(metadata_json, '$.kind'), '') != 'memory'`
  );
  const spend = await scalar(
    db,
    "SELECT ROUND(COALESCE(SUM(cost_usd), 0), 6) FROM runs"
  );
  const ticketCount = await scalar(db, "SELECT COUNT(*) FROM tickets");
  const waitingCount = await scalar(
    db,
    "SELECT COUNT(*) FROM runs WHERE status IN ('awaiting_approval', 'suspended')"
  );
  const provider = await scalar(
    db,
    "SELECT provider FROM models WHERE kind = 'llm' ORDER BY rowid LIMIT 1"
  );
  let model = await scalar(
    db,
    "SELECT provider_model FROM models WHERE kind = 'llm' ORDER BY rowid LIMIT 1"
  );
  if (provider === "kimi" || provider === "moonshot") {
    model = kimiModel();
  }
  return {
    version,
    docs,
    spend,
    tickets: ticketCount,
    waiting: waitingCount,
    provider,
    model,
  };
}
