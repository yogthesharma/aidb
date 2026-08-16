//! Phase 3 contracts: runs are first-class durable data and every AI operation
//! goes through them. DESIGN.md §14 ("Run and state") and §15 ("Run kinds").

mod common;

use std::time::Duration;

use common::*;

/// Run kinds allowed by the schema. There is no second run store.
const KINDS: [&str; 7] = [
    "index_document",
    "embed_query",
    "search",
    "generate",
    "workflow",
    "agent",
    "tool",
];

#[test]
fn the_schema_accepts_exactly_the_documented_run_kinds_and_statuses() {
    let tmp = TempDb::new("run-kinds");
    let db = tmp.open();
    for (i, kind) in KINDS.iter().enumerate() {
        db.execute(&format!(
            "INSERT INTO runs (id, kind, status, created_at_ms) VALUES ('k{i}', '{kind}', 'pending', 1)"
        ))
        .unwrap_or_else(|e| panic!("kind {kind}: {e}"));
    }
    for (i, status) in [
        "pending",
        "running",
        "succeeded",
        "failed",
        "cancelled",
        "suspended",
        "awaiting_approval",
    ]
    .iter()
    .enumerate()
    {
        db.execute(&format!(
            "INSERT INTO runs (id, kind, status, created_at_ms) VALUES ('s{i}', 'workflow', '{status}', 1)"
        ))
        .unwrap_or_else(|e| panic!("status {status}: {e}"));
    }
    assert!(db
        .execute(
            "INSERT INTO runs (id, kind, status, created_at_ms) VALUES ('x', 'plan', 'pending', 1)"
        )
        .is_err());
    assert!(db
        .execute("INSERT INTO runs (id, kind, status, created_at_ms) VALUES ('y', 'workflow', 'waiting', 1)")
        .is_err());
}

#[test]
fn indexing_writes_a_run_with_ordered_events_and_both_checkpoints() {
    let tmp = TempDb::new("run-index");
    let db = tmp.open();
    let doc = insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let run = scalar(
        &db,
        &format!("SELECT index_run_id FROM documents WHERE id = '{doc}'"),
    );

    let row = db
        .query(&format!(
            "SELECT kind, status, document_id, created_at_ms, started_at_ms, finished_at_ms
             FROM runs WHERE id = '{run}'"
        ))
        .expect("run row");
    assert_eq!(cell(&row, 0, "kind"), "index_document");
    assert_eq!(cell(&row, 0, "status"), "succeeded");
    assert_eq!(cell(&row, 0, "document_id"), doc);
    let created: i64 = cell(&row, 0, "created_at_ms").parse().expect("created");
    let started: i64 = cell(&row, 0, "started_at_ms").parse().expect("started");
    let finished: i64 = cell(&row, 0, "finished_at_ms").parse().expect("finished");
    assert!(
        created <= started && started <= finished,
        "{created} {started} {finished}"
    );

    // Events are an append-only log with a dense, gapless sequence.
    let events = db
        .query(&format!(
            "SELECT seq, kind FROM run_events WHERE run_id = '{run}' ORDER BY seq"
        ))
        .expect("events");
    let seqs: Vec<i64> = column_values(&events, "seq")
        .iter()
        .map(|s| s.parse().expect("seq"))
        .collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>());
    let kinds = column_values(&events, "kind");
    assert_eq!(kinds.first().map(String::as_str), Some("enqueued"));
    assert_eq!(kinds.last().map(String::as_str), Some("ready"));
    assert!(kinds.contains(&"chunk".to_string()));
    assert!(kinds.contains(&"embed".to_string()));

    // DESIGN §15: index_document checkpoints after chunk and after embed.
    let nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{run}' ORDER BY seq"
        ))
        .expect("checkpoints"),
        "node_id",
    );
    assert_eq!(nodes, vec!["chunk".to_string(), "embed".to_string()]);
    let chunk_artifact = scalar(
        &db,
        &format!(
            "SELECT artifact_json FROM checkpoints WHERE run_id = '{run}' AND node_id = 'chunk'"
        ),
    );
    assert!(chunk_artifact.contains("\"chunks\""), "{chunk_artifact}");
}

