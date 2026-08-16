-- Phase 0 demo. Run:
--   cargo run -p aidb-cli -- sql ./app.db "$(cat examples/sql/phase0_open.sql)"
SELECT key, value FROM aidb_meta ORDER BY key;
