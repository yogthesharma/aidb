-- Phase 11 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'How do refunds work? Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_generate('How do refunds work?', content) FROM documents;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT prompt_tokens, completion_tokens, cost_usd, output_json, finished_at_ms - started_at_ms FROM runs WHERE kind = 'generate';"
-- Budgets: AIDB_MAX_USD, AIDB_MAX_MS, AIDB_MAX_LLM_CALLS
EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents;
