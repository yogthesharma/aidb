-- Phase 8 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT aidb_agent('{\"instructions\":\"Answer from documents. End with DONE.\",\"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"generate\"],\"max_steps\":3}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_agent('{\"instructions\":\"Answer from documents. End with DONE.\",\"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"generate\"],\"max_steps\":3}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT id, parent_id, kind, status FROM runs ORDER BY created_at_ms;"
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":3}');
