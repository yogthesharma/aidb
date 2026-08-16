import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { loadEnv } from "../../scripts/load-env.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../..");
loadEnv([repo]);

export async function loadAI() {
  const local = pathToFileURL(
    path.resolve(repo, "bindings/typescript/src/index.mjs")
  ).href;
  const failures = [];
  for (const specifier of ["aidb", local]) {
    try {
      return (await import(specifier)).AI;
    } catch (err) {
      failures.push(`${specifier}: ${err.message}`);
    }
  }
  throw new Error(
    `could not load aidb\n  ${failures.join("\n  ")}\n` +
      "From the repo root run: pnpm example:support  or  pnpm example:chat"
  );
}

export function sql(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

export function json(value) {
  return sql(JSON.stringify(value));
}

export async function scalar(db, query) {
  const { rows } = await db.query(query);
  return rows.length ? String(rows[0][0] ?? "") : "";
}

export function pauseMessage(output) {
  try {
    const value = JSON.parse(output);
    if (value && typeof value === "object" && value.message) {
      return String(value.message);
    }
  } catch {
    // files written before parked output was JSON
  }
  return output || "waiting";
}

export function kimiKeyName() {
  if (process.env.KIMI_API_KEY) {
    return "KIMI_API_KEY";
  }
  if (process.env.MOONSHOT_API_KEY) {
    return "MOONSHOT_API_KEY";
  }
  return "MOONSHOT_API_KEY";
}

export function liveRequested(flags = {}) {
  if (flags.offline) {
    return false;
  }
  return (
    Boolean(flags.live) ||
    Boolean(process.env.KIMI_API_KEY) ||
    Boolean(process.env.MOONSHOT_API_KEY)
  );
}

export function kimiModel() {
  return process.env.AIDB_LLM_MODEL || "kimi-k2.5";
}

export function parseFlags(argv) {
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

export function serialize(db) {
  let tail = Promise.resolve();
  return (fn) => {
    const run = tail.then(() => fn(db));
    tail = run.then(
      () => undefined,
      () => undefined
    );
    return run;
  };
}

export function fail(err) {
  const error = String(err?.message || err);
  const statusCode = /HTTP 4\d\d/.test(error) ? 502 : 500;
  return { statusCode, error };
}
