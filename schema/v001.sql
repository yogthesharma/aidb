-- AIDB schema v001
-- Applied by the Rust engine on AI.open(). Do not run by hand against a live file
-- except in tests. vec0 is created lazily once embedding dimensions are known.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS aidb_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- schema_version, created_at_ms, embedding_provider, embedding_model,
-- embedding_dimensions, embedding_distance
INSERT OR IGNORE INTO aidb_meta (key, value) VALUES
    ('schema_version', '1'),
    ('embedding_distance', 'cosine');

CREATE TABLE IF NOT EXISTS models (
    name           TEXT PRIMARY KEY,
    kind           TEXT NOT NULL CHECK (kind IN ('llm', 'embedding', 'rerank')),
    provider       TEXT NOT NULL,
    provider_model TEXT NOT NULL,
    dimensions     INTEGER,
    created_at_ms  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS documents (
    id            TEXT PRIMARY KEY,
    title         TEXT,
    content       TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    source_uri    TEXT,
    content_hash  TEXT NOT NULL,
    index_status  TEXT NOT NULL DEFAULT 'pending'
        CHECK (index_status IN ('pending', 'indexing', 'ready', 'failed')),
    index_error   TEXT,
    index_run_id  TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS documents_index_status_idx
    ON documents (index_status);
CREATE INDEX IF NOT EXISTS documents_updated_at_idx
    ON documents (updated_at_ms);

CREATE TABLE IF NOT EXISTS chunks (
    id          INTEGER PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    ordinal     INTEGER NOT NULL,
    content     TEXT NOT NULL,
    token_count INTEGER,
    created_at_ms INTEGER NOT NULL,
    UNIQUE (document_id, ordinal)
);

CREATE INDEX IF NOT EXISTS chunks_document_id_idx
    ON chunks (document_id);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5 (
    content,
    content = 'chunks',
    content_rowid = 'id',
    tokenize = 'porter'
);

CREATE TRIGGER IF NOT EXISTS chunks_fts_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts (rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS chunks_fts_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts (chunks_fts, rowid, content)
        VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS chunks_fts_au AFTER UPDATE OF content ON chunks BEGIN
    INSERT INTO chunks_fts (chunks_fts, rowid, content)
        VALUES ('delete', old.id, old.content);
    INSERT INTO chunks_fts (rowid, content) VALUES (new.id, new.content);
END;

-- vec_chunks is created by the engine, not this file:
--
--   CREATE VIRTUAL TABLE vec_chunks USING vec0(
--     chunk_id INTEGER PRIMARY KEY,
--     embedding float[<dimensions>] distance_metric=cosine,
--     document_id TEXT
--   );
--
-- document_id is a metadata column for filtered KNN. Keep it short (id, not
-- title). Chunk text stays in chunks and is joined after KNN.

CREATE TABLE IF NOT EXISTS runs (
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
            'cancelled'
        )),
    document_id        TEXT REFERENCES documents (id) ON DELETE SET NULL,
    parent_id          TEXT REFERENCES runs (id) ON DELETE SET NULL,
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

CREATE INDEX IF NOT EXISTS runs_status_idx ON runs (status);
CREATE INDEX IF NOT EXISTS runs_kind_idx ON runs (kind);
CREATE INDEX IF NOT EXISTS runs_document_id_idx ON runs (document_id);
CREATE INDEX IF NOT EXISTS runs_created_at_idx ON runs (created_at_ms);

CREATE TABLE IF NOT EXISTS run_events (
    id         INTEGER PRIMARY KEY,
    run_id     TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    payload_json TEXT,
    created_at_ms INTEGER NOT NULL,
    UNIQUE (run_id, seq)
);

CREATE TABLE IF NOT EXISTS checkpoints (
    run_id        TEXT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    node_id       TEXT NOT NULL,
    seq           INTEGER NOT NULL,
    artifact_json TEXT,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (run_id, node_id)
);
