//! Phase 8 / 10 contracts: an agent is a composition of runs, and memory is
//! documents plus search. Neither may become a second store.

mod common;

use common::*;

const ANSWER_AGENT: &str = "{\"instructions\":\"Answer from documents. End with DONE.\",\
    \"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"generate\"],\"max_steps\":3}";

fn seeded(tag: &str) -> (TempDb, aidb::Aidb) {
    let tmp = TempDb::new(tag);
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    (tmp, db)
}

fn agent(db: &aidb::Aidb, spec: &str) -> (String, String) {
    let out = db
        .query(&format!("SELECT aidb_agent('{}')", sql_escape(spec)))
        .expect("agent");
    (cell(&out, 0, "run_id"), cell(&out, 0, "status"))
}

#[test]
fn an_agent_is_a_parent_run_whose_steps_are_child_runs_and_checkpoints() {
    let (_tmp, db) = seeded("agent-shape");
    let (parent, status) = agent(&db, ANSWER_AGENT);
    assert_eq!(status, "succeeded");
    assert_eq!(
        scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{parent}'")),
        "agent"
    );

    let kinds = column_values(
        &db.query(&format!(
            "SELECT DISTINCT kind FROM runs WHERE parent_id = '{parent}' ORDER BY kind"
        ))
        .expect("children"),
        "kind",
    );
    assert_eq!(kinds, vec!["generate", "search"], "{kinds:?}");

    // One checkpoint per executed step, addressed by step and tool.
    let nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{parent}' ORDER BY node_id"
        ))
        .expect("checkpoints"),
        "node_id",
    );
    assert!(nodes.contains(&"a.0.search".to_string()), "{nodes:?}");
    assert!(nodes.contains(&"a.0.generate".to_string()), "{nodes:?}");
    let mut unique = nodes.clone();
    unique.dedup();
    assert_eq!(unique.len(), nodes.len(), "no duplicate step records");

    // The output is durable on the parent run.
    let output = scalar(
        &db,
        &format!("SELECT output_json FROM runs WHERE id = '{parent}'"),
    );
    assert!(output.to_lowercase().contains("refund"), "{output}");
}

#[test]
fn an_agent_keeps_no_state_outside_the_run_system() {
    let (_tmp, db) = seeded("agent-no-table");
    let (parent, _) = agent(&db, ANSWER_AGENT);
    let tables = column_values(
        &db.query(
            "SELECT name FROM sqlite_master WHERE type = 'table'
             AND (name LIKE '%agent%' OR name LIKE '%goal%' OR name LIKE '%session%')",
        )
        .expect("tables"),
        "name",
    );
    assert!(tables.is_empty(), "no agent store may appear: {tables:?}");
    // Everything the agent did is reachable from its run id.
    assert!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM run_events WHERE run_id = '{parent}'")
        ) > 0
    );
}

#[test]
fn max_steps_bounds_the_agent_loop() {
    let (_tmp, db) = seeded("agent-max-steps");
    // Instructions that never say DONE, so only max_steps can stop the loop.
    let spec = "{\"instructions\":\"Keep thinking\",\"goal\":\"How do refunds work?\",\
        \"tools\":[\"search\"],\"max_steps\":2}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'search'")
        ),
        2,
        "the loop runs exactly max_steps times"
    );
    let nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{parent}' ORDER BY node_id"
        ))
        .expect("checkpoints"),
        "node_id",
    );
    assert_eq!(nodes, vec!["a.0.search", "a.1.search"], "{nodes:?}");
}

#[test]
fn an_agent_that_reaches_done_stops_before_max_steps() {
    let (_tmp, db) = seeded("agent-done");
    let spec = "{\"instructions\":\"Say DONE\",\"goal\":\"How do refunds work?\",\
        \"tools\":[\"generate\"],\"max_steps\":8}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'generate'"
            )
        ),
        1,
        "DONE terminates the loop instead of burning the step budget"
    );
}

