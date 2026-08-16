// TypeScript face contracts. The addon is the real Rust engine in-process: no
// child `aidb sql`, no second store, and the same file every other face uses.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { AI, RUNTIME } from "./src/index.mjs";

const tests = [];
const test = (name, fn) => tests.push([name, fn]);

const dirs = [];
function tempDir(tag) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), `aidb-ts-${tag}-`));
  dirs.push(dir);
  return dir;
}

function tempDb(tag) {
  return path.join(tempDir(tag), "app.db");
}

async function rejects(promise, needle) {
  try {
    await promise;
  } catch (err) {
    assert.match(String(err.message ?? err), new RegExp(needle, "i"));
    return;
  }
  throw new Error(`expected a rejection mentioning ${needle}`);
}

test("the binding loads the native addon rather than shelling out", async () => {
  assert.equal(RUNTIME, "napi");
  assert.equal(AI.runtime, "napi");
  // The package ships an addon for this platform, and that is what got loaded.
  const staged = fs
    .readdirSync(path.resolve("."))
    .filter((name) => name.endsWith(".node"));
  assert.ok(staged.length > 0, `no staged addon: ${staged}`);
});

test("open creates the file and reports the engine schema version", async () => {
  const dbPath = tempDb("open");
  assert.equal(fs.existsSync(dbPath), false);
  const db = await AI.open(dbPath);
  assert.equal(fs.existsSync(dbPath), true);
  const version = await db.query(
    "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
  );
  assert.match(String(version.rows[0][0]), /^\d+$/);
  assert.deepEqual(version.columns, ["value"]);
  await db.close();
});

test("insert, search and generate all go through the same engine", async () => {
  const db = await AI.open(tempDb("crud"));
  const doc = await db.documents.insert({
    title: "Refunds",
    content: "Refunds are issued within 14 days of purchase.",
    metadata: { dept: "support" },
  });
  assert.match(doc.id, /^doc_/);

  const status = await db.query(
    `SELECT index_status FROM documents WHERE id = '${doc.id}'`
  );
  assert.equal(status.rows[0][0], "ready");

  const hits = await db.search("How do refunds work?", { limit: 5 });
  assert.deepEqual(hits.columns, ["document_id", "chunk_id", "content", "distance"]);
  assert.ok(hits.rows.length > 0);
  assert.equal(hits.rows[0][0], doc.id);

  const answer = await db.query(
    "SELECT aidb_generate('Answer from the sources', content) FROM aidb_search('how do refunds work', 3)"
  );
  const value = JSON.parse(String(answer.rows[0][0]));
  assert.ok(value.answer.length > 0);
  assert.equal(value.sources[0].document_id, doc.id);

  const last = await db.lastRunId();
  assert.match(last, /^run_/);
  const tokens = await db.runs.tokens(last);
  assert.ok(tokens.rows.length > 0, "generate tokens are on that run");

  // Metadata survives the round trip as JSON.
  const meta = await db.query(
    `SELECT json_extract(metadata_json, '$.dept') FROM documents WHERE id = '${doc.id}'`
  );
  assert.equal(meta.rows[0][0], "support");
  await db.close();
});

test("subscribeTokens sees generate chunks while the query is in flight", async () => {
  const seen = [];
  AI.subscribeTokens((event) => {
    if (event.text) {
      seen.push(event.text);
    }
  });
  const db = await AI.open(tempDb("stream"));
  const text = String(
    (
      await db.query(
        "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.')"
      )
    ).rows[0][0]
  );
  assert.ok(seen.length > 1, `expected streamed chunks, got ${JSON.stringify(seen)}`);
  assert.equal(seen.join(""), text);
  await db.close();
});

test("execute reports affected rows and query returns typed values", async () => {
  const db = await AI.open(tempDb("execute"));
  await db.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT, score REAL)");
  const changed = await db.execute(
    "INSERT INTO notes (body, score) VALUES ('hello', 1.5)"
  );
  assert.equal(Number(changed), 1);
  const rows = await db.query("SELECT id, body, score FROM notes");
  assert.equal(rows.rows[0][0], 1);
  assert.equal(rows.rows[0][1], "hello");
  assert.equal(rows.rows[0][2], 1.5);
  const empty = await db.query("SELECT id FROM notes WHERE id = 999");
  assert.deepEqual(empty.rows, []);
  await db.close();
});

test("memory is documents and search, scoped per user", async () => {
  const db = await AI.open(tempDb("memory"));
  const mine = await db.memory.insert({
    userId: "123",
    content: "Prefers concise technical explanations. Explain things briefly.",
  });
  assert.match(mine.id, /^doc_/);
  await db.memory.insert({
    userId: "456",
    content: "Prefers long worked examples with diagrams.",
  });

  const hits = await db.memory.search({ userId: "123", query: "How should I explain this?" });
  assert.ok(hits.rows.length > 0);
  for (const row of hits.rows) {
    assert.notEqual(String(row[1]), "Prefers long worked examples with diagrams.");
  }

  // Memory is stored as documents in the same file, not a side store.
  const stored = await db.query(
    `SELECT json_extract(metadata_json, '$.scope') FROM documents WHERE id = '${mine.id}'`
  );
  assert.equal(stored.rows[0][0], "user:123");
  await db.close();
});

