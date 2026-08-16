//! `SELECT aidb_last_run_id()` is this thread's last insert, not a guess
//! by timestamp. Same connection/thread model as `aidb_session`. The bind
//! is not stored; the file stores the run.

mod common;

use common::*;

#[test]
fn aidb_last_run_id_matches_the_generate_that_just_ran() {
    let tmp = TempDb::new("last-run-generate");
    let db = tmp.open();
    assert_eq!(scalar(&db, "SELECT aidb_last_run_id()"), "");

    db.query("SELECT aidb_generate('Summarize this', 'Refunds take 14 days.')")
        .expect("generate");
    let last = scalar(&db, "SELECT aidb_last_run_id()");
    let newest = scalar(
        &db,
        "SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1",
    );
    assert_eq!(last, newest);
    assert!(last.starts_with("run_"), "{last}");
}

#[test]
fn aidb_classify_stamps_last_run_id_to_its_generate_run() {
    let tmp = TempDb::new("last-run-classify");
    let db = tmp.open();
    db.query("SELECT aidb_classify('bullish or bearish or neutral', 'Hyperscaler trims accelerator orders')")
        .expect("classify");
    let last = scalar(&db, "SELECT aidb_last_run_id()");
    let generate = scalar(
        &db,
        "SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1",
    );
    assert_eq!(last, generate);
    assert_eq!(
        scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{last}'")),
        "generate"
    );
}

#[test]
fn a_second_generate_updates_aidb_last_run_id() {
    let tmp = TempDb::new("last-run-second");
    let db = tmp.open();
    db.query("SELECT aidb_generate('What is NVDA?', 'Data center revenue was 47.5 billion.')")
        .expect("first");
    let first = scalar(&db, "SELECT aidb_last_run_id()");
    db.query("SELECT aidb_generate('And the risk?', 'Supply concentration in Taiwan.')")
        .expect("second");
    let second = scalar(&db, "SELECT aidb_last_run_id()");
    assert_ne!(first, second, "a later insert must overwrite the last id");
    assert_eq!(
        second,
        scalar(
            &db,
            "SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1"
        )
    );
}

#[test]
fn a_fresh_aidb_does_not_see_the_previous_last_run_id() {
    let tmp = TempDb::new("last-run-isolate");
    let db = tmp.open();
    db.query("SELECT aidb_generate('Summarize this', 'Refunds take 14 days.')")
        .expect("generate");
    assert!(!scalar(&db, "SELECT aidb_last_run_id()").is_empty());
    drop(db);

    // A new thread is a new process as far as the thread-local bind is concerned.
    let path = tmp.path();
    let leaked = std::thread::spawn(move || {
        let db = aidb::open(&path).expect("open");
        let value = scalar(&db, "SELECT aidb_last_run_id()");
        drop(db);
        value
    })
    .join()
    .expect("thread");
    assert_eq!(leaked, "");
}
