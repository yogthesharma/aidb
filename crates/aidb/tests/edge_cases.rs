//! Section 26/27: the failure matrix and a few invariants checked over generated
//! input. Each case here corresponds to a way a real caller can get it wrong.

mod common;

use std::time::Duration;

use common::*;

/// Deterministic pseudo-random text, so a failure is always reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*, chosen only because it needs no dependency.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn word(&mut self) -> String {
        let len = 1 + (self.next() % 12) as usize;
        (0..len)
            .map(|_| {
                let alphabet =
                    b"abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,\n\t";
                alphabet[(self.next() % alphabet.len() as u64) as usize] as char
            })
            .collect()
    }

    fn text(&mut self, words: usize) -> String {
        (0..words)
            .map(|_| self.word())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[test]
fn chunking_keeps_its_invariants_over_generated_text() {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for case in 0..200 {
        let text = rng.text(1 + case % 400);
        let chunks = aidb_index::chunk_text(&text);
        if text.trim().is_empty() {
            assert!(chunks.is_empty(), "case {case}: empty text made a chunk");
            continue;
        }
        assert!(!chunks.is_empty(), "case {case}: text made no chunk");
        for chunk in &chunks {
            assert!(!chunk.trim().is_empty(), "case {case}: blank chunk");
            assert!(
                chunk.chars().count() <= 1600,
                "case {case}: chunk of {} chars",
                chunk.chars().count()
            );
        }
        // Chunking is a pure function of the text.
        assert_eq!(chunks, aidb_index::chunk_text(&text), "case {case}");
        // Every word survives somewhere, so retrieval can still find it.
        let joined = chunks.join(" ");
        for word in text.split_whitespace().take(40) {
            assert!(
                joined.contains(word),
                "case {case}: {word:?} disappeared from the chunks"
            );
        }
    }
}

#[test]
fn unicode_emoji_and_very_long_documents_round_trip_and_stay_searchable() {
    let tmp = TempDb::new("edge-unicode");
    let db = tmp.open();
    let cases = [
        ("Japanese", "返金は購入から14日以内に発行されます。"),
        (
            "Emoji",
            "Refunds 💸 are issued within 14 days ✅ of purchase 🎉",
        ),
        (
            "Accents",
            "Les remboursements sont émis sous 14 jours après l'achat.",
        ),
        ("Mixed", "Refund → возврат → 退款 → ↩️"),
    ];
    let mut ids = Vec::new();
    for (title, content) in cases {
        ids.push((title, insert_ready(&db, title, content)));
    }
    for (title, id) in &ids {
        assert_eq!(
            scalar(
                &db,
                &format!("SELECT index_status FROM documents WHERE id = '{id}'")
            ),
            "ready",
            "{title} never indexed"
        );
        assert!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{id}'")
            ) > 0,
            "{title} has no vectors"
        );
    }
    // The stored text is byte-for-byte what went in.
    let stored = scalar(
        &db,
        &format!("SELECT content FROM documents WHERE id = '{}'", ids[1].1),
    );
    assert_eq!(
        stored,
        "Refunds 💸 are issued within 14 days ✅ of purchase 🎉"
    );

    // A large document is chunked and fully indexed.
    let long = "Refund policy sentence number ".repeat(4000);
    let big = insert_ready(&db, "Long", &long);
    let chunks = count(
        &db,
        &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{big}'"),
    );
    assert!(chunks > 10, "expected many chunks, got {chunks}");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{big}'")
        ),
        chunks,
        "every chunk of a large document gets a vector"
    );
    assert!(!db
        .query("SELECT * FROM aidb_search('refund policy sentence', 5)")
        .expect("search")
        .rows
        .is_empty());
}

#[test]
fn empty_and_degenerate_input_is_handled_rather_than_crashing() {
    let tmp = TempDb::new("edge-empty");
    let db = tmp.open();
    // An empty document is legal, reaches ready, and contributes no chunks.
    let empty = insert_ready(&db, "", "");
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{empty}'")
        ),
        "ready"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{empty}'")
        ),
        0
    );
    // Whitespace only behaves the same way.
    let blank = insert_ready(&db, "Blank", "   \n\t  ");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{blank}'")
        ),
        0
    );
    // Searching a corpus with only empty documents returns nothing, not an error.
    assert!(db
        .query("SELECT * FROM aidb_search('anything', 5)")
        .expect("search")
        .rows
        .is_empty());
    // An empty query is still a well-formed search.
    assert!(db.query("SELECT * FROM aidb_search('', 5)").is_ok());
    // k is clamped upward, never zero or negative.
    for k in ["0", "-3"] {
        let hits = db
            .query(&format!("SELECT * FROM aidb_search('anything', {k})"))
            .unwrap_or_else(|e| panic!("k={k}: {e}"));
        assert!(hits.rows.is_empty());
    }
}

