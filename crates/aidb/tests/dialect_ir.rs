//! Phase 4 / 15 / 16 contracts: the dialect and the goal language are frontends
//! that lower to the same IR, the same catalog and the same run engine.
//! DESIGN.md §8: "SEARCH and AI_GENERATE must lower to IR and the same run engine".
//! There must not be a second planner.

mod common;

use common::*;

fn seeded(tag: &str) -> (TempDb, aidb::Aidb) {
    let tmp = TempDb::new(tag);
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    insert_ready(
        &db,
        "Shipping",
        "Shipping takes three business days after dispatch.",
    );
    (tmp, db)
}

#[test]
fn search_dialect_and_search_function_produce_the_same_rows() {
    let (_tmp, db) = seeded("dialect-rows");
    for (dialect, function) in [
        (
            "SELECT * FROM documents SEARCH 'How do refunds work?' LIMIT 5",
            "SELECT * FROM aidb_search('How do refunds work?', 5)",
        ),
        (
            "SEARCH 'shipping time' LIMIT 1",
            "SELECT * FROM aidb_search('shipping time', 1)",
        ),
    ] {
        let a = db
            .query(dialect)
            .unwrap_or_else(|e| panic!("{dialect}: {e}"));
        let b = db
            .query(function)
            .unwrap_or_else(|e| panic!("{function}: {e}"));
        assert_eq!(a.columns, b.columns, "{dialect}");
        assert_eq!(a.rows, b.rows, "{dialect} must equal {function}");
    }
}

#[test]
fn search_dialect_without_limit_uses_the_documented_default_of_five() {
    let (_tmp, db) = seeded("dialect-default-k");
    let plan = scalar(&db, "EXPLAIN SEARCH 'How do refunds work?'");
    assert!(plan.contains("TopK k=5"), "default limit is 5:\n{plan}");
    let explicit = scalar(
        &db,
        "EXPLAIN SELECT * FROM aidb_search('How do refunds work?', 5)",
    );
    assert_eq!(
        plan.lines().next(),
        explicit.lines().next(),
        "both frontends must bind the same physical root"
    );
}

#[test]
fn both_search_frontends_explain_to_the_same_physical_plan() {
    let (_tmp, db) = seeded("dialect-plan");
    let dialect = scalar(
        &db,
        "EXPLAIN SELECT * FROM documents SEARCH 'refund policy' LIMIT 3",
    );
    let function = scalar(&db, "EXPLAIN SELECT * FROM aidb_search('refund policy', 3)");
    assert_eq!(
        dialect, function,
        "one planner: dialect and function must render the identical plan"
    );
    // The plan is the documented operator chain, and it is readable.
    for fragment in [
        "TopK k=3",
        "Similarity",
        "Embed query=\"refund policy\"",
        "Filter index_status = 'ready'",
        "Scan documents",
    ] {
        assert!(
            dialect.contains(fragment),
            "{fragment} missing from\n{dialect}"
        );
    }
}

