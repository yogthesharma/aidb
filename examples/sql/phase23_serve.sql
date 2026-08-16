-- Phase 23 demo (HTTP over the same file; CLI sql still works without the server):
--   cargo run -p aidb-cli -- serve ./app.db
--   curl -s http://127.0.0.1:8080/health
--   curl -s http://127.0.0.1:8080/sql -d "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
-- Optional: AIDB_BEARER / AIDB_TOKEN. No users table in the file. Same runs table.
SELECT value FROM aidb_meta WHERE key = 'schema_version';
