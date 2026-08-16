//! Phase 6 / 7 contracts: one search function covers vector, keyword and hybrid
//! retrieval, and an answer grounded in retrieval carries real citations.

mod common;

use common::*;

fn corpus(tag: &str) -> (TempDb, aidb::Aidb) {
    let tmp = TempDb::new(tag);
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase. Error code E4021 means the card expired.",
    );
    insert_ready(
        &db,
        "Shipping",
        "Shipping takes three business days after dispatch and tracking is emailed to the customer.",
    );
    insert_ready(
        &db,
        "Warranty",
        "The warranty covers manufacturing defects for one year from delivery.",
    );
    (tmp, db)
}

fn answer_json(db: &aidb::Aidb, sql: &str) -> serde_json::Value {
    let out = db.query(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let text = out.rows[0][0].to_string();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("answer was not JSON ({e}): {text}"))
}

#[test]
fn one_search_function_serves_vector_keyword_and_hybrid_retrieval() {
    let (_tmp, db) = corpus("rag-modes");
    // Same public function, three algorithms chosen from the query and the indexes
    // that exist. There is no second search API to call.
    let semantic = scalar(
        &db,
        "EXPLAIN SELECT * FROM aidb_search('how long do refunds take', 5)",
    );
    assert!(semantic.contains("hybrid"), "{semantic}");
    let keyword = scalar(&db, "EXPLAIN SELECT * FROM aidb_search('E4021', 5)");
    assert!(
        keyword.contains("fts"),
        "an identifier-like query is keyword work:\n{keyword}"
    );

    for query in ["how long do refunds take", "E4021"] {
        let hits = db
            .query(&format!("SELECT * FROM aidb_search('{query}', 5)"))
            .unwrap_or_else(|e| panic!("{query}: {e}"));
        assert!(!hits.rows.is_empty(), "{query} found nothing");
        assert_eq!(
            hits.columns,
            vec!["document_id", "chunk_id", "content", "distance"],
            "every mode returns the same shape"
        );
    }
}

#[test]
fn a_keyword_query_finds_the_exact_token_and_a_semantic_query_finds_the_topic() {
    let (_tmp, db) = corpus("rag-keyword");
    let keyword = column_values(
        &db.query("SELECT * FROM aidb_search('E4021', 5)")
            .expect("fts"),
        "content",
    );
    assert!(
        keyword.iter().any(|c| c.contains("E4021")),
        "an exact token must be retrievable: {keyword:?}"
    );

    let semantic = db
        .query("SELECT * FROM aidb_search('when will my parcel arrive', 3)")
        .expect("semantic");
    assert!(!semantic.rows.is_empty());
    let algorithm = scalar(
        &db,
        "SELECT json_extract(input_json, '$.algorithm') FROM runs
         WHERE kind = 'search' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    );
    assert!(
        algorithm.contains("vec") && algorithm.contains("fts"),
        "a phrase with no identifier blends both signals, got {algorithm}"
    );
}

