-- Phase 26 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Indemnity', 'The legal indemnity clause survives termination.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_create_space('legal', 'local', 384, 'BAAI/bge-small-en-v1.5', 'cosine');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT name, provider, provider_model, dimensions, distance, vec_table FROM embedding_spaces;"
-- The space owns the embedder. Default open() (fake) does not leak into legal.
SELECT aidb_create_space('legal', 'local', 384, 'BAAI/bge-small-en-v1.5', 'cosine');
