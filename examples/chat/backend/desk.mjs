// Chat operations. SQL lives here. No canned corpus — the file starts empty.

import { json, kimiKeyName, kimiModel, scalar, sql } from "../../lib/aidb.mjs";

const POLICY = {
  name: "chat",
  allow: ["search", "generate"],
  max_usd: 1,
  max_llm_calls: 32,
};

function embedSpec() {
  if (process.env.OPENAI_API_KEY) {
    return {
      provider: "openai",
      model: process.env.AIDB_EMBED_MODEL || "text-embedding-3-small",
      dimensions: 1536,
      keyName: "OPENAI_API_KEY",
    };
  }
  return {
    provider: "fake",
    model: "aidb-fake-chat",
    dimensions: Number(process.env.AIDB_EMBED_DIMS || 256),
    keyName: null,
  };
}

async function ensureSpace(db, spec) {
  const exists = Number(
    await scalar(db, "SELECT COUNT(*) FROM embedding_spaces WHERE name = 'chat'")
  );
  if (exists > 0) {
    return;
  }
  const key = spec.keyName ? ` KEY_NAME ${sql(spec.keyName)}` : "";
  await db.execute(
    `CREATE MODEL embed PROVIDER ${spec.provider} KIND embedding MODEL ${sql(spec.model)} DIMENSIONS ${sql(String(spec.dimensions))}${key}`
  );
  await db.query(
    `SELECT aidb_create_space('chat', ${sql(spec.provider)}, ${spec.dimensions}, ${sql(spec.model)}, 'cosine')`
  );
  await waitIndexed(db);
}

