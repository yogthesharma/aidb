-- Phase 14 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations. Explain things briefly.');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT document_id, content FROM aidb_search('How should I explain this?', 5);"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT scope, content FROM memory;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_agent('{\"instructions\":\"Coordinate. End with DONE.\",\"goal\":\"How do refunds work?\",\"tools\":[\"search\"],\"memory\":\"user:123\",\"agents\":[{\"instructions\":\"Answer from documents. End with DONE.\",\"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"generate\"],\"max_steps\":2}]}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT id, parent_id, kind, status FROM runs WHERE kind = 'agent' ORDER BY created_at_ms;"
-- Memory is documents. Multi-agent is child runs. No agents table.
SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations. Explain things briefly.');
