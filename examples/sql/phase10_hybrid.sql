-- Phase 10 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'How do refunds work? Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Bin', 'Warehouse bin ZX19QPLUGH holds the discontinued adapter.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work ZX19QPLUGH', 3);"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work ZX19QPLUGH', 3);"
EXPLAIN SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work ZX19QPLUGH', 3);
