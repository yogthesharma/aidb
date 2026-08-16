import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));
const pkgRoot = path.resolve(here, "..");

function platformTag() {
  const os =
    process.platform === "darwin"
      ? "darwin"
      : process.platform === "linux"
        ? "linux"
        : process.platform === "win32"
          ? "win32"
          : null;
  const arch =
    process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "x64" : null;
  if (!os || !arch) {
    return `${process.platform}-${process.arch}`;
  }
  return `${os}-${arch}`;
}

function repoRoot() {
  const root = path.resolve(pkgRoot, "../..");
  if (fs.existsSync(path.join(root, "crates/aidb-node/Cargo.toml"))) {
    return root;
  }
  return null;
}

function nativeCandidates() {
  const tag = platformTag();
  const out = [];
  if (process.env.AIDB_NODE_LIB) {
    out.push(process.env.AIDB_NODE_LIB);
  }
  out.push(path.join(pkgRoot, `aidb.${tag}.node`));
  out.push(path.join(pkgRoot, "aidb.node"));
  out.push(path.join(here, `aidb.${tag}.node`));
  out.push(path.join(here, "aidb.node"));
  const cargoTarget = process.env.CARGO_TARGET_DIR;
  const root = repoRoot();
  const dirs = [
    cargoTarget && path.join(cargoTarget, "debug"),
    cargoTarget && path.join(cargoTarget, "release"),
    root && path.join(root, "target/debug"),
    root && path.join(root, "target/release"),
  ].filter(Boolean);
  const names = [
    "aidb.node",
    "libaidb_node.dylib",
    "libaidb_node.so",
    "aidb_node.dll",
    "libaidb_node.dll",
  ];
  for (const dir of dirs) {
    for (const name of names) {
      out.push(path.join(dir, name));
    }
  }
  return out;
}

function ensureNodeAddon(source) {
  if (source.endsWith(".node")) {
    return source;
  }
  const dest = path.join(path.dirname(source), "aidb.node");
  if (!fs.existsSync(dest) || fs.statSync(source).mtimeMs > fs.statSync(dest).mtimeMs) {
    fs.copyFileSync(source, dest);
  }
  return dest;
}

function loadNative() {
  for (const candidate of nativeCandidates()) {
    if (!candidate || !fs.existsSync(candidate)) {
      continue;
    }
    return require(ensureNodeAddon(candidate));
  }
  throw new Error(
    `aidb native addon not found for ${platformTag()}. Install with: npm i aidb`
  );
}

const native = loadNative();

export const RUNTIME = native.RUNTIME ?? "napi";

function sqlString(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

export class Database {
  constructor(inner, dbPath) {
    this._inner = inner;
    this.path = dbPath;
  }

  async query(sql) {
    return this._inner.query(sql);
  }

  async execute(sql) {
    return this._inner.execute(sql);
  }

  async session(name) {
    if (name === undefined) {
      const result = await this.query("SELECT aidb_session()");
      return String(result.rows[0]?.[0] ?? "");
    }
    if (name === null) {
      const result = await this.query("SELECT aidb_session(NULL)");
      return String(result.rows[0]?.[0] ?? "");
    }
    const result = await this.query(`SELECT aidb_session(${sqlString(name)})`);
    return String(result.rows[0]?.[0] ?? "");
  }

  async lastRunId() {
    const result = await this.query("SELECT aidb_last_run_id()");
    return String(result.rows[0]?.[0] ?? "");
  }

  get memory() {
    const db = this;
    return {
      async insert({ scope, userId, content }) {
        const key = scope ?? (userId != null ? `user:${userId}` : "");
        const result = await db.query(
          `SELECT aidb_memory_insert(${sqlString(key)}, ${sqlString(content)})`
        );
        return { id: String(result.rows[0]?.[0] ?? "") };
      },
      async search({ query, scope, userId, limit = 5 }) {
        const key = scope ?? (userId != null ? `user:${userId}` : "");
        if (key) {
          return db.query(
            `SELECT document_id, content FROM aidb_memory_search(${sqlString(query)}, ${limit}, ${sqlString(key)})`
          );
        }
        return db.query(
          `SELECT document_id, content FROM aidb_memory_search(${sqlString(query)}, ${limit})`
        );
      },
    };
  }

  get documents() {
    const db = this;
    return {
      async insert({ title = "", content, metadata = {} }) {
        const sql = `SELECT aidb_insert_document(${sqlString(title)}, ${sqlString(content)}, ${sqlString(JSON.stringify(metadata))})`;
        const result = await db.query(sql);
        return { id: String(result.rows[0]?.[0] ?? "") };
      },
    };
  }

  async search(query, options = {}) {
    const limit = options.limit ?? 5;
    return this.query(
      `SELECT document_id, chunk_id, content, distance FROM aidb_search(${sqlString(query)}, ${limit})`
    );
  }

  get agent() {
    const db = this;
    return {
      async run({ instructions, goal, tools = ["search", "generate"], maxSteps = 4, k = 5, memory, agents, decide, session }) {
        const spec = JSON.stringify({
          instructions,
          goal,
          tools,
          max_steps: maxSteps,
          k,
          memory,
          agents,
          ...(decide ? { decide: true } : {}),
          ...(session ? { session } : {}),
        });
        const result = await db.query(`SELECT aidb_agent(${sqlString(spec)})`);
        const row = result.rows[0] ?? [];
        return {
          run_id: String(row[0] ?? ""),
          status: String(row[1] ?? ""),
          output: String(row[2] ?? ""),
        };
      },
    };
  }

  get runs() {
    const db = this;
    return {
      async waiting() {
        return db.query(
          "SELECT id, kind, status, output_json FROM runs WHERE status IN ('awaiting_approval', 'suspended') ORDER BY created_at_ms"
        );
      },
      async resume(id, decision = { approved: true }) {
        const result = await db.query(
          `SELECT aidb_resume(${sqlString(id)}, ${sqlString(JSON.stringify(decision))})`
        );
        const row = result.rows[0] ?? [];
        return {
          run_id: String(row[0] ?? ""),
          status: String(row[1] ?? ""),
          output: String(row[2] ?? ""),
        };
      },
      async events(id) {
        return db.query(
          `SELECT seq, kind, payload_json, created_at_ms FROM run_events WHERE run_id = ${sqlString(id)} ORDER BY seq`
        );
      },
      async tokens(id) {
        return db.query(
          `SELECT seq, json_extract(payload_json, '$.text') AS text, created_at_ms FROM run_events WHERE run_id = ${sqlString(id)} AND kind = 'token' ORDER BY seq`
        );
      },
    };
  }

  async close() {
    this._inner.close();
  }
}

export class AI {
  static runtime = RUNTIME;

  static subscribeTokens(callback) {
    native.subscribeTokens((event) => {
      callback({
        runId: String(event.run_id ?? event.runId ?? ""),
        seq: Number(event.seq ?? 0),
        text: String(event.text ?? ""),
      });
    });
  }

  static async open(dbPath, options = {}) {
    const inner = native.openDb(dbPath, options.embedding ?? null);
    return new Database(inner, dbPath);
  }
}
