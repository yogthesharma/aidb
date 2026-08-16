-- Phase 22 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Indemnity', 'The legal indemnity clause survives termination.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_create_space('legal', 'fake', 32);"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT document_id FROM aidb_search('indemnity', 5);"
-- Named space gets its own vec table. Default space stays vec_chunks + aidb_meta.
SELECT aidb_create_space('legal', 'fake', 32);
