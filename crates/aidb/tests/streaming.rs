//! Phase 35: tokens append to the generate run. A reconnect still has the prefix.
//! Streaming is not a second generate path.

mod common;

use common::*;

fn latest_generate_id(db: &aidb::Aidb) -> String {
    scalar(
        db,
        "SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    )
}

fn token_texts(db: &aidb::Aidb, run_id: &str) -> Vec<String> {
    column_values(
        &db.query(&format!(
            "SELECT json_extract(payload_json, '$.text') AS text
             FROM run_events WHERE run_id = '{run_id}' AND kind = 'token' ORDER BY seq"
        ))
        .expect("tokens"),
        "text",
    )
}

#[test]
fn generate_appends_token_events_whose_concatenation_is_the_output() {
    let tmp = TempDb::new("stream-concat");
    let db = tmp.open();
    let text = scalar(
        &db,
        "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.')",
    );
    assert!(!text.is_empty());
    let run_id = latest_generate_id(&db);
    let parts = token_texts(&db, &run_id);
    assert!(
        parts.len() > 1,
        "fake generate must stream more than one token: {parts:?}"
    );
    assert_eq!(parts.concat(), text, "the prefix is the answer");
    let kinds = column_values(
        &db.query(&format!(
            "SELECT kind FROM run_events WHERE run_id = '{run_id}' ORDER BY seq"
        ))
        .expect("events"),
        "kind",
    );
    assert_eq!(kinds.first().map(String::as_str), Some("started"));
    assert_eq!(kinds.last().map(String::as_str), Some("generated"));
    assert!(kinds.contains(&"token".to_string()), "{kinds:?}");
}

#[test]
fn a_cache_hit_does_not_pretend_to_stream() {
    let tmp = TempDb::new("stream-cache");
    let db = tmp.open();
    db.query("SELECT aidb_generate('Summarize this', 'Refunds take 14 days.')")
        .expect("cold");
    db.query("SELECT aidb_generate('Summarize this', 'Refunds take 14 days.')")
        .expect("warm");
    let run_id = latest_generate_id(&db);
    assert_eq!(
        scalar(
            &db,
            &format!(
                "SELECT kind FROM run_events WHERE run_id = '{run_id}' AND kind = 'cache_hit'"
            )
        ),
        "cache_hit"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM run_events WHERE run_id = '{run_id}' AND kind = 'token'"
            )
        ),
        0,
        "a cache hit is not a live token stream"
    );
}

#[test]
fn two_arg_generate_is_still_the_same_path() {
    let tmp = TempDb::new("stream-same-path");
    let db = tmp.open();
    let text = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT status FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1"
        ),
        "succeeded"
    );
    assert!(
        text.contains("Refunds") || text.contains("Summarize"),
        "{text}"
    );
}

#[test]
fn listeners_see_tokens_as_they_are_written() {
    use std::sync::{Arc, Mutex};

    let tmp = TempDb::new("stream-listen");
    let db = tmp.open();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let slot = Arc::clone(&seen);
    aidb::subscribe_tokens(Arc::new(move |event: &aidb::TokenEvent| {
        slot.lock()
            .expect("seen")
            .push((event.run_id.clone(), event.text.clone()));
    }));
    let text = scalar(
        &db,
        "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.')",
    );
    let run_id = latest_generate_id(&db);
    let parts: Vec<String> = seen
        .lock()
        .expect("seen")
        .iter()
        .filter(|(id, _)| id == &run_id)
        .map(|(_, text)| text.clone())
        .collect();
    assert!(parts.len() > 1, "{parts:?}");
    assert_eq!(parts.concat(), text);
}
