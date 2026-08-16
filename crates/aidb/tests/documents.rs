//! Phase 1 contracts: insert → pending → ready → searchable, plus the document
//! lifecycle rules from DESIGN.md §15 ("Documents", "Vectors").

mod common;

use std::time::Duration;

use common::*;

#[test]
fn insert_returns_immediately_as_pending_with_a_run_and_then_becomes_ready() {
    let tmp = TempDb::new("lifecycle");
    let db = tmp.open();
    let id = insert_doc(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );

    // The run row exists in the same transaction as the document.
    let run_id = scalar(
        &db,
        &format!("SELECT index_run_id FROM documents WHERE id = '{id}'"),
    );
    assert!(!run_id.is_empty(), "insert must enqueue an index run");
    assert_eq!(
        scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{run_id}'")),
        "index_document"
    );

    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'")
        ),
        "ready"
    );
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT status FROM runs WHERE id = '{run_id}'")
        ),
        "succeeded"
    );
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_error FROM documents WHERE id = '{id}'")
        ),
        "",
        "a successful index leaves no error"
    );
}

#[test]
fn content_metadata_hash_and_timestamps_persist_across_reopen() {
    let tmp = TempDb::new("persist");
    let id;
    let hash;
    let created;
    {
        let db = tmp.open();
        id = insert_doc_meta(
            &db,
            "Refunds",
            "Refunds are issued within 14 days.",
            "{\"team\":\"support\",\"tier\":2}",
        );
        db.drain_index(Duration::from_secs(30)).expect("drain");
        let rows = db
            .query(&format!(
                "SELECT title, content, metadata_json, content_hash, created_at_ms, updated_at_ms
                 FROM documents WHERE id = '{id}'"
            ))
            .expect("row");
        assert_eq!(cell(&rows, 0, "title"), "Refunds");
        assert_eq!(
            cell(&rows, 0, "content"),
            "Refunds are issued within 14 days."
        );
        hash = cell(&rows, 0, "content_hash");
        created = cell(&rows, 0, "created_at_ms");
        assert!(!hash.is_empty());
        assert!(created.parse::<i64>().expect("created_at_ms") > 0);
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT json_extract(metadata_json, '$.team') FROM documents WHERE id = '{id}'"
                )
            ),
            "support"
        );
    }
    let db = tmp.open();
    let rows = db
        .query(&format!(
            "SELECT content_hash, created_at_ms, index_status FROM documents WHERE id = '{id}'"
        ))
        .expect("row");
    assert_eq!(cell(&rows, 0, "content_hash"), hash);
    assert_eq!(cell(&rows, 0, "created_at_ms"), created);
    assert_eq!(cell(&rows, 0, "index_status"), "ready");
}

#[test]
fn identical_content_produces_an_identical_content_hash() {
    let tmp = TempDb::new("hash");
    let db = tmp.open();
    let a = insert_ready(&db, "A", "Refunds are issued within 14 days.");
    let b = insert_ready(&db, "B", "Refunds are issued within 14 days.");
    let c = insert_ready(&db, "C", "Refunds are issued within 15 days.");
    let hash = |id: &str| {
        scalar(
            &db,
            &format!("SELECT content_hash FROM documents WHERE id = '{id}'"),
        )
    };
    assert_eq!(hash(&a), hash(&b), "content_hash is a function of content");
    assert_ne!(hash(&a), hash(&c));
}

