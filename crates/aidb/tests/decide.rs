//! Phase 33: a decide agent chooses the next operator and its arguments.
//! Each choice is a child run plus a checkpoint. The recipe loop stays the default.

mod common;

use common::*;

const EMAIL: &str = "{\"tools\":[{\"name\":\"send.email\",\
    \"inputs\":{\"to\":\"string\",\"subject\":\"string\",\"body\":\"string\"},\
    \"side_effect\":\"irreversible\",\"retry\":\"forbidden\"}]}";

fn agent(db: &aidb::Aidb, spec: &str) -> (String, String) {
    let out = db
        .query(&format!("SELECT aidb_agent('{}')", sql_escape(spec)))
        .expect("agent");
    (cell(&out, 0, "run_id"), cell(&out, 0, "status"))
}

fn seeded(tag: &str) -> (TempDb, aidb::Aidb) {
    let tmp = TempDb::new(tag);
    let db = tmp.open();
    insert_doc_meta(
        &db,
        "NVDA 10-K",
        "Data center revenue was 47.5 billion dollars.",
        "{\"ticker\":\"NVDA\"}",
    );
    insert_doc_meta(
        &db,
        "AAPL earnings",
        "Gross margin is expected to land between 46 and 47 percent.",
        "{\"ticker\":\"AAPL\"}",
    );
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("drain");
    (tmp, db)
}

#[test]
fn a_recipe_agent_is_unchanged_when_decide_is_off() {
    let (_tmp, db) = seeded("decide-default");
    let spec = "{\"instructions\":\"Answer from documents. End with DONE.\",\
        \"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"generate\"],\"max_steps\":3}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    let nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{parent}' ORDER BY node_id"
        ))
        .expect("nodes"),
        "node_id",
    );
    assert!(nodes.contains(&"a.0.search".to_string()), "{nodes:?}");
    assert!(
        !nodes.iter().any(|n| n.contains("decide")),
        "recipe agents must not grow a decide step: {nodes:?}"
    );
}

#[test]
fn a_decide_agent_records_each_choice_as_a_child_run_and_checkpoint() {
    let (_tmp, db) = seeded("decide-shape");
    let spec = "{\"instructions\":\"Answer from documents. End with DONE.\",\
        \"goal\":\"Risks for NVDA\",\"tools\":[\"search\",\"generate\"],\"max_steps\":4,\
        \"k\":3,\"decide\":true}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    assert_eq!(
        scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{parent}'")),
        "agent"
    );

    let nodes = column_values(
        &db.query(&format!(
            "SELECT node_id FROM checkpoints WHERE run_id = '{parent}' ORDER BY node_id"
        ))
        .expect("nodes"),
        "node_id",
    );
    assert!(nodes.contains(&"a.0.decide".to_string()), "{nodes:?}");
    assert!(nodes.contains(&"a.0.search".to_string()), "{nodes:?}");
    assert!(nodes.contains(&"a.1.decide".to_string()), "{nodes:?}");
    assert!(nodes.contains(&"a.1.generate".to_string()), "{nodes:?}");

    let kinds = column_values(
        &db.query(&format!(
            "SELECT DISTINCT kind FROM runs WHERE parent_id = '{parent}' ORDER BY kind"
        ))
        .expect("children"),
        "kind",
    );
    assert!(kinds.contains(&"generate".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"search".to_string()), "{kinds:?}");
}

#[test]
fn a_decide_agent_stops_instead_of_repeating_the_recipe() {
    let (_tmp, db) = seeded("decide-stop");
    let spec = "{\"instructions\":\"Keep going\",\"goal\":\"Risks for NVDA\",\
        \"tools\":[\"search\"],\"max_steps\":8,\"decide\":true}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'search'")
        ),
        1,
        "decide must not replay search just to burn max_steps"
    );
}

