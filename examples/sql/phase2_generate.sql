-- Phase 2 demo (run each statement separately):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "INSERT INTO models (name, kind, provider, provider_model, created_at_ms) VALUES ('fake-llm', 'llm', 'fake', 'aidb-fake', 0);"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_generate('Summarize this', content) FROM documents;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT id, status, prompt_tokens, cost_usd FROM runs WHERE kind = 'generate';"
SELECT aidb_generate('Summarize this', content) FROM documents;
