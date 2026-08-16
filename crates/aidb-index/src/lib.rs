//! Document indexing: chunk, embed, sqlite-vec, search.

mod space;

use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use std::collections::HashMap;

use aidb_ai::Embedder;
use aidb_core::{content_hash, new_id, now_ms, Error, QueryResult, Result, Retrieval, Value};
use aidb_storage::{sqlite_err, Connection, Store};

pub use space::{create_and_fill, EmbeddingSpace, DEFAULT_SPACE, DEFAULT_VEC_TABLE};

const CHUNK_CHARS: usize = 800;
const CHUNK_OVERLAP: usize = 80;
/// Largest k a `vec0` KNN probe accepts.
const MAX_KNN_K: i64 = 4096;

/// Metadata is queried with `json_extract` by search filters and the memory view,
/// so a non-object value would silently never match. Reject it at the door.
fn validate_metadata(metadata_json: &str) -> Result<()> {
    let trimmed = metadata_json.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::Object(_)) => Ok(()),
        Ok(_) => Err(Error::usage("document metadata must be a JSON object")),
        Err(err) => Err(Error::usage(format!(
            "document metadata must be a JSON object: {err}"
        ))),
    }
}

pub fn insert_document(
    conn: &Connection,
    title: Option<&str>,
    content: &str,
    metadata_json: &str,
) -> Result<String> {
    validate_metadata(metadata_json)?;
    let id = new_id("doc");
    let now = now_ms();
    let hash = content_hash(content);
    let run_id = new_id("run");
    conn.execute(
        "INSERT INTO documents
            (id, title, content, metadata_json, content_hash, index_status, index_run_id, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?7)",
        rusqlite::params![id, title, content, metadata_json, hash, run_id, now],
    )
    .map_err(sqlite_err)?;
    aidb_run::insert_run(conn, &run_id, "index_document", "pending", Some(&id))?;
    aidb_run::append_event(conn, &run_id, "enqueued", None)?;
    Ok(id)
}

pub fn enqueue_untracked(conn: &Connection) -> Result<usize> {
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM documents
                 WHERE index_status IN ('pending', 'indexing')
                   AND (index_run_id IS NULL OR index_run_id = '')",
            )
            .map_err(sqlite_err)?;
        let rows = stmt.query_map([], |row| row.get(0)).map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)?
    };
    for id in &ids {
        let run_id = new_id("run");
        aidb_run::insert_run(conn, &run_id, "index_document", "pending", Some(id))?;
        conn.execute(
            "UPDATE documents SET index_run_id = ?1 WHERE id = ?2",
            rusqlite::params![run_id, id],
        )
        .map_err(sqlite_err)?;
    }
    Ok(ids.len())
}

pub fn next_document_id(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM documents
         WHERE index_status IN ('pending', 'indexing')
         ORDER BY CASE index_status WHEN 'indexing' THEN 0 ELSE 1 END, created_at_ms
         LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(sqlite_err)
}

pub fn pending_count(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM documents WHERE index_status IN ('pending', 'indexing')",
        [],
        |row| row.get(0),
    )
    .map_err(sqlite_err)
}

pub fn lock_embedding_space(
    conn: &Connection,
    provider: &str,
    model: &str,
    dimensions: usize,
) -> Result<()> {
    let existing = meta(conn, "embedding_dimensions")?;
    if let Some(value) = existing {
        let stored: usize = value
            .parse()
            .map_err(|_| Error::schema("embedding_dimensions is not an integer"))?;
        if stored != dimensions {
            return Err(Error::schema(format!(
                "database embedding dimensions are {stored}, engine opened with {dimensions}"
            )));
        }
        if let Some(stored) = meta(conn, "embedding_provider")? {
            if stored != provider {
                return Err(Error::schema(format!(
                    "database embedding provider is {stored}, engine opened with {provider}"
                )));
            }
        }
        if let Some(stored) = meta(conn, "embedding_model")? {
            if stored != model {
                return Err(Error::schema(format!(
                    "database embedding model is {stored}, engine opened with {model}"
                )));
            }
        }
        return Ok(());
    }
    upsert_meta(conn, "embedding_provider", provider)?;
    upsert_meta(conn, "embedding_model", model)?;
    upsert_meta(conn, "embedding_dimensions", &dimensions.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO models (name, kind, provider, provider_model, dimensions, created_at_ms)
         VALUES (?1, 'embedding', ?2, ?3, ?4, ?5)",
        rusqlite::params![model, provider, model, dimensions as i64, now_ms()],
    )
    .map_err(sqlite_err)?;
    Ok(())
}

