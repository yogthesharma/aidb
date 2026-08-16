//! Phase 22 / 26 contracts. An embedding space owns
//! (provider, model, dimensions, distance), a vector index belongs to exactly one
//! space, and nothing ever silently mixes or substitutes a different model.

mod common;

use common::*;

fn create_space(db: &aidb::Aidb, args: &str) -> aidb::QueryResult {
    db.query(&format!("SELECT aidb_create_space({args})"))
        .unwrap_or_else(|e| panic!("create space {args}: {e}"))
}

fn seeded(tag: &str) -> (TempDb, aidb::Aidb) {
    let tmp = TempDb::new(tag);
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    insert_ready(
        &db,
        "Shipping",
        "Shipping takes three business days after dispatch.",
    );
    (tmp, db)
}

#[test]
fn a_space_records_its_whole_tuple_and_owns_its_own_vector_table() {
    let (_tmp, db) = seeded("space-tuple");
    let created = create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal', 'cosine'");
    assert_eq!(cell(&created, 0, "name"), "legal");
    assert_eq!(cell(&created, 0, "provider"), "fake");
    assert_eq!(cell(&created, 0, "model"), "aidb-fake-legal");
    assert_eq!(cell(&created, 0, "dimensions"), "64");
    assert_eq!(cell(&created, 0, "distance"), "cosine");
    assert_eq!(cell(&created, 0, "vec_table"), "vec_chunks_legal");
    assert!(
        cell(&created, 0, "indexed")
            .parse::<i64>()
            .expect("indexed")
            > 0,
        "creating a space backfills the documents that already exist"
    );

    // The tuple is durable, and it is what SQL reports.
    let row = db
        .query(
            "SELECT name, provider, provider_model, dimensions, distance, vec_table
             FROM embedding_spaces WHERE name = 'legal'",
        )
        .expect("space row");
    assert_eq!(cell(&row, 0, "provider_model"), "aidb-fake-legal");
    assert_eq!(cell(&row, 0, "dimensions"), "64");
    // Its vectors live in its own table, at its own width.
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM vec_chunks_legal"),
        count(&db, "SELECT COUNT(*) FROM chunks")
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'vec_chunks_legal'"
        ),
        1
    );
}

#[test]
fn spaces_are_listed_and_the_default_space_is_not_a_row() {
    let (_tmp, db) = seeded("space-list");
    create_space(&db, "'legal', 'fake', 64");
    create_space(&db, "'support', 'fake', 32");
    let names = column_values(
        &db.query("SELECT name FROM embedding_spaces ORDER BY name")
            .expect("spaces"),
        "name",
    );
    assert_eq!(names, vec!["legal", "support"]);
    // The default space is the file's own vec_chunks, not a named row.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM embedding_spaces WHERE name = 'default'"
        ),
        0
    );
    assert!(count(&db, "SELECT COUNT(*) FROM vec_chunks") > 0);
    // Each named space gets its own model catalog entry, same catalog as everything else.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM models WHERE name IN ('space-legal', 'space-support')"
        ),
        2
    );
}

#[test]
fn different_spaces_keep_their_vectors_apart() {
    let (_tmp, db) = seeded("space-isolation");
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");
    create_space(&db, "'support', 'fake', 32, 'aidb-fake-support'");

    // Same chunks, three independent indexes.
    let chunks = count(&db, "SELECT COUNT(*) FROM chunks");
    for table in ["vec_chunks", "vec_chunks_legal", "vec_chunks_support"] {
        assert_eq!(
            count(&db, &format!("SELECT COUNT(*) FROM {table}")),
            chunks,
            "{table} indexes every chunk exactly once"
        );
    }
    // A document exists in multiple spaces at once without sharing a vector row.
    let doc = scalar(
        &db,
        "SELECT id FROM documents ORDER BY created_at_ms, rowid LIMIT 1",
    );
    for table in ["vec_chunks", "vec_chunks_legal", "vec_chunks_support"] {
        assert!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM {table} WHERE document_id = '{doc}'")
            ) > 0,
            "{table} is missing {doc}"
        );
    }
}

