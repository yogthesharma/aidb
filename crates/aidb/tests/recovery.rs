//! Crash / recovery contracts. A real child process is aborted at a named
//! execution boundary (`AIDB_TEST_CRASH_POINT`), then the file is reopened and
//! the invariant is checked: work already committed is not lost, work not yet
//! committed is redone, and nothing durable is duplicated.
//!
//! DESIGN.md §14: "load run → replay Succeeded from artifacts → reschedule".

mod common;

use std::process::Command;
use std::time::Duration;

use common::*;

/// Run `aidb sql` in a child process that aborts at `crash_at`.
/// Returns true when the child really died instead of finishing.
fn crash_at(path: &std::path::Path, crash_point: &str, sql: &str) -> bool {
    let output = Command::new(cli_bin())
        .args(["sql", path.to_str().expect("path"), sql])
        .env("AIDB_TEST_CRASH_POINT", crash_point)
        .output()
        .expect("spawn crashing child");
    // `abort()` is a signal death, so there is no exit code on unix.
    !output.status.success()
}

fn run_sql(path: &std::path::Path, sql: &str) -> std::process::Output {
    Command::new(cli_bin())
        .args(["sql", path.to_str().expect("path"), sql])
        .output()
        .expect("spawn child")
}

const INSERT: &str =
    "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');";

#[test]
fn crash_before_chunking_leaves_a_pending_document_that_indexes_on_reopen() {
    let tmp = TempDb::new("crash-before-chunk");
    let path = tmp.path();
    assert!(crash_at(&path, "before_chunk", INSERT), "child must die");

    {
        // Inspect the persisted state before resuming.
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let status: String = raw
            .query_row("SELECT index_status FROM documents", [], |r| r.get(0))
            .expect("document survived the crash");
        assert!(
            status == "pending" || status == "indexing",
            "crash before chunking must leave the document unindexed, got {status}"
        );
        let chunks: i64 = raw
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .expect("chunks");
        assert_eq!(chunks, 0, "nothing was chunked yet");
    }

    let db = tmp.open();
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(scalar(&db, "SELECT index_status FROM documents"), "ready");
    let chunks = count(&db, "SELECT COUNT(*) FROM chunks");
    assert!(chunks > 0);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM vec_chunks"), chunks);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'index_document'"
        ),
        1,
        "resuming must not enqueue a second index run"
    );
}

#[test]
fn crash_after_chunking_resumes_from_the_chunk_checkpoint_without_re_chunking() {
    let tmp = TempDb::new("crash-after-chunk");
    let path = tmp.path();
    assert!(crash_at(&path, "after_chunk", INSERT), "child must die");

    let chunk_ids: Vec<i64> = {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let chunks: i64 = raw
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .expect("chunks");
        assert!(chunks > 0, "chunking was committed before the crash");
        // A raw connection has no vec0 module, so durability of the embed step is
        // observed through its checkpoint instead of the vector table.
        let embedded: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE node_id = 'embed'",
                [],
                |r| r.get(0),
            )
            .expect("embed checkpoint");
        assert_eq!(embedded, 0, "embedding had not completed");
        let node: String = raw
            .query_row(
                "SELECT node_id FROM checkpoints WHERE node_id = 'chunk'",
                [],
                |r| r.get(0),
            )
            .expect("chunk checkpoint is durable");
        assert_eq!(node, "chunk");
        let mut stmt = raw
            .prepare("SELECT id FROM chunks ORDER BY ordinal")
            .expect("prepare");
        let ids = stmt
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<Vec<i64>, _>>()
            .expect("ids");
        ids
    };

    let db = tmp.open();
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(scalar(&db, "SELECT index_status FROM documents"), "ready");
    let after: Vec<String> = column_values(
        &db.query("SELECT id FROM chunks ORDER BY ordinal")
            .expect("chunks"),
        "id",
    );
    assert_eq!(
        after,
        chunk_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
        "already-committed chunks must be reused, not rebuilt"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM vec_chunks"),
        chunk_ids.len() as i64
    );
    // The resume is recorded, and the chunk operator was not repeated.
    let events = column_values(
        &db.query("SELECT kind FROM run_events ORDER BY seq")
            .expect("events"),
        "kind",
    );
    assert!(events.contains(&"resume".to_string()), "{events:?}");
    assert_eq!(
        events.iter().filter(|k| *k == "chunk").count(),
        1,
        "the chunk operator must be recorded exactly once: {events:?}"
    );
}

