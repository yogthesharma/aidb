-- AIDB schema v004
-- Memory is a view over documents. Applied when schema_version is 3.
-- Do not add an agents table or a second store.

CREATE VIEW IF NOT EXISTS memory AS
SELECT
    id,
    json_extract(metadata_json, '$.scope') AS scope,
    title,
    content,
    metadata_json,
    index_status,
    created_at_ms,
    updated_at_ms
FROM documents
WHERE json_extract(metadata_json, '$.kind') = 'memory';
