-- Phase 35 — streaming
--
--   aidb sql app.db "<each statement below>"
--
-- Tokens append to the generate run as `run_events` of kind `token`. Concatenate
-- `$.text` in seq order and you have the prefix a reconnect would read. This is
-- the same generate path — not a second engine. A cache hit does not stream.
-- Same connection: `run_id = aidb_last_run_id()`. A new `aidb sql` process starts
-- empty, so the subquery below names the run from the file.

-- 1. Ordinary generate. The SQL result is still the full text.
SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.');

-- 2. The prefix is events on that run.
SELECT seq, kind, json_extract(payload_json, '$.text') AS text
  FROM run_events
 WHERE run_id = (SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1)
 ORDER BY seq;

-- 3. HTTP can read the same rows: GET /runs/{id}/events
SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1;