export async function init(db, { live }) {
  if (live) {
    const model = kimiModel();
    const keyName = kimiKeyName();
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
  const embed = embedSpec();
  await ensureSpace(db, embed);
  await db.query(`SELECT aidb_set_policy(${json(POLICY)})`);
  return {
    provider: live ? "kimi" : "fake",
    model: live ? kimiModel() : "aidb-fake",
    embed,
  };
}

export const IDENTITY = {
  name: "Ada",
  instructions:
    "You are Ada. You live in this AIDB file — one SQLite database that holds documents, model runs, and this chat. " +
    "Speak like a sharp colleague: concise, specific, no corporate filler. " +
    "You are not a support desk and you have no canned product policies. " +
    "If the user has not put documents in the file, answer from general knowledge and say when you are guessing. " +
    "When they ask who you are, say you are Ada, an assistant that runs as generate runs in this file.",
};

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

function parseOutput(raw) {
  try {
    const value = JSON.parse(raw);
    if (value && typeof value === "object" && value.answer) {
      return { answer: String(value.answer), sources: value.sources ?? [] };
    }
  } catch {
    // plain model text
  }
  return { answer: raw, sources: [] };
}

async function titleOf(db, documentId) {
  return scalar(db, `SELECT title FROM documents WHERE id = ${sql(documentId)}`);
}

async function readyDocs(db) {
  return Number(
    await scalar(
      db,
      `SELECT COUNT(*) FROM documents WHERE index_status = 'ready'
       AND COALESCE(json_extract(metadata_json, '$.kind'), '') != 'memory'`
    )
  );
}

async function historyBlock(db, session) {
  const prior = (await turns(db, session)).slice(-8);
  if (!prior.length) {
    return "";
  }
  return prior
    .map((turn) => `User: ${turn.user}\nAssistant: ${turn.assistant}`)
    .join("\n\n");
}

export async function chat(db, { session, text }) {
  await db.session(session);
  const history = await historyBlock(db, session);
  const prompt = [
    IDENTITY.instructions,
    history ? `Conversation so far:\n${history}` : "",
    `User: ${text}`,
  ]
    .filter(Boolean)
    .join("\n\n");

  const docs = await readyDocs(db);
  const raw =
    docs > 0
      ? await scalar(
          db,
          `SELECT aidb_generate(${sql(prompt)}, content)
           FROM aidb_search(${sql(text)}, 5, NULL, 'chat')`
        )
      : await scalar(db, `SELECT aidb_generate(${sql(prompt)}, ${sql(text)})`);

  const parsed = parseOutput(raw);
  const sources = [];
  for (const source of parsed.sources ?? []) {
    sources.push({ ...source, title: await titleOf(db, source.document_id) });
  }
  return {
    answer: parsed.answer ?? raw,
    sources,
    runId: await db.lastRunId(),
  };
}

export async function addDocument(db, { title, content }) {
  const id = await scalar(
    db,
    `SELECT aidb_insert_document(${sql(title || "Untitled")}, ${sql(content)}, '{}')`
  );
  await waitIndexed(db);
  return { id };
}

export async function documents(db) {
  const { rows } = await db.query(
    `SELECT id, title, length(content), index_status
     FROM documents
     WHERE COALESCE(json_extract(metadata_json, '$.kind'), '') != 'memory'
     ORDER BY updated_at_ms DESC LIMIT 40`
  );
  return rows.map(([id, title, bytes, indexStatus]) => ({
    id,
    title,
    bytes,
    indexStatus,
  }));
}

function userFromInput(inputJson) {
  try {
    const value = JSON.parse(inputJson);
    const prompt = String(value.prompt ?? "");
    const idx = prompt.lastIndexOf("User: ");
    if (idx >= 0) {
      return prompt.slice(idx + 6).trim();
    }
    return String(value.content ?? "");
  } catch {
    return "";
  }
}

export async function turns(db, session) {
  const { rows } = await db.query(
    `SELECT turn, kind, status, COALESCE(input_json, ''), COALESCE(output_json, ''), COALESCE(run_id, '')
     FROM session_turns
     WHERE session_id = ${sql(session)} AND kind = 'generate'
     ORDER BY turn`
  );
  return rows.map(([turn, kind, status, input, output, runId]) => {
    const parsed = parseOutput(String(output));
    return {
      turn,
      kind,
      status,
      user: userFromInput(String(input)),
      assistant: parsed.answer,
      sources: parsed.sources ?? [],
      runId,
    };
  });
}

export async function sessions(db) {
  const { rows } = await db.query(
    `SELECT id, runs, turns, last_at_ms, COALESCE(cost_usd, 0)
     FROM sessions ORDER BY last_at_ms DESC LIMIT 40`
  );
  return rows.map(([id, runs, turns, last, cost]) => ({
    id,
    runs,
    turns,
    last,
    cost,
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
  const pending = await scalar(
    db,
    "SELECT COUNT(*) FROM documents WHERE index_status != 'ready'"
  );
  const chunks = await scalar(db, "SELECT COUNT(*) FROM chunks");
  const spend = await scalar(
    db,
    "SELECT ROUND(COALESCE(SUM(cost_usd), 0), 6) FROM runs"
  );
  const promptTokens = await scalar(
    db,
    "SELECT COALESCE(SUM(prompt_tokens), 0) FROM runs"
  );
  const completionTokens = await scalar(
    db,
    "SELECT COALESCE(SUM(completion_tokens), 0) FROM runs"
  );
  const generates = await scalar(
    db,
    "SELECT COUNT(*) FROM runs WHERE kind = 'generate'"
  );
  const embeds = await scalar(
    db,
    "SELECT COUNT(*) FROM runs WHERE kind IN ('index_document', 'embed_query')"
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
  const embedProvider = await scalar(
    db,
    "SELECT provider FROM embedding_spaces WHERE name = 'chat' LIMIT 1"
  );
  const embedModel = await scalar(
    db,
    "SELECT provider_model FROM embedding_spaces WHERE name = 'chat' LIMIT 1"
  );
  const embedDims = await scalar(
    db,
    "SELECT dimensions FROM embedding_spaces WHERE name = 'chat' LIMIT 1"
  );
  let vectors = "0";
  try {
    vectors = await scalar(db, "SELECT COUNT(*) FROM vec_chunks_chat");
  } catch {
    // space table appears after the first index
  }
  return {
    version,
    docs,
    pending,
    chunks,
    vectors,
    spend,
    promptTokens,
    completionTokens,
    generates,
    embeds,
    provider,
    model,
    embedProvider,
    embedModel,
    embedDims,
    name: IDENTITY.name,
  };
}
