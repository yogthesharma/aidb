-- AIDB schema v009
-- Phase 34: a session is a thread of runs, not a second store.
-- session_id is a nullable column. sessions / session_turns are views.
-- Memory stays the documents view. Applied when schema_version is 8.
-- Do not rewrite v001.sql.

ALTER TABLE runs ADD COLUMN session_id TEXT;

CREATE INDEX IF NOT EXISTS runs_session_id_idx ON runs (session_id);

CREATE VIEW IF NOT EXISTS sessions AS
SELECT
    session_id AS id,
    COUNT(*) AS runs,
    SUM(CASE WHEN parent_id IS NULL THEN 1 ELSE 0 END) AS turns,
    MIN(created_at_ms) AS started_at_ms,
    MAX(created_at_ms) AS last_at_ms,
    SUM(COALESCE(cost_usd, 0.0)) AS cost_usd
FROM runs
WHERE session_id IS NOT NULL
GROUP BY session_id;

CREATE VIEW IF NOT EXISTS session_turns AS
SELECT
    session_id,
    id AS run_id,
    kind,
    status,
    input_json,
    output_json,
    cost_usd,
    created_at_ms,
    ROW_NUMBER() OVER (
        PARTITION BY session_id
        ORDER BY created_at_ms, id
    ) AS turn
FROM runs
WHERE session_id IS NOT NULL
  AND parent_id IS NULL;