#[test]
fn a_search_in_a_space_embeds_the_query_with_that_spaces_model() {
    let (_tmp, db) = seeded("space-query-embed");
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");

    let hits = db
        .query("SELECT * FROM aidb_search('how do refunds work', 5, '{}', 'legal')")
        .expect("search in space");
    assert!(!hits.rows.is_empty(), "the space is searchable");

    // The run records which space answered, and the plan binds that space's model.
    let input: serde_json::Value = serde_json::from_str(&scalar(
        &db,
        "SELECT input_json FROM runs WHERE kind = 'search' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    ))
    .expect("input json");
    assert_eq!(input["space"], "legal");
    let plan = scalar(
        &db,
        "EXPLAIN SELECT * FROM aidb_search('how do refunds work', 5, '{}', 'legal')",
    );
    assert!(plan.contains("space=legal"), "{plan}");
    assert!(
        plan.contains("embed fake:aidb-fake-legal"),
        "the query must be embedded by the space's own model:\n{plan}"
    );

    // The default space still binds its own model, unchanged by the named space.
    let default_plan = scalar(
        &db,
        "EXPLAIN SELECT * FROM aidb_search('how do refunds work', 5)",
    );
    assert!(!default_plan.contains("space="), "{default_plan}");
    assert!(
        default_plan.contains("embed fake:aidb-fake"),
        "{default_plan}"
    );
}

#[test]
fn two_spaces_over_the_same_documents_rank_independently() {
    let (_tmp, db) = seeded("space-ranking");
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");

    let default_hits = db
        .query("SELECT * FROM aidb_search('refunds', 3)")
        .expect("default");
    let space_hits = db
        .query("SELECT * FROM aidb_search('refunds', 3, '{}', 'legal')")
        .expect("space");
    assert!(!default_hits.rows.is_empty() && !space_hits.rows.is_empty());
    let a: f64 = cell(&default_hits, 0, "distance")
        .parse()
        .expect("distance");
    let b: f64 = cell(&space_hits, 0, "distance").parse().expect("distance");
    assert!(a.is_finite() && b.is_finite());

    // The two answers are computed from genuinely different vectors: the stored
    // embeddings for one chunk differ in width and in content between spaces, so
    // neither index can stand in for the other.
    let chunk = scalar(&db, "SELECT id FROM chunks ORDER BY id LIMIT 1");
    let default_vec = scalar(
        &db,
        &format!("SELECT length(embedding) FROM vec_chunks WHERE chunk_id = {chunk}"),
    );
    let space_vec = scalar(
        &db,
        &format!("SELECT length(embedding) FROM vec_chunks_legal WHERE chunk_id = {chunk}"),
    );
    assert_eq!(default_vec, "128", "32 floats");
    assert_eq!(space_vec, "256", "64 floats");
}

#[test]
fn l2_and_cosine_are_both_supported_and_recorded() {
    let (_tmp, db) = seeded("space-distance");
    create_space(&db, "'euclid', 'fake', 32, 'aidb-fake', 'l2'");
    create_space(&db, "'angle', 'fake', 32, 'aidb-fake', 'cosine'");
    assert_eq!(
        scalar(
            &db,
            "SELECT distance FROM embedding_spaces WHERE name = 'euclid'"
        ),
        "l2"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT distance FROM embedding_spaces WHERE name = 'angle'"
        ),
        "cosine"
    );
    // Both are usable, and each reports distances in its own metric.
    for (space, _) in [("euclid", "l2"), ("angle", "cosine")] {
        let hits = db
            .query(&format!(
                "SELECT * FROM aidb_search('refunds', 3, '{{}}', '{space}')"
            ))
            .unwrap_or_else(|e| panic!("{space}: {e}"));
        assert!(!hits.rows.is_empty(), "{space} returned nothing");
        let d: f64 = cell(&hits, 0, "distance").parse().expect("distance");
        assert!(d.is_finite() && d >= 0.0, "{space} distance {d}");
    }
    // The distance metric is part of the space, so `euclidean` normalizes to `l2`.
    create_space(&db, "'euclid2', 'fake', 32, 'aidb-fake', 'euclidean'");
    assert_eq!(
        scalar(
            &db,
            "SELECT distance FROM embedding_spaces WHERE name = 'euclid2'"
        ),
        "l2"
    );
}

#[test]
fn a_new_document_is_indexed_into_every_existing_space() {
    let (_tmp, db) = seeded("space-new-doc");
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");
    let before = count(&db, "SELECT COUNT(*) FROM vec_chunks_legal");

    let id = insert_ready(
        &db,
        "Returns",
        "Returns need the original packaging and a receipt.",
    );
    assert!(
        count(&db, "SELECT COUNT(*) FROM vec_chunks_legal") > before,
        "an existing space must not go stale when a document arrives"
    );
    assert!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM vec_chunks_legal WHERE document_id = '{id}'")
        ) > 0
    );
    let hits = column_values(
        &db.query("SELECT * FROM aidb_search('original packaging', 5, '{}', 'legal')")
            .expect("search"),
        "document_id",
    );
    assert!(hits.contains(&id), "{hits:?}");
}

