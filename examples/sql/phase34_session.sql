-- Phase 34 — sessions
--
--   aidb sql app.db "<each statement below>"
--
-- A session is a thread of runs, not a second memory store. Bind a name on this
-- connection, then generate / agent / workflow rows pick it up. Turn 1 / 2 / 3
-- is `session_turns`. Memory stays the documents view.

-- 1. Bind this connection. EXPLAIN names the control, not a second engine.
EXPLAIN SELECT aidb_session('desk:nvda');
SELECT aidb_session('desk:nvda');

-- 2. Two generates are two turns.
SELECT aidb_generate('What is NVDA?', 'Data center revenue was 47.5 billion.');
SELECT aidb_generate('And the risk?', 'Supply concentration in Taiwan.');

SELECT turn, kind, json_extract(input_json, '$.prompt') AS prompt
  FROM session_turns
 WHERE session_id = 'desk:nvda'
 ORDER BY turn;

-- 3. A second session does not leak into the first.
SELECT aidb_session('desk:aapl');
SELECT aidb_generate('What is AAPL?', 'Gross margin 46 to 47 percent.');

SELECT id, turns, runs FROM sessions ORDER BY id;

-- 4. An agent can name the session without relying on the current bind.
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":2,"session":"desk:nvda"}');

-- 5. Children inherit. Turns stay the parent rows. Memory is still documents.
SELECT kind, COUNT(*) FROM runs WHERE session_id = 'desk:nvda' GROUP BY kind ORDER BY kind;
SELECT type FROM sqlite_master WHERE name IN ('memory', 'sessions', 'session_turns') ORDER BY name;
