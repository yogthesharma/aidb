-- Phase 25 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "CREATE MODEL gpt PROVIDER openai KEY_NAME 'OPENAI_API_KEY';"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT name, provider, key_name FROM models;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_secret_store();"
-- Keys stay out of app.db. Env is first. Optional store: AIDB_SECRET_STORE=keychain or file:/path.
-- Reopen without the store is the same missing-key error, not a corrupt file.
CREATE MODEL gpt PROVIDER openai KEY_NAME 'OPENAI_API_KEY';
SELECT name, provider, key_name FROM models;
SELECT aidb_secret_store();