#[test]
fn crash_after_embedding_but_before_the_vector_write_redoes_only_the_missing_vectors() {
    let tmp = TempDb::new("crash-after-embed");
    let path = tmp.path();
    assert!(crash_at(&path, "after_embed", INSERT), "child must die");

    {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let chunks: i64 = raw
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .expect("chunks");
        assert!(chunks > 0);
        let embedded: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE node_id = 'embed'",
                [],
                |r| r.get(0),
            )
            .expect("embed checkpoint");
        assert_eq!(embedded, 0, "the embed checkpoint was not reached");
    }

    let db = tmp.open();
    db.drain_index(Duration::from_secs(30)).expect("drain");
    let chunks = count(&db, "SELECT COUNT(*) FROM chunks");
    assert_eq!(scalar(&db, "SELECT index_status FROM documents"), "ready");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM vec_chunks"),
        chunks,
        "every chunk ends up with exactly one vector"
    );
    let hits = db
        .query("SELECT * FROM aidb_search('how do refunds work', 5)")
        .expect("search");
    assert!(!hits.rows.is_empty(), "the resumed document is searchable");
}

#[test]
fn crash_after_the_vector_write_does_not_duplicate_vectors_on_resume() {
    let tmp = TempDb::new("crash-after-vec");
    let path = tmp.path();
    assert!(crash_at(&path, "after_vec", INSERT), "child must die");

    {
        // A raw connection has no sqlite-vec module, so inspect the durable
        // bookkeeping instead of the virtual table.
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let vec_table: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'vec_chunks'",
                [],
                |r| r.get(0),
            )
            .expect("vec table");
        assert_eq!(
            vec_table, 1,
            "the vector store was created before the crash"
        );
        let embedded: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE node_id = 'embed'",
                [],
                |r| r.get(0),
            )
            .expect("embed checkpoint");
        assert_eq!(
            embedded, 0,
            "the crash landed after the write, before the commit record"
        );
        let status: String = raw
            .query_row("SELECT index_status FROM documents", [], |r| r.get(0))
            .expect("status");
        assert_ne!(status, "ready", "the document was not finished yet");
    }

    let db = tmp.open();
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(scalar(&db, "SELECT index_status FROM documents"), "ready");
    let chunks = count(&db, "SELECT COUNT(*) FROM chunks");
    assert!(chunks > 0);
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM vec_chunks"),
        chunks,
        "resume must not insert a second vector per chunk"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(DISTINCT chunk_id) FROM vec_chunks"),
        chunks,
        "every chunk has exactly one vector"
    );
}

#[test]
fn crash_between_the_embed_checkpoint_and_the_finish_still_completes_on_reopen() {
    let tmp = TempDb::new("crash-after-cp");
    let path = tmp.path();
    assert!(
        crash_at(&path, "after_embed_checkpoint", INSERT),
        "child must die"
    );

    {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let nodes: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE node_id = 'embed'",
                [],
                |r| r.get(0),
            )
            .expect("embed checkpoint");
        assert_eq!(nodes, 1, "the embed checkpoint is durable");
        let status: String = raw
            .query_row("SELECT index_status FROM documents", [], |r| r.get(0))
            .expect("status");
        assert_ne!(status, "ready");
    }

    let db = tmp.open();
    db.drain_index(Duration::from_secs(30)).expect("drain");
    assert_eq!(scalar(&db, "SELECT index_status FROM documents"), "ready");
    assert_eq!(
        scalar(&db, "SELECT status FROM runs WHERE kind = 'index_document'"),
        "succeeded"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM checkpoints WHERE node_id = 'embed'"
        ),
        1,
        "the embed checkpoint stays a single row"
    );
}

#[test]
fn crash_during_generate_leaves_a_running_run_that_reopen_marks_failed() {
    let tmp = TempDb::new("crash-generate");
    let path = tmp.path();
    assert!(run_sql(&path, INSERT).status.success());
    assert!(
        crash_at(
            &path,
            "before_llm",
            "SELECT aidb_generate('Summarize', 'Refunds take 14 days.');"
        ),
        "child must die"
    );

    {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let status: String = raw
            .query_row("SELECT status FROM runs WHERE kind = 'generate'", [], |r| {
                r.get(0)
            })
            .expect("the generate run was written before the call");
        assert_eq!(status, "running", "the crash happened mid-call");
    }

    let db = tmp.open();
    let row = db
        .query("SELECT status, error FROM runs WHERE kind = 'generate'")
        .expect("run");
    assert_eq!(cell(&row, 0, "status"), "failed");
    assert_eq!(cell(&row, 0, "error"), "interrupted");
    // A retry after recovery succeeds and produces its own run.
    let text = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    assert!(!text.is_empty());
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND status = 'succeeded'"
        ),
        1
    );
}

