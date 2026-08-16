-- Phase 29 demo: an application on one file. The full app is examples/stock
-- (node examples/stock/stock.mjs demo --db ./desk.db); this is the SQL underneath it.
--   cargo run -p aidb-cli -- sql ./app.db "CREATE TABLE IF NOT EXISTS watchlist (ticker TEXT PRIMARY KEY, name TEXT NOT NULL, added_at_ms INTEGER NOT NULL);"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_generate('Answer only from the sources', content) FROM aidb_search('how do refunds work', 3);"
--   cargo run -p aidb-cli -- runs ./app.db --waiting
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_resume('run_…', '{\"approved\":true}');"
-- The desk's own tables live beside the AI state, so a report is one join.
CREATE TABLE IF NOT EXISTS watchlist (
    ticker        TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    added_at_ms   INTEGER NOT NULL
);
INSERT OR IGNORE INTO watchlist (ticker, name, added_at_ms) VALUES ('AAPL', 'Apple Inc.', 0);
-- Tools are catalog rows, and an irreversible one needs a human.
SELECT aidb_mcp_register('{"tools":[{"name":"send.email","inputs":{"to":"string","subject":"string","body":"string"},"side_effect":"irreversible","retry":"forbidden"}]}');
SELECT aidb_set_policy('{"name":"desk","allow":["search","generate","send.email"],"max_usd":0.01,"max_llm_calls":12,"require_approval":["send.email"]}');
-- An answer the desk can defend: generated only from what retrieval returned.
SELECT aidb_generate('Answer only from the sources', content) FROM aidb_search('how do refunds work', 3);
-- The brief is an agent: child runs in the same file.
SELECT aidb_agent('{"instructions":"Brief the desk from the documents. End with DONE.","goal":"What should the desk know today?","tools":["search","generate"],"max_steps":2,"k":3}');
-- The digest wants to email a client, so it parks instead of sending.
SELECT aidb_agent('{"instructions":"Draft the digest, then email it. End with DONE.","goal":"Morning digest","tools":["search","generate","send.email"],"max_steps":3,"k":3}');
SELECT id, kind, status FROM runs WHERE status = 'awaiting_approval';
-- The report is a join between the desk's data and its AI history.
SELECT w.ticker, w.name, (SELECT COUNT(*) FROM runs WHERE kind = 'agent') AS briefs, (SELECT ROUND(COALESCE(SUM(cost_usd), 0), 6) FROM runs) AS spend_usd FROM watchlist w ORDER BY w.ticker;
