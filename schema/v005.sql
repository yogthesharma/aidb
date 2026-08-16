-- AIDB schema v005
-- Named embedding spaces. Applied when schema_version is 4.
-- Do not rewrite vec_chunks. The default space stays aidb_meta + vec_chunks.

CREATE TABLE IF NOT EXISTS embedding_spaces (
    name           TEXT PRIMARY KEY,
    provider       TEXT NOT NULL,
    provider_model TEXT NOT NULL,
    dimensions     INTEGER NOT NULL,
    distance       TEXT NOT NULL DEFAULT 'cosine',
    vec_table      TEXT NOT NULL UNIQUE,
    created_at_ms  INTEGER NOT NULL
);