pub fn ensure_vec_chunks(conn: &Connection, dimensions: usize) -> Result<()> {
    let distance = meta(conn, "embedding_distance")?.unwrap_or_else(|| "cosine".into());
    ensure_vec_table(conn, DEFAULT_VEC_TABLE, dimensions, &distance)
}

/// vec0 tables cannot be a foreign key target, so a chunk delete cannot cascade
/// into them. This trigger is the vec equivalent of the FTS delete trigger in
/// v001: it keeps `DELETE FROM documents` from leaving orphaned vectors behind.
/// It lives in the engine because the set of vec tables is per embedding space.
fn ensure_vec_delete_trigger(conn: &Connection, table: &str) -> Result<()> {
    assert_vec_table(table)?;
    conn.execute_batch(&format!(
        "CREATE TRIGGER IF NOT EXISTS {table}_chunk_ad AFTER DELETE ON chunks BEGIN
            DELETE FROM {table} WHERE chunk_id = old.id;
        END"
    ))
    .map_err(sqlite_err)?;
    Ok(())
}

/// Drop vectors whose chunk is gone. Only needed for files written before the
/// delete trigger existed; new deletes are handled by the trigger.
pub fn prune_orphan_vectors(conn: &Connection) -> Result<usize> {
    let mut removed = 0;
    for table in vec_tables(conn)? {
        ensure_vec_delete_trigger(conn, &table)?;
        let orphans: Vec<i64> = {
            let sql = format!(
                "SELECT v.chunk_id FROM {table} v
                 WHERE NOT EXISTS (SELECT 1 FROM chunks c WHERE c.id = v.chunk_id)"
            );
            let mut stmt = conn.prepare(&sql).map_err(sqlite_err)?;
            let rows = stmt.query_map([], |row| row.get(0)).map_err(sqlite_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_err)?
        };
        for chunk_id in orphans {
            conn.execute(
                &format!("DELETE FROM {table} WHERE chunk_id = ?1"),
                [chunk_id],
            )
            .map_err(sqlite_err)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Every vector table in the file: the default space plus one per named space.
pub(crate) fn vec_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut tables = Vec::new();
    if table_exists(conn, DEFAULT_VEC_TABLE) {
        tables.push(DEFAULT_VEC_TABLE.to_string());
    }
    for space in space::list(conn)? {
        if table_exists(conn, &space.vec_table) {
            tables.push(space.vec_table);
        }
    }
    Ok(tables)
}

pub(crate) fn ensure_vec_table(
    conn: &Connection,
    table: &str,
    dimensions: usize,
    distance: &str,
) -> Result<()> {
    assert_vec_table(table)?;
    if table_exists(conn, table) {
        return ensure_vec_delete_trigger(conn, table);
    }
    let metric = if distance.eq_ignore_ascii_case("l2") {
        "l2"
    } else {
        "cosine"
    };
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {table} USING vec0(
            chunk_id INTEGER PRIMARY KEY,
            embedding float[{dimensions}] distance_metric={metric},
            document_id TEXT
        )"
    ))
    .map_err(sqlite_err)?;
    ensure_vec_delete_trigger(conn, table)?;
    Ok(())
}

