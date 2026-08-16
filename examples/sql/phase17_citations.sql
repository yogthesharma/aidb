-- Phase 17 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT aidb_generate('What is the refund policy?', content) FROM aidb_search('refund policy', 5);"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_generate('What is the refund policy?', content) FROM aidb_search('refund policy', 5);"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_generate('ping', 'pong');"
-- Generate over search returns { answer, sources[] } from the retrieval nodes.
-- Plain generate stays a string. No citations table.
SELECT aidb_generate('What is the refund policy?', content) FROM aidb_search('refund policy', 5);