test("an agent run is parent and child runs in the same table", async () => {
  const db = await AI.open(tempDb("agent"));
  await db.documents.insert({
    title: "Refunds",
    content: "Refunds are issued within 14 days of purchase.",
  });
  const agent = await db.agent.run({
    instructions: "Answer from documents. End with DONE.",
    goal: "How do refunds work?",
    maxSteps: 3,
  });
  assert.equal(agent.status, "succeeded");
  assert.match(agent.output.toLowerCase(), /refund/);

  const parent = await db.query(
    `SELECT kind, status FROM runs WHERE id = '${agent.run_id}'`
  );
  assert.deepEqual(parent.rows[0], ["agent", "succeeded"]);
  const children = await db.query(
    `SELECT COUNT(*) FROM runs WHERE parent_id = '${agent.run_id}'`
  );
  assert.ok(Number(children.rows[0][0]) > 0);
  await db.close();
});

test("approval is a run state that the binding can wait on and resume", async () => {
  const db = await AI.open(tempDb("hitl"));
  const paused = await db.query(
    `SELECT aidb_workflow('{"then":[{"search":{"query":"How do refunds work?","k":5}},{"approve":{"message":"Send this answer?"}},{"generate":{"prompt":"Draft the reply"}}]}')`
  );
  assert.equal(paused.rows[0][1], "awaiting_approval");
  const runId = String(paused.rows[0][0]);

  const waiting = await db.runs.waiting();
  assert.equal(waiting.rows.length, 1);
  assert.equal(String(waiting.rows[0][0]), runId);
  const parked = await db.query(
    `SELECT json_valid(output_json), json_extract(output_json, '$.message') FROM runs WHERE id = '${runId}'`
  );
  assert.equal(String(parked.rows[0][0]), "1");
  assert.equal(String(parked.rows[0][1]), "Send this answer?");

  const resumed = await db.runs.resume(runId, { approved: true });
  assert.equal(resumed.status, "succeeded");
  assert.equal((await db.runs.waiting()).rows.length, 0);
  await db.close();
});

test("errors surface as rejections, not as silent empty results", async () => {
  const db = await AI.open(tempDb("errors"));
  await rejects(db.query("SELECT * FROM table_that_does_not_exist"), "table_that_does_not_exist");
  await rejects(db.query("this is not sql"), "");
  await rejects(
    db.query("SELECT * FROM aidb_search('refunds', 5, '{}', 'ghost')"),
    "unknown embedding space"
  );
  // The connection still works after an error.
  const ok = await db.query("SELECT 1 AS n");
  assert.equal(ok.rows[0][0], 1);
  await db.close();
});

test("an unopenable path fails instead of returning a broken handle", async () => {
  const dir = tempDir("badpath");
  await rejects(AI.open(dir), "");
});

test("close then reopen sees the same data", async () => {
  const dbPath = tempDb("reopen");
  const first = await AI.open(dbPath);
  const doc = await first.documents.insert({
    title: "Refunds",
    content: "Refunds are issued within 14 days of purchase.",
  });
  await first.close();

  const second = await AI.open(dbPath);
  const rows = await second.query(
    `SELECT title, index_status FROM documents WHERE id = '${doc.id}'`
  );
  assert.deepEqual(rows.rows[0], ["Refunds", "ready"]);
  const hits = await second.search("refunds", { limit: 3 });
  assert.ok(hits.rows.length > 0, "vectors survived the reopen");
  await second.close();
});

test("the file the binding writes is the file the CLI reads", async () => {
  const cli = process.env.AIDB_CLI_BIN;
  if (!cli || !fs.existsSync(cli)) {
    console.log("  (skipped: set AIDB_CLI_BIN to the aidb binary)");
    return;
  }
  const dbPath = tempDb("shared");
  const db = await AI.open(dbPath);
  const doc = await db.documents.insert({
    title: "Refunds",
    content: "Refunds are issued within 14 days of purchase.",
  });
  await db.close();

  const out = execFileSync(cli, ["sql", dbPath, "SELECT id, title FROM documents"], {
    encoding: "utf8",
  });
  assert.match(out, new RegExp(doc.id));
  assert.match(out, /Refunds/);
});

let failed = 0;
for (const [name, fn] of tests) {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (err) {
    failed += 1;
    console.error(`FAIL ${name}\n     ${err.stack ?? err}`);
  }
}
for (const dir of dirs) {
  fs.rmSync(dir, { recursive: true, force: true });
}
console.log(`\ntypescript: ${tests.length - failed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);
