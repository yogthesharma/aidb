#!/usr/bin/env node
// Build AIDB, then run an example: frontend (Vite) + backend (Fastify + AI.open).
//
//   pnpm example:support
//   pnpm example:chat

import { execFileSync, spawn } from "node:child_process";
import path from "node:path";

import { ensureEnvFile, loadEnv, repoRoot } from "./load-env.mjs";

ensureEnvFile();
loadEnv();

const EXAMPLES = {
  support: {
    title: "Harbor",
    backend: "examples/support/backend",
    frontend: "harbor-frontend",
    apiPort: "8091",
    vitePort: "5174",
    db: "examples/support/desk.db",
  },
  chat: {
    title: "Chat",
    backend: "examples/chat/backend",
    frontend: "chat-frontend",
    apiPort: "8092",
    vitePort: "5175",
    db: "examples/chat/desk.db",
  },
};

const root = repoRoot();

function run(command, args, opts = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      stdio: "inherit",
      cwd: root,
      env: process.env,
      ...opts,
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`${command} exited ${code}`));
      }
    });
  });
}

function spawnGroup(command, args, opts) {
  const child = spawn(command, args, {
    stdio: "inherit",
    detached: process.platform !== "win32",
    env: process.env,
    ...opts,
  });
  child.on("error", (err) => {
    console.error(err.message);
    process.exit(1);
  });
  return child;
}

function killTree(child) {
  if (!child?.pid) {
    return;
  }
  try {
    if (process.platform === "win32") {
      spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"]);
    } else {
      process.kill(-child.pid, "SIGKILL");
    }
  } catch {
    try {
      child.kill("SIGKILL");
    } catch {
      // already gone
    }
  }
}

function freePort(port) {
  try {
    const out = execFileSync(
      "lsof",
      ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN", "-t"],
      { encoding: "utf8" }
    );
    for (const pid of out.trim().split(/\s+/).filter(Boolean)) {
      try {
        process.kill(Number(pid), "SIGKILL");
      } catch {
        // not ours / already gone
      }
    }
  } catch {
    // nothing listening
  }
}

async function waitHealth(url, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  let last = "not started";
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        const body = await response.json();
        if (body.ok === true) {
          return;
        }
        last = JSON.stringify(body);
      } else {
        last = `HTTP ${response.status}`;
      }
    } catch (err) {
      last = err.cause?.message || err.message;
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`backend never became healthy at ${url} (${last})`);
}

function usage() {
  const names = Object.keys(EXAMPLES).join(", ");
  console.error(`usage: pnpm example:<name>\n  names: ${names}`);
  process.exit(1);
}

async function main() {
  const name = String(process.argv[2] ?? "").trim();
  const spec = EXAMPLES[name];
  if (!spec) {
    usage();
  }

  if (!process.env.KIMI_API_KEY && !process.env.MOONSHOT_API_KEY) {
    console.log("no Moonshot key in .env — example will use the fake model");
  }

  const apiHost = process.env.AIDB_API_HOST || "127.0.0.1";
  const apiPort = process.env.AIDB_API_PORT || spec.apiPort;
  const vitePort = process.env.AIDB_VITE_PORT || spec.vitePort;
  const apiUrl = `http://${apiHost}:${apiPort}`;
  const backend = path.join(root, spec.backend);

  console.log(`build aidb  (${spec.title})`);
  await run("cargo", ["build", "--workspace"]);
  await run(process.execPath, [
    path.join(root, "bindings/typescript/scripts/stage-native.mjs"),
  ]);

  const pnpm = (() => {
    try {
      execFileSync("pnpm", ["--version"], { stdio: "ignore" });
      return "pnpm";
    } catch {
      execFileSync("corepack", ["enable"], { stdio: "inherit" });
      execFileSync("corepack", ["prepare", "pnpm@10.15.0", "--activate"], {
        stdio: "inherit",
      });
      return "pnpm";
    }
  })();
  await run(pnpm, ["install"]);

  freePort(Number(apiPort));
  freePort(Number(vitePort));

  const children = [];
  const stop = () => {
    for (const child of children) {
      killTree(child);
    }
  };
  process.on("SIGINT", () => {
    stop();
    process.exit(0);
  });
  process.on("SIGTERM", () => {
    stop();
    process.exit(0);
  });

  const server = spawnGroup(process.execPath, [path.join(backend, "index.mjs")], {
    cwd: backend,
    env: {
      ...process.env,
      AIDB_API_HOST: apiHost,
      AIDB_API_PORT: String(apiPort),
    },
  });
  children.push(server);
  server.on("exit", (code, signal) => {
    if (signal) {
      return;
    }
    console.error(`${spec.title} backend exited (${code ?? "unknown"})`);
    stop();
    process.exit(code || 1);
  });

  await waitHealth(`${apiUrl}/api/health`);

  const vite = spawnGroup(pnpm, ["--filter", spec.frontend, "dev", "--", "--port", String(vitePort)], {
    cwd: root,
    env: {
      ...process.env,
      AIDB_API_URL: apiUrl,
    },
  });
  children.push(vite);
  vite.on("exit", (code) => {
    stop();
    process.exit(code ?? 0);
  });

  console.log(`\n${spec.title} UI        http://127.0.0.1:${vitePort}`);
  console.log(`${spec.title} backend   ${apiUrl}`);
  console.log(`file               ${path.join(root, spec.db)}\n`);
}

main().catch((err) => {
  console.error(err.message || err);
  process.exit(1);
});