#[test]
fn search_writes_a_search_run_that_records_the_query_and_the_hit_count() {
    let tmp = TempDb::new("run-search");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let before = count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'search'");
    let hits = db
        .query("SELECT * FROM aidb_search('how do refunds work', 3)")
        .expect("search");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'search'"),
        before + 1,
        "one search produces exactly one durable run"
    );
    let row = db
        .query(
            "SELECT input_json, output_json, status FROM runs
             WHERE kind = 'search' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
        )
        .expect("run");
    assert_eq!(cell(&row, 0, "status"), "succeeded");
    let input: serde_json::Value =
        serde_json::from_str(&cell(&row, 0, "input_json")).expect("input json");
    assert_eq!(input["query"], "how do refunds work");
    assert_eq!(input["k"], 3);
    assert_eq!(input["space"], "default");
    let output: serde_json::Value =
        serde_json::from_str(&cell(&row, 0, "output_json")).expect("output json");
    assert_eq!(output["hits"], hits.rows.len());
}

#[test]
fn generate_writes_a_run_with_tokens_and_cost() {
    let tmp = TempDb::new("run-generate");
    let db = tmp.open();
    let text = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    assert!(!text.is_empty());

    let row = db
        .query(
            "SELECT status, input_json, output_json, prompt_tokens, completion_tokens, cost_usd,
                    created_at_ms, finished_at_ms
             FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
        )
        .expect("generate run");
    assert_eq!(cell(&row, 0, "status"), "succeeded");
    let input: serde_json::Value =
        serde_json::from_str(&cell(&row, 0, "input_json")).expect("input");
    assert_eq!(input["prompt"], "Summarize");
    assert_eq!(input["content"], "Refunds take 14 days.");
    assert!(cell(&row, 0, "output_json").contains(&text[..10.min(text.len())]));
    assert!(
        cell(&row, 0, "prompt_tokens")
            .parse::<i64>()
            .expect("prompt tokens")
            > 0,
        "token counts must persist"
    );
    assert!(
        cell(&row, 0, "completion_tokens").parse::<i64>().is_ok(),
        "completion tokens must persist"
    );
    let cost: f64 = cell(&row, 0, "cost_usd").parse().expect("cost");
    assert!(cost > 0.0, "cost must persist");
    assert!(
        cell(&row, 0, "finished_at_ms")
            .parse::<i64>()
            .expect("finished")
            >= cell(&row, 0, "created_at_ms")
                .parse::<i64>()
                .expect("created")
    );
}

#[test]
fn a_failed_operation_persists_its_error_on_the_run() {
    let tmp = TempDb::new("run-error");
    let db = tmp.open();
    db.execute(
        "CREATE MODEL broken PROVIDER openai MODEL 'gpt-4.1-mini' KEY_NAME 'AIDB_MISSING_KEY_FOR_TESTS'",
    )
    .expect("create model");
    let failed = db.query("SELECT aidb_generate('Summarize', 'body')");
    assert!(failed.is_err(), "a missing key must fail closed");

    let row = db
        .query(
            "SELECT status, error FROM runs WHERE kind = 'generate'
             ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
        )
        .expect("run");
    assert_eq!(cell(&row, 0, "status"), "failed");
    assert!(
        cell(&row, 0, "error").contains("AIDB_MISSING_KEY_FOR_TESTS"),
        "the error must name the missing key: {}",
        cell(&row, 0, "error")
    );
}

