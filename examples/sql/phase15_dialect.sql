-- Phase 15 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "CREATE MODEL gpt (kind = llm, provider = fake, provider_model = 'aidb-fake');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "SEARCH 'How do refunds work?' LIMIT 5;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT * FROM documents SEARCH 'How do refunds work?' LIMIT 5;"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SEARCH 'How do refunds work?' LIMIT 5;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT AI_GENERATE('Summarize this', content) FROM documents;"
-- Dialect is convenience. Same IR, same runs, no keys in the file.
SEARCH 'How do refunds work?' LIMIT 5;