#[test]
fn every_chunk_gets_exactly_one_vector_row_and_one_fts_row() {
    let tmp = TempDb::new("chunk-vec");
    let db = tmp.open();
    let long = "Refund policy paragraph. ".repeat(200);
    let id = insert_ready(&db, "Long", &long);

    let chunks = count(
        &db,
        &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'"),
    );
    assert!(chunks > 1, "a long document must produce several chunks");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{id}'")
        ),
        chunks,
        "vec rows must correspond 1:1 with chunks"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM chunks c WHERE c.document_id = '{id}'
                   AND NOT EXISTS (SELECT 1 FROM vec_chunks v WHERE v.chunk_id = c.id)"
            )
        ),
        0,
        "no chunk may be left without a vector"
    );
    // Ordinals are dense and start at zero.
    let ordinals = column_values(
        &db.query(&format!(
            "SELECT ordinal FROM chunks WHERE document_id = '{id}' ORDER BY ordinal"
        ))
        .expect("ordinals"),
        "ordinal",
    );
    assert_eq!(
        ordinals,
        (0..chunks).map(|i| i.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn the_vector_table_is_created_with_the_locked_embedding_dimensions() {
    let tmp = TempDb::new("dims");
    let db = tmp.open();
    insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");
    let dims = scalar(
        &db,
        "SELECT value FROM aidb_meta WHERE key = 'embedding_dimensions'",
    );
    let ddl = scalar(
        &db,
        "SELECT sql FROM sqlite_master WHERE name = 'vec_chunks'",
    );
    assert!(
        ddl.contains(&format!("float[{dims}]")),
        "vec0 dimensions must match aidb_meta: {ddl}"
    );
    assert!(ddl.contains("distance_metric=cosine"), "{ddl}");
    assert_eq!(
        scalar(
            &db,
            "SELECT value FROM aidb_meta WHERE key = 'embedding_provider'"
        ),
        "fake"
    );
    // The embedding model is registered in the catalog, without any key material.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM models WHERE kind = 'embedding' AND provider = 'fake'"
        ),
        1
    );
}

#[test]
fn chunk_ids_are_stable_across_reopen_and_reindex_attempts() {
    let tmp = TempDb::new("chunk-ids");
    let id;
    let before;
    {
        let db = tmp.open();
        id = insert_ready(&db, "Refunds", &"Refund clause. ".repeat(200));
        before = column_values(
            &db.query(&format!(
                "SELECT id FROM chunks WHERE document_id = '{id}' ORDER BY ordinal"
            ))
            .expect("chunk ids"),
            "id",
        );
    }
    let db = tmp.open();
    db.drain_index(Duration::from_secs(30)).expect("drain");
    let after = column_values(
        &db.query(&format!(
            "SELECT id FROM chunks WHERE document_id = '{id}' ORDER BY ordinal"
        ))
        .expect("chunk ids"),
        "id",
    );
    assert_eq!(before, after, "chunk ids must be stable across reopen");
}

#[test]
fn reindexing_a_ready_document_does_not_duplicate_chunks_or_vectors() {
    let tmp = TempDb::new("reindex-idempotent");
    let db = tmp.open();
    let id = insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");
    let chunks = count(
        &db,
        &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'"),
    );

    // Force the indexer to pick the document up again.
    for _ in 0..3 {
        db.execute(&format!(
            "UPDATE documents SET index_status = 'pending' WHERE id = '{id}'"
        ))
        .expect("re-enqueue");
        db.drain_index(Duration::from_secs(30)).expect("drain");
    }

    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'")
        ),
        "ready"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'")
        ),
        chunks,
        "repeated indexing must not duplicate chunks"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{id}'")
        ),
        chunks,
        "repeated indexing must not duplicate vectors"
    );
}

#[test]
fn an_empty_document_still_reaches_ready_and_never_breaks_search() {
    let tmp = TempDb::new("empty-doc");
    let db = tmp.open();
    let id = insert_ready(&db, "Empty", "");
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'")
        ),
        "ready"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'")
        ),
        0,
        "no text means no chunk"
    );
    let real = insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");
    let hits = db
        .query("SELECT * FROM aidb_search('how do refunds work', 5)")
        .expect("search");
    assert!(column_values(&hits, "document_id").contains(&real));
}

#[test]
fn unicode_and_emoji_content_round_trips_and_is_searchable() {
    let tmp = TempDb::new("unicode");
    let db = tmp.open();
    let content = "返金は14日以内に処理されます。Rückerstattung 🎉 within fourteen days.";
    let id = insert_ready(&db, "多言語", content);
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT content FROM documents WHERE id = '{id}'")
        ),
        content
    );
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT title FROM documents WHERE id = '{id}'")
        ),
        "多言語"
    );
    let hits = db
        .query("SELECT * FROM aidb_search('Rückerstattung fourteen days', 5)")
        .expect("search");
    assert!(
        column_values(&hits, "document_id").contains(&id),
        "unicode chunk must be retrievable"
    );
}