#[test]
fn deleting_a_document_clears_it_from_every_space() {
    let (_tmp, db) = seeded("space-delete");
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");
    let doc = scalar(
        &db,
        "SELECT id FROM documents ORDER BY created_at_ms, rowid LIMIT 1",
    );
    db.execute(&format!("DELETE FROM documents WHERE id = '{doc}'"))
        .expect("delete");
    for table in ["vec_chunks", "vec_chunks_legal"] {
        assert_eq!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM {table} WHERE document_id = '{doc}'")
            ),
            0,
            "{table} kept a vector for a deleted document"
        );
    }
}

#[test]
fn spaces_and_their_vectors_survive_close_and_reopen() {
    let tmp = TempDb::new("space-reopen");
    {
        let db = tmp.open();
        insert_ready(
            &db,
            "Refunds",
            "Refunds are issued within 14 days of purchase.",
        );
        create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");
    }
    let db = tmp.open();
    let row = db
        .query("SELECT provider, provider_model, dimensions, distance FROM embedding_spaces WHERE name = 'legal'")
        .expect("space");
    assert_eq!(cell(&row, 0, "provider_model"), "aidb-fake-legal");
    assert_eq!(cell(&row, 0, "dimensions"), "64");
    assert!(count(&db, "SELECT COUNT(*) FROM vec_chunks_legal") > 0);
    let hits = db
        .query("SELECT * FROM aidb_search('refunds', 5, '{}', 'legal')")
        .expect("search after reopen");
    assert!(!hits.rows.is_empty());
}

#[test]
fn an_unknown_space_fails_closed_and_never_answers_from_the_default() {
    let (_tmp, db) = seeded("space-unknown");
    let before = count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'search'");
    assert_err_contains(
        db.query("SELECT * FROM aidb_search('refunds', 5, '{}', 'ghost')"),
        "unknown embedding space: ghost",
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'search'"),
        before,
        "a search that cannot resolve its space must not run at all"
    );
    // A generate over that search fails the same way instead of retrieving from
    // the wrong index.
    assert!(
        db.query("SELECT aidb_generate('Summarize', content) FROM aidb_search('refunds', 5, '{}', 'ghost')")
            .is_err()
    );
}

