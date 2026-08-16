//! Phase 30 contracts: Studio is an inspect face. Every page is a SELECT over the
//! same file `aidb serve` exposes at POST /sql. Approve is `aidb_resume`. There is
//! no second engine and no users table.

mod common;

use common::*;

/// The SELECTs Studio's pages run. Keep in lockstep with `studio/src/lib/catalog.mjs`.
const META: &str = "SELECT key, value FROM aidb_meta ORDER BY key";
const TABLES: &str = "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%_fts%' ORDER BY type, name";
const DOCUMENTS: &str = "SELECT id, title, index_status, (SELECT COUNT(*) FROM chunks c WHERE c.document_id = documents.id) AS chunks, length(content) AS bytes, updated_at_ms, substr(content, 1, 80) AS preview FROM documents ORDER BY updated_at_ms DESC LIMIT 200";
const RUNS: &str = "SELECT id, kind, status, model, prompt_tokens, completion_tokens, cost_usd, created_at_ms, substr(coalesce(error,''), 1, 80) AS error FROM runs ORDER BY created_at_ms DESC LIMIT 100";
const MODELS: &str =
    "SELECT name, kind, provider, provider_model, key_name, dimensions FROM models ORDER BY name";
const EXPERIMENTS: &str = "SELECT experiment_id, plan, dataset, examples, accuracy, recall, llm_calls, cost_usd, latency_ms, status, error, run_id, created_at_ms FROM experiment_results ORDER BY created_at_ms DESC LIMIT 100";
const SESSIONS: &str =
    "SELECT id, runs, turns, started_at_ms, last_at_ms, cost_usd FROM sessions ORDER BY last_at_ms DESC LIMIT 100";
const SESSION_TURNS: &str = "SELECT session_id, turn, run_id, kind, status, cost_usd, created_at_ms FROM session_turns ORDER BY created_at_ms LIMIT 200";
const N_WAITING: &str = "SELECT COUNT(*) FROM runs WHERE status = 'awaiting_approval'";
const N_EXPERIMENTS: &str = "SELECT COUNT(*) FROM experiment_results";
const TOKENS: &str = "SELECT seq, kind, json_extract(payload_json, '$.text') AS text, created_at_ms FROM run_events WHERE kind = 'token' AND run_id = (SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1) ORDER BY seq LIMIT 200";

fn search_sql(query: &str, k: i64) -> String {
    format!(
        "SELECT document_id, chunk_id, substr(content, 1, 200) AS content, distance FROM aidb_search('{}', {k})",
        sql_escape(query)
    )
}

fn resume_sql(run_id: &str, approved: bool) -> String {
    let decision = format!("{{\"approved\":{approved}}}");
    format!(
        "SELECT aidb_resume('{}', '{}')",
        sql_escape(run_id),
        sql_escape(&decision)
    )
}

#[test]
fn every_studio_page_is_a_select_that_runs_on_a_fresh_file() {
    let tmp = TempDb::new("studio-pages");
    let db = tmp.open();

    for (name, sql) in [
        ("file/meta", META),
        ("file/tables", TABLES),
        ("documents", DOCUMENTS),
        ("runs", RUNS),
        ("models", MODELS),
        ("experiments", EXPERIMENTS),
        ("sessions", SESSIONS),
        ("session turns", SESSION_TURNS),
        ("waiting badge", N_WAITING),
        ("experiments badge", N_EXPERIMENTS),
        ("tokens", TOKENS),
    ] {
        db.query(sql)
            .unwrap_or_else(|e| panic!("{name}: {sql}\n  failed: {e}"));
    }

    assert_eq!(
        scalar(
            &db,
            "SELECT type FROM sqlite_master WHERE name = 'experiment_results'"
        ),
        "view"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT type FROM sqlite_master WHERE name = 'sessions'"
        ),
        "view"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT type FROM sqlite_master WHERE name = 'session_turns'"
        ),
        "view"
    );
}

#[test]
fn the_document_search_and_run_pages_show_columns_a_developer_can_click() {
    let tmp = TempDb::new("studio-cols");
    let db = tmp.open();
    let id = insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );

    let docs = db.query(DOCUMENTS).expect("documents page");
    assert!(docs.columns.iter().any(|c| c == "id"));
    assert!(docs.columns.iter().any(|c| c == "index_status"));
    assert!(column_values(&docs, "id").contains(&id));

    let hits = db
        .query(&search_sql("How do refunds work?", 5))
        .expect("search page");
    assert_eq!(
        hits.columns,
        vec!["document_id", "chunk_id", "content", "distance"]
    );
    assert_eq!(cell(&hits, 0, "document_id"), id);

    let runs = db.query(RUNS).expect("runs page");
    assert!(runs.columns.iter().any(|c| c == "status"));
    assert!(
        column_values(&runs, "kind")
            .iter()
            .any(|k| k == "index_document" || k == "search"),
        "the runs page must see the work the other pages caused"
    );
}

