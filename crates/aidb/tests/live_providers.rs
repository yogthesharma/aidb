//! Section 31: opt-in smoke tests against real providers. The normal suite is
//! offline, so every test here returns early unless AIDB_LIVE_TESTS=1 and the key
//! the provider needs is in the environment.
//!
//!     AIDB_LIVE_TESTS=1 OPENAI_API_KEY=... cargo test -p aidb --test live_providers
//!
//! These cost money and talk to the network. They are never required.

mod common;

use std::time::Duration;

use common::*;

fn live() -> bool {
    match std::env::var("AIDB_LIVE_TESTS") {
        Ok(value) => value == "1" || value.eq_ignore_ascii_case("true"),
        Err(_) => false,
    }
}

/// True when live tests are on and the named key is present.
fn enabled(key: &str) -> bool {
    if !live() {
        eprintln!("skipping: set AIDB_LIVE_TESTS=1 to run live provider tests");
        return false;
    }
    if std::env::var(key).is_err() {
        eprintln!("skipping: {key} is not set");
        return false;
    }
    true
}

#[test]
fn openai_embeddings_fill_a_space_and_answer_a_search() {
    if !enabled("OPENAI_API_KEY") {
        return;
    }
    let tmp = TempDb::new("live-openai-embed");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    insert_ready(&db, "Shipping", "Orders ship within two business days.");
    db.query(
        "SELECT aidb_create_space('openai', 'openai', 1536, 'text-embedding-3-small', 'cosine')",
    )
    .expect("create space");
    db.drain_index(Duration::from_secs(120)).expect("backfill");

    let hits = db
        .query("SELECT * FROM aidb_search('how long do I have to return something', 3, '{}', 'openai')")
        .expect("search in the openai space");
    assert!(!hits.rows.is_empty());
    assert!(
        cell(&hits, 0, "content").contains("Refunds"),
        "a real embedding should rank the refund text first: {:?}",
        column_values(&hits, "content")
    );
    // The vectors are the width the space declares.
    assert_eq!(
        scalar(
            &db,
            "SELECT length(embedding) FROM vec_chunks_openai LIMIT 1"
        ),
        "6144",
        "1536 floats"
    );
}

#[test]
fn openai_generation_answers_and_records_cost_on_the_run() {
    if !enabled("OPENAI_API_KEY") {
        return;
    }
    let tmp = TempDb::new("live-openai-llm");
    let db = tmp.open();
    db.execute(
        "CREATE MODEL gpt PROVIDER openai KIND llm MODEL 'gpt-4.1-mini' KEY_NAME 'OPENAI_API_KEY'",
    )
    .expect("model");
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let answer = db
        .query("SELECT aidb_generate('In one sentence, how long do refunds take?', content) FROM aidb_search('refunds', 3)")
        .expect("generate");
    let text = answer.rows[0][0].to_string();
    assert!(text.to_lowercase().contains("14"), "{text}");

    let row = db
        .query(
            "SELECT status, cost_usd, prompt_tokens, completion_tokens FROM runs
             WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
        )
        .expect("run");
    assert_eq!(row.rows[0][0].to_string(), "succeeded");
    assert!(
        row.rows[0][2].to_string().parse::<i64>().unwrap_or(0) > 0,
        "a live call reports prompt tokens"
    );
}

#[test]
fn anthropic_generation_goes_through_the_same_run_engine() {
    if !enabled("ANTHROPIC_API_KEY") {
        return;
    }
    let tmp = TempDb::new("live-anthropic");
    let db = tmp.open();
    db.execute("CREATE MODEL claude PROVIDER anthropic KIND llm KEY_NAME 'ANTHROPIC_API_KEY'")
        .expect("model");
    let answer = db
        .query("SELECT aidb_generate('Reply with the single word: ready', 'ignored context')")
        .expect("generate");
    assert!(!answer.rows[0][0].to_string().trim().is_empty());
    assert_eq!(
        scalar(
            &db,
            "SELECT status FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1"
        ),
        "succeeded"
    );
}

#[test]
fn a_local_fastembed_space_indexes_without_the_network() {
    if !live() {
        eprintln!("skipping: set AIDB_LIVE_TESTS=1 to download a local model");
        return;
    }
    let tmp = TempDb::new("live-local");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.query("SELECT aidb_create_space('local', 'local', 384, 'BAAI/bge-small-en-v1.5', 'cosine')")
        .expect("create space");
    db.drain_index(Duration::from_secs(300)).expect("backfill");
    let hits = db
        .query("SELECT * FROM aidb_search('refund window', 3, '{}', 'local')")
        .expect("search");
    assert!(!hits.rows.is_empty());
}