#[test]
fn metadata_must_be_a_json_object_and_anything_else_is_refused() {
    let tmp = TempDb::new("edge-metadata");
    let db = tmp.open();
    for bad in ["[1,2]", "\"text\"", "42", "{not json", "null"] {
        assert_err_contains(
            db.query(&format!(
                "SELECT aidb_insert_document('T', 'body', '{}')",
                sql_escape(bad)
            )),
            "metadata must be a JSON object",
        );
    }
    assert_eq!(count(&db, "SELECT COUNT(*) FROM documents"), 0);

    // An empty object and a deeply nested one are both fine.
    insert_doc_meta(&db, "Empty", "body", "{}");
    let nested = serde_json::json!({
        "dept": "support",
        "tags": ["a", "b"],
        "nested": { "level": { "deeper": 1 } },
        "emoji": "✅",
    })
    .to_string();
    let id = insert_doc_meta(&db, "Nested", "body", &nested);
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT json_extract(metadata_json, '$.nested.level.deeper') FROM documents WHERE id = '{id}'")
        ),
        "1"
    );
    // A filter on a key that does not exist matches nothing, quietly.
    assert!(db
        .query("SELECT * FROM aidb_search('body', 5, '{\"missing\":\"x\"}')")
        .expect("filtered search")
        .rows
        .is_empty());
}

#[test]
fn a_malformed_filter_or_spec_is_a_usage_error_not_a_panic() {
    let tmp = TempDb::new("edge-malformed");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    for sql in [
        "SELECT * FROM aidb_search('refunds', 5, '{not json}')",
        "SELECT aidb_workflow('{not json}')",
        "SELECT aidb_workflow('{\"unknown_node\":1}')",
        "SELECT aidb_workflow('[]')",
        "SELECT aidb_agent('{not json}')",
        "SELECT aidb_agent('{}')",
        "SELECT aidb_mcp_register('nope')",
        "SELECT aidb_set_policy('nope')",
        "SELECT aidb_resume('run_missing', 'nope')",
    ] {
        let result = db.query(sql);
        assert!(result.is_err(), "{sql} should be a usage error");
        let message = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(!message.is_empty(), "{sql} produced an empty error");
    }
    // The database is still healthy afterwards.
    assert_eq!(count(&db, "SELECT COUNT(*) FROM documents"), 1);
    assert!(!db
        .query("SELECT * FROM aidb_search('refunds', 5)")
        .expect("search still works")
        .rows
        .is_empty());
}

#[test]
fn the_same_content_twice_makes_two_documents_that_share_a_hash() {
    let tmp = TempDb::new("edge-duplicate");
    let db = tmp.open();
    let first = insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");
    let second = insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");
    assert_ne!(first, second, "each insert is its own document");
    assert_eq!(
        count(&db, "SELECT COUNT(DISTINCT content_hash) FROM documents"),
        1,
        "identical content hashes identically"
    );
    // Both are independently indexed, and neither steals the other's vectors.
    for id in [&first, &second] {
        assert!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{id}'")
            ) > 0
        );
    }
    // Deleting one leaves the other searchable.
    db.execute(&format!("DELETE FROM documents WHERE id = '{first}'"))
        .expect("delete");
    let hits = column_values(
        &db.query("SELECT * FROM aidb_search('refunds', 5)")
            .expect("search"),
        "document_id",
    );
    assert!(hits.contains(&second) && !hits.contains(&first), "{hits:?}");
}

#[test]
fn redefining_a_catalog_entry_replaces_it_instead_of_duplicating_it() {
    let tmp = TempDb::new("edge-catalog");
    let db = tmp.open();
    db.execute("CREATE MODEL gpt PROVIDER openai KIND llm MODEL 'gpt-4.1-mini'")
        .expect("first");
    db.execute("CREATE MODEL gpt PROVIDER anthropic KIND llm")
        .expect("redefine");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM models WHERE name = 'gpt'"),
        1
    );
    assert_eq!(
        scalar(&db, "SELECT provider FROM models WHERE name = 'gpt'"),
        "anthropic",
        "the newest definition wins"
    );
    // IF NOT EXISTS keeps the original.
    db.execute("CREATE MODEL IF NOT EXISTS gpt PROVIDER fake KIND llm")
        .expect("if not exists");
    assert_eq!(
        scalar(&db, "SELECT provider FROM models WHERE name = 'gpt'"),
        "anthropic"
    );
    // A capability re-registration updates the same row too.
    for side_effect in ["none", "irreversible"] {
        db.query(&format!(
            "SELECT aidb_mcp_register('{{\"tools\":[{{\"name\":\"github.read\",\"side_effect\":\"{side_effect}\"}}]}}')"
        ))
        .expect("register");
    }
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM capabilities WHERE name = 'github.read'"
        ),
        1
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT side_effect FROM capabilities WHERE name = 'github.read'"
        ),
        "irreversible"
    );
}

