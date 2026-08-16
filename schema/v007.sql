-- AIDB schema v007
-- DESIGN.md §15: "Update with a new content_hash: delete old chunks (cascade +
-- FTS triggers + vec delete), enqueue a new index run. Same hash is a no-op."
--
-- Clearing index_run_id is what enqueues the new run: the engine adopts
-- pending documents without a run on the next write / open. Re-chunking and the
-- vec delete happen in the engine, because vec0 tables are per embedding space.
--
-- Applied when schema_version is 6. Do not rewrite v001.sql.

CREATE TRIGGER IF NOT EXISTS documents_reindex_au
AFTER UPDATE OF content ON documents
WHEN new.content IS NOT old.content
BEGIN
    UPDATE documents
       SET index_status = 'pending',
           index_error  = NULL,
           index_run_id = NULL
     WHERE id = new.id;
END;
