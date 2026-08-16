-- Phase 6 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_generate('How do refunds work?', content) FROM documents;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT kind, status, prompt_tokens, cost_usd FROM runs ORDER BY created_at_ms;"
EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents WHERE index_status = 'ready';