#[test]
fn child_runs_point_at_their_parent_and_the_parent_survives_reopen() {
    let tmp = TempDb::new("run-parent");
    let parent;
    {
        let db = tmp.open();
        insert_ready(
            &db,
            "Refunds",
            "Refunds are issued within 14 days of purchase.",
        );
        let out = db
            .query(
                "SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":2}},\
                 {\"generate\":{\"prompt\":\"Summarize\"}}]}')",
            )
            .expect("workflow");
        parent = cell(&out, 0, "run_id");
        assert_eq!(cell(&out, 0, "status"), "succeeded");
    }
    let db = tmp.open();
    assert_eq!(
        scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{parent}'")),
        "workflow"
    );
    let children = db
        .query(&format!(
            "SELECT kind, status FROM runs WHERE parent_id = '{parent}' ORDER BY created_at_ms, rowid"
        ))
        .expect("children");
    let kinds = column_values(&children, "kind");
    assert!(kinds.contains(&"search".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"generate".to_string()), "{kinds:?}");
    assert!(
        column_values(&children, "status")
            .iter()
            .all(|s| s == "succeeded"),
        "child runs must record their own terminal status"
    );
    // A checkpoint per operator, all under the parent run.
    assert!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM checkpoints WHERE run_id = '{parent}'")
        ) >= 2
    );
}

#[test]
fn deleting_a_run_cascades_its_events_and_checkpoints() {
    let tmp = TempDb::new("run-cascade");
    let db = tmp.open();
    let doc = insert_ready(&db, "Refunds", "Refunds take 14 days.");
    let run = scalar(
        &db,
        &format!("SELECT index_run_id FROM documents WHERE id = '{doc}'"),
    );
    assert!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM run_events WHERE run_id = '{run}'")
        ) > 0
    );
    db.execute(&format!("DELETE FROM runs WHERE id = '{run}'"))
        .expect("delete run");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM run_events WHERE run_id = '{run}'")
        ),
        0
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM checkpoints WHERE run_id = '{run}'")
        ),
        0
    );
}

#[test]
fn a_run_left_running_by_a_crash_is_marked_failed_on_reopen() {
    let tmp = TempDb::new("run-recover");
    {
        let db = tmp.open();
        // Simulate the durable trace of a process that died mid-call.
        db.execute(
            "INSERT INTO runs (id, kind, status, input_json, created_at_ms, started_at_ms)
             VALUES ('run_stuck', 'generate', 'running', '{}', 1, 1)",
        )
        .expect("seed");
    }
    let db = tmp.open();
    let row = db
        .query("SELECT status, error, finished_at_ms FROM runs WHERE id = 'run_stuck'")
        .expect("row");
    assert_eq!(cell(&row, 0, "status"), "failed");
    assert_eq!(cell(&row, 0, "error"), "interrupted");
    assert!(!cell(&row, 0, "finished_at_ms").is_empty());
    let events = column_values(
        &db.query("SELECT kind FROM run_events WHERE run_id = 'run_stuck' ORDER BY seq")
            .expect("events"),
        "kind",
    );
    assert!(events.contains(&"interrupted".to_string()));
}

#[test]
fn a_workflow_left_running_by_a_crash_is_resumed_on_reopen_not_restarted() {
    let tmp = TempDb::new("run-resume-open");
    let parent = "run_wf_crashed";
    {
        let db = tmp.open();
        insert_ready(
            &db,
            "Refunds",
            "Refunds are issued within 14 days of purchase.",
        );
        db.execute(&format!(
            "INSERT INTO runs (id, kind, status, input_json, created_at_ms, started_at_ms)
             VALUES ('{parent}', 'workflow',  'running',
                     '{{\"then\":[{{\"search\":{{\"query\":\"refunds\",\"k\":2}}}},\
                       {{\"generate\":{{\"prompt\":\"Summarize\"}}}}]}}', 1, 1)"
        ))
        .expect("seed workflow");
        // The first operator already completed durably.
        db.execute(&format!(
            "INSERT INTO checkpoints (run_id, node_id, seq, artifact_json, created_at_ms)
             VALUES ('{parent}', 'w.0', 1, '{{\"hits\":1,\"text\":\"cached search text\"}}', 1)"
        ))
        .expect("seed checkpoint");
    }

    let db = tmp.open();
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT status FROM runs WHERE id = '{parent}'")
        ),
        "succeeded",
        "a running workflow must be resumed to completion on open"
    );
    // The completed operator was replayed from its artifact, not re-executed:
    // no new search child run was created under this parent.
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'search'")
        ),
        0,
        "resume must not repeat an operator that already has a checkpoint"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'generate'"
            )
        ),
        1,
        "resume must run exactly the operators that were still missing"
    );
    let output = scalar(
        &db,
        &format!("SELECT output_json FROM runs WHERE id = '{parent}'"),
    );
    assert!(
        output.contains("cached search text"),
        "the resumed step must consume the checkpointed artifact: {output}"
    );
}

