-- Phase 5 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"How do refunds work?\",\"k\":5}},{\"generate\":{\"prompt\":\"Summarize this\"}}]}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"How do refunds work?\",\"k\":5}},{\"generate\":{\"prompt\":\"Summarize this\"}}]}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT id, parent_id, kind, status FROM runs ORDER BY created_at_ms;"
SELECT aidb_workflow('{"then":[{"search":{"query":"How do refunds work?","k":5}},{"generate":{"prompt":"Summarize this"}}]}');