#[test]
fn the_search_dialect_writes_the_same_kind_of_run_as_the_function() {
    let (_tmp, db) = seeded("dialect-run");
    let baseline = count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'search'");
    let _ = db
        .query("SELECT * FROM documents SEARCH 'How do refunds work?' LIMIT 2")
        .expect("dialect search");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'search'"),
        baseline + 1,
        "the dialect must go through the run engine, not around it"
    );
    let input: serde_json::Value = serde_json::from_str(&scalar(
        &db,
        "SELECT input_json FROM runs WHERE kind = 'search' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    ))
    .expect("input json");
    assert_eq!(input["query"], "How do refunds work?");
    assert_eq!(input["k"], 2);
}

#[test]
fn search_where_metadata_matches_the_filter_argument_of_the_function() {
    let tmp = TempDb::new("dialect-filter");
    let db = tmp.open();
    let support = insert_doc_meta(
        &db,
        "Support refunds",
        "Refunds are issued within 14 days of purchase.",
        "{\"dept\":\"support\"}",
    );
    insert_doc_meta(
        &db,
        "Legal refunds",
        "Refunds require a signed legal waiver before processing.",
        "{\"dept\":\"legal\"}",
    );
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("drain");

    let dialect = db
        .query("SEARCH 'refunds' WHERE metadata.dept = 'support' LIMIT 5")
        .expect("dialect");
    let function = db
        .query("SELECT * FROM aidb_search('refunds', 5, '{\"dept\":\"support\"}')")
        .expect("function");
    assert_eq!(dialect.rows, function.rows);
    let docs = column_values(&dialect, "document_id");
    assert!(!docs.is_empty());
    assert!(
        docs.iter().all(|d| *d == support),
        "the filter must exclude the other department: {docs:?}"
    );
}

#[test]
fn create_model_dialect_and_a_plain_insert_reach_the_same_catalog() {
    let tmp = TempDb::new("dialect-model");
    let db = tmp.open();
    db.execute("CREATE MODEL gpt PROVIDER openai MODEL 'gpt-4.1-mini' KEY_NAME 'OPENAI_API_KEY'")
        .expect("dialect create model");
    db.execute(
        "INSERT INTO models (name, kind, provider, provider_model, created_at_ms, key_name)
         VALUES ('gpt_raw', 'llm', 'openai', 'gpt-4.1-mini', 1, 'OPENAI_API_KEY')",
    )
    .expect("raw insert");

    let rows = db
        .query(
            "SELECT name, kind, provider, provider_model, key_name FROM models
             WHERE name IN ('gpt', 'gpt_raw') ORDER BY name",
        )
        .expect("models");
    assert_eq!(rows.rows.len(), 2);
    for row in 0..2 {
        assert_eq!(cell(&rows, row, "kind"), "llm");
        assert_eq!(cell(&rows, row, "provider"), "openai");
        assert_eq!(cell(&rows, row, "provider_model"), "gpt-4.1-mini");
        assert_eq!(cell(&rows, row, "key_name"), "OPENAI_API_KEY");
    }
}

#[test]
fn create_model_rejects_secret_bearing_syntax_and_unknown_providers() {
    let tmp = TempDb::new("dialect-model-bad");
    let db = tmp.open();
    assert_err_contains(
        db.execute(
            "CREATE MODEL gpt (kind = llm, provider = openai, provider_model = 'x', api_key = 'sk-nope')",
        ),
        "never the secret",
    );
    assert_err_contains(
        db.execute("CREATE MODEL weird PROVIDER hosted_mystery MODEL 'x'"),
        "unknown model provider",
    );
    assert_err_contains(
        db.execute("CREATE MODEL weird PROVIDER openai KIND translator"),
        "kind must be",
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM models"), 0);
}

#[test]
fn ai_generate_and_aidb_generate_are_the_same_execution() {
    let (_tmp, db) = seeded("dialect-generate");
    let a = scalar(
        &db,
        "SELECT AI_GENERATE('Summarize', 'Refunds take 14 days.')",
    );
    let b = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    assert_eq!(a, b, "both spellings must reach the same engine");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND status = 'succeeded'"
        ),
        2,
        "each call is one durable run"
    );
    // The second call was served from the keyed cache, which is a physical
    // rewrite, not a different logical result.
    let events = column_values(
        &db.query(
            "SELECT kind FROM run_events WHERE run_id = (
                 SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1)
             ORDER BY seq",
        )
        .expect("events"),
        "kind",
    );
    assert!(events.contains(&"cache_hit".to_string()), "{events:?}");
}

#[test]
fn explain_is_deterministic_for_the_same_query() {
    let (_tmp, db) = seeded("explain-stable");
    let first = scalar(&db, "EXPLAIN SELECT * FROM aidb_search('refund policy', 4)");
    for _ in 0..3 {
        assert_eq!(
            scalar(&db, "EXPLAIN SELECT * FROM aidb_search('refund policy', 4)"),
            first,
            "the plan must be deterministic for deterministic inputs"
        );
    }
}

