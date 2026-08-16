//! Phase 32: generate/classify can take a JSON schema. Invalid model output
//! fails the run (a row the app can SELECT), not a library exception type.

mod common;

use common::*;

const SUMMARY_SCHEMA: &str =
    r#"{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}"#;
const NONCE_SCHEMA: &str =
    r#"{"type":"object","properties":{"nonce":{"const":"UNSAT"}},"required":["nonce"]}"#;
const ENUM_SCHEMA: &str = r#"{"enum":["positive","negative"]}"#;

fn latest_generate(db: &aidb::Aidb) -> aidb::QueryResult {
    db.query(
        "SELECT status, error, input_json, output_json, prompt_tokens, cost_usd
         FROM runs WHERE kind = 'generate'
         ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    )
    .expect("latest generate run")
}

fn extract_summary(text: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or_else(|_| panic!("{text}"));
    if let Some(summary) = value.get("summary").and_then(|v| v.as_str()) {
        return summary.to_string();
    }
    // Retrieval wraps the model JSON in {answer, sources}.
    let answer = value.get("answer").and_then(|v| v.as_str()).unwrap_or(text);
    serde_json::from_str::<serde_json::Value>(answer)
        .ok()
        .and_then(|inner| inner.get("summary")?.as_str().map(str::to_string))
        .unwrap_or_else(|| answer.to_string())
}

fn generate_runs(db: &aidb::Aidb) -> i64 {
    count(db, "SELECT COUNT(*) FROM runs WHERE kind = 'generate'")
}

#[test]
fn two_arg_generate_and_classify_are_unchanged() {
    let tmp = TempDb::new("struct-two-arg");
    let db = tmp.open();
    let text = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    assert!(
        text.starts_with("Summarize:"),
        "two-arg generate still returns the model text, got {text}"
    );
    let label = scalar(
        &db,
        "SELECT aidb_classify('billing or shipping', 'My invoice is wrong')",
    );
    assert_eq!(label, "billing");
}

#[test]
fn a_schema_returns_canonical_json_and_records_it_on_the_run() {
    let tmp = TempDb::new("struct-ok");
    let db = tmp.open();
    let text = scalar(
        &db,
        &format!(
            "SELECT aidb_generate('Extract a summary', 'Refunds take 14 days.', '{SUMMARY_SCHEMA}')"
        ),
    );
    let value: serde_json::Value = serde_json::from_str(&text).expect("canonical json");
    assert!(
        value["summary"].as_str().unwrap().contains("Refunds"),
        "{value}"
    );

    let row = latest_generate(&db);
    assert_eq!(cell(&row, 0, "status"), "succeeded");
    let input: serde_json::Value =
        serde_json::from_str(&cell(&row, 0, "input_json")).expect("input");
    assert_eq!(input["schema"]["required"][0], "summary");
    assert!(cell(&row, 0, "output_json").contains("summary"));
}

#[test]
fn output_that_misses_the_schema_fails_the_run() {
    let tmp = TempDb::new("struct-miss");
    let db = tmp.open();
    let before = generate_runs(&db);
    assert_err_contains(
        db.query(&format!(
            "SELECT aidb_generate('Extract', 'Refunds take 14 days.', '{NONCE_SCHEMA}')"
        )),
        "output did not match schema",
    );

    assert_eq!(generate_runs(&db), before + 1);
    let row = latest_generate(&db);
    assert_eq!(cell(&row, 0, "status"), "failed");
    assert!(
        cell(&row, 0, "error").contains("output did not match schema"),
        "{}",
        cell(&row, 0, "error")
    );
    let output: serde_json::Value =
        serde_json::from_str(&cell(&row, 0, "output_json")).expect("output");
    assert!(output["text"].as_str().is_some(), "{output}");
    assert!(
        output["schema_error"]
            .as_str()
            .unwrap()
            .contains("output did not match schema"),
        "{output}"
    );
    assert!(
        cell(&row, 0, "prompt_tokens")
            .parse::<i64>()
            .expect("tokens")
            > 0,
        "spend is still recorded on a schema failure"
    );
    assert!(cell(&row, 0, "cost_usd").parse::<f64>().expect("cost") > 0.0);
}