#[test]
fn the_retrieval_algorithm_is_recorded_on_the_run_and_shown_in_the_plan() {
    let (_tmp, db) = corpus("rag-plan");
    db.query("SELECT * FROM aidb_search('E4021', 3)")
        .expect("search");
    let input: serde_json::Value = serde_json::from_str(&scalar(
        &db,
        "SELECT input_json FROM runs WHERE kind = 'search' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    ))
    .expect("input");
    assert!(
        input["algorithm"]
            .as_str()
            .is_some_and(|a| a.contains("fts")),
        "{input}"
    );
    assert_eq!(input["k"], 3);
    assert_eq!(input["query"], "E4021");

    // EXPLAIN is deterministic for the same query.
    let first = scalar(&db, "EXPLAIN SELECT * FROM aidb_search('E4021', 3)");
    let second = scalar(&db, "EXPLAIN SELECT * FROM aidb_search('E4021', 3)");
    assert_eq!(first, second);
    // The plan is readable: it names the retrieval, the bound k and the model.
    assert!(first.contains("TopK k=3"), "{first}");
    assert!(first.contains("fts5 match"), "{first}");
    assert!(first.contains("fake:aidb-fake"), "{first}");
}

#[test]
fn the_same_query_returns_the_same_order_every_time() {
    let (tmp, db) = corpus("rag-stable-order");
    // Hybrid fusion produces tied scores routinely, and a tie must not be decided
    // by hash iteration order: repeating a query has to repeat the ranking.
    let baseline = column_values(
        &db.query("SELECT * FROM aidb_search('refunds shipping warranty', 3)")
            .expect("search"),
        "chunk_id",
    );
    assert!(baseline.len() > 1, "need several hits to see an order");
    for attempt in 0..12 {
        let again = column_values(
            &db.query("SELECT * FROM aidb_search('refunds shipping warranty', 3)")
                .expect("search"),
            "chunk_id",
        );
        assert_eq!(again, baseline, "attempt {attempt} reordered the results");
    }
    // And a reopened database over the same file agrees.
    drop(db);
    let reopened = tmp.open();
    assert_eq!(
        column_values(
            &reopened
                .query("SELECT * FROM aidb_search('refunds shipping warranty', 3)")
                .expect("search"),
            "chunk_id"
        ),
        baseline
    );
}

#[test]
fn a_generate_over_a_search_returns_an_answer_with_real_citations() {
    let (_tmp, db) = corpus("rag-cite");
    let value = answer_json(
        &db,
        "SELECT aidb_generate('Answer from the sources', content)
         FROM aidb_search('how long do refunds take', 3)",
    );
    assert!(
        value["answer"].as_str().is_some_and(|a| !a.is_empty()),
        "{value}"
    );
    let sources = value["sources"].as_array().expect("sources array");
    assert!(!sources.is_empty(), "a grounded answer must cite something");

    // Every citation points at a row that actually exists in the file.
    for source in sources {
        let doc = source["document_id"].as_str().expect("document_id");
        let chunk = source["chunk_id"].as_str().expect("chunk_id");
        assert_eq!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM documents WHERE id = '{doc}'")
            ),
            1,
            "invented document {doc}"
        );
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM chunks WHERE id = {chunk} AND document_id = '{doc}'"
                )
            ),
            1,
            "chunk {chunk} does not belong to {doc}"
        );
        assert!(
            source["score"].as_f64().is_some_and(f64::is_finite),
            "provenance must keep its score: {source}"
        );
    }
}

