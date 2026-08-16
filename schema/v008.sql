-- AIDB schema v008
-- Phase 31: experiments live in the file. A dataset is data you INSERT, an
-- experiment is a run, and a plan's result is that run's child. There is no
-- second run store and no eval service.
--
-- Applied when schema_version is 7. Do not rewrite v001.sql.

PRAGMA foreign_keys = OFF;

-- Labeled examples. Gold is either the text an answer has to contain, or the
-- documents retrieval has to find, or both — but never neither.
CREATE TABLE IF NOT EXISTS eval_examples (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    dataset           TEXT NOT NULL,
    question          TEXT NOT NULL,
    expect_text       TEXT,
    expect_documents  TEXT NOT NULL DEFAULT '[]',
    created_at_ms     INTEGER NOT NULL DEFAULT 0,
    CHECK (expect_text IS NOT NULL OR expect_documents <> '[]')
);

CREATE INDEX IF NOT EXISTS eval_examples_dataset_idx ON eval_examples (dataset);

-- `experiment` is a run kind: the parent is the comparison, each child is a plan.
CREATE TABLE runs_v008 (
    id                 TEXT PRIMARY KEY,
    kind               TEXT NOT NULL
        CHECK (kind IN (
            'index_document',
            'embed_query',
            'search',
            'generate',
            'workflow',
            'agent',
            'tool',
            'experiment'
        )),
    status             TEXT NOT NULL
        CHECK (status IN (
            'pending',
            'running',
            'succeeded',
            'failed',
            'cancelled',
            'suspended',
            'awaiting_approval'
        )),
    document_id        TEXT REFERENCES documents (id) ON DELETE SET NULL,
    parent_id          TEXT REFERENCES runs_v008 (id) ON DELETE SET NULL,
    model              TEXT REFERENCES models (name) ON DELETE SET NULL,
    input_json         TEXT,
    output_json        TEXT,
    error              TEXT,
    prompt_tokens      INTEGER,
    completion_tokens  INTEGER,
    cost_usd           REAL,
    created_at_ms      INTEGER NOT NULL,
    started_at_ms      INTEGER,
    finished_at_ms     INTEGER
);

INSERT INTO runs_v008 (
    id, kind, status, document_id, parent_id, model, input_json, output_json, error,
    prompt_tokens, completion_tokens, cost_usd, created_at_ms, started_at_ms, finished_at_ms
)
SELECT
    id, kind, status, document_id, parent_id, model, input_json, output_json, error,
    prompt_tokens, completion_tokens, cost_usd, created_at_ms, started_at_ms, finished_at_ms
FROM runs;

DROP TABLE runs;
ALTER TABLE runs_v008 RENAME TO runs;

CREATE INDEX IF NOT EXISTS runs_status_idx ON runs (status);
CREATE INDEX IF NOT EXISTS runs_kind_idx ON runs (kind);
CREATE INDEX IF NOT EXISTS runs_document_id_idx ON runs (document_id);
CREATE INDEX IF NOT EXISTS runs_created_at_idx ON runs (created_at_ms);
CREATE INDEX IF NOT EXISTS runs_parent_id_idx ON runs (parent_id);

-- One row per plan per experiment: what it cost, how long it took, how good it was.
-- A view, not a table, because the runs already hold every one of these numbers.
CREATE VIEW IF NOT EXISTS experiment_results AS
SELECT
    r.parent_id                                      AS experiment_id,
    json_extract(r.output_json, '$.plan')            AS plan,
    json_extract(r.output_json, '$.dataset')         AS dataset,
    json_extract(r.output_json, '$.examples')        AS examples,
    json_extract(r.output_json, '$.accuracy')        AS accuracy,
    json_extract(r.output_json, '$.recall')          AS recall,
    json_extract(r.output_json, '$.llm_calls')       AS llm_calls,
    COALESCE(r.cost_usd, 0.0)                        AS cost_usd,
    COALESCE(r.finished_at_ms - r.started_at_ms, 0)  AS latency_ms,
    r.status                                         AS status,
    r.error                                          AS error,
    r.id                                             AS run_id,
    r.created_at_ms                                  AS created_at_ms
FROM runs r
WHERE r.kind = 'experiment' AND r.parent_id IS NOT NULL;

PRAGMA foreign_keys = ON;