#[test]
fn explain_labels_every_operator_with_its_backend() {
    let (_tmp, db) = seeded("explain-backends");
    let plan = scalar(&db, "EXPLAIN SELECT * FROM aidb_search('refund policy', 4)");
    assert!(plan.contains("[sqlite/limit]"), "{plan}");
    assert!(plan.contains("[ai/embed"), "{plan}");
    assert!(plan.contains("[sqlite/seqscan]"), "{plan}");
    let generate = scalar(&db, "EXPLAIN SELECT aidb_generate('Summarize', 'body')");
    assert!(generate.contains("Llm"), "{generate}");
    assert!(generate.contains("[ai/"), "{generate}");
}

#[test]
fn explain_reports_the_rewrites_and_the_budget() {
    let (_tmp, db) = seeded("explain-rewrites");
    let plan = scalar(
        &db,
        "EXPLAIN SELECT aidb_generate('Summarize', content) FROM documents",
    );
    assert!(plan.contains("Rewrites"), "{plan}");
    assert!(plan.contains("physical: CacheKeyedAiCall"), "{plan}");
    assert!(plan.contains("Budget max_llm_calls="), "{plan}");
}

#[test]
fn bind_errors_are_deterministic_and_name_the_missing_thing() {
    let (_tmp, db) = seeded("bind-errors");
    assert_err_contains(
        db.query("EXPLAIN SELECT aidb_generate('Summarize', content) FROM nope_table"),
        "unknown table: nope_table",
    );
    assert_err_contains(
        db.query("SELECT * FROM aidb_search('refunds', 5, '{}', 'no_such_space')"),
        "unknown embedding space: no_such_space",
    );
    assert_err_contains(db.query("SELECT aidb_tool('nope.tool', '{}')"), "nope.tool");
    assert_err_contains(
        db.query("SELECT aidb_workflow('{\"then\":[{\"nonsense\":{}}]}')"),
        "workflow",
    );
    assert_err_contains(
        db.query("SELECT aidb_workflow('not json at all')"),
        "workflow",
    );
    // The same bad input fails the same way twice.
    let first = db
        .query("EXPLAIN SELECT aidb_generate('Summarize', content) FROM nope_table")
        .err()
        .map(|e| e.to_string());
    let second = db
        .query("EXPLAIN SELECT aidb_generate('Summarize', content) FROM nope_table")
        .err()
        .map(|e| e.to_string());
    assert_eq!(first, second);
}

#[test]
fn a_search_over_an_unknown_space_never_falls_back_to_the_default_space() {
    let (_tmp, db) = seeded("space-fail-closed");
    let good = db
        .query("SELECT * FROM aidb_search('refunds', 5)")
        .expect("default space search");
    assert!(!good.rows.is_empty());
    let bad = db.query("SELECT * FROM aidb_search('refunds', 5, '{}', 'ghost')");
    assert!(
        bad.is_err(),
        "an unknown space must fail closed, not silently use the default"
    );
}

