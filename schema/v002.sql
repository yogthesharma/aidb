-- AIDB schema v002
-- Human-in-the-loop run states. Applied by migrate when schema_version is 1.
-- Do not add workflow / approval tables. Approval is a run status, not an IR node.

PRAGMA foreign_keys = OFF;

CREATE TABLE runs_v002 (
    id                 TEXT PRIMARY KEY,
    kind               TEXT NOT NULL
        CHECK (kind IN (
            'index_document',
            'embed_query',
            'search',
            'generate',
            'workflow',
            'agent'
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
    parent_id          TEXT REFERENCES runs_v002 (id) ON DELETE SET NULL,
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

INSERT INTO runs_v002 (
    id, kind, status, document_id, parent_id, model, input_json, output_json, error,
    prompt_tokens, completion_tokens, cost_usd, created_at_ms, started_at_ms, finished_at_ms
)
SELECT
    id, kind, status, document_id, parent_id, model, input_json, output_json, error,
    prompt_tokens, completion_tokens, cost_usd, created_at_ms, started_at_ms, finished_at_ms
FROM runs;

DROP TABLE runs;
ALTER TABLE runs_v002 RENAME TO runs;

CREATE INDEX IF NOT EXISTS runs_status_idx ON runs (status);
CREATE INDEX IF NOT EXISTS runs_kind_idx ON runs (kind);
CREATE INDEX IF NOT EXISTS runs_document_id_idx ON runs (document_id);
CREATE INDEX IF NOT EXISTS runs_created_at_idx ON runs (created_at_ms);

PRAGMA foreign_keys = ON;