#[test]
fn the_citations_are_exactly_the_chunks_that_were_retrieved() {
    let (_tmp, db) = corpus("rag-provenance");
    let retrieved: Vec<(String, String)> = {
        let hits = db
            .query("SELECT * FROM aidb_search('how long do refunds take', 2)")
            .expect("search");
        (0..hits.rows.len())
            .map(|i| (cell(&hits, i, "document_id"), cell(&hits, i, "chunk_id")))
            .collect()
    };

    let value = answer_json(
        &db,
        "SELECT aidb_generate('Answer from the sources', content)
         FROM aidb_search('how long do refunds take', 2)",
    );
    let cited: Vec<(String, String)> = value["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .map(|s| {
            (
                s["document_id"].as_str().unwrap_or_default().to_string(),
                s["chunk_id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        cited, retrieved,
        "sources must be the retrieval nodes, in retrieval order"
    );
}

#[test]
fn a_plain_generate_without_retrieval_stays_a_string() {
    let (_tmp, db) = corpus("rag-plain");
    let out = db
        .query("SELECT aidb_generate('Summarize this', 'Refunds take 14 days')")
        .expect("generate");
    let text = out.rows[0][0].to_string();
    assert!(!text.is_empty());
    assert!(
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("sources").cloned())
            .is_none(),
        "an ungrounded answer must not invent a citation record: {text}"
    );
    // And the run records no sources either.
    let output = scalar(
        &db,
        "SELECT output_json FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    );
    let value: serde_json::Value = serde_json::from_str(&output).expect("output json");
    assert!(value.get("sources").is_none(), "{output}");
}

#[test]
fn the_citations_are_preserved_on_the_run_so_they_outlive_the_query() {
    let tmp = TempDb::new("rag-durable");
    {
        let (_t, db) = (&tmp, tmp.open());
        insert_ready(
            &db,
            "Refunds",
            "Refunds are issued within 14 days of purchase.",
        );
        answer_json(
            &db,
            "SELECT aidb_generate('Answer from the sources', content)
             FROM aidb_search('how long do refunds take', 3)",
        );
    }
    // Reopen: the answer and its provenance are still in the file.
    let db = tmp.open();
    let output = scalar(
        &db,
        "SELECT output_json FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    );
    let value: serde_json::Value = serde_json::from_str(&output).expect("output json");
    let sources = value["sources"].as_array().expect("sources on the run");
    assert!(!sources.is_empty());
    let doc = sources[0]["document_id"].as_str().expect("document_id");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM documents WHERE id = '{doc}'")
        ),
        1
    );
}

#[test]
fn a_search_that_matches_nothing_produces_an_answer_with_no_sources() {
    let tmp = TempDb::new("rag-empty");
    let db = tmp.open();
    // No documents at all: retrieval is empty, and that is not an error.
    let hits = db
        .query("SELECT * FROM aidb_search('anything at all', 5)")
        .expect("search on an empty corpus");
    assert!(hits.rows.is_empty());
    let value = answer_json(
        &db,
        "SELECT aidb_generate('Answer from the sources', content)
         FROM aidb_search('anything at all', 5)",
    );
    assert!(
        value["sources"].as_array().expect("sources").is_empty(),
        "nothing was retrieved, so nothing may be cited: {value}"
    );
    assert!(value["answer"].as_str().is_some());
}

#[test]
fn a_filtered_search_only_cites_documents_that_pass_the_filter() {
    let tmp = TempDb::new("rag-filter");
    let db = tmp.open();
    let support = insert_doc_meta(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
        r#"{"dept":"support"}"#,
    );
    insert_doc_meta(
        &db,
        "Legal",
        "Refunds may be withheld where the contract says so.",
        r#"{"dept":"legal"}"#,
    );
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("drain");

    let value = answer_json(
        &db,
        "SELECT aidb_generate('Answer from the sources', content)
         FROM aidb_search('refunds', 5, '{\"dept\":\"support\"}')",
    );
    let sources = value["sources"].as_array().expect("sources");
    assert!(!sources.is_empty());
    for source in sources {
        assert_eq!(
            source["document_id"].as_str().expect("document_id"),
            support,
            "a filtered retrieval must not cite an excluded document"
        );
    }
}

#[test]
fn the_dialect_and_the_function_form_retrieve_the_same_way() {
    let (_tmp, db) = corpus("rag-equivalence");
    let dialect = db
        .query("SELECT * FROM documents SEARCH 'how long do refunds take' LIMIT 2")
        .expect("dialect");
    let function = db
        .query("SELECT * FROM aidb_search('how long do refunds take', 2)")
        .expect("function");
    assert_eq!(dialect.columns, function.columns);
    assert_eq!(
        column_values(&dialect, "document_id"),
        column_values(&function, "document_id"),
        "the dialect is a frontend, not a second retriever"
    );
    assert_eq!(
        column_values(&dialect, "chunk_id"),
        column_values(&function, "chunk_id")
    );
}

#[test]
fn k_bounds_the_retrieval_and_therefore_the_citations() {
    let (_tmp, db) = corpus("rag-k");
    for k in [1, 2, 3] {
        let value = answer_json(
            &db,
            &format!(
                "SELECT aidb_generate('Answer from the sources', content)
                 FROM aidb_search('refunds shipping warranty', {k})"
            ),
        );
        let sources = value["sources"].as_array().expect("sources");
        assert!(
            sources.len() <= k as usize,
            "k={k} produced {} citations",
            sources.len()
        );
        assert!(!sources.is_empty(), "k={k} produced nothing");
    }
}

#[test]
fn duplicate_chunks_are_cited_once() {
    let (_tmp, db) = corpus("rag-dedupe");
    let value = answer_json(
        &db,
        "SELECT aidb_generate('Answer from the sources', content)
         FROM aidb_search('refunds', 5)",
    );
    let sources = value["sources"].as_array().expect("sources");
    let mut seen = std::collections::HashSet::new();
    for source in sources {
        let key = (
            source["document_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            source["chunk_id"].as_str().unwrap_or_default().to_string(),
        );
        assert!(seen.insert(key.clone()), "{key:?} was cited twice");
    }
}
