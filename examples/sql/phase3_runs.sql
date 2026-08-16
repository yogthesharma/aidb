-- Phase 3 demo (run each statement separately via `aidb sql` / `aidb runs`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5);"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_generate('Summarize this', content) FROM documents;"
--   cargo run -p aidb-cli -- runs ./app.db
--   cargo run -p aidb-cli -- sql ./app.db "SELECT * FROM runs WHERE status = 'failed';"
SELECT id, kind, status, error, created_at_ms FROM runs ORDER BY created_at_ms DESC;