#[test]
fn an_agent_asking_for_an_unknown_tool_fails_before_doing_any_work() {
    let (_tmp, db) = seeded("agent-unknown-tool");
    let before = count(&db, "SELECT COUNT(*) FROM runs");
    let spec =
        "{\"instructions\":\"Do it\",\"goal\":\"x\",\"tools\":[\"ghost.tool\"],\"max_steps\":1}";
    assert_err_contains(
        db.query(&format!("SELECT aidb_agent('{}')", sql_escape(spec))),
        "unknown capability: ghost.tool",
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs"),
        before,
        "an agent that cannot bind must not start"
    );
}

#[test]
fn an_agent_spec_missing_its_required_fields_is_a_clean_error() {
    let (_tmp, db) = seeded("agent-bad-spec");
    for (spec, needle) in [
        ("{\"goal\":\"x\"}", "instructions"),
        ("{\"instructions\":\"x\"}", "goal"),
        ("not json", "agent JSON"),
    ] {
        assert_err_contains(
            db.query(&format!("SELECT aidb_agent('{}')", sql_escape(spec))),
            needle,
        );
    }
}

#[test]
fn a_child_agent_is_a_child_run_of_its_parent() {
    let (_tmp, db) = seeded("agent-children");
    let spec = "{\"instructions\":\"Coordinate. End with DONE.\",\"goal\":\"How do refunds work?\",\
        \"tools\":[\"search\"],\"agents\":[{\"instructions\":\"Answer from documents. End with DONE.\",\
        \"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"generate\"],\"max_steps\":2}]}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");

    let child = scalar(
        &db,
        &format!("SELECT id FROM runs WHERE parent_id = '{parent}' AND kind = 'agent'"),
    );
    assert_ne!(child, parent);
    // The child's own work hangs off the child, so the tree is walkable.
    assert!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{child}'")
        ) > 0,
        "the child agent must own its steps"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM checkpoints WHERE run_id = '{parent}' AND node_id = 'a.child.0'")
        ),
        1,
        "the parent records the child as one durable step"
    );
}

#[test]
fn an_agent_plan_is_readable_before_it_runs() {
    let (_tmp, db) = seeded("agent-explain");
    let plan = scalar(
        &db,
        &format!("EXPLAIN SELECT aidb_agent('{}')", sql_escape(ANSWER_AGENT)),
    );
    for fragment in ["Then", "TopK k=5", "Loop until=\"done\"", "Llm prompt="] {
        assert!(plan.contains(fragment), "{fragment} missing from\n{plan}");
    }
}

#[test]
fn memory_insert_is_a_document_that_the_memory_view_exposes() {
    let tmp = TempDb::new("memory-insert");
    let db = tmp.open();
    let id = scalar(
        &db,
        "SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations.')",
    );
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("drain");

    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM documents WHERE id = '{id}'")
        ),
        1,
        "memory is stored as a document, not in a private table"
    );
    let row = db
        .query(&format!(
            "SELECT scope, content FROM memory WHERE id = '{id}'"
        ))
        .expect("memory view");
    assert_eq!(cell(&row, 0, "scope"), "user:123");
    assert!(cell(&row, 0, "content").contains("concise"));
    // And it went through the ordinary index lifecycle.
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'")
        ),
        "ready"
    );
    assert!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'")
        ) > 0
    );
}