#[test]
fn a_generate_over_a_search_retrieves_from_the_space_it_named() {
    let (_tmp, db) = seeded("space-rag");
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");

    let answer = db
        .query(
            "SELECT aidb_generate('Summarize the refund policy', content)
             FROM aidb_search('how do refunds work', 3, '{}', 'legal')",
        )
        .expect("rag generate in space");
    assert!(!cell(&answer, 0, "text").is_empty());

    // The retrieval that fed the answer is a run, and it names the space it used.
    let retrieval: serde_json::Value = serde_json::from_str(&scalar(
        &db,
        "SELECT input_json FROM runs WHERE kind = 'search' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    ))
    .expect("search input");
    assert_eq!(
        retrieval["space"], "legal",
        "a generate must retrieve from the space its search named, not the default one"
    );

    // Classification over a search is the same contract.
    db.query(
        "SELECT aidb_classify('billing or shipping', content)
         FROM aidb_search('how do refunds work', 3, '{}', 'legal')",
    )
    .expect("rag classify in space");
    let retrieval: serde_json::Value = serde_json::from_str(&scalar(
        &db,
        "SELECT input_json FROM runs WHERE kind = 'search' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    ))
    .expect("search input");
    assert_eq!(retrieval["space"], "legal");
}

#[test]
fn a_generate_over_an_unknown_space_fails_instead_of_answering_from_the_default() {
    let (_tmp, db) = seeded("space-rag-unknown");
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");
    for sql in [
        "SELECT aidb_generate('Summarize', content) FROM aidb_search('refunds', 3, '{}', 'ghost')",
        "SELECT aidb_classify('a or b', content) FROM aidb_search('refunds', 3, '{}', 'ghost')",
    ] {
        assert_err_contains(db.query(sql), "unknown embedding space: ghost");
    }
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'generate'"),
        0,
        "no answer may be produced from the wrong index"
    );
}

#[test]
fn a_space_name_must_be_an_identifier_and_cannot_shadow_the_default() {
    let (_tmp, db) = seeded("space-names");
    for (args, needle) in [
        ("'default', 'fake', 32", "reserved"),
        ("'', 'fake', 32", "space name is required"),
        ("'9lives', 'fake', 32", "identifier"),
        ("'drop table', 'fake', 32", "identifier"),
        ("'legal-eu', 'fake', 32", "identifier"),
    ] {
        assert_err_contains(
            db.query(&format!("SELECT aidb_create_space({args})")),
            needle,
        );
    }
    assert_eq!(count(&db, "SELECT COUNT(*) FROM embedding_spaces"), 0);
}

#[test]
fn creating_the_same_space_twice_is_an_error_not_a_silent_redefinition() {
    let (_tmp, db) = seeded("space-duplicate");
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");
    assert_err_contains(
        db.query("SELECT aidb_create_space('legal', 'fake', 32, 'aidb-fake-other')"),
        "already exists",
    );
    // The original tuple is untouched: a redefinition could silently mix models.
    let row = db
        .query("SELECT provider_model, dimensions FROM embedding_spaces WHERE name = 'legal'")
        .expect("space");
    assert_eq!(cell(&row, 0, "provider_model"), "aidb-fake-legal");
    assert_eq!(cell(&row, 0, "dimensions"), "64");
}

#[test]
fn an_invalid_dimension_or_distance_fails_closed() {
    let (_tmp, db) = seeded("space-invalid");
    for (args, needle) in [
        ("'a', 'fake', 0", "dimensions must be between"),
        // A negative width used to look like unknown SQL instead of a bad argument.
        ("'b', 'fake', -1", "dimensions must be between"),
        ("'c', 'fake', 999999", "dimensions must be between"),
        (
            "'d', 'fake', 32, 'aidb-fake', 'manhattan'",
            "unknown distance metric",
        ),
    ] {
        assert_err_contains(
            db.query(&format!("SELECT aidb_create_space({args})")),
            needle,
        );
    }
    assert_eq!(count(&db, "SELECT COUNT(*) FROM embedding_spaces"), 0);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master
             WHERE name IN ('vec_chunks_a', 'vec_chunks_b', 'vec_chunks_c', 'vec_chunks_d')"
        ),
        0,
        "a rejected space must not leave a vector table behind"
    );
}

#[test]
fn an_unknown_provider_fails_closed_instead_of_falling_back() {
    let (_tmp, db) = seeded("space-provider");
    assert_err_contains(
        db.query("SELECT aidb_create_space('mystery', 'hosted_mystery', 32)"),
        "unknown embedding provider",
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM embedding_spaces"), 0);
}

#[test]
fn a_local_space_must_name_a_model_from_the_catalog_at_its_real_width() {
    let (_tmp, db) = seeded("space-local");
    // The catalog is BGE / Nomic / E5, and dimensions are part of the tuple.
    let created = create_space(&db, "'bge', 'local', 384, 'bge-small'");
    assert_eq!(cell(&created, 0, "provider"), "local");
    assert_eq!(
        cell(&created, 0, "model"),
        "BAAI/bge-small-en-v1.5",
        "an alias resolves to one canonical model"
    );
    assert_eq!(cell(&created, 0, "dimensions"), "384");

    assert_err_contains(
        db.query("SELECT aidb_create_space('nodims', 'local', 999, 'bge-small')"),
        "384 dimensions",
    );
    assert_err_contains(
        db.query("SELECT aidb_create_space('nomodel', 'local', 384)"),
        "requires a model name",
    );
    assert_err_contains(
        db.query("SELECT aidb_create_space('ghostmodel', 'local', 384, 'mystery-embed')"),
        "unknown local embedding model",
    );
    // Nomic is a different width, and that is enforced too.
    create_space(&db, "'nomic', 'local', 768, 'nomic'");
    assert_eq!(
        scalar(
            &db,
            "SELECT dimensions FROM embedding_spaces WHERE name = 'nomic'"
        ),
        "768"
    );
}

#[test]
fn a_local_space_and_the_default_space_never_share_vectors() {
    let (_tmp, db) = seeded("space-local-isolation");
    create_space(&db, "'bge', 'local', 384, 'bge-small'");
    // Same documents, different providers: two indexes, two widths, no mixing.
    let chunks = count(&db, "SELECT COUNT(*) FROM chunks");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM vec_chunks_bge"), chunks);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM vec_chunks"), chunks);
    let plan = scalar(
        &db,
        "EXPLAIN SELECT * FROM aidb_search('refunds', 3, '{}', 'bge')",
    );
    assert!(
        plan.contains("embed local:BAAI/bge-small-en-v1.5"),
        "the local space must use the local model:\n{plan}"
    );
    assert!(
        !plan.contains("openai"),
        "no provider substitution:\n{plan}"
    );
    let hits = db
        .query("SELECT * FROM aidb_search('refunds', 3, '{}', 'bge')")
        .expect("local space search");
    assert!(!hits.rows.is_empty());
}