fn assert_vec_table(table: &str) -> Result<()> {
    if table == DEFAULT_VEC_TABLE {
        return Ok(());
    }
    let rest = table
        .strip_prefix("vec_chunks_")
        .ok_or_else(|| Error::usage(format!("invalid vec table: {table}")))?;
    space::validate_name(rest)?;
    Ok(())
}

pub fn index_document(store: &Store, embedder: &dyn Embedder, doc_id: &str) -> Result<()> {
    store.write(|conn| {
        lock_embedding_space(
            conn,
            embedder.provider(),
            embedder.model(),
            embedder.dimensions(),
        )?;
        ensure_vec_chunks(conn, embedder.dimensions())?;
        Ok(())
    })?;

    aidb_core::crash_point("before_chunk");
    let needed = store.write(|conn| prepare_chunks(conn, doc_id))?;
    aidb_core::crash_point("after_chunk");
    if needed.is_empty() {
        return store.write(|conn| finish_document(conn, doc_id, true, None));
    }

    let texts: Vec<String> = needed.iter().map(|(_, text)| text.clone()).collect();
    let vectors = embedder.embed(&texts)?;
    aidb_core::crash_point("after_embed");
    if vectors.len() != needed.len() {
        return store.write(|conn| {
            finish_document(
                conn,
                doc_id,
                false,
                Some("embedder returned the wrong number of vectors"),
            )
        });
    }

    store.write(|conn| {
        for ((chunk_id, _), vector) in needed.iter().zip(vectors.iter()) {
            if vector.len() != embedder.dimensions() {
                return finish_document(conn, doc_id, false, Some("embedding dimension mismatch"));
            }
            conn.execute(
                "INSERT INTO vec_chunks (chunk_id, embedding, document_id) VALUES (?1, ?2, ?3)",
                rusqlite::params![chunk_id, f32s_to_bytes(vector), doc_id],
            )
            .map_err(sqlite_err)?;
        }
        for extra in space::list(conn)? {
            if space::matches_embedder(&extra, embedder) {
                space::index_doc_into(conn, doc_id, &extra, embedder)?;
            } else {
                let owned = space::embedder_for_space(&extra)?;
                space::index_doc_into(conn, doc_id, &extra, owned.as_ref())?;
            }
        }
        aidb_core::crash_point("after_vec");
        if let Ok(Some(run_id)) = conn.query_row(
            "SELECT index_run_id FROM documents WHERE id = ?1",
            [doc_id],
            |row| row.get::<_, Option<String>>(0),
        ) {
            let artifact = format!(r#"{{"embedded":{}}}"#, needed.len());
            aidb_run::put_checkpoint(conn, &run_id, "embed", Some(&artifact))?;
            aidb_run::append_event(conn, &run_id, "embed", Some(&artifact))?;
        }
        aidb_core::crash_point("after_embed_checkpoint");
        finish_document(conn, doc_id, true, None)
    })
}

fn prepare_chunks(conn: &Connection, doc_id: &str) -> Result<Vec<(i64, String)>> {
    let (content, run_id): (String, Option<String>) = conn
        .query_row(
            "SELECT content, index_run_id FROM documents WHERE id = ?1",
            [doc_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite_err)?;

    conn.execute(
        "UPDATE documents SET index_status = 'indexing' WHERE id = ?1",
        [doc_id],
    )
    .map_err(sqlite_err)?;
    if let Some(run_id) = &run_id {
        conn.execute(
            "UPDATE runs SET status = 'running', started_at_ms = COALESCE(started_at_ms, ?1) WHERE id = ?2",
            rusqlite::params![now_ms(), run_id],
        )
        .map_err(sqlite_err)?;
    }

    let wanted = chunk_text(&content);
    let existing: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT content FROM chunks WHERE document_id = ?1 ORDER BY ordinal")
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map([doc_id], |row| row.get(0))
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)?
    };
    if existing != wanted {
        // DESIGN §15: new content deletes the old chunks. The FTS trigger and the
        // per-space vec triggers clean up the derived rows.
        conn.execute("DELETE FROM chunks WHERE document_id = ?1", [doc_id])
            .map_err(sqlite_err)?;
        for (ordinal, text) in wanted.iter().enumerate() {
            conn.execute(
                "INSERT INTO chunks (document_id, ordinal, content, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![doc_id, ordinal as i64, text, now_ms()],
            )
            .map_err(sqlite_err)?;
        }
        // content_hash is the identity of the indexed content.
        conn.execute(
            "UPDATE documents SET content_hash = ?1 WHERE id = ?2",
            rusqlite::params![content_hash(&content), doc_id],
        )
        .map_err(sqlite_err)?;
    }

    if let Some(run_id) = &run_id {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
                [doc_id],
                |row| row.get(0),
            )
            .map_err(sqlite_err)?;
        if aidb_run::has_checkpoint(conn, run_id, "chunk")? {
            aidb_run::append_event(conn, run_id, "resume", Some("chunk"))?;
        } else {
            let artifact = format!(r#"{{"chunks":{count}}}"#);
            aidb_run::put_checkpoint(conn, run_id, "chunk", Some(&artifact))?;
            aidb_run::append_event(conn, run_id, "chunk", Some(&artifact))?;
        }
    }

    let mut stmt = conn
        .prepare(
            "SELECT c.id, c.content
             FROM chunks c
             WHERE c.document_id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM vec_chunks v WHERE v.chunk_id = c.id
               )
             ORDER BY c.ordinal",
        )
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([doc_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(sqlite_err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)
}

pub(crate) fn missing_vec_in(
    conn: &Connection,
    doc_id: &str,
    table: &str,
) -> Result<Vec<(i64, String)>> {
    assert_vec_table(table)?;
    let sql = format!(
        "SELECT c.id, c.content
         FROM chunks c
         WHERE c.document_id = ?1
           AND NOT EXISTS (
               SELECT 1 FROM {table} v WHERE v.chunk_id = c.id
           )
         ORDER BY c.ordinal"
    );
    let mut stmt = conn.prepare(&sql).map_err(sqlite_err)?;
    let rows = stmt
        .query_map([doc_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(sqlite_err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)
}

pub(crate) fn upsert_vec(
    conn: &Connection,
    table: &str,
    chunk_id: i64,
    vector: &[f32],
    doc_id: &str,
) -> Result<()> {
    assert_vec_table(table)?;
    conn.execute(
        &format!("INSERT INTO {table} (chunk_id, embedding, document_id) VALUES (?1, ?2, ?3)"),
        rusqlite::params![chunk_id, f32s_to_bytes(vector), doc_id],
    )
    .map_err(sqlite_err)?;
    Ok(())
}

fn finish_document(conn: &Connection, doc_id: &str, ok: bool, error: Option<&str>) -> Result<()> {
    let status = if ok { "ready" } else { "failed" };
    conn.execute(
        "UPDATE documents SET index_status = ?1, index_error = ?2, updated_at_ms = ?3 WHERE id = ?4",
        rusqlite::params![status, error, now_ms(), doc_id],
    )
    .map_err(sqlite_err)?;
    let run_id: Option<String> = conn
        .query_row(
            "SELECT index_run_id FROM documents WHERE id = ?1",
            [doc_id],
            |row| row.get(0),
        )
        .map_err(sqlite_err)?;
    if let Some(run_id) = run_id {
        aidb_run::finish_run(
            conn,
            &run_id,
            if ok { "succeeded" } else { "failed" },
            error,
        )?;
        aidb_run::append_event(conn, &run_id, if ok { "ready" } else { "failed" }, error)?;
    }
    Ok(())
}

pub fn search(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
) -> Result<QueryResult> {
    search_with_parent(conn, embedder, query, k, None)
}

pub fn search_with_parent(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
    parent_id: Option<&str>,
) -> Result<QueryResult> {
    search_with(conn, embedder, query, k, parent_id, None, None)
}

pub fn search_with(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
    parent_id: Option<&str>,
    mode: Option<Retrieval>,
    filter: Option<&str>,
) -> Result<QueryResult> {
    search_in(conn, embedder, query, k, parent_id, mode, filter, None)
}

#[allow(clippy::too_many_arguments)]
pub fn search_in(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
    parent_id: Option<&str>,
    mode: Option<Retrieval>,
    filter: Option<&str>,
    space: Option<&str>,
) -> Result<QueryResult> {
    let resolved = space::resolve(conn, space)?;
    let owned = match &resolved {
        Some(spec) => Some(space::embedder_for_space(spec)?),
        None => None,
    };
    let used: &dyn Embedder = match &owned {
        Some(extra) => extra.as_ref(),
        None => embedder,
    };
    let vec_table = resolved
        .as_ref()
        .map(|s| s.vec_table.as_str())
        .unwrap_or(DEFAULT_VEC_TABLE);
    let mode = mode
        .unwrap_or_else(|| Retrieval::choose(query, has_vec_table(conn, vec_table), has_fts(conn)));
    let filter_map = parse_metadata_filter(filter)?;
    let run_id = new_id("run");
    let input = match &filter_map {
        Some(map) => format!(
            r#"{{"query":{},"k":{},"algorithm":{},"filter":{},"space":{}}}"#,
            json_escape(query),
            k.max(1),
            json_escape(mode.algorithm()),
            serde_json::Value::Object(map.clone()),
            json_escape(
                resolved
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or(DEFAULT_SPACE)
            )
        ),
        None => format!(
            r#"{{"query":{},"k":{},"algorithm":{},"space":{}}}"#,
            json_escape(query),
            k.max(1),
            json_escape(mode.algorithm()),
            json_escape(
                resolved
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or(DEFAULT_SPACE)
            )
        ),
    };
    aidb_run::insert_search_run(conn, &run_id, &input, None, "running", None, parent_id)?;
    aidb_run::append_event(conn, &run_id, "started", None)?;

    let result = retrieve(conn, used, query, k, mode, filter_map.as_ref(), vec_table);
    match &result {
        Ok(hits) => {
            let output = format!(
                r#"{{"hits":{},"algorithm":{}}}"#,
                hits.rows.len(),
                json_escape(mode.algorithm())
            );
            conn.execute(
                "UPDATE runs SET status = 'succeeded', output_json = ?1, finished_at_ms = ?2 WHERE id = ?3",
                rusqlite::params![output, now_ms(), run_id],
            )
            .map_err(sqlite_err)?;
            aidb_run::put_checkpoint(conn, &run_id, "search", Some(&output))?;
            aidb_run::append_event(conn, &run_id, "searched", Some(&output))?;
        }
        Err(err) => {
            let message = err.to_string();
            conn.execute(
                "UPDATE runs SET status = 'failed', error = ?1, finished_at_ms = ?2 WHERE id = ?3",
                rusqlite::params![message, now_ms(), run_id],
            )
            .map_err(sqlite_err)?;
            aidb_run::append_event(conn, &run_id, "failed", Some(&message))?;
        }
    }
    result
}

/// Retrieval without writing a run. Used to measure sample recall / widen k.
pub fn hits(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
) -> Result<QueryResult> {
    let mode = Retrieval::choose(query, has_vec(conn), has_fts(conn));
    retrieve(conn, embedder, query, k, mode, None, DEFAULT_VEC_TABLE)
}

fn retrieve(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
    mode: Retrieval,
    filter: Option<&serde_json::Map<String, serde_json::Value>>,
    vec_table: &str,
) -> Result<QueryResult> {
    match mode {
        Retrieval::Vec => knn(conn, embedder, query, k, filter, vec_table),
        Retrieval::Fts => fts(conn, query, k, filter, space_join(vec_table)),
        Retrieval::Hybrid => hybrid(conn, embedder, query, k, filter, vec_table),
    }
}

fn space_join(vec_table: &str) -> Option<&str> {
    if vec_table == DEFAULT_VEC_TABLE {
        None
    } else {
        Some(vec_table)
    }
}

pub fn knn(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
    filter: Option<&serde_json::Map<String, serde_json::Value>>,
    vec_table: &str,
) -> Result<QueryResult> {
    let empty = empty_hits();
    assert_vec_table(vec_table)?;
    if !has_vec_table(conn, vec_table) {
        return Ok(empty);
    }

    let vector = embedder
        .embed(&[query.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::ai("embedder returned no query vector"))?;
    // sqlite-vec caps the k of a KNN probe. Asking for more rows than the index
    // will ever hold is not an error: the corpus is the real bound.
    let k = k.clamp(1, MAX_KNN_K);
    let extra = metadata_sql(filter)?;
    let sql = format!(
        "SELECT v.document_id, v.chunk_id, c.content, v.distance
         FROM {vec_table} v
         JOIN chunks c ON c.id = v.chunk_id
         JOIN documents d ON d.id = v.document_id
         WHERE v.embedding MATCH ?1
           AND k = ?2
           AND d.index_status = 'ready'{extra}
         ORDER BY v.distance"
    );
    let mut stmt = conn.prepare(&sql).map_err(sqlite_err)?;
    let rows = stmt
        .query_map(rusqlite::params![f32s_to_bytes(&vector), k], |row| {
            Ok((
                Value::Text(row.get(0)?),
                Value::Integer(row.get(1)?),
                Value::Text(row.get(2)?),
                row.get::<_, Option<f64>>(3)?,
            ))
        })
        .map_err(sqlite_err)?;
    let mut hits = Vec::new();
    for row in rows {
        let (doc, chunk, content, distance) = row.map_err(sqlite_err)?;
        // A zero vector has no cosine distance. Such a row is not rankable, so it
        // is not a hit: one degenerate chunk must not break every search.
        let Some(distance) = distance.filter(|d| d.is_finite()) else {
            continue;
        };
        hits.push(vec![doc, chunk, content, Value::Real(distance)]);
    }
    Ok(QueryResult {
        columns: empty.columns,
        rows: hits,
    })
}

pub fn fts(
    conn: &Connection,
    query: &str,
    k: i64,
    filter: Option<&serde_json::Map<String, serde_json::Value>>,
    vec_table: Option<&str>,
) -> Result<QueryResult> {
    if !has_fts(conn) {
        return Ok(empty_hits());
    }
    let Some(match_query) = fts_match_query(query) else {
        return Ok(empty_hits());
    };
    let k = k.max(1);
    let extra = metadata_sql(filter)?;
    let space_sql = match vec_table {
        Some(table) => {
            assert_vec_table(table)?;
            format!(" AND EXISTS (SELECT 1 FROM {table} v WHERE v.chunk_id = c.id)")
        }
        None => String::new(),
    };
    let sql = format!(
        "SELECT c.document_id, c.id, c.content, bm25(chunks_fts)
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         JOIN documents d ON d.id = c.document_id
         WHERE chunks_fts MATCH ?1
           AND d.index_status = 'ready'{extra}{space_sql}
         ORDER BY bm25(chunks_fts)
         LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql).map_err(sqlite_err)?;
    let rows = stmt
        .query_map(rusqlite::params![match_query, k], |row| {
            Ok(vec![
                Value::Text(row.get(0)?),
                Value::Integer(row.get(1)?),
                Value::Text(row.get(2)?),
                Value::Real(row.get(3)?),
            ])
        })
        .map_err(sqlite_err)?;
    let rows = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)?;
    Ok(QueryResult {
        columns: empty_hits().columns,
        rows,
    })
}

fn hybrid(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
    filter: Option<&serde_json::Map<String, serde_json::Value>>,
    vec_table: &str,
) -> Result<QueryResult> {
    let k = k.max(1);
    let fetch = (k * 4).max(16);
    let vec_hits = knn(conn, embedder, query, fetch, filter, vec_table)?;
    let fts_hits = fts(conn, query, fetch, filter, space_join(vec_table))?;
    Ok(rrf_merge(&vec_hits, &fts_hits, k))
}

fn rrf_merge(vec_hits: &QueryResult, fts_hits: &QueryResult, k: i64) -> QueryResult {
    const RRF_K: f64 = 60.0;
    let mut scores: HashMap<i64, (f64, Vec<Value>)> = HashMap::new();
    add_ranks(&mut scores, vec_hits, RRF_K);
    add_ranks(&mut scores, fts_hits, RRF_K);
    let mut ranked: Vec<(i64, f64, Vec<Value>)> = scores
        .into_iter()
        .map(|(chunk_id, (score, row))| (chunk_id, score, row))
        .collect();
    // Ties break on chunk_id so the same query always returns the same order:
    // scores alone would leave the result at the mercy of hash iteration order.
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    let rows = ranked
        .into_iter()
        .take(k.max(1) as usize)
        .map(|(_, score, mut row)| {
            if row.len() >= 4 {
                row[3] = Value::Real(1.0 / (1.0 + score));
            }
            row
        })
        .collect();
    QueryResult {
        columns: empty_hits().columns,
        rows,
    }
}

fn add_ranks(scores: &mut HashMap<i64, (f64, Vec<Value>)>, hits: &QueryResult, rrf_k: f64) {
    let chunk_idx = hits
        .columns
        .iter()
        .position(|c| c == "chunk_id")
        .unwrap_or(1);
    for (rank, row) in hits.rows.iter().enumerate() {
        let Some(Value::Integer(chunk_id)) = row.get(chunk_idx) else {
            continue;
        };
        let add = 1.0 / (rrf_k + rank as f64 + 1.0);
        scores
            .entry(*chunk_id)
            .and_modify(|(score, _)| *score += add)
            .or_insert_with(|| (add, row.clone()));
    }
}

fn empty_hits() -> QueryResult {
    QueryResult {
        columns: vec![
            "document_id".into(),
            "chunk_id".into(),
            "content".into(),
            "distance".into(),
        ],
        rows: Vec::new(),
    }
}

pub fn has_vec(conn: &Connection) -> bool {
    has_vec_table(conn, DEFAULT_VEC_TABLE)
}

pub fn has_vec_table(conn: &Connection, table: &str) -> bool {
    table_exists(conn, table)
}

pub fn has_fts(conn: &Connection) -> bool {
    table_exists(conn, "chunks_fts")
}

pub fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(true),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or(false)
}

fn fts_match_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 2)
        .map(|w| {
            let token = w.to_ascii_lowercase().replace('"', "");
            format!("\"{token}\"")
        })
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" OR "))
    }
}

