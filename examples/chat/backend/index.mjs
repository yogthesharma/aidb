#!/usr/bin/env node
// Chat backend: Fastify + AI.open. Empty file. SQL never leaves this process.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import Fastify from "fastify";

import {
  fail,
  kimiModel,
  liveRequested,
  loadAI,
  parseFlags,
  serialize,
} from "../../lib/aidb.mjs";
import {
  addDocument,
  chat,
  documents,
  IDENTITY,
  init,
  sessions,
  status,
  turns,
} from "./desk.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const exampleRoot = path.resolve(here, "..");
const started = Date.now();
let cpuPrev = process.cpuUsage();
let cpuAt = Date.now();

function fileBytes(dbPath) {
  let total = 0;
  for (const suffix of ["", "-wal", "-shm"]) {
    try {
      total += fs.statSync(`${dbPath}${suffix}`).size;
    } catch {
      // sidecar missing
    }
  }
  return total;
}

function processUsage(dbPath) {
  const mem = process.memoryUsage();
  const delta = process.cpuUsage(cpuPrev);
  const elapsed = Math.max(1, Date.now() - cpuAt);
  cpuPrev = process.cpuUsage();
  cpuAt = Date.now();
  const usedMs = (delta.user + delta.system) / 1000;
  return {
    rssMb: Number((mem.rss / 1048576).toFixed(1)),
    heapMb: Number((mem.heapUsed / 1048576).toFixed(1)),
    cpuPct: Number(Math.min(100, (usedMs / elapsed) * 100).toFixed(1)),
    uptimeSec: Math.round((Date.now() - started) / 1000),
    fileMb: Number((fileBytes(dbPath) / 1048576).toFixed(2)),
    load: Number(os.loadavg()[0].toFixed(2)),
  };
}

async function demo(db) {
  const session = "chat:demo";
  const hello = await chat(db, { session, text: "Say hello in one sentence." });
  console.log(`generate: ${hello.answer}`);

  await addDocument(db, {
    title: "Note",
    content: "The office wifi password is cedar-river-17.",
  });
  const asked = await chat(db, { session, text: "What is the wifi password?" });
  console.log(`retrieve: ${asked.answer}`);

  const file = await status(db);
  console.log(
    `file schema=${file.version} docs=${file.docs} chunks=${file.chunks} ` +
      `embed=${file.embedProvider}/${file.embedDims}d spend=${file.spend} ` +
      `model=${file.provider}/${file.model}`
  );
}

async function main() {
  const { flags, positional } = parseFlags(process.argv.slice(2));
  const isDemo = Boolean(flags.demo) || positional[0] === "demo";
  const dbPath = path.resolve(
    String(flags.db ?? process.env.AIDB_DB ?? path.join(exampleRoot, "desk.db"))
  );
  const host = process.env.AIDB_API_HOST || "127.0.0.1";
  const port = Number(process.env.AIDB_API_PORT || flags.port || 8092);
  const live = liveRequested(flags);
  if (live && !process.env.KIMI_API_KEY && !process.env.MOONSHOT_API_KEY) {
    throw new Error(
      "live Kimi needs MOONSHOT_API_KEY or KIMI_API_KEY in .env (never in the file)"
    );
  }

  const AI = await loadAI();
  const db = await AI.open(dbPath);
  const use = serialize(db);
  const tokenListeners = new Set();
  if (!isDemo) {
    AI.subscribeTokens((event) => {
      for (const listener of tokenListeners) {
        listener(event);
      }
    });
  }

  const model = await use((conn) => init(conn, { live }));
  const file = await use(status);
  console.log(
    `Chat backend  ${dbPath}\n` +
      `  provider ${model.provider}` +
      (live ? ` model ${model.model || kimiModel()}` : "") +
      `\n  embed ${model.embed.provider}/${model.embed.model} ${model.embed.dimensions}d` +
      `\n  docs ${file.docs} (Knowledge in the UI adds your own; none are shipped)`
  );

  if (isDemo) {
    try {
      await use(demo);
    } finally {
      await db.close();
    }
    return;
  }

  const app = Fastify({ logger: false });

  app.get("/api/health", async () => ({ ok: true, file: dbPath }));

  app.get("/api/status", async (_req, reply) => {
    try {
      return {
        ok: true,
        ...(await use(status)),
        process: processUsage(dbPath),
      };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.post("/api/chat", async (req, reply) => {
    const text = String(req.body?.text ?? "").trim();
    const session = String(req.body?.session ?? "").trim();
    if (!text || !session) {
      return reply.code(400).send({ ok: false, error: "session and text are required" });
    }
    const stream =
      req.body?.stream === true ||
      String(req.headers.accept || "").includes("text/event-stream");
    if (!stream) {
      try {
        const result = await use((conn) => chat(conn, { session, text }));
        return { ok: true, identity: IDENTITY.name, ...result };
      } catch (err) {
        const { statusCode, error } = fail(err);
        return reply.code(statusCode).send({ ok: false, error });
      }
    }

    reply.hijack();
    reply.raw.writeHead(200, {
      "Content-Type": "text/event-stream; charset=utf-8",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    });
    const send = (event) => {
      reply.raw.write(`data: ${JSON.stringify(event)}\n\n`);
    };
    const onToken = (event) => send({ type: "token", ...event });
    tokenListeners.add(onToken);
    try {
      const result = await use((conn) => chat(conn, { session, text }));
      send({ type: "done", identity: IDENTITY.name, ...result });
    } catch (err) {
      send({ type: "error", error: String(err?.message || err) });
    } finally {
      tokenListeners.delete(onToken);
      reply.raw.end();
    }
  });

  app.post("/api/documents", async (req, reply) => {
    const content = String(req.body?.content ?? "").trim();
    if (!content) {
      return reply.code(400).send({ ok: false, error: "content is required" });
    }
    try {
      const inserted = await use((conn) =>
        addDocument(conn, { title: req.body?.title, content })
      );
      return { ok: true, ...inserted };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.get("/api/documents", async (_req, reply) => {
    try {
      return { ok: true, documents: await use(documents) };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.get("/api/sessions", async (_req, reply) => {
    try {
      return { ok: true, sessions: await use(sessions) };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.get("/api/turns", async (req, reply) => {
    const session = String(req.query?.session ?? "").trim();
    if (!session) {
      return reply.code(400).send({ ok: false, error: "session is required" });
    }
    try {
      return { ok: true, turns: await use((conn) => turns(conn, session)) };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  const close = async () => {
    await app.close();
    await db.close();
  };
  process.on("SIGINT", () => {
    close().finally(() => process.exit(0));
  });
  process.on("SIGTERM", () => {
    close().finally(() => process.exit(0));
  });

  await app.listen({ host, port });
  console.log(`listening  http://${host}:${port}`);
}

main().catch((err) => {
  console.error(err.message || err);
  process.exit(1);
});