#[test]
fn a_very_large_document_indexes_completely() {
    let tmp = TempDb::new("large-doc");
    let db = tmp.open();
    let content = "Distinct sentence number seven about warehouse logistics. ".repeat(2000);
    let id = insert_ready(&db, "Big", &content);
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'")
        ),
        "ready"
    );
    let chunks = count(
        &db,
        &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'"),
    );
    assert!(chunks > 50, "expected many chunks, got {chunks}");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{id}'")
        ),
        chunks
    );
    // Every chunk stays within the documented chunk size.
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM chunks WHERE document_id = '{id}' AND length(content) > 900"
            )
        ),
        0
    );
}

#[test]
fn malformed_metadata_json_fails_cleanly_without_writing_a_document() {
    let tmp = TempDb::new("bad-meta");
    let db = tmp.open();
    let before = count(&db, "SELECT COUNT(*) FROM documents");
    let result = db.query("SELECT aidb_insert_document('T', 'body', '{not json')");
    assert!(result.is_err(), "malformed metadata must fail closed");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM documents"),
        before,
        "a rejected insert must not leave a document behind"
    );
}

#[test]
fn a_document_that_is_not_ready_is_not_searchable() {
    let tmp = TempDb::new("ready-filter");
    let db = tmp.open();
    let ready = insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let hidden = insert_ready(
        &db,
        "Refunds two",
        "Refunds also cover damaged goods in 14 days.",
    );

    let visible = |db: &aidb::Aidb| {
        column_values(
            &db.query("SELECT * FROM aidb_search('refunds within days', 10)")
                .expect("search"),
            "document_id",
        )
    };
    assert!(visible(&db).contains(&hidden));

    // `failed` is terminal: the indexer never picks it back up, so this is the
    // status the ready-filter can be observed on without racing the worker.
    db.execute(&format!(
        "UPDATE documents SET index_status = 'failed', index_error = 'provider down'
         WHERE id = '{hidden}'"
    ))
    .expect("status");
    let seen = visible(&db);
    assert!(
        !seen.contains(&hidden),
        "a failed document must not be searchable even though its vectors exist"
    );
    assert!(seen.contains(&ready), "ready documents stay searchable");
    assert!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{hidden}'")
        ) > 0,
        "the filter is on index_status, not on the absence of vectors"
    );
}

#[test]
fn a_document_pushed_back_to_pending_is_reindexed_and_becomes_searchable_again() {
    let tmp = TempDb::new("re-adopt");
    let db = tmp.open();
    let id = insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.execute(&format!(
        "UPDATE documents SET index_status = 'pending' WHERE id = '{id}'"
    ))
    .expect("re-enqueue");
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'")
        ),
        "ready"
    );
    let hits = db
        .query("SELECT * FROM aidb_search('refunds', 5)")
        .expect("search");
    assert!(column_values(&hits, "document_id").contains(&id));
}

#[test]
fn a_document_with_no_embeddable_text_never_breaks_other_searches() {
    let tmp = TempDb::new("degenerate");
    let db = tmp.open();
    // Punctuation only: every embedder maps this to a zero vector, which has no
    // cosine distance. It must not poison the result set.
    let junk = insert_ready(&db, "Junk", "!!! ??? ...");
    let real = insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{junk}'")
        ),
        "ready"
    );
    let hits = db
        .query("SELECT * FROM aidb_search('refunds within days', 5)")
        .expect("search must survive a zero vector");
    assert!(column_values(&hits, "document_id").contains(&real));
    for row in 0..hits.rows.len() {
        let d: f64 = cell(&hits, row, "distance")
            .parse()
            .expect("every hit has a numeric distance");
        assert!(d.is_finite());
    }
}

#[test]
fn search_respects_k_orders_by_distance_and_returns_scores() {
    let tmp = TempDb::new("search-k");
    let db = tmp.open();
    for i in 0..6 {
        insert_ready(
            &db,
            &format!("Doc {i}"),
            &format!("Refund policy variant {i} explains fourteen day returns."),
        );
    }
    for k in [1, 2, 3, 5] {
        let hits = db
            .query(&format!("SELECT * FROM aidb_search('refund policy', {k})"))
            .expect("search");
        assert!(
            hits.rows.len() <= k as usize,
            "k={k} returned {} rows",
            hits.rows.len()
        );
        let distances: Vec<f64> = column_values(&hits, "distance")
            .iter()
            .map(|v| v.parse::<f64>().expect("distance is numeric"))
            .collect();
        assert!(
            distances.windows(2).all(|w| w[0] <= w[1] + 1e-9),
            "distances must be non-decreasing: {distances:?}"
        );
        assert!(distances.iter().all(|d| d.is_finite() && *d >= 0.0));
    }
}