#[test]
fn crash_mid_stream_keeps_the_token_prefix_on_the_run() {
    let tmp = TempDb::new("crash-token");
    let path = tmp.path();
    assert!(
        crash_at(
            &path,
            "after_token",
            "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.');"
        ),
        "child must die"
    );

    {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let tokens: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM run_events WHERE kind = 'token'",
                [],
                |r| r.get(0),
            )
            .expect("tokens survived");
        assert!(tokens >= 1, "at least the first token must be durable");
        let status: String = raw
            .query_row("SELECT status FROM runs WHERE kind = 'generate'", [], |r| {
                r.get(0)
            })
            .expect("run");
        assert_eq!(status, "running");
    }

    let db = tmp.open();
    assert_eq!(
        scalar(&db, "SELECT status FROM runs WHERE kind = 'generate'"),
        "failed"
    );
    assert_eq!(
        scalar(&db, "SELECT error FROM runs WHERE kind = 'generate'"),
        "interrupted"
    );
    assert_eq!(
        scalar(&db, "SELECT output_json FROM runs WHERE kind = 'generate'"),
        "",
        "the finished output is not committed; the prefix is events"
    );
    let prefix = column_values(
        &db.query(
            "SELECT json_extract(payload_json, '$.text') AS text
             FROM run_events WHERE kind = 'token' ORDER BY seq",
        )
        .expect("prefix"),
        "text",
    )
    .concat();
    assert!(!prefix.is_empty(), "reconnect still has the prefix");
}

#[test]
fn crash_after_the_model_call_does_not_leave_a_half_written_generate_run() {
    let tmp = TempDb::new("crash-after-llm");
    let path = tmp.path();
    assert!(run_sql(&path, INSERT).status.success());
    assert!(
        crash_at(
            &path,
            "after_llm",
            "SELECT aidb_generate('Summarize', 'Refunds take 14 days.');"
        ),
        "child must die"
    );

    let db = tmp.open();
    let row = db
        .query("SELECT status, output_json, error FROM runs WHERE kind = 'generate'")
        .expect("run");
    // The model answered but nothing was committed, so the run is interrupted and
    // carries no output. It must never look like a success.
    assert_eq!(cell(&row, 0, "status"), "failed");
    assert!(
        cell(&row, 0, "output_json").is_empty(),
        "an interrupted call must not publish an output: {}",
        cell(&row, 0, "output_json")
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM checkpoints WHERE node_id = 'generate'"
        ),
        0
    );
}

#[test]
fn crash_between_workflow_operators_resumes_at_the_first_missing_operator() {
    let tmp = TempDb::new("crash-workflow");
    let path = tmp.path();
    assert!(run_sql(&path, INSERT).status.success());
    let workflow = "SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":2}},\
                    {\"generate\":{\"prompt\":\"Summarize\"}}]}');";
    assert!(
        crash_at(&path, "after_checkpoint", workflow),
        "child must die"
    );

    let (parent, done_nodes) = {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let parent: String = raw
            .query_row("SELECT id FROM runs WHERE kind = 'workflow'", [], |r| {
                r.get(0)
            })
            .expect("workflow run is durable");
        let status: String = raw
            .query_row("SELECT status FROM runs WHERE id = ?1", [&parent], |r| {
                r.get(0)
            })
            .expect("status");
        assert_eq!(
            status, "running",
            "the parent was left running by the crash"
        );
        let mut stmt = raw
            .prepare("SELECT node_id FROM checkpoints WHERE run_id = ?1 ORDER BY seq")
            .expect("prepare");
        let nodes = stmt
            .query_map([&parent], |r| r.get::<_, String>(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("nodes");
        assert_eq!(
            nodes,
            vec!["w.0".to_string()],
            "exactly the first operator was checkpointed"
        );
        (parent, nodes)
    };

    let db = tmp.open();
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT status FROM runs WHERE id = '{parent}'")
        ),
        "succeeded",
        "reopen resumes the interrupted workflow"
    );
    // The already-checkpointed search was replayed from its artifact: only one
    // search child run exists in total (the one from before the crash).
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'search'")
        ),
        1,
        "a completed operator must not run twice"
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'generate'"
            )
        ),
        1
    );
    let nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{parent}' ORDER BY seq"
        ))
        .expect("checkpoints"),
        "node_id",
    );
    assert!(nodes.len() > done_nodes.len(), "{nodes:?}");
    assert!(nodes.contains(&"w.0".to_string()));
}

