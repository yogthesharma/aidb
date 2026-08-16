//! Phase 34: a session is a thread of runs, not a second store.
//! Turn 1 / 2 / 3 is a SELECT over `session_turns`. Memory stays documents.

mod common;

use common::*;

fn agent(db: &aidb::Aidb, spec: &str) -> (String, String) {
    let out = db
        .query(&format!("SELECT aidb_agent('{}')", sql_escape(spec)))
        .expect("agent");
    (cell(&out, 0, "run_id"), cell(&out, 0, "status"))
}

#[test]
fn a_generate_without_a_session_has_a_null_session_id() {
    let tmp = TempDb::new("session-unscoped");
    let db = tmp.open();
    db.query("SELECT aidb_generate('Summarize this', 'Refunds take 14 days.')")
        .expect("generate");
    assert_eq!(
        scalar(
            &db,
            "SELECT session_id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1"
        ),
        ""
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM session_turns"), 0);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sessions"), 0);
}

#[test]
fn two_generates_after_aidb_session_are_two_turns() {
    let tmp = TempDb::new("session-turns");
    let db = tmp.open();
    assert_eq!(scalar(&db, "SELECT aidb_session('desk:nvda')"), "desk:nvda");
    assert_eq!(scalar(&db, "SELECT aidb_session()"), "desk:nvda");
    db.query("SELECT aidb_generate('What is NVDA?', 'Data center revenue was 47.5 billion.')")
        .expect("turn 1");
    db.query("SELECT aidb_generate('And the risk?', 'Supply concentration in Taiwan.')")
        .expect("turn 2");

    let turns = db
        .query(
            "SELECT turn, kind, json_extract(input_json, '$.prompt') AS prompt
             FROM session_turns
             WHERE session_id = 'desk:nvda'
             ORDER BY turn",
        )
        .expect("turns");
    assert_eq!(turns.rows.len(), 2, "{turns:?}");
    assert_eq!(cell(&turns, 0, "turn"), "1");
    assert_eq!(cell(&turns, 0, "kind"), "generate");
    assert_eq!(cell(&turns, 0, "prompt"), "What is NVDA?");
    assert_eq!(cell(&turns, 1, "turn"), "2");
    assert_eq!(cell(&turns, 1, "prompt"), "And the risk?");

    assert_eq!(
        scalar(&db, "SELECT turns FROM sessions WHERE id = 'desk:nvda'"),
        "2"
    );
    assert_eq!(
        scalar(&db, "SELECT runs FROM sessions WHERE id = 'desk:nvda'"),
        "2"
    );
}

#[test]
fn a_second_session_does_not_leak_into_the_first() {
    let tmp = TempDb::new("session-isolate");
    let db = tmp.open();
    db.query("SELECT aidb_session('desk:nvda')")
        .expect("bind nvda");
    db.query("SELECT aidb_generate('NVDA?', 'Data center revenue.')")
        .expect("nvda");
    db.query("SELECT aidb_session('desk:aapl')")
        .expect("bind aapl");
    db.query("SELECT aidb_generate('AAPL?', 'Gross margin 46 percent.')")
        .expect("aapl");

    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_turns WHERE session_id = 'desk:nvda'"
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_turns WHERE session_id = 'desk:aapl'"
        ),
        1
    );
    let ids = column_values(
        &db.query("SELECT id FROM sessions ORDER BY id")
            .expect("sessions"),
        "id",
    );
    assert_eq!(ids, vec!["desk:aapl".to_string(), "desk:nvda".to_string()]);
}

#[test]
fn an_agent_session_stamps_the_parent_and_children_inherit() {
    let tmp = TempDb::new("session-agent");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let spec = "{\"instructions\":\"Answer from documents. End with DONE.\",\
        \"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"generate\"],\
        \"max_steps\":3,\"session\":\"chat-1\"}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT session_id FROM runs WHERE id = '{parent}'")
        ),
        "chat-1"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND session_id = 'chat-1'"
            )
        ),
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}'")
        ),
        "every child inherits the parent session"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_turns WHERE session_id = 'chat-1' AND kind = 'agent'"
        ),
        1,
        "children are not turns"
    );
}

#[test]
fn unbinding_leaves_later_generates_unscoped() {
    let tmp = TempDb::new("session-clear");
    let db = tmp.open();
    db.query("SELECT aidb_session('chat-1')").expect("bind");
    db.query("SELECT aidb_generate('Hello', 'World')")
        .expect("scoped");
    db.query("SELECT aidb_session(NULL)").expect("clear");
    assert_eq!(scalar(&db, "SELECT aidb_session()"), "");
    db.query("SELECT aidb_generate('Later', 'Unscoped')")
        .expect("unscoped");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND session_id IS NULL"
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_turns WHERE session_id = 'chat-1'"
        ),
        1
    );
}

#[test]
fn an_empty_session_name_is_a_usage_error() {
    let tmp = TempDb::new("session-empty");
    let db = tmp.open();
    assert!(
        db.query("SELECT aidb_session('')").is_err(),
        "empty name must fail"
    );
    assert!(
        db.query("SELECT aidb_session('   ')").is_err(),
        "whitespace name must fail"
    );
}

#[test]
fn sessions_is_a_view_and_memory_stays_documents() {
    let tmp = TempDb::new("session-shape");
    let db = tmp.open();
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
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE '%session%'"
        ),
        0
    );
    assert_eq!(
        scalar(&db, "SELECT type FROM sqlite_master WHERE name = 'memory'"),
        "view"
    );
    db.query("SELECT aidb_memory_insert('user:123', 'Prefers brief answers.')")
        .expect("memory");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM memory WHERE scope = 'user:123'"),
        1
    );
}

#[test]
fn indexing_during_a_session_does_not_become_a_turn() {
    let tmp = TempDb::new("session-index");
    let db = tmp.open();
    db.query("SELECT aidb_session('chat-1')").expect("bind");
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'index_document' AND session_id IS NOT NULL"
        ),
        0,
        "index runs are not a chat thread"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM session_turns"), 0);
}

#[test]
fn explain_session_is_a_control_plan() {
    let tmp = TempDb::new("session-explain");
    let db = tmp.open();
    let plan = db
        .query("EXPLAIN SELECT aidb_session('desk:nvda')")
        .expect("explain");
    let rendered = plan.rows[0][0].to_string();
    assert!(
        rendered.contains("SessionBind") && rendered.contains("plan"),
        "{rendered}"
    );
}

#[test]
fn a_workflow_can_name_its_session() {
    let tmp = TempDb::new("session-workflow");
    let db = tmp.open();
    db.query(
        "SELECT aidb_workflow('{\"session\":\"desk:nvda\",\"then\":[{\"generate\":{\"prompt\":\"Summarize this\"}}]}')",
    )
    .expect("workflow");
    assert_eq!(
        scalar(
            &db,
            "SELECT session_id FROM runs WHERE kind = 'workflow' ORDER BY created_at_ms DESC LIMIT 1"
        ),
        "desk:nvda"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM session_turns WHERE session_id = 'desk:nvda' AND kind = 'workflow'"
        ),
        1
    );
}
