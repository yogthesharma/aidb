-- Phase 18 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Support refunds', 'Refunds are issued within 14 days of purchase.', '{\"dept\":\"support\"}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Legal refunds', 'Refunds require a signed legal waiver before processing.', '{\"dept\":\"legal\"}');"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT document_id, chunk_id, content, distance FROM aidb_search('refund policy', 5, '{\"dept\":\"support\"}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT document_id, chunk_id, content, distance FROM aidb_search('refund policy', 5, '{\"dept\":\"support\"}');"
--   cargo run -p aidb-cli -- sql ./app.db "SEARCH 'refund policy' WHERE metadata.dept = 'support' LIMIT 5;"
-- Same aidb_search. Filter is JSON on documents.metadata_json. Same IR Filter.
SELECT document_id, chunk_id, content, distance
FROM aidb_search('refund policy', 5, '{"dept":"support"}');
