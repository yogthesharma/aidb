-- Phase 20 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "CREATE MODEL IF NOT EXISTS cls PROVIDER 'fake' KIND 'llm';"
--   cargo run -p aidb-cli -- sql ./app.db "CREATE MODEL IF NOT EXISTS claude PROVIDER 'anthropic' KIND 'llm';"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'This refund was a negative surprise.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_classify('positive or negative', content) FROM documents LIMIT 3;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT kind, input_json FROM runs WHERE kind = 'generate';"
-- Same connection after classify: SELECT aidb_last_run_id();
-- Classify is a thin UDF. Same models catalog, same generate runs. No classify store.
-- Extra providers (anthropic) register the same way. Keys stay in the environment.
CREATE MODEL IF NOT EXISTS cls PROVIDER 'fake' KIND 'llm';