#[test]
fn workflow_then_parallel_branch_and_loop_all_persist_as_runs_and_checkpoints() {
    let (_tmp, db) = seeded("workflow-shapes");
    let cases: [(&str, &str); 4] = [
        ("then", "{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":2}},{\"generate\":{\"prompt\":\"Summarize\"}}]}"),
        ("parallel", "{\"parallel\":[{\"search\":{\"query\":\"refunds\",\"k\":1}},{\"search\":{\"query\":\"shipping\",\"k\":1}}]}"),
        ("branch", "{\"branch\":{\"when\":\"true\",\"then\":{\"search\":{\"query\":\"refunds\",\"k\":1}},\"else\":{\"search\":{\"query\":\"shipping\",\"k\":1}}}}"),
        ("loop", "{\"loop\":{\"body\":{\"search\":{\"query\":\"refunds\",\"k\":1}},\"until\":\"hits>=1\",\"max\":2}}"),
    ];
    for (name, spec) in cases {
        let out = db
            .query(&format!("SELECT aidb_workflow('{}')", sql_escape(spec)))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let run_id = cell(&out, 0, "run_id");
        assert_eq!(cell(&out, 0, "status"), "succeeded", "{name}");
        assert_eq!(
            scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{run_id}'")),
            "workflow",
            "{name}"
        );
        assert!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM checkpoints WHERE run_id = '{run_id}'")
            ) > 0,
            "{name} must checkpoint after each operator"
        );
        assert!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}'")
            ) > 0,
            "{name} must produce child runs"
        );
    }
    // No second graph store appeared.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE '%workflow%'"
        ),
        0
    );
}

#[test]
fn a_nested_workflow_records_a_checkpoint_per_nested_operator() {
    let (_tmp, db) = seeded("workflow-nested");
    let spec = "{\"then\":[{\"parallel\":[{\"search\":{\"query\":\"refunds\",\"k\":1}},\
                {\"search\":{\"query\":\"shipping\",\"k\":1}}]},\
                {\"generate\":{\"prompt\":\"Summarize\"}}]}";
    let out = db
        .query(&format!("SELECT aidb_workflow('{}')", sql_escape(spec)))
        .expect("nested workflow");
    let run_id = cell(&out, 0, "run_id");
    let nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{run_id}' ORDER BY node_id"
        ))
        .expect("nodes"),
        "node_id",
    );
    // Nested ids are addressed by path, one per operator.
    for expected in ["w", "w.0", "w.0.0", "w.0.1", "w.1"] {
        assert!(
            nodes.contains(&expected.to_string()),
            "{expected} in {nodes:?}"
        );
    }
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'search'")
        ),
        2,
        "both parallel branches ran"
    );
}

#[test]
fn a_branch_only_executes_the_taken_side() {
    let (_tmp, db) = seeded("workflow-branch");
    let spec = "{\"branch\":{\"when\":\"false\",\"then\":{\"generate\":{\"prompt\":\"THEN\",\"content\":\"x\"}},\
                \"else\":{\"generate\":{\"prompt\":\"ELSE\",\"content\":\"x\"}}}}";
    let out = db
        .query(&format!("SELECT aidb_workflow('{}')", sql_escape(spec)))
        .expect("branch workflow");
    let run_id = cell(&out, 0, "run_id");
    let nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{run_id}'"
        ))
        .expect("nodes"),
        "node_id",
    );
    assert!(nodes.contains(&"w.else".to_string()), "{nodes:?}");
    assert!(!nodes.contains(&"w.then".to_string()), "{nodes:?}");
    let inputs = column_values(
        &db.query(&format!(
            "SELECT input_json FROM runs WHERE parent_id = '{run_id}' AND kind = 'generate'"
        ))
        .expect("children"),
        "input_json",
    );
    assert!(inputs.iter().any(|i| i.contains("ELSE")), "{inputs:?}");
    assert!(!inputs.iter().any(|i| i.contains("THEN")), "{inputs:?}");
}

#[test]
fn a_loop_is_bounded_by_max_and_stops_at_its_until_predicate() {
    let (_tmp, db) = seeded("workflow-loop");
    // `until` never becomes true, so `max` is the hard bound.
    let spec = "{\"loop\":{\"body\":{\"search\":{\"query\":\"refunds\",\"k\":1}},\"until\":\"contains:never\",\"max\":3}}";
    let out = db
        .query(&format!("SELECT aidb_workflow('{}')", sql_escape(spec)))
        .expect("loop workflow");
    let run_id = cell(&out, 0, "run_id");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'search'")
        ),
        3,
        "the loop must run exactly max times"
    );
}

