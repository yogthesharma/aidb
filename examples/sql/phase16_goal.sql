-- Phase 16 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Logs', 'Deploy failed after the checkout timeout.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "EXPLAIN TASK investigate_incident
-- DATA logs, deployments
-- CONSTRAINTS read_only, budget \$1, timeout 5m
-- GOAL identify_root_cause"
--   cargo run -p aidb-cli -- sql ./app.db "TASK investigate_incident
-- DATA logs, deployments
-- CONSTRAINTS read_only, budget \$1, timeout 5m
-- GOAL identify_root_cause"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT kind, parent_id, status FROM runs ORDER BY created_at_ms;"
-- Goal language emits IR. Optimizer rewrites it. Persisted as a workflow run, not a goals table.
TASK investigate_incident
DATA logs, deployments
CONSTRAINTS read_only, budget $1, timeout 5m
GOAL identify_root_cause
