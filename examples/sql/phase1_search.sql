-- Phase 1 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT id, index_status FROM documents;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5);"
SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');