#[test]
fn crash_before_a_workflow_checkpoint_repeats_only_the_uncommitted_operator() {
    let tmp = TempDb::new("crash-wf-before-cp");
    let path = tmp.path();
    assert!(run_sql(&path, INSERT).status.success());
    let workflow =
        "SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":2}}]}');";
    assert!(
        crash_at(&path, "before_checkpoint", workflow),
        "child must die"
    );

    let parent = {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let parent: String = raw
            .query_row("SELECT id FROM runs WHERE kind = 'workflow'", [], |r| {
                r.get(0)
            })
            .expect("workflow run");
        let checkpoints: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE run_id = ?1",
                [&parent],
                |r| r.get(0),
            )
            .expect("checkpoints");
        assert_eq!(checkpoints, 0, "nothing was checkpointed");
        parent
    };

    let db = tmp.open();
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT status FROM runs WHERE id = '{parent}'")
        ),
        "succeeded"
    );
    // The search ran once before the crash and once on resume: the operator is
    // retry-safe (DESIGN §11: Embed / Similarity retry = Safe). What must not
    // happen is a duplicated durable record of the same node.
    let mut nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{parent}'"
        ))
        .expect("checkpoints"),
        "node_id",
    );
    let total = nodes.len();
    nodes.sort();
    nodes.dedup();
    assert_eq!(total, nodes.len(), "one checkpoint per node: {nodes:?}");
    assert!(nodes.contains(&"w.0".to_string()), "{nodes:?}");
}

#[test]
fn crash_during_an_agent_step_resumes_without_repeating_the_committed_step() {
    let tmp = TempDb::new("crash-agent");
    let path = tmp.path();
    assert!(run_sql(&path, INSERT).status.success());
    let agent = "SELECT aidb_agent('{\"instructions\":\"Answer from documents.\",\
                 \"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"generate\"],\"max_steps\":2}');";
    assert!(
        crash_at(&path, "after_agent_step_checkpoint", agent),
        "child must die"
    );

    let parent = {
        let raw = rusqlite::Connection::open(&path).expect("raw open");
        let parent: String = raw
            .query_row("SELECT id FROM runs WHERE kind = 'agent'", [], |r| r.get(0))
            .expect("agent run is durable");
        let status: String = raw
            .query_row("SELECT status FROM runs WHERE id = ?1", [&parent], |r| {
                r.get(0)
            })
            .expect("status");
        assert_eq!(status, "running");
        let nodes: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE run_id = ?1",
                [&parent],
                |r| r.get(0),
            )
            .expect("checkpoints");
        assert_eq!(nodes, 1, "exactly the first step was committed");
        parent
    };

    let db = tmp.open();
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT status FROM runs WHERE id = '{parent}'")
        ),
        "succeeded",
        "an interrupted agent is resumed on open"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM checkpoints WHERE run_id = '{parent}' AND node_id = 'a.0.search'")
        ),
        1,
        "the committed step stays a single checkpoint"
    );

    // The real invariant: crash + resume produces the same durable shape as a
    // clean run of the same agent. The committed step is replayed, not redone.
    let clean = TempDb::new("crash-agent-baseline");
    assert!(run_sql(&clean.path(), INSERT).status.success());
    assert!(run_sql(&clean.path(), agent).status.success());
    let baseline = clean.open();
    let shape = |db: &aidb::Aidb, parent: &str| {
        (
            count(
                db,
                &format!(
                    "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'search'"
                ),
            ),
            count(
                db,
                &format!(
                    "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'generate'"
                ),
            ),
            count(
                db,
                &format!("SELECT COUNT(*) FROM checkpoints WHERE run_id = '{parent}'"),
            ),
        )
    };
    let clean_parent = scalar(&baseline, "SELECT id FROM runs WHERE kind = 'agent'");
    assert_eq!(
        shape(&db, &parent),
        shape(&baseline, &clean_parent),
        "a resumed agent must not do more work than a clean one"
    );

    // Agents remain a composition of runs: no agent state outside the run system.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE '%agent%'"
        ),
        0
    );
}

#[test]
fn a_crash_never_leaves_the_file_unopenable() {
    let tmp = TempDb::new("crash-integrity");
    let path = tmp.path();
    for point in [
        "before_chunk",
        "after_chunk",
        "after_embed",
        "after_vec",
        "after_embed_checkpoint",
    ] {
        let scoped = TempDb::new(&format!("crash-integrity-{point}"));
        let p = scoped.path();
        assert!(crash_at(&p, point, INSERT), "child must die at {point}");
        let db = aidb::open(&p).unwrap_or_else(|e| panic!("reopen after {point}: {e}"));
        assert_eq!(scalar(&db, "PRAGMA integrity_check"), "ok");
        db.drain_index(Duration::from_secs(30))
            .unwrap_or_else(|e| panic!("drain after {point}: {e}"));
        assert_eq!(
            scalar(&db, "SELECT index_status FROM documents"),
            "ready",
            "recovery must finish the work after a crash at {point}"
        );
    }
    // And the crash hook is inert without the environment variable.
    assert!(run_sql(&path, INSERT).status.success());
}
