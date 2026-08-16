-- Phase 33 — dynamic agent (decide)
--
--   aidb sql app.db "<each statement below>"
--
-- The recipe agent still runs listed tools in order. `"decide":true` makes each
-- step a schema-valid choice: search, generate, a catalog tool, or stop. The
-- model passes arguments (a search filter, an email recipient). Each choice is a
-- child run plus a checkpoint. There is still no agents table.

-- 1. Recipe agents are unchanged.
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":3}');

-- 2. A decide agent chooses the next operator. EXPLAIN names the loop.
EXPLAIN SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":4,"decide":true}');

-- 3. Run it. Checkpoints are a.{step}.decide then a.{step}.{op}.
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":4,"decide":true}');

SELECT node_id, substr(artifact_json, 1, 80)
  FROM checkpoints
 WHERE run_id = (SELECT id FROM runs WHERE kind = 'agent' ORDER BY created_at_ms DESC LIMIT 1)
 ORDER BY node_id;

-- 4. The choices are ordinary child runs. Spend rolls up to the agent.
SELECT kind, status, COALESCE(cost_usd, 0)
  FROM runs
 WHERE parent_id = (SELECT id FROM runs WHERE kind = 'agent' ORDER BY created_at_ms DESC LIMIT 1)
 ORDER BY created_at_ms, rowid;
