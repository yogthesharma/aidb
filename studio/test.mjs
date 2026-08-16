// Studio is a face: these tests prove the pages are SELECTs, the bearer is a
// header (and a WS query token), and nothing here is a second engine.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  CATALOG_SQL,
  PAGE_SEGMENT,
  resumeSql,
  searchSql,
  sqlString,
} from "./src/lib/catalog.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tests = [];
const test = (name, fn) => tests.push([name, fn]);

function cliBin() {
  if (process.env.AIDB_CLI_BIN) {
    return process.env.AIDB_CLI_BIN;
  }
  const candidate = path.join(
    root,
    "target",
    "debug",
    process.platform === "win32" ? "aidb.exe" : "aidb",
  );
  return fs.existsSync(candidate) ? candidate : null;
}

function tempDir(tag) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `aidb-studio-${tag}-`));
}

async function waitFor(url, headers = {}, timeoutMs = 8000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    try {
      const response = await fetch(url, { headers });
      if (response.status === 200 || response.status === 401) {
        return response;
      }
    } catch {
      // still booting
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`serve did not become reachable at ${url}`);
}

async function serve({ bearer } = {}) {
  const bin = cliBin();
  if (!bin) {
    throw new Error(
      "aidb binary not found; set AIDB_CLI_BIN or run cargo build --workspace",
    );
  }
  const dir = tempDir("serve");
  const db = path.join(dir, "app.db");
  const env = { ...process.env };
  if (bearer) {
    env.AIDB_BEARER = bearer;
  } else {
    delete env.AIDB_BEARER;
    delete env.AIDB_TOKEN;
  }
  const child = spawn(bin, ["serve", db, "--bind", "127.0.0.1:0"], {
    env,
    stdio: ["ignore", "ignore", "pipe"],
  });
  let addr = null;
  let stderr = "";
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`serve produced no bind line: ${stderr}`));
    }, 8000);
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
      const match = stderr.match(/https?:\/\/127\.0\.0\.1:\d+/);
      if (match && !addr) {
        addr = match[0];
        clearTimeout(timer);
        resolve();
      }
    });
    child.on("exit", (code) => {
      if (!addr) {
        clearTimeout(timer);
        reject(new Error(`serve exited ${code}: ${stderr}`));
      }
    });
  });
  await waitFor(`${addr}/health`, bearer ? { Authorization: `Bearer ${bearer}` } : {});
  return {
    addr,
    db,
    async close() {
      child.kill("SIGTERM");
      await new Promise((resolve) => child.once("exit", resolve));
      fs.rmSync(dir, { recursive: true, force: true });
    },
  };
}

async function sql(addr, statement, bearer) {
  const headers = { "content-type": "text/plain; charset=utf-8" };
  if (bearer) {
    headers.Authorization = `Bearer ${bearer}`;
  }
  const response = await fetch(`${addr}/sql`, {
    method: "POST",
    headers,
    body: statement,
  });
  const body = await response.json();
  return { status: response.status, body };
}

test("sqlString quotes a value the way SQLite literals work", () => {
  assert.equal(sqlString("How do refunds work?"), "'How do refunds work?'");
  assert.equal(sqlString("O'Brien"), "'O''Brien'");
});

test("every Studio route is a named page, including experiments", () => {
  assert.equal(PAGE_SEGMENT.overview, "file");
  assert.equal(PAGE_SEGMENT.experiments, "experiments");
  assert.equal(PAGE_SEGMENT.runs, "runs");
  assert.ok(CATALOG_SQL.experiments.includes("FROM experiment_results"));
  assert.ok(CATALOG_SQL.nWaiting.includes("awaiting_approval"));
});

test("search and resume SQL are still POST /sql, not a second API", () => {
  assert.equal(
    searchSql("How do refunds work?", 5),
    "SELECT document_id, chunk_id, substr(content, 1, 200) AS content, distance FROM aidb_search('How do refunds work?', 5)",
  );
  assert.equal(
    resumeSql("run_abc", true),
    "SELECT aidb_resume('run_abc', '{\"approved\":true}')",
  );
});

test("the inspect pages are SELECTs over the served file", async () => {
  const server = await serve();
  try {
    const health = await fetch(`${server.addr}/health`);
    assert.equal(health.status, 200);
    assert.equal((await health.json()).ok, true);

    for (const [name, statement] of Object.entries(CATALOG_SQL)) {
      const { status, body } = await sql(server.addr, statement);
      assert.equal(status, 200, `${name}: ${JSON.stringify(body)}`);
      assert.equal(body.ok, true, `${name}: ${JSON.stringify(body)}`);
    }

    const inserted = await sql(
      server.addr,
      `SELECT aidb_insert_document(${sqlString("Refunds")}, ${sqlString("Refunds are issued within 14 days of purchase.")}, '{}')`,
    );
    assert.equal(inserted.body.ok, true, JSON.stringify(inserted.body));
    const hits = await sql(server.addr, searchSql("How do refunds work?", 5));
    assert.equal(hits.body.ok, true, JSON.stringify(hits.body));
    assert.ok(hits.body.rows.length > 0, "search page must see the document");
  } finally {
    await server.close();
  }
});

test("a protected serve refuses Studio until the bearer is sent", async () => {
  const server = await serve({ bearer: "s3cret" });
  try {
    const denied = await fetch(`${server.addr}/health`);
    assert.equal(denied.status, 401);

    const { status } = await sql(server.addr, CATALOG_SQL.meta);
    assert.equal(status, 401);

    const authedHealth = await fetch(`${server.addr}/health`, {
      headers: { Authorization: "Bearer s3cret" },
    });
    assert.equal(authedHealth.status, 200);

    const { status: ok, body } = await sql(
      server.addr,
      CATALOG_SQL.meta,
      "s3cret",
    );
    assert.equal(ok, 200);
    assert.equal(body.ok, true);
  } finally {
    await server.close();
  }
});

let failed = 0;
for (const [name, fn] of tests) {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (err) {
    failed += 1;
    console.log(`FAIL ${name}`);
    console.error(err);
  }
}
console.log(`studio: ${tests.length - failed} passed, ${failed} failed`);
if (failed) {
  process.exit(1);
}
