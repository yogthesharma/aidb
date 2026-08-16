// The inspect face is these SELECTs. Studio renders them; it does not compute
// them. Tests and the SQL demo run the same strings.

export const PAGE_SEGMENT = {
  overview: "file",
  sql: "sql",
  documents: "documents",
  search: "search",
  runs: "runs",
  models: "models",
  experiments: "experiments",
};

export const CATALOG_SQL = {
  meta: "SELECT key, value FROM aidb_meta ORDER BY key",
  tables:
    "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%_fts%' ORDER BY type, name",
  documents:
    "SELECT id, title, index_status, (SELECT COUNT(*) FROM chunks c WHERE c.document_id = documents.id) AS chunks, length(content) AS bytes, updated_at_ms, substr(content, 1, 80) AS preview FROM documents ORDER BY updated_at_ms DESC LIMIT 200",
  runs:
    "SELECT id, kind, status, model, prompt_tokens, completion_tokens, cost_usd, created_at_ms, substr(coalesce(error,''), 1, 80) AS error FROM runs ORDER BY created_at_ms DESC LIMIT 100",
  models:
    "SELECT name, kind, provider, provider_model, key_name, dimensions FROM models ORDER BY name",
  experiments:
    "SELECT experiment_id, plan, dataset, examples, accuracy, recall, llm_calls, cost_usd, latency_ms, status, error, run_id, created_at_ms FROM experiment_results ORDER BY created_at_ms DESC LIMIT 100",
  sessions:
    "SELECT id, runs, turns, started_at_ms, last_at_ms, cost_usd FROM sessions ORDER BY last_at_ms DESC LIMIT 100",
  sessionTurns:
    "SELECT session_id, turn, run_id, kind, status, cost_usd, created_at_ms FROM session_turns ORDER BY created_at_ms LIMIT 200",
  nDocuments: "SELECT COUNT(*) FROM documents",
  nRuns: "SELECT COUNT(*) FROM runs",
  nModels: "SELECT COUNT(*) FROM models",
  nWaiting: "SELECT COUNT(*) FROM runs WHERE status = 'awaiting_approval'",
  nExperiments: "SELECT COUNT(*) FROM experiment_results",
  tokens:
    "SELECT seq, kind, json_extract(payload_json, '$.text') AS text, created_at_ms FROM run_events WHERE kind = 'token' AND run_id = (SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1) ORDER BY seq LIMIT 200",
};

export function sqlString(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

export function searchSql(query, k) {
  const n = Math.max(1, Number(k) || 5);
  return `SELECT document_id, chunk_id, substr(content, 1, 200) AS content, distance FROM aidb_search(${sqlString(query)}, ${n})`;
}

export function resumeSql(runId, approved) {
  const decision = JSON.stringify({ approved: Boolean(approved) });
  return `SELECT aidb_resume(${sqlString(runId)}, ${sqlString(decision)})`;
}