#[test]
fn max_steps_still_bounds_a_decide_loop() {
    let (_tmp, db) = seeded("decide-max");
    let spec = "{\"instructions\":\"Answer. End with DONE.\",\"goal\":\"Risks for NVDA\",\
        \"tools\":[\"search\",\"generate\"],\"max_steps\":1,\"decide\":true}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'search'")
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'generate'"
            )
        ),
        1,
        "max_steps=1 is one decide (a generate run) plus the chosen search, not a generate step"
    );
}

#[test]
fn decide_search_can_pass_a_metadata_filter() {
    let (_tmp, db) = seeded("decide-filter");
    let spec = "{\"instructions\":\"Brief the ticker. End with DONE.\",\
        \"goal\":\"Brief me on NVDA only\",\"tools\":[\"search\",\"generate\"],\
        \"max_steps\":3,\"k\":5,\"decide\":true}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "succeeded");
    let input = scalar(
        &db,
        &format!(
            "SELECT input_json FROM runs WHERE parent_id = '{parent}' AND kind = 'search'
             ORDER BY created_at_ms LIMIT 1"
        ),
    );
    assert!(
        input.contains("NVDA"),
        "decide must be able to scope search: {input}"
    );
    assert!(
        input.contains("filter") || input.contains("ticker"),
        "the search run has to record the filter it used: {input}"
    );
}

#[test]
fn decide_passes_tool_arguments_instead_of_hardcoded_ones() {
    let tmp = TempDb::new("decide-args");
    let db = tmp.open();
    db.query(&format!(
        "SELECT aidb_mcp_register('{}')",
        sql_escape(EMAIL)
    ))
    .expect("catalog");
    db.query("SELECT aidb_set_policy('{\"allow\":[\"search\",\"generate\",\"send.email\"]}')")
        .expect("policy");
    insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");

    let spec = "{\"instructions\":\"Draft, then email. End with DONE.\",\
        \"goal\":\"Email alice@desk.test a refund summary\",\
        \"tools\":[\"search\",\"generate\",\"send.email\"],\"max_steps\":4,\"decide\":true}";
    let (parent, status) = agent(&db, spec);
    assert_eq!(status, "awaiting_approval");

    let resumed = db
        .query(&format!(
            "SELECT aidb_resume('{parent}', '{{\"approved\":true}}')"
        ))
        .expect("approve");
    assert_eq!(cell(&resumed, 0, "status"), "succeeded");
    let sent = scalar(
        &db,
        "SELECT output_json FROM runs WHERE kind = 'tool' ORDER BY created_at_ms DESC LIMIT 1",
    );
    assert!(
        sent.contains("alice@desk.test"),
        "the model-chosen recipient has to reach the tool, not user@example.com: {sent}"
    );
    assert!(!sent.contains("user@example.com"), "{sent}");
}

#[test]
fn a_decide_plan_is_readable_before_it_runs() {
    let (_tmp, db) = seeded("decide-explain");
    let spec =
        "{\"instructions\":\"Answer.\",\"goal\":\"NVDA\",\"tools\":[\"search\",\"generate\"],\
        \"max_steps\":3,\"decide\":true}";
    let plan = scalar(
        &db,
        &format!("EXPLAIN SELECT aidb_agent('{}')", sql_escape(spec)),
    );
    assert!(plan.contains("Decide"), "{plan}");
    assert!(plan.contains("search"), "{plan}");
    assert!(plan.contains("max=3"), "{plan}");
}

#[test]
fn decide_still_creates_no_agents_table() {
    let (_tmp, db) = seeded("decide-no-table");
    let spec = "{\"instructions\":\"Answer. End with DONE.\",\"goal\":\"NVDA\",\
        \"tools\":[\"search\"],\"max_steps\":2,\"decide\":true}";
    let _ = agent(&db, spec);
    let tables = column_values(
        &db.query("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE '%agent%'")
            .expect("tables"),
        "name",
    );
    assert!(tables.is_empty(), "{tables:?}");
}