#[test]
fn memory_search_is_the_same_retrieval_path_and_respects_its_scope() {
    let tmp = TempDb::new("memory-search");
    let db = tmp.open();
    db.query(
        "SELECT aidb_memory_insert('user:123', 'Alice prefers concise technical explanations.')",
    )
    .expect("insert a");
    db.query("SELECT aidb_memory_insert('user:456', 'Bob prefers long narrative explanations.')")
        .expect("insert b");
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("drain");

    let scoped = db
        .query("SELECT * FROM aidb_memory_search('how should I explain things?', 5, 'user:123')")
        .expect("scoped memory search");
    let contents = column_values(&scoped, "content");
    assert!(!contents.is_empty(), "expected a memory hit");
    assert!(
        contents.iter().all(|c| c.contains("Alice")),
        "another user's memory must not leak: {contents:?}"
    );

    // Unscoped memory search sees memories but not ordinary documents.
    let all = column_values(
        &db.query("SELECT * FROM aidb_memory_search('explanations', 5)")
            .expect("memory search"),
        "content",
    );
    assert!(all.iter().any(|c| c.contains("Alice")), "{all:?}");
    assert!(
        !all.iter().any(|c| c.contains("Refunds are issued")),
        "memory search is filtered retrieval, not a table scan: {all:?}"
    );

    // The same retrieval path is reachable through the plain search function,
    // because memory is documents.
    let docs = column_values(
        &db.query("SELECT * FROM aidb_search('how should I explain things?', 5)")
            .expect("search"),
        "content",
    );
    assert!(docs.iter().any(|c| c.contains("Alice")), "{docs:?}");
}

#[test]
fn memory_survives_close_and_reopen() {
    let tmp = TempDb::new("memory-reopen");
    {
        let db = tmp.open();
        db.query(
            "SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations.')",
        )
        .expect("insert");
        db.drain_index(std::time::Duration::from_secs(30))
            .expect("drain");
    }
    let db = tmp.open();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM memory WHERE scope = 'user:123'"),
        1
    );
    let hits = db
        .query("SELECT * FROM aidb_memory_search('explain', 5, 'user:123')")
        .expect("search after reopen");
    assert!(
        !hits.rows.is_empty(),
        "memory stays searchable after reopen"
    );
}

#[test]
fn an_agent_reads_the_memory_scope_it_was_given() {
    let tmp = TempDb::new("agent-memory");
    let db = tmp.open();
    db.query("SELECT aidb_memory_insert('user:123', 'Always answer in exactly one sentence.')")
        .expect("memory");
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("drain");

    let spec = "{\"instructions\":\"Answer from memory and documents. End with DONE.\",\
        \"goal\":\"How do refunds work?\",\"tools\":[\"generate\"],\"memory\":\"user:123\",\"max_steps\":1}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    let sent = scalar(
        &db,
        &format!("SELECT input_json FROM runs WHERE parent_id = '{parent}' AND kind = 'generate'"),
    );
    assert!(
        sent.contains("exactly one sentence"),
        "the agent must see its memory scope: {sent}"
    );
    // A different scope is not visible.
    assert!(!sent.contains("narrative"), "{sent}");
}

#[test]
fn two_agents_sharing_a_scope_see_the_same_memory() {
    let tmp = TempDb::new("agent-memory-shared");
    let db = tmp.open();
    db.query("SELECT aidb_memory_insert('team:ops', 'The on-call rotation starts on Monday.')")
        .expect("memory");
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("drain");
    let spec = "{\"instructions\":\"Coordinate. End with DONE.\",\"goal\":\"When does on-call start?\",\
        \"tools\":[\"generate\"],\"memory\":\"team:ops\",\"max_steps\":1,\
        \"agents\":[{\"instructions\":\"Answer. End with DONE.\",\"goal\":\"When does on-call start?\",\
        \"tools\":[\"generate\"],\"max_steps\":1}]}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    let prompts = column_values(
        &db.query(&format!(
            "SELECT input_json FROM runs WHERE kind = 'generate'
             AND parent_id IN ('{parent}', (SELECT id FROM runs WHERE parent_id = '{parent}' AND kind = 'agent'))"
        ))
        .expect("generate runs"),
        "input_json",
    );
    assert!(
        prompts.len() >= 2,
        "both agents must have generated: {prompts:?}"
    );
    assert!(
        prompts.iter().all(|p| p.contains("on-call rotation")),
        "the child inherits the shared scope: {prompts:?}"
    );
}
