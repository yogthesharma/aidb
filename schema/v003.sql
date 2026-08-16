-- AIDB schema v003
-- Capability catalog + tool run kind. Applied by migrate when schema_version is 2.
-- Do not rewrite v001.sql or v002.sql. MCP writes rows here; it is not a second store.

PRAGMA foreign_keys = OFF;

CREATE TABLE IF NOT EXISTS capabilities (
    name           TEXT PRIMARY KEY,
    inputs         TEXT NOT NULL DEFAULT '{}',
    outputs        TEXT NOT NULL DEFAULT '{}',
    side_effect    TEXT NOT NULL
        CHECK (side_effect IN ('none', 'reversible', 'irreversible')),
    retry          TEXT NOT NULL
        CHECK (retry IN ('safe', 'conditional', 'forbidden')),
    source         TEXT NOT NULL DEFAULT 'builtin'
        CHECK (source IN ('builtin', 'mcp', 'app')),
    enabled        INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at_ms  INTEGER NOT NULL
);

INSERT OR IGNORE INTO capabilities
    (name, inputs, outputs, side_effect, retry, source, enabled, created_at_ms)
VALUES
    ('search', '{"query":"string","k":"integer"}', '{"hits":"rows"}', 'none', 'safe', 'builtin', 1, 0),
    ('generate', '{"prompt":"string","content":"string"}', '{"text":"string"}', 'none', 'safe', 'builtin', 1, 0);

CREATE TABLE runs_v003 (
    id                 TEXT PRIMARY KEY,
    kind               TEXT NOT NULL
        CHECK (kind IN (
            'index_document',
            'embed_query',
            'search',
            'generate',
            'workflow',
            'agent',
            'tool'
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
    parent_id          TEXT REFERENCES runs_v003 (id) ON DELETE SET NULL,
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

INSERT INTO runs_v003 (
    id, kind, status, document_id, parent_id, model, input_json, output_json, error,
    prompt_tokens, completion_tokens, cost_usd, created_at_ms, started_at_ms, finished_at_ms
)
SELECT
    id, kind, status, document_id, parent_id, model, input_json, output_json, error,
    prompt_tokens, completion_tokens, cost_usd, created_at_ms, started_at_ms, finished_at_ms
FROM runs;

DROP TABLE runs;
ALTER TABLE runs_v003 RENAME TO runs;

CREATE INDEX IF NOT EXISTS runs_status_idx ON runs (status);
CREATE INDEX IF NOT EXISTS runs_kind_idx ON runs (kind);
CREATE INDEX IF NOT EXISTS runs_document_id_idx ON runs (document_id);
CREATE INDEX IF NOT EXISTS runs_created_at_idx ON runs (created_at_ms);
CREATE INDEX IF NOT EXISTS runs_parent_id_idx ON runs (parent_id);

PRAGMA foreign_keys = ON;
