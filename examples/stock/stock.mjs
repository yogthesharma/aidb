#!/usr/bin/env node
// The stock desk: an equity research assistant whose entire backend is one AIDB file.
//
// No orchestration framework, no vector service, no trace backend, no second database.
// Watchlist rows, filings, embeddings, answers, agent steps, approvals and spend all
// live in the same file, so `aidb sql desk.db 'SELECT ...'` is the whole admin surface.
//
//   node examples/stock/stock.mjs demo --db ./desk.db
//
// Run `node examples/stock/stock.mjs help` for the command list.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { CORPUS, HEADLINES, WATCHLIST } from "./corpus.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));

// ---------------------------------------------------------------- AIDB access

// Installed apps do `import { AI } from "aidb"`. In-repo we fall back to the
// binding source so the example runs from a checkout with no npm install.
async function loadAI() {
  const local = pathToFileURL(
    path.resolve(here, "../../bindings/typescript/src/index.mjs")
  ).href;
  const failures = [];
  for (const specifier of ["aidb", local]) {
    try {
      return (await import(specifier)).AI;
    } catch (err) {
      failures.push(`${specifier}: ${err.message}`);
    }
  }
  throw new Error(`could not load aidb\n  ${failures.join("\n  ")}`);
}

function sql(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

function json(value) {
  return sql(JSON.stringify(value));
}

async function scalar(db, query) {
  const { rows } = await db.query(query);
  return rows.length ? String(rows[0][0] ?? "") : "";
}

// ------------------------------------------------------------------ app setup

// The desk's own business data. Ordinary SQL tables in the same file as the AI
// state: no ORM, no second store, and a join between them is just a join.
const APP_SCHEMA = [
  `CREATE TABLE IF NOT EXISTS watchlist (
     ticker      TEXT PRIMARY KEY,
     name        TEXT NOT NULL,
     added_at_ms INTEGER NOT NULL
   )`,
  `CREATE TABLE IF NOT EXISTS signals (
     id            INTEGER PRIMARY KEY AUTOINCREMENT,
     ticker        TEXT NOT NULL,
     headline      TEXT NOT NULL,
     label         TEXT NOT NULL,
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

// The desk cannot spend more than a cent per question, cannot be talked into a
// tool it does not own, and cannot email a client without a human saying yes.
const POLICY = {
  name: "desk",
  allow: ["search", "generate", "send.email"],
  max_usd: 0.01,
  max_llm_calls: 12,
  require_approval: ["send.email"],
};

async function init(db, { live }) {
  for (const statement of APP_SCHEMA) {
    await db.execute(statement);
  }
  const provider = live ? "openai" : "fake";
  await db.execute(`CREATE MODEL IF NOT EXISTS desk PROVIDER ${provider} KIND llm`);
  await db.query(`SELECT aidb_mcp_register(${json(TOOLS)})`);
  await db.query(`SELECT aidb_set_policy(${json(POLICY)})`);
  return { provider };
}

// Indexing is a background run, so a freshly inserted document is not searchable
// yet. Applications wait by polling the status they can already see.
async function waitIndexed(db, timeoutMs = 60_000) {
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

async function ingest(db) {
  const now = Date.now();
  for (const { ticker, name } of WATCHLIST) {
    await db.execute(
      `INSERT OR IGNORE INTO watchlist (ticker, name, added_at_ms)
       VALUES (${sql(ticker)}, ${sql(name)}, ${now})`
    );
  }
  // Two inserts of the same filing would be two documents, so knowing what is
  // already ingested is the app's job. Its key travels in the metadata.
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
      metadata: {
        source_id: doc.sourceId,
        ticker: doc.ticker,
        kind: doc.kind,
        period: doc.period,
      },
    });
    ids.push(id);
  }
  await waitIndexed(db);
  return { ids, skipped };
}

async function ingested(db) {
  const { rows } = await db.query(
    `SELECT DISTINCT json_extract(metadata_json, '$.source_id') FROM documents
     WHERE json_extract(metadata_json, '$.source_id') IS NOT NULL`
  );
  return new Set(rows.map((row) => String(row[0])));
}

// -------------------------------------------------------------------- reading

function filterFor({ ticker, kind }) {
  const filter = {};
  if (ticker) {
    filter.ticker = ticker.toUpperCase();
  }
  if (kind) {
    filter.kind = kind;
  }
  return filter;
}

async function titleOf(db, documentId) {
  return scalar(db, `SELECT title FROM documents WHERE id = ${sql(documentId)}`);
}

// Retrieve, then answer only from what was retrieved. The answer arrives with the
// chunks it used, so "where did that come from" is a lookup, not a guess.
async function ask(db, question, { ticker, kind, user, k = 4 }) {
  const preferences = user ? await recall(db, question, user) : [];
  const instructions = [
    "You are an equity research assistant. Answer only from the sources.",
    ...preferences.map((p) => `Analyst preference: ${p}`),
  ].join(" ");

  const answer = await scalar(
    db,
    `SELECT aidb_generate(${sql(`${instructions} Question: ${question}`)}, content)
     FROM aidb_search(${sql(question)}, ${k}, ${json(filterFor({ ticker, kind }))})`
  );
  const parsed = JSON.parse(answer);
  const sources = [];
  for (const source of parsed.sources ?? []) {
    sources.push({ ...source, title: await titleOf(db, source.document_id) });
  }
  return { answer: parsed.answer, sources };
}

async function recall(db, question, user) {
  const { rows } = await db.memory.search({
    query: question,
    scope: `user:${user}`,
    limit: 3,
  });
  return rows.map((row) => String(row[1]));
}

async function remember(db, user, content) {
  return db.memory.insert({ scope: `user:${user}`, content });
}

// The agent is a recipe: the listed tools in order until the model says DONE. Every
// step is a child run in the same file, so the transcript survives the process.
async function brief(db, ticker) {
  const symbol = ticker.toUpperCase();
  const result = await db.agent.run({
    instructions:
      "You are an equity research assistant. Use the documents. " +
      "Write a two line brief on risk and guidance. End with DONE.",
    goal: `What are the key risks and guidance for ${symbol}?`,
    tools: ["search", "generate"],
    maxSteps: 3,
    k: 4,
    decide: true,
  });
  return { ...result, steps: await steps(db, result.run_id) };
}

// Same recipe plus an irreversible tool. The run parks itself instead of sending.
async function digest(db, ticker) {
  const symbol = ticker.toUpperCase();
  const result = await db.agent.run({
    instructions:
      "You are an equity research assistant. Draft the morning digest from the documents, " +
      "then email it. End with DONE.",
    goal: `Morning digest for ${symbol}`,
    tools: ["search", "generate", "send.email"],
    maxSteps: 4,
    k: 4,
    decide: true,
  });
  return { ...result, steps: await steps(db, result.run_id) };
}

async function steps(db, runId) {
  const { rows } = await db.query(
    `SELECT kind, status, COALESCE(cost_usd, 0) FROM runs
     WHERE parent_id = ${sql(runId)} ORDER BY created_at_ms, rowid`
  );
  return rows.map(([kind, status, cost]) => ({
    kind: String(kind),
    status: String(status),
    cost: Number(cost),
  }));
}

// A label is only useful if you can find the run that produced it later.
async function sentiment(db, ticker, headline) {
  const label = await scalar(
    db,
    `SELECT aidb_classify('bullish or bearish or neutral', ${sql(headline)})`
  );
  const runId = await scalar(db, "SELECT aidb_last_run_id()");
  await db.execute(
    `INSERT INTO signals (ticker, headline, label, run_id, created_at_ms)
     VALUES (${sql(ticker.toUpperCase())}, ${sql(headline)}, ${sql(label)}, ${sql(runId)}, ${Date.now()})`
  );
  return { label, runId };
}

async function waiting(db) {
  const { rows } = await db.query(
    `SELECT id, kind, output_json FROM runs
     WHERE status IN ('awaiting_approval', 'suspended') ORDER BY created_at_ms`
  );
  return rows.map(([id, kind, output]) => ({
    id: String(id),
    kind: String(kind),
    message: pauseMessage(String(output ?? "")),
  }));
}

// A parked run stores why it stopped in output_json as JSON
// ({paused, status, message}). Old files may still have a plain string.
function pauseMessage(output) {
  try {
    const value = JSON.parse(output);
    if (value && typeof value === "object" && value.message) {
      return String(value.message);
    }
  } catch {
    // keep the raw text for files written before parked output was JSON
  }
  return output || "waiting";
}

async function decide(db, runId, approved) {
  return db.runs.resume(runId, { approved });
}

async function status(db) {
  const [version, docs, spend, tickers, waitingCount] = await Promise.all([
    scalar(db, "SELECT value FROM aidb_meta WHERE key = 'schema_version'"),
    // Memory is documents too, so the research count has to exclude it.
    scalar(
      db,
      `SELECT COUNT(*) FROM documents WHERE index_status = 'ready'
       AND COALESCE(json_extract(metadata_json, '$.kind'), '') != 'memory'`
    ),
    scalar(db, "SELECT ROUND(COALESCE(SUM(cost_usd), 0), 6) FROM runs"),
    scalar(db, "SELECT COUNT(*) FROM watchlist"),
    scalar(
      db,
      "SELECT COUNT(*) FROM runs WHERE status IN ('awaiting_approval', 'suspended')"
    ),
  ]);
  return { version, docs, spend, tickers, waiting: waitingCount };
}

async function recentRuns(db, limit = 12) {
  const { rows } = await db.query(
    `SELECT id, kind, status, COALESCE(cost_usd, 0), COALESCE(parent_id, '')
     FROM runs ORDER BY created_at_ms DESC, rowid DESC LIMIT ${limit}`
  );
  return rows.map(([id, kind, status, cost, parent]) => ({
    id: String(id),
    kind: String(kind),
    status: String(status),
    cost: Number(cost),
    parent: String(parent),
  }));
}

// ------------------------------------------------------------------------ CLI

function parseArgs(argv) {
  const flags = {};
  const positional = [];
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg.startsWith("--")) {
      const name = arg.slice(2);
      const next = argv[i + 1];
      if (next === undefined || next.startsWith("--")) {
        flags[name] = true;
      } else {
        flags[name] = next;
        i += 1;
      }
    } else {
      positional.push(arg);
    }
  }
  return { flags, positional };
}

function printAnswer({ answer, sources }) {
  console.log(`\n${answer}\n`);
  if (!sources.length) {
    console.log("sources: none (nothing in the file matched)");
    return;
  }
  console.log("sources:");
  for (const source of sources) {
    console.log(
      `  ${source.title} [doc ${source.document_id} chunk ${source.chunk_id} score ${source.score.toFixed(4)}]`
    );
  }
}

function printSteps(result) {
  console.log(`run ${result.run_id} ${result.status}`);
  for (const step of result.steps) {
    console.log(`  ${step.kind.padEnd(9)} ${step.status.padEnd(18)} $${step.cost.toFixed(6)}`);
  }
  if (result.output) {
    console.log(`\n${result.output}\n`);
  }
}

const HELP = `stock desk — an AI application on one AIDB file

usage: node stock.mjs <command> [--db ./desk.db] [--live]

  demo                      init, ingest, ask, brief, digest, approve, report
  init                      app tables, model, tool catalog, policy
  ingest                    load the research corpus and the watchlist
  ask "<question>"          cited answer  [--ticker AAPL] [--kind filing] [--user u1] [--k 4]
  remember <user> "<text>"  store an analyst preference
  brief <TICKER>            agent brief (search then generate)
  digest <TICKER>           agent digest that wants to email — parks for approval
  sentiment <TICKER> "<h>"  classify a headline into the signals table
  waiting                   runs parked for a human
  approve <run_id>          resume an approved run
  reject <run_id>           resume a rejected run
  runs                      recent runs with cost
  status                    file, documents, spend, parked runs

--live uses a real provider (needs OPENAI_API_KEY). The default path is offline.`;

async function main() {
  const [command = "help", ...rest] = process.argv.slice(2);
  const { flags, positional } = parseArgs(rest);
  if (command === "help" || flags.help) {
    console.log(HELP);
    return;
  }

  const dbPath = path.resolve(String(flags.db ?? "desk.db"));
  const live = Boolean(flags.live) || process.env.AIDB_STOCK_LIVE === "1";
  if (live && !process.env.OPENAI_API_KEY) {
    throw new Error("--live needs OPENAI_API_KEY (the file never stores the key)");
  }
  const AI = await loadAI();
  const db = await AI.open(dbPath);
  try {
    await run(db, command, { flags, positional, live, dbPath });
  } finally {
    await db.close();
  }
}

async function run(db, command, { flags, positional, live, dbPath }) {
  switch (command) {
    case "init": {
      const { provider } = await init(db, { live });
      console.log(`initialized ${dbPath} (llm provider: ${provider})`);
      break;
    }
    case "ingest": {
      const { ids, skipped } = await ingest(db);
      const already = skipped ? `, ${skipped} already present` : "";
      console.log(
        `ingested ${ids.length} documents, ${WATCHLIST.length} tickers${already}`
      );
      break;
    }
    case "ask": {
      const question = positional.join(" ");
      if (!question) {
        throw new Error('ask needs a question: ask "how concentrated is NVDA revenue"');
      }
      printAnswer(
        await ask(db, question, {
          ticker: flags.ticker,
          kind: flags.kind,
          user: flags.user,
          k: Number(flags.k ?? 4),
        })
      );
      break;
    }
    case "remember": {
      const [user, ...text] = positional;
      if (!user || !text.length) {
        throw new Error('remember needs a user and a preference');
      }
      const { id } = await remember(db, user, text.join(" "));
      console.log(`remembered ${id} for user:${user}`);
      break;
    }
    case "brief": {
      const [ticker] = positional;
      if (!ticker) {
        throw new Error("brief needs a ticker");
      }
      printSteps(await brief(db, ticker));
      break;
    }
    case "digest": {
      const [ticker] = positional;
      if (!ticker) {
        throw new Error("digest needs a ticker");
      }
      const result = await digest(db, ticker);
      printSteps(result);
      if (result.status === "awaiting_approval") {
        console.log(`parked. approve with: node stock.mjs approve ${result.run_id}`);
      }
      break;
    }
    case "sentiment": {
      const [ticker, ...headline] = positional;
      if (!ticker || !headline.length) {
        throw new Error('sentiment needs a ticker and a headline');
      }
      const { label, runId } = await sentiment(db, ticker, headline.join(" "));
      console.log(`${ticker.toUpperCase()}: ${label} (run ${runId})`);
      break;
    }
    case "waiting": {
      const parked = await waiting(db);
      if (!parked.length) {
        console.log("nothing is waiting for a human");
        break;
      }
      for (const item of parked) {
        console.log(`${item.id}  ${item.kind}  ${item.message}`);
      }
      break;
    }
    case "approve":
    case "reject": {
      const [runId] = positional;
      if (!runId) {
        throw new Error(`${command} needs a run id`);
      }
      const result = await decide(db, runId, command === "approve");
      console.log(`run ${result.run_id} ${result.status}`);
      if (result.output) {
        console.log(result.output);
      }
      break;
    }
    case "runs": {
      for (const item of await recentRuns(db, Number(flags.limit ?? 12))) {
        const parent = item.parent ? ` parent=${item.parent}` : "";
        console.log(
          `${item.id}  ${item.kind.padEnd(14)} ${item.status.padEnd(18)} $${item.cost.toFixed(6)}${parent}`
        );
      }
      break;
    }
    case "status": {
      const s = await status(db);
      console.log(`file      ${dbPath} (${fileSize(dbPath)})`);
      console.log(`schema    v${s.version}`);
      console.log(`documents ${s.docs} indexed`);
      console.log(`watchlist ${s.tickers} tickers`);
      console.log(`spend     $${s.spend}`);
      console.log(`waiting   ${s.waiting} run(s)`);
      break;
    }
    case "demo":
      await demo(db, { live, dbPath });
      break;
    default:
      throw new Error(`unknown command: ${command}\n\n${HELP}`);
  }
}

function fileSize(dbPath) {
  try {
    return `${(fs.statSync(dbPath).size / 1024).toFixed(0)} KB`;
  } catch {
    return "new";
  }
}

// The whole desk in one pass, so a reader can see the shape of an AIDB application
// without reading the source.
async function demo(db, { live, dbPath }) {
  const section = (title) => console.log(`\n=== ${title} ===`);

  section("init");
  const { provider } = await init(db, { live });
  console.log(`${dbPath} ready (llm provider: ${provider})`);

  section("ingest");
  const { ids } = await ingest(db);
  console.log(`${ids.length} documents indexed and searchable`);

  section("ask (whole desk)");
  printAnswer(await ask(db, "how concentrated is data center revenue", { k: 3 }));

  section("ask (one ticker, one analyst)");
  await remember(db, "u1", "Prefers two sentence answers with the number first.");
  printAnswer(
    await ask(db, "what is the margin guidance", { ticker: "AAPL", user: "u1", k: 3 })
  );

  section("classify headlines into app tables");
  for (const item of HEADLINES) {
    const { label } = await sentiment(db, item.ticker, item.headline);
    console.log(`${item.ticker}: ${label}  ${item.headline}`);
  }

  section("agent brief");
  printSteps(await brief(db, "NVDA"));

  section("agent digest wants to email a human's client");
  const parked = await digest(db, "NVDA");
  printSteps(parked);

  section("approval");
  const queue = await waiting(db);
  for (const item of queue) {
    console.log(`waiting: ${item.id} (${item.message})`);
  }
  if (queue.length) {
    const resumed = await decide(db, queue[0].id, true);
    console.log(`resumed ${resumed.run_id} -> ${resumed.status}`);
    const sent = await scalar(
      db,
      `SELECT COALESCE(output_json, '') FROM runs
       WHERE kind = 'tool' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1`
    );
    console.log(`tool run output: ${sent}`);
  }

  section("the file is the report");
  const joined = await db.query(
    `SELECT w.ticker, w.name, COUNT(s.id)
     FROM watchlist w LEFT JOIN signals s ON s.ticker = w.ticker
     GROUP BY w.ticker ORDER BY w.ticker`
  );
  for (const [ticker, name, signals] of joined.rows) {
    console.log(`${ticker}  ${String(name).padEnd(24)} ${signals} signal(s)`);
  }
  for (const item of await recentRuns(db, 8)) {
    console.log(
      `${item.kind.padEnd(14)} ${item.status.padEnd(18)} $${item.cost.toFixed(6)}`
    );
  }
  const s = await status(db);
  console.log(`\nspend $${s.spend} across every AI call, in ${dbPath}`);
}

main().catch((err) => {
  console.error(`error: ${err.message}`);
  process.exitCode = 1;
});