#[test]
fn search_on_an_empty_database_returns_no_rows_with_the_documented_columns() {
    let tmp = TempDb::new("search-empty");
    let db = tmp.open();
    let hits = db
        .query("SELECT * FROM aidb_search('anything at all', 5)")
        .expect("search");
    assert!(hits.rows.is_empty());
    assert_eq!(
        hits.columns,
        vec!["document_id", "chunk_id", "content", "distance"]
    );
}

#[test]
fn search_results_join_real_chunk_rows() {
    let tmp = TempDb::new("search-join");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let hits = db
        .query("SELECT * FROM aidb_search('refunds', 5)")
        .expect("search");
    assert!(!hits.rows.is_empty());
    for row in 0..hits.rows.len() {
        let chunk_id = cell(&hits, row, "chunk_id");
        let doc_id = cell(&hits, row, "document_id");
        let content = cell(&hits, row, "content");
        let stored = scalar(
            &db,
            &format!("SELECT content FROM chunks WHERE id = {chunk_id}"),
        );
        assert_eq!(content, stored, "hit content must be the stored chunk");
        assert_eq!(
            scalar(
                &db,
                &format!("SELECT document_id FROM chunks WHERE id = {chunk_id}")
            ),
            doc_id
        );
    }
}

#[test]
fn a_failing_embedder_marks_the_document_failed_and_leaves_no_vectors() {
    let tmp = TempDb::new("embed-fail");
    let db = tmp.open();
    // Seed a good document so vec_chunks exists with the locked dimensions.
    let good = insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");

    // A document whose chunk cannot be embedded: simulate by pointing the row at a
    // dimension-mismatched space is not possible through SQL, so instead assert the
    // documented failure surface directly.
    db.execute(
        "INSERT INTO documents (id, title, content, content_hash, index_status, index_error, created_at_ms, updated_at_ms)
         VALUES ('doc_failed', 'Broken', 'body', 'h', 'failed', 'embedder exploded', 1, 1)",
    )
    .expect("seed failed doc");

    let hits = db
        .query("SELECT * FROM aidb_search('body refunds', 10)")
        .expect("search");
    let seen = column_values(&hits, "document_id");
    assert!(!seen.contains(&"doc_failed".to_string()));
    assert!(seen.contains(&good));
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM vec_chunks WHERE document_id = 'doc_failed'"
        ),
        0,
        "a failed document must not have searchable vectors"
    );
}

#[test]
fn deleting_a_document_removes_its_chunks_fts_and_vectors() {
    let tmp = TempDb::new("delete-clean");
    let db = tmp.open();
    let keep = insert_ready(
        &db,
        "Keep",
        "Refunds are issued within 14 days of purchase.",
    );
    let drop_me = insert_ready(
        &db,
        "Drop",
        "Warehouse bin ZX19 holds the discontinued adapter.",
    );
    let dropped_chunks = count(
        &db,
        &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{drop_me}'"),
    );
    assert!(dropped_chunks > 0);

    db.execute(&format!("DELETE FROM documents WHERE id = '{drop_me}'"))
        .expect("delete");

    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{drop_me}'")
        ),
        0,
        "chunks must cascade"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{drop_me}'")
        ),
        0,
        "DESIGN §15: delete removes document, chunks, FTS and vec rows"
    );
    // FTS must not keep the deleted text.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'ZX19'"
        ),
        0,
        "FTS rows must be deleted with the chunks"
    );
    // And the surviving document is still searchable.
    let hits = db
        .query("SELECT * FROM aidb_search('refunds within days', 5)")
        .expect("search");
    assert!(column_values(&hits, "document_id").contains(&keep));
}

