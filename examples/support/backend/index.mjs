#!/usr/bin/env node
// Harbor backend: Fastify + AI.open. SQL never leaves this process.

import path from "node:path";
import { fileURLToPath } from "node:url";
import Fastify from "fastify";

import { SAMPLE_TICKETS } from "./corpus.mjs";
import {
  ask,
  brief,
  classifyTicket,
  digest,
  ingest,
  init,
  recentRuns,
  remember,
  resume,
  status,
  tickets,
  turns,
  waiting,
} from "./desk.mjs";
import { kimiModel, liveRequested, loadAI, parseFlags } from "./load.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const exampleRoot = path.resolve(here, "..");

function serialize(db) {
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

function fail(err) {
  const error = String(err?.message || err);
  const statusCode = /HTTP 4\d\d/.test(error) ? 502 : 500;
  return { statusCode, error };
}

function printAnswer({ answer, sources }) {
  console.log(`\n${answer}\n`);
  if (!sources.length) {
    console.log("sources: none (nothing in the file matched)");
    return;
  }
  console.log("sources:");
  for (const source of sources) {
    console.log(`  ${source.title} [doc ${source.document_id}]`);
  }
}

async function demo(db) {
  const question = "How long do I have to return unused headphones?";
  console.log(`ask: ${question}`);
  printAnswer(await ask(db, question, {}));

  const ticket = SAMPLE_TICKETS[1];
  const classified = await classifyTicket(db, ticket);
  console.log(
    `\nclassified "${ticket.subject}" → ${classified.label} (run ${classified.runId})`
  );

  const parked = await digest(db);
  console.log(`\ndigest ${parked.run_id} ${parked.status}`);
  if (parked.output) {
    console.log(parked.output);
  }

  const file = await status(db);
  console.log(
    `\nfile schema=${file.version} docs=${file.docs} tickets=${file.tickets} ` +
      `spend=${file.spend} waiting=${file.waiting} model=${file.provider}/${file.model}`
  );
}

async function main() {
  const { flags, positional } = parseFlags(process.argv.slice(2));
  const isDemo = Boolean(flags.demo) || positional[0] === "demo";
  const dbPath = path.resolve(
    String(flags.db ?? process.env.AIDB_DB ?? path.join(exampleRoot, "desk.db"))
  );
  const host = process.env.AIDB_API_HOST || "127.0.0.1";
  const port = Number(process.env.AIDB_API_PORT || flags.port || 8091);
  const live = liveRequested(flags);
  if (live && !process.env.KIMI_API_KEY && !process.env.MOONSHOT_API_KEY) {
    throw new Error(
      "live Kimi needs MOONSHOT_API_KEY or KIMI_API_KEY in .env (never in the file)"
    );
  }

  const AI = await loadAI();
  const db = await AI.open(dbPath);
  const use = serialize(db);

  const seeded = await use(async (conn) => {
    const model = await init(conn, { live });
    const loaded = await ingest(conn);
    return { ...model, ...loaded };
  });
  console.log(
    `Harbor backend  ${dbPath}\n` +
      `  provider ${seeded.provider}` +
      (live ? ` model ${seeded.model || kimiModel()}` : "") +
      `\n  ingested ${seeded.ids.length} policy doc(s), skipped ${seeded.skipped}`
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
      return { ok: true, ...(await use(status)) };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.post("/api/ask", async (req, reply) => {
    const question = String(req.body?.question ?? "").trim();
    if (!question) {
      return reply.code(400).send({ ok: false, error: "question is required" });
    }
    try {
      const result = await use((conn) =>
        ask(conn, question, {
          dept: req.body?.dept || undefined,
          agent: req.body?.agent || undefined,
        })
      );
      return { ok: true, ...result };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.post("/api/remember", async (req, reply) => {
    const agent = String(req.body?.agent ?? "maya").trim() || "maya";
    const content = String(req.body?.content ?? "").trim();
    if (!content) {
      return reply.code(400).send({ ok: false, error: "content is required" });
    }
    try {
      const inserted = await use((conn) => remember(conn, agent, content));
      return { ok: true, ...inserted };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.post("/api/brief", async (req, reply) => {
    try {
      const result = await use((conn) =>
        brief(conn, {
          goal: req.body?.goal,
          agent: req.body?.agent,
        })
      );
      return { ok: true, ...result };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.post("/api/digest", async (req, reply) => {
    try {
      const result = await use((conn) => digest(conn, { agent: req.body?.agent }));
      return { ok: true, ...result };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.post("/api/classify", async (req, reply) => {
    const subject = String(req.body?.subject ?? "").trim();
    const body = String(req.body?.body ?? "").trim();
    if (!subject || !body) {
      return reply.code(400).send({ ok: false, error: "subject and body are required" });
    }
    try {
      const result = await use((conn) => classifyTicket(conn, { subject, body }));
      return { ok: true, ...result };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.get("/api/tickets", async (_req, reply) => {
    try {
      return { ok: true, tickets: await use(tickets) };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.get("/api/waiting", async (_req, reply) => {
    try {
      return { ok: true, waiting: await use(waiting) };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.post("/api/resume", async (req, reply) => {
    const runId = String(req.body?.runId ?? req.body?.run_id ?? "").trim();
    if (!runId) {
      return reply.code(400).send({ ok: false, error: "runId is required" });
    }
    try {
      const result = await use((conn) => resume(conn, runId, Boolean(req.body?.approved)));
      return { ok: true, ...result };
    } catch (err) {
      const { statusCode, error } = fail(err);
      return reply.code(statusCode).send({ ok: false, error });
    }
  });

  app.get("/api/runs", async (_req, reply) => {
    try {
      return { ok: true, runs: await use(recentRuns) };
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