fn parse_metadata_filter(
    raw: Option<&str>,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if raw == "{}" || raw.eq_ignore_ascii_case("null") {
        return Ok(None);
    }
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| Error::usage(format!("search filter must be a JSON object: {err}")))?;
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(map) if map.is_empty() => Ok(None),
        serde_json::Value::Object(map) => {
            for key in map.keys() {
                if !is_safe_meta_key(key) {
                    return Err(Error::usage(format!("invalid metadata filter key: {key}")));
                }
            }
            Ok(Some(map))
        }
        _ => Err(Error::usage("search filter must be a JSON object")),
    }
}

fn metadata_sql(filter: Option<&serde_json::Map<String, serde_json::Value>>) -> Result<String> {
    let Some(map) = filter else {
        return Ok(String::new());
    };
    let mut parts = Vec::new();
    for (key, value) in map {
        if !is_safe_meta_key(key) {
            return Err(Error::usage(format!("invalid metadata filter key: {key}")));
        }
        let path = format!("json_extract(d.metadata_json, '$.{key}')");
        let clause = match value {
            serde_json::Value::Null => format!("{path} IS NULL"),
            serde_json::Value::String(text) => format!("{path} = {}", sql_quote(text)),
            serde_json::Value::Number(n) => format!("{path} = {n}"),
            serde_json::Value::Bool(true) => format!("{path} = 1"),
            serde_json::Value::Bool(false) => format!("{path} = 0"),
            other => {
                return Err(Error::usage(format!(
                    "search filter values must be scalars (got {other} for {key})"
                )));
            }
        };
        parts.push(clause);
    }
    if parts.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!(" AND {}", parts.join(" AND ")))
    }
}