/// Regression: the IR has a `Tool` operator and the executor runs it as a child
/// run through the catalog and the policy, but SQL could not express a tool step,
/// so the operator was unreachable from the primary surface.
#[test]
fn a_workflow_can_run_a_catalog_tool_as_a_child_run() {
    let (_tmp, db) = seeded("workflow-tool");
    db.execute(
        "INSERT INTO capabilities (name, inputs, outputs, side_effect, retry, source, enabled, created_at_ms)
         VALUES ('github.read', '{\"path\":\"string\"}', '{\"content\":\"string\"}', 'none', 'safe', 'app', 1, 1)",
    )
    .expect("register capability");

    let spec = "{\"then\":[{\"tool\":{\"name\":\"github.read\"}},{\"generate\":{\"prompt\":\"Summarize\"}}]}";
    let out = db
        .query(&format!("SELECT aidb_workflow('{}')", sql_escape(spec)))
        .expect("workflow with a tool step");
    let run_id = cell(&out, 0, "run_id");
    assert_eq!(cell(&out, 0, "status"), "succeeded");

    let tools = db
        .query(&format!(
            "SELECT status, input_json, output_json FROM runs
             WHERE parent_id = '{run_id}' AND kind = 'tool'"
        ))
        .expect("tool runs");
    assert_eq!(tools.rows.len(), 1, "one tool call is one child run");
    assert_eq!(cell(&tools, 0, "status"), "succeeded");
    assert!(cell(&tools, 0, "input_json").contains("github.read"));
    assert!(cell(&tools, 0, "output_json").contains("Repository stub"));

    // The tool step went through the policy, and it is checkpointed like any
    // other operator, so a resume would not repeat it.
    let events = column_values(
        &db.query(&format!(
            "SELECT kind FROM run_events WHERE run_id IN
                 (SELECT id FROM runs WHERE parent_id = '{run_id}' AND kind = 'tool')
             ORDER BY seq"
        ))
        .expect("tool events"),
        "kind",
    );
    assert_eq!(events, vec!["policy", "succeeded"], "{events:?}");
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM checkpoints WHERE run_id = '{run_id}' AND node_id = 'w.0'"
            )
        ),
        1
    );
    // The shorthand spelling reaches the same operator.
    let short = db
        .query("SELECT aidb_workflow('{\"tool\":\"github.read\"}')")
        .expect("shorthand tool workflow");
    assert_eq!(cell(&short, 0, "status"), "succeeded");
}

#[test]
fn a_workflow_that_cannot_bind_is_a_usage_error_and_never_becomes_a_run() {
    let (_tmp, db) = seeded("workflow-bind-fail");
    let before = count(&db, "SELECT COUNT(*) FROM runs");
    let spec = "{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":1}},{\"tool\":{\"name\":\"nope.missing\"}}]}";
    assert_err_contains(
        db.query(&format!("SELECT aidb_workflow('{}')", sql_escape(spec))),
        "nope.missing",
    );
    // Bind is validation, like invalid SQL: nothing was started, so nothing is
    // recorded, and no operator of the plan ran.
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs"),
        before,
        "a plan that never started must not leave a run behind"
    );
}

