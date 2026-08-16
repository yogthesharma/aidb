-- Phase 4 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5);"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT aidb_generate('Summarize this', content) FROM documents;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_explain('SELECT aidb_search(''How do refunds work?'', 5)');"
EXPLAIN SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5);