#[test]
fn referring_to_something_that_does_not_exist_names_it_in_the_error() {
    let tmp = TempDb::new("edge-missing");
    let db = tmp.open();
    for (sql, needle) in [
        (
            "SELECT aidb_generate('Summarize', content) FROM missing_table",
            "missing_table",
        ),
        ("SELECT * FROM aidb_search('q', 5, '{}', 'ghost')", "ghost"),
        ("SELECT aidb_tool('ghost.tool', '{}')", "ghost.tool"),
        (
            "SELECT aidb_resume('run_does_not_exist', '{\"approved\":true}')",
            "run_does_not_exist",
        ),
    ] {
        let message = match db.query(sql) {
            Ok(_) => panic!("{sql} should have failed"),
            Err(err) => err.to_string(),
        };
        assert!(
            message.contains(needle),
            "{sql} error {message:?} does not name {needle:?}"
        );
    }
}

#[test]
fn repeating_the_same_write_is_idempotent_where_the_contract_says_so() {
    let tmp = TempDb::new("edge-idempotent");
    let db = tmp.open();
    let id = insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");
    let chunks = count(&db, "SELECT COUNT(*) FROM chunks");
    let vectors = count(&db, "SELECT COUNT(*) FROM vec_chunks");

    // Draining again, or touching the row without changing the content, must not
    // re-chunk or duplicate vectors.
    db.drain_index(Duration::from_secs(30)).expect("drain");
    db.execute(&format!(
        "UPDATE documents SET title = 'Refund policy' WHERE id = '{id}'"
    ))
    .expect("retitle");
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM chunks"), chunks);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM vec_chunks"), vectors);
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'")
        ),
        "ready"
    );

    // Changing the content does re-index, and still leaves one vector per chunk.
    db.execute(&format!(
        "UPDATE documents SET content = 'Refunds now take 30 days to process.' WHERE id = '{id}'"
    ))
    .expect("rewrite");
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM vec_chunks"),
        count(&db, "SELECT COUNT(*) FROM chunks")
    );
    let hits = column_values(
        &db.query("SELECT * FROM aidb_search('30 days to process', 5)")
            .expect("search"),
        "content",
    );
    assert!(hits.iter().any(|c| c.contains("30 days")), "{hits:?}");
}

#[test]
fn two_writers_on_one_file_serialize_instead_of_corrupting_it() {
    let tmp = TempDb::new("edge-writers");
    let path = tmp.path();
    {
        let db = tmp.open();
        db.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, who TEXT)")
            .expect("table");
    }
    let handles: Vec<_> = (0..4)
        .map(|worker| {
            let path = path.clone();
            std::thread::spawn(move || {
                let db = aidb::open(&path).expect("open");
                for i in 0..25 {
                    db.execute(&format!(
                        "INSERT INTO notes (who) VALUES ('worker {worker} row {i}')"
                    ))
                    .expect("insert");
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("worker");
    }
    let db = tmp.open();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM notes"), 100);
    assert_eq!(
        scalar(&db, "PRAGMA integrity_check"),
        "ok",
        "the file must still be sound"
    );
}

#[test]
fn a_rolled_back_transaction_leaves_nothing_behind() {
    let tmp = TempDb::new("edge-rollback");
    let db = tmp.open();
    db.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)")
        .expect("table");
    db.execute("BEGIN").expect("begin");
    db.execute("INSERT INTO notes (body) VALUES ('temporary')")
        .expect("insert");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM notes"), 1);
    db.execute("ROLLBACK").expect("rollback");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM notes"), 0);

    // A statement that fails mid-way does not half-commit either.
    db.execute("INSERT INTO notes (id, body) VALUES (1, 'first')")
        .expect("first");
    assert!(db
        .execute("INSERT INTO notes (id, body) VALUES (2, 'second'), (1, 'conflict')")
        .is_err());
    assert_eq!(count(&db, "SELECT COUNT(*) FROM notes"), 1);
    assert_eq!(scalar(&db, "SELECT body FROM notes"), "first");
}

#[test]
fn huge_and_awkward_arguments_do_not_break_the_sql_surface() {
    let tmp = TempDb::new("edge-args");
    let db = tmp.open();
    // Quotes inside content are escaped, not injected.
    let tricky = "It's a 'quoted' \"title\" -- with a comment; DROP TABLE documents;";
    let id = insert_ready(&db, "Tricky", tricky);
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT content FROM documents WHERE id = '{id}'")
        ),
        tricky
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'documents'"
        ),
        1,
        "the documents table is still there"
    );

    // A very long query string is answered rather than truncated into nonsense.
    let long_query = "refund ".repeat(2000);
    assert!(db
        .query(&format!(
            "SELECT * FROM aidb_search('{}', 3)",
            sql_escape(&long_query)
        ))
        .is_ok());

    // A huge k is bounded by the corpus, not by an internal index limit.
    for k in ["100000", "4096", "5000"] {
        let hits = db
            .query(&format!("SELECT * FROM aidb_search('refund', {k})"))
            .unwrap_or_else(|e| panic!("k={k}: {e}"));
        assert!(hits.rows.len() <= count(&db, "SELECT COUNT(*) FROM chunks") as usize);
    }
}
