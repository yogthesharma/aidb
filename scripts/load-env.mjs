import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export function repoRoot() {
  return root;
}

function parseLine(line) {
  const trimmed = line.trim();
  if (!trimmed || trimmed.startsWith("#")) {
    return null;
  }
  const body = trimmed.startsWith("export ") ? trimmed.slice(7).trim() : trimmed;
  const eq = body.indexOf("=");
  if (eq <= 0) {
    return null;
  }
  const key = body.slice(0, eq).trim();
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
    return null;
  }
  let value = body.slice(eq + 1).trim();
  if (value.endsWith("\\")) {
    value = value.slice(0, -1).trim();
  }
  if (
    (value.startsWith('"') && value.endsWith('"')) ||
    (value.startsWith("'") && value.endsWith("'"))
  ) {
    value = value.slice(1, -1);
  }
  return { key, value };
}

function applyFile(file) {
  if (!fs.existsSync(file)) {
    return;
  }
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    const parsed = parseLine(line);
    if (!parsed || parsed.value === "") {
      continue;
    }
    process.env[parsed.key] = parsed.value;
  }
}

/** Load `.env` then `.env.local`. Non-empty file values override the shell. */
export function loadEnv(extraDirs = []) {
  const dirs = [...extraDirs.map((dir) => path.resolve(dir)), root];
  for (const dir of dirs) {
    applyFile(path.join(dir, ".env"));
    applyFile(path.join(dir, ".env.local"));
  }
}

export function ensureEnvFile() {
  const example = path.join(root, ".env.example");
  const dest = path.join(root, ".env");
  if (fs.existsSync(dest) || !fs.existsSync(example)) {
    return dest;
  }
  fs.copyFileSync(example, dest);
  console.log(`wrote ${dest} — put KIMI_API_KEY there (never in the SQLite file)`);
  return dest;
}