#[test]
fn resuming_an_already_finished_workflow_does_not_duplicate_work() {
    let tmp = TempDb::new("run-idempotent");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let out = db
        .query(
            "SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":2}},\
             {\"generate\":{\"prompt\":\"Summarize\"}}]}')",
        )
        .expect("workflow");
    let parent = cell(&out, 0, "run_id");
    let children = count(
        &db,
        &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}'"),
    );
    let checkpoints = count(
        &db,
        &format!("SELECT COUNT(*) FROM checkpoints WHERE run_id = '{parent}'"),
    );

    // Reopening repeatedly must be a no-op for a completed run.
    for _ in 0..3 {
        let again = tmp.open();
        again.drain_index(Duration::from_secs(30)).expect("drain");
        assert_eq!(
            count(
                &again,
                &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}'")
            ),
            children
        );
        assert_eq!(
            count(
                &again,
                &format!("SELECT COUNT(*) FROM checkpoints WHERE run_id = '{parent}'")
            ),
            checkpoints
        );
    }
}

#[test]
fn checkpoints_are_unique_per_node_and_keep_a_monotonic_sequence() {
    let tmp = TempDb::new("run-checkpoints");
    let db = tmp.open();
    db.execute(
        "INSERT INTO runs (id, kind, status, created_at_ms) VALUES ('r', 'workflow', 'running', 1)",
    )
    .expect("run");
    db.execute(
        "INSERT INTO checkpoints (run_id, node_id, seq, artifact_json, created_at_ms)
         VALUES ('r', 'a', 1, '{}', 1)",
    )
    .expect("first");
    assert!(
        db.execute(
            "INSERT INTO checkpoints (run_id, node_id, seq, artifact_json, created_at_ms)
             VALUES ('r', 'a', 2, '{}', 2)"
        )
        .is_err(),
        "(run_id, node_id) is the checkpoint identity"
    );
    // run_events enforce a unique sequence per run.
    db.execute("INSERT INTO run_events (run_id, seq, kind, created_at_ms) VALUES ('r', 1, 'x', 1)")
        .expect("event");
    assert!(db
        .execute(
            "INSERT INTO run_events (run_id, seq, kind, created_at_ms) VALUES ('r', 1, 'y', 2)"
        )
        .is_err());
}

#[test]
fn every_ai_operation_lands_in_the_same_runs_table() {
    let tmp = TempDb::new("one-run-store");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let _ = db
        .query("SELECT * FROM aidb_search('refunds', 2)")
        .expect("search");
    let _ = db
        .query("SELECT aidb_generate('Summarize', 'body')")
        .expect("generate");
    let _ = db
        .query("SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":1}}]}')")
        .expect("workflow");
    let _ = db
        .query(
            "SELECT aidb_agent('{\"instructions\":\"Answer from documents.\",\"goal\":\"refunds\",\
             \"tools\":[\"search\"],\"max_steps\":1}')",
        )
        .expect("agent");
    let _ = db.query("SELECT aidb_tool('http.get', '{\"url\":\"aidb://docs\"}')");

    let kinds = column_values(
        &db.query("SELECT DISTINCT kind FROM runs ORDER BY kind")
            .expect("kinds"),
        "kind",
    );
    for expected in ["index_document", "search", "generate", "workflow", "agent"] {
        assert!(
            kinds.contains(&expected.to_string()),
            "{expected} must be recorded in runs: {kinds:?}"
        );
    }
    // There is exactly one durable execution store.
    let tables = column_values(
        &db.query(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND (name LIKE '%run%' OR name LIKE '%agent%'
                   OR name LIKE '%workflow%' OR name LIKE '%step%')
             ORDER BY name",
        )
        .expect("tables"),
        "name",
    );
    assert_eq!(
        tables,
        vec!["run_events".to_string(), "runs".to_string()],
        "no second run store, no agents table"
    );
}