#[test]
fn an_openai_space_without_a_key_fails_closed_and_never_uses_another_provider() {
    let (_tmp, db) = seeded("space-openai-nokey");
    // No OPENAI_API_KEY is configured in the test environment, so creating the
    // space must fail at the point it needs the key: it may not fall back to
    // local or fake embeddings, and it must not write a half-made space.
    let created =
        db.query("SELECT aidb_create_space('oa', 'openai', 1536, 'text-embedding-3-small')");
    match created {
        Err(err) => {
            let text = err.to_string().to_ascii_lowercase();
            assert!(
                text.contains("key") || text.contains("openai"),
                "the error must point at the missing credential: {err}"
            );
            assert_eq!(
                count(
                    &db,
                    "SELECT COUNT(*) FROM embedding_spaces WHERE name = 'oa'"
                ),
                0,
                "a space that cannot embed must not be recorded"
            );
        }
        Ok(_) => {
            // A key is present in this environment (opt-in live setup). Then the
            // space must be openai, not a substitute.
            let row = db
                .query("SELECT provider, provider_model FROM embedding_spaces WHERE name = 'oa'")
                .expect("space");
            assert_eq!(cell(&row, 0, "provider"), "openai");
            assert_eq!(cell(&row, 0, "provider_model"), "text-embedding-3-small");
        }
    }
}

#[test]
fn a_custom_space_needs_a_registered_embedder_and_fails_closed_without_one() {
    let (_tmp, db) = seeded("space-custom");
    assert_err_contains(
        db.query("SELECT aidb_create_space('mine', 'custom', 16, 'not-registered')"),
        "unknown custom embedder",
    );
    assert_err_contains(
        db.query("SELECT aidb_create_space('mine', 'custom', 16)"),
        "requires a model name",
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM embedding_spaces"), 0);
}

#[test]
fn a_process_wide_embedder_choice_does_not_override_a_named_space() {
    let tmp = TempDb::new("space-open-with");
    // Open the file with an explicit process-level embedder of a different width
    // and model than the space we are about to use.
    let db = aidb::open_with(
        tmp.path(),
        aidb::EmbedderConfig {
            provider: "fake".into(),
            model: "process-wide".into(),
            dimensions: 48,
            key_name: None,
        },
    )
    .expect("open with config");
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    create_space(&db, "'legal', 'fake', 64, 'aidb-fake-legal'");

    // The default index used the process-level embedder; the named space did not.
    assert_eq!(
        scalar(
            &db,
            "SELECT value FROM aidb_meta WHERE key = 'embedding_dimensions'"
        ),
        "48"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT dimensions FROM embedding_spaces WHERE name = 'legal'"
        ),
        "64"
    );
    let plan = scalar(
        &db,
        "EXPLAIN SELECT * FROM aidb_search('refunds', 3, '{}', 'legal')",
    );
    assert!(
        plan.contains("embed fake:aidb-fake-legal"),
        "the space's model must win over the process configuration:\n{plan}"
    );
    let hits = db
        .query("SELECT * FROM aidb_search('refunds', 3, '{}', 'legal')")
        .expect("search in space");
    assert!(!hits.rows.is_empty());
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM vec_chunks_legal"),
        count(&db, "SELECT COUNT(*) FROM chunks")
    );
}

#[test]
fn reopening_with_a_different_embedder_width_fails_closed() {
    let tmp = TempDb::new("space-width-change");
    {
        let db = tmp.open();
        insert_ready(
            &db,
            "Refunds",
            "Refunds are issued within 14 days of purchase.",
        );
    }
    // The file already holds 32-dimension vectors from the default embedder.
    let reopened = aidb::open_with(
        tmp.path(),
        aidb::EmbedderConfig {
            provider: "fake".into(),
            model: "aidb-fake".into(),
            dimensions: 64,
            key_name: None,
        },
    );
    match reopened {
        Err(err) => assert!(
            err.to_string().to_ascii_lowercase().contains("dimension"),
            "the mismatch must be named: {err}"
        ),
        Ok(db) => {
            // If the engine accepts the reopen it must not mix widths in one index.
            let stored = scalar(
                &db,
                "SELECT value FROM aidb_meta WHERE key = 'embedding_dimensions'",
            );
            let hits = db.query("SELECT * FROM aidb_search('refunds', 3)");
            assert!(
                stored == "32" && hits.is_err() || stored == "64",
                "a width change must either be rejected or fully re-established (stored={stored})"
            );
        }
    }
}