fn is_safe_meta_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn sql_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn json_escape(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn chunk_text(content: &str) -> Vec<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        // No text is no chunk. An empty chunk would embed to a zero vector, which
        // has no distance and would pollute every KNN result.
        return Vec::new();
    }
    if trimmed.chars().count() <= CHUNK_CHARS {
        return vec![trimmed.to_string()];
    }

    let mut chunks = Vec::new();
    let chars: Vec<char> = trimmed.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + CHUNK_CHARS).min(chars.len());
        if end < chars.len() {
            if let Some(rel) = chars[start..end].iter().rposition(|c| c.is_whitespace()) {
                if rel > CHUNK_CHARS / 4 {
                    end = start + rel;
                }
            }
        }
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim();
        if !piece.is_empty() {
            chunks.push(piece.to_string());
        }
        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP);
        if start >= end {
            start = end;
        }
    }
    if chunks.is_empty() {
        chunks.push(trimmed.to_string());
    }
    chunks
}

fn f32s_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub(crate) fn meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row("SELECT value FROM aidb_meta WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .optional()
    .map_err(sqlite_err)
}

fn upsert_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO aidb_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )
    .map_err(sqlite_err)?;
    Ok(())
}

trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

pub struct Indexer {
    store: Arc<Store>,
    state: Arc<WorkerState>,
    thread: Option<JoinHandle<()>>,
}