#[test]
fn the_waiting_badge_is_the_awaiting_approval_count_and_approve_is_resume() {
    let tmp = TempDb::new("studio-hitl");
    let db = tmp.open();
    assert_eq!(scalar_i64(&db, N_WAITING), 0);

    let parked = db
        .query(
            "SELECT aidb_workflow('{\"then\":[{\"approve\":{\"message\":\"Send this?\"}},{\"generate\":{\"prompt\":\"Draft\"}}]}')",
        )
        .expect("park");
    assert_eq!(cell(&parked, 0, "status"), "awaiting_approval");
    let run_id = cell(&parked, 0, "run_id");

    assert_eq!(scalar_i64(&db, N_WAITING), 1);
    let waiting = db
        .query("SELECT id, kind, status FROM runs WHERE status = 'awaiting_approval'")
        .expect("waiting filter");
    assert_eq!(cell(&waiting, 0, "id"), run_id);

    let resumed = db.query(&resume_sql(&run_id, true)).expect("approve");
    assert_eq!(cell(&resumed, 0, "status"), "succeeded");
    assert_eq!(scalar_i64(&db, N_WAITING), 0);
}

#[test]
fn the_token_snippet_is_the_latest_generate_prefix() {
    let tmp = TempDb::new("studio-tokens");
    let db = tmp.open();
    db.query(
        "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.')",
    )
    .expect("generate");
    let tokens = db.query(TOKENS).expect("tokens");
    assert!(
        tokens.rows.len() > 1,
        "studio should see the prefix, not one concatenated blob: {:?}",
        tokens.rows
    );
    let text: String = tokens
        .rows
        .iter()
        .map(|row| row[col(&tokens, "text")].to_string())
        .collect();
    assert!(text.contains("Refund"), "{text}");
}

#[test]
fn the_experiments_page_is_the_view_the_engine_already_writes() {
    let tmp = TempDb::new("studio-evals");
    let db = tmp.open();
    let gold = insert_ready(
        &db,
        "Refunds",
        "A refund lands on the original card within 14 days of approval.",
    );
    insert_ready(
        &db,
        "Shipping",
        "Standard shipping takes five business days.",
    );
    db.execute(&format!(
        "INSERT INTO eval_examples (dataset, question, expect_text, expect_documents)
         VALUES ('refunds_gold', 'how long does a refund take', '14 days', '[\"{gold}\"]')"
    ))
    .expect("label");

    db.query("SELECT aidb_experiment('{\"dataset\":\"refunds_gold\",\"plans\":[\"naive\",\"cascade\"],\"k\":3}')")
        .expect("experiment");

    let rows = db.query(EXPERIMENTS).expect("experiments page");
    assert!(rows.columns.iter().any(|c| c == "plan"));
    assert!(rows.columns.iter().any(|c| c == "cost_usd"));
    assert!(rows.columns.iter().any(|c| c == "accuracy"));
    let plans = column_values(&rows, "plan");
    assert!(plans.contains(&"naive".to_string()));
    assert!(plans.contains(&"cascade".to_string()));
    assert_eq!(scalar_i64(&db, N_EXPERIMENTS), 2);
}

#[test]
fn studio_does_not_create_a_users_table_or_a_second_store() {
    let tmp = TempDb::new("studio-one-file");
    let db = tmp.open();
    let names = column_values(&db.query(TABLES).expect("catalog"), "name");
    assert!(!names.iter().any(|n| n == "users"));
    assert!(names.contains(&"sessions".to_string()));
    assert!(names.contains(&"session_turns".to_string()));
    assert_eq!(
        scalar(
            &db,
            "SELECT type FROM sqlite_master WHERE name = 'sessions'"
        ),
        "view",
        "sessions is a view over runs, not a table"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'sessions'"
        ),
        0
    );
    assert!(names.contains(&"experiment_results".to_string()));
    assert!(names.contains(&"documents".to_string()));
    assert!(names.contains(&"runs".to_string()));

    let extra: Vec<String> = std::fs::read_dir(tmp.dir())
        .expect("dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with("app.db"))
        .collect();
    assert!(extra.is_empty(), "unexpected files: {extra:?}");
}
