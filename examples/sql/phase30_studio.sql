-- Phase 30 — Studio inspect face
--
--   cargo run -p aidb-cli -- serve ./app.db
--   cd studio && npm install && npm run dev
--
-- Studio is a browser over POST /sql. These are the statements the pages run.
-- Optional: AIDB_BEARER / AIDB_TOKEN. Approve is still aidb_resume, not a users table.

-- File: aidb_meta
SELECT key, value FROM aidb_meta ORDER BY key;

-- Documents
SELECT id, title, index_status FROM documents ORDER BY updated_at_ms DESC LIMIT 20;

-- Search
SELECT document_id, chunk_id, substr(content, 1, 200) AS content, distance
  FROM aidb_search('How do refunds work?', 5);

-- Runs, including the waiting badge
SELECT id, kind, status, cost_usd, created_at_ms
  FROM runs
 ORDER BY created_at_ms DESC
 LIMIT 20;

SELECT COUNT(*) FROM runs WHERE status = 'awaiting_approval';

-- Models (key_name only)
SELECT name, kind, provider, provider_model, key_name FROM models;

-- Experiments: a view over runs, not a second store
SELECT plan, dataset, examples, accuracy, recall, llm_calls, cost_usd, latency_ms, status
  FROM experiment_results
 ORDER BY created_at_ms DESC;