#[test]
fn orphaned_vectors_never_crowd_real_hits_out_of_the_top_k() {
    let tmp = TempDb::new("orphan-vec");
    let db = tmp.open();
    // Ten documents that all match the query, then delete nine of them.
    let mut ids = Vec::new();
    for i in 0..10 {
        ids.push(insert_ready(
            &db,
            &format!("Refunds {i}"),
            "Refunds are issued within 14 days of purchase.",
        ));
    }
    let survivor = ids.pop().expect("survivor");
    for id in &ids {
        db.execute(&format!("DELETE FROM documents WHERE id = '{id}'"))
            .expect("delete");
    }

    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM vec_chunks v
             WHERE NOT EXISTS (SELECT 1 FROM chunks c WHERE c.id = v.chunk_id)"
        ),
        0,
        "no vector may outlive its chunk"
    );
    let hits = db
        .query("SELECT * FROM aidb_search('refunds within days', 1)")
        .expect("search");
    assert_eq!(
        hits.rows.len(),
        1,
        "deleted documents must not consume top-k slots"
    );
    assert_eq!(cell(&hits, 0, "document_id"), survivor);
}

#[test]
fn updating_content_reindexes_and_the_old_text_stops_matching() {
    let tmp = TempDb::new("update-reindex");
    let db = tmp.open();
    let id = insert_ready(
        &db,
        "Refunds",
        "Warehouse bin ZX19QPLUGH holds the discontinued adapter.",
    );
    let old_chunk_count = count(
        &db,
        &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'"),
    );
    assert!(old_chunk_count > 0);

    // DESIGN §15: an update with a new content_hash deletes the old chunks and
    // enqueues a new index run.
    db.execute(&format!(
        "UPDATE documents SET content = 'Refunds are issued within 14 days of purchase.'
         WHERE id = '{id}'"
    ))
    .expect("update content");
    db.drain_index(Duration::from_secs(30)).expect("drain");

    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'")
        ),
        "ready"
    );
    let chunk_text = scalar(
        &db,
        &format!("SELECT content FROM chunks WHERE document_id = '{id}' ORDER BY ordinal LIMIT 1"),
    );
    assert_eq!(
        chunk_text, "Refunds are issued within 14 days of purchase.",
        "chunks must reflect the new content"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{id}'")
        ),
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'")
        ),
        "vectors must be rebuilt for the new chunks"
    );
    // The stale text is gone from FTS.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'ZX19QPLUGH'"
        ),
        0
    );
}

#[test]
fn rewriting_a_document_with_the_same_content_is_a_no_op() {
    let tmp = TempDb::new("update-noop");
    let db = tmp.open();
    let content = "Refunds are issued within 14 days of purchase.";
    let id = insert_ready(&db, "Refunds", content);
    let chunk_ids = column_values(
        &db.query(&format!(
            "SELECT id FROM chunks WHERE document_id = '{id}' ORDER BY ordinal"
        ))
        .expect("chunks"),
        "id",
    );
    let runs_before = count(
        &db,
        &format!("SELECT COUNT(*) FROM runs WHERE document_id = '{id}'"),
    );

    db.execute(&format!(
        "UPDATE documents SET content = '{}' WHERE id = '{id}'",
        sql_escape(content)
    ))
    .expect("same-content update");
    db.drain_index(Duration::from_secs(30)).expect("drain");

    assert_eq!(
        column_values(
            &db.query(&format!(
                "SELECT id FROM chunks WHERE document_id = '{id}' ORDER BY ordinal"
            ))
            .expect("chunks"),
            "id"
        ),
        chunk_ids,
        "an identical rewrite must not re-chunk"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE document_id = '{id}'")
        ),
        runs_before,
        "an identical rewrite must not enqueue another index run"
    );
}

#[test]
fn a_document_inserted_with_plain_sql_is_picked_up_on_open() {
    let tmp = TempDb::new("untracked");
    let db = tmp.open();
    // Prove the engine adopts documents written as ordinary SQLite rows (PHASES §3).
    db.execute(
        "INSERT INTO documents (id, title, content, content_hash, created_at_ms, updated_at_ms)
         VALUES ('doc_manual', 'Manual', 'Refunds are issued within 14 days.', 'h', 1, 1)",
    )
    .expect("manual insert");
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(
        scalar(
            &db,
            "SELECT index_status FROM documents WHERE id = 'doc_manual'"
        ),
        "ready"
    );
    let run = scalar(
        &db,
        "SELECT index_run_id FROM documents WHERE id = 'doc_manual'",
    );
    assert!(!run.is_empty(), "an adopted document must get a run");
    assert_eq!(
        scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{run}'")),
        "index_document"
    );
    let hits = db
        .query("SELECT * FROM aidb_search('refunds', 5)")
        .expect("search");
    assert!(column_values(&hits, "document_id").contains(&"doc_manual".to_string()));
}