struct WorkerState {
    stop: Mutex<bool>,
    wake: Condvar,
}

impl Indexer {
    pub fn start(store: Arc<Store>, embedder: Arc<dyn Embedder>) -> Self {
        let state = Arc::new(WorkerState {
            stop: Mutex::new(false),
            wake: Condvar::new(),
        });
        let thread_store = Arc::clone(&store);
        let thread_embedder = Arc::clone(&embedder);
        let thread_state = Arc::clone(&state);
        let thread =
            thread::spawn(move || worker_loop(thread_store, thread_embedder, thread_state));
        Self {
            store,
            state,
            thread: Some(thread),
        }
    }

    pub fn notify(&self) {
        self.state.wake.notify_one();
    }

    pub fn drain(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.notify();
            let pending = self.store.write(pending_count)?;
            if pending == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::usage(format!(
                    "timed out waiting for {pending} document(s) to index"
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        if let Ok(mut stop) = self.state.stop.lock() {
            *stop = true;
        }
        self.state.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn worker_loop(store: Arc<Store>, embedder: Arc<dyn Embedder>, state: Arc<WorkerState>) {
    loop {
        if state.stop.lock().map(|s| *s).unwrap_or(true) {
            break;
        }
        // Adopt in the same lock as the pick, so a document can never be indexed
        // without the run row that records the work.
        let next = store
            .write(|conn| {
                enqueue_untracked(conn)?;
                next_document_id(conn)
            })
            .ok()
            .flatten();
        match next {
            Some(doc_id) => {
                if let Err(err) = index_document(&store, embedder.as_ref(), &doc_id) {
                    let _ = store.write(|conn| {
                        finish_document(conn, &doc_id, false, Some(&err.to_string()))
                    });
                }
            }
            None => {
                let guard = match state.stop.lock() {
                    Ok(g) => g,
                    Err(_) => break,
                };
                if *guard {
                    break;
                }
                let _ = state.wake.wait_timeout(guard, Duration::from_millis(200));
            }
        }
    }
}