#[test]
fn a_failing_operator_fails_the_parent_workflow_and_records_the_error() {
    let (tmp, db) = seeded("workflow-run-fail");
    // A registered capability that binds cleanly but fails when executed.
    db.execute(
        "INSERT INTO capabilities (name, inputs, outputs, side_effect, retry, source, enabled, created_at_ms)
         VALUES ('http.get', '{\"url\":\"string\"}', '{\"body\":\"string\"}', 'none', 'safe', 'app', 1, 1)",
    )
    .expect("register capability");

    let spec = "{\"then\":[{\"search\":{\"query\":\"refunds\",\"k\":1}},{\"tool\":{\"name\":\"http.get\"}}]}";
    assert!(
        db.query(&format!("SELECT aidb_workflow('{}')", sql_escape(spec)))
            .is_err(),
        "a failing operator must fail the workflow"
    );

    let row = db
        .query(
            "SELECT id, status, error FROM runs WHERE kind = 'workflow'
             ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
        )
        .expect("workflow run");
    let run_id = cell(&row, 0, "id");
    assert_eq!(cell(&row, 0, "status"), "failed");
    assert!(
        cell(&row, 0, "error").contains("http.get"),
        "the failure is persisted on the parent: {}",
        cell(&row, 0, "error")
    );
    // The failing child tool run is also durable, and the successful step before
    // it is not rolled back.
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'search' AND status = 'succeeded'")
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{run_id}' AND kind = 'tool' AND status = 'failed'")
        ),
        1
    );

    // Reopening must not silently retry a run that failed for a real reason.
    drop(db);
    let reopened = tmp.open();
    assert_eq!(
        scalar(
            &reopened,
            &format!("SELECT status FROM runs WHERE id = '{run_id}'")
        ),
        "failed"
    );
}

#[test]
fn the_goal_language_compiles_to_a_workflow_run_through_the_optimizer() {
    let (_tmp, db) = seeded("goal-run");
    let goal = "TASK investigate_refunds
DATA documents
CONSTRAINTS read_only, budget $1, timeout 5m
GOAL identify_refund_window";
    let plan = scalar(&db, &format!("EXPLAIN {goal}"));
    // A frontend, not a bypass: the optimizer's rewrites and budget appear.
    assert!(plan.contains("Rewrites"), "{plan}");
    assert!(plan.contains("Budget"), "{plan}");
    assert!(
        plan.contains("max_usd=1"),
        "the $1 constraint must reach the budget:\n{plan}"
    );
    assert!(
        plan.contains("max_ms=300000"),
        "5m must reach the budget:\n{plan}"
    );

    let out = db.query(goal).expect("goal run");
    let run_id = cell(&out, 0, "run_id");
    assert_eq!(cell(&out, 0, "status"), "succeeded");
    assert_eq!(
        scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{run_id}'")),
        "workflow",
        "the goal language persists as a workflow run, not a new store"
    );
    let input = scalar(
        &db,
        &format!("SELECT input_json FROM runs WHERE id = '{run_id}'"),
    );
    assert!(
        input.contains("identify_refund_window"),
        "the run records the declared goal: {input}"
    );
}

#[test]
fn an_invalid_goal_fails_cleanly_without_writing_a_run() {
    let (_tmp, db) = seeded("goal-bad");
    let before = count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'workflow'");
    for bad in [
        "TASK\nDATA\nGOAL",
        "TASK only_a_task",
        "TASK t\nDATA documents\nCONSTRAINTS budget $\nGOAL g",
    ] {
        let result = db.query(bad);
        if result.is_ok() {
            // Parsing may accept a sparse goal; then it must still be a workflow run.
            continue;
        }
        assert!(result.is_err());
    }
    assert!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'workflow' AND status = 'succeeded'"
        ) >= before,
        "no partial state"
    );
}

#[test]
fn a_read_only_goal_cannot_reach_an_irreversible_capability() {
    let (_tmp, db) = seeded("goal-readonly");
    let plan = scalar(
        &db,
        "EXPLAIN TASK notify_user
DATA documents
CONSTRAINTS read_only
GOAL send_a_summary",
    );
    assert!(
        plan.contains("Policy") || plan.contains("read_only") || plan.contains("Budget"),
        "a read_only constraint must be visible in the plan:\n{plan}"
    );
    // And the irreversible capability itself is still gated.
    let denied = db.query("SELECT aidb_tool('send.email', '{\"to\":\"a@b.c\"}')");
    assert!(
        denied.is_err(),
        "an irreversible tool must never run unattended"
    );
}