#[test]
fn a_schema_that_is_not_json_is_a_usage_error_with_no_run() {
    let tmp = TempDb::new("struct-bad-schema");
    let db = tmp.open();
    let before = generate_runs(&db);
    assert_err_contains(
        db.query("SELECT aidb_generate('Extract', 'body', 'not json')"),
        "not JSON",
    );
    assert_eq!(
        generate_runs(&db),
        before,
        "junk schema must not open a generate run"
    );
}

#[test]
fn classify_with_an_enum_schema_returns_a_listed_label() {
    let tmp = TempDb::new("struct-enum");
    let db = tmp.open();
    let text = scalar(
        &db,
        &format!(
            "SELECT aidb_classify('positive or negative', 'This is a positive surprise.', '{ENUM_SCHEMA}')"
        ),
    );
    let value: serde_json::Value = serde_json::from_str(&text).expect("json string");
    assert_eq!(value, "positive");
    assert_eq!(cell(&latest_generate(&db), 0, "status"), "succeeded");
}

#[test]
fn a_label_outside_the_enum_fails_the_classify_run() {
    let tmp = TempDb::new("struct-enum-miss");
    let db = tmp.open();
    assert_err_contains(
        db.query(&format!(
            "SELECT aidb_classify('positive or negative', 'zzz unrelated text', '{ENUM_SCHEMA}')"
        )),
        "output did not match schema",
    );
    assert_eq!(cell(&latest_generate(&db), 0, "status"), "failed");
}

#[test]
fn from_documents_and_from_search_accept_a_schema() {
    let tmp = TempDb::new("struct-from");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );

    let over_docs = scalar(
        &db,
        &format!(
            "SELECT aidb_generate('Extract a summary', content, '{SUMMARY_SCHEMA}') FROM documents"
        ),
    );
    assert!(
        extract_summary(&over_docs).contains("Refunds"),
        "{over_docs}"
    );

    let over_search = scalar(
        &db,
        &format!(
            "SELECT aidb_generate('Extract a summary', content, '{SUMMARY_SCHEMA}') \
             FROM aidb_search('refunds', 3)"
        ),
    );
    assert!(
        extract_summary(&over_search).contains("Refunds"),
        "{over_search}"
    );
}

#[test]
fn the_cache_key_includes_the_schema() {
    let tmp = TempDb::new("struct-cache");
    let db = tmp.open();
    let plain = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    let again = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    assert_eq!(plain, again);

    let structured = scalar(
        &db,
        &format!("SELECT aidb_generate('Summarize', 'Refunds take 14 days.', '{SUMMARY_SCHEMA}')"),
    );
    assert_ne!(
        plain, structured,
        "a schema is a different call, not a replay of the untyped cache"
    );
    let structured_again = scalar(
        &db,
        &format!("SELECT aidb_generate('Summarize', 'Refunds take 14 days.', '{SUMMARY_SCHEMA}')"),
    );
    assert_eq!(structured, structured_again);

    let runs = db
        .query("SELECT output_json FROM runs WHERE kind = 'generate' ORDER BY created_at_ms, rowid")
        .expect("runs");
    assert_eq!(runs.rows.len(), 4);
    let last: serde_json::Value =
        serde_json::from_str(&cell(&runs, 3, "output_json")).expect("cached output");
    assert_eq!(last["cache"], true, "{last}");
}

#[test]
fn invalid_output_is_not_cached() {
    let tmp = TempDb::new("struct-no-cache-fail");
    let db = tmp.open();
    let sql = format!("SELECT aidb_generate('Extract', 'Refunds take 14 days.', '{NONCE_SCHEMA}')");
    assert_err_contains(db.query(&sql), "output did not match schema");
    assert_err_contains(db.query(&sql), "output did not match schema");

    let rows = db
        .query(
            "SELECT output_json FROM runs WHERE kind = 'generate' AND status = 'failed'
             ORDER BY created_at_ms, rowid",
        )
        .expect("failed runs");
    assert_eq!(rows.rows.len(), 2);
    for i in 0..2 {
        let output: serde_json::Value =
            serde_json::from_str(&cell(&rows, i, "output_json")).expect("output");
        assert!(
            output.get("schema_error").is_some(),
            "a failed schema run keeps the raw output rather than a cache-hit stub: {output}"
        );
    }
}
