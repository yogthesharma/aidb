//! Phase 8 / 14 contracts: the optimizer's rewrite classes must mean what they
//! say. Equivalence rewrites preserve results, approximation rewrites respect the
//! declared quality floor, and budgets are enforced instead of exceeded.

mod common;

use common::*;

fn generate_runs(db: &aidb::Aidb) -> i64 {
    count(db, "SELECT COUNT(*) FROM runs WHERE kind = 'generate'")
}

#[test]
fn pushing_a_filter_before_the_llm_preserves_the_result() {
    let tmp = TempDb::new("opt-push-equiv");
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
    insert_ready(&db, "Returns", "Returns need the original packaging.");

    let optimized = db
        .query("SELECT aidb_generate('Summarize', content) FROM documents WHERE title = 'Refunds'")
        .expect("filtered generate");
    // The same logical result, computed without the optimizer's help: filter the
    // rows first, then generate over exactly those.
    let content = scalar(&db, "SELECT content FROM documents WHERE title = 'Refunds'");
    let expected = scalar(
        &db,
        &format!(
            "SELECT aidb_generate('Summarize', '{}')",
            sql_escape(&content)
        ),
    );
    assert_eq!(optimized.rows.len(), 1);
    assert_eq!(cell(&optimized, 0, "text"), expected);

    // Equivalence is about results; the point of the rewrite is that the
    // expensive operator never saw the filtered-out rows.
    let prompts = column_values(
        &db.query("SELECT input_json FROM runs WHERE kind = 'generate'")
            .expect("generate runs"),
        "input_json",
    );
    assert!(
        !prompts
            .iter()
            .any(|p| p.contains("Shipping takes") || p.contains("original packaging")),
        "the llm must not run on rows the filter excludes: {prompts:?}"
    );
}

#[test]
fn a_predicate_over_the_llm_output_is_never_pushed_under_it() {
    let tmp = TempDb::new("opt-no-push");
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

    let plan = scalar(
        &db,
        "EXPLAIN SELECT aidb_generate('Summarize', content) FROM documents WHERE text LIKE '%refund%'",
    );
    assert!(
        !plan.contains("PushFilterBeforeExpensive"),
        "a predicate on the generated text is not a pre-filter:\n{plan}"
    );
    // And it must never silently behave as if the filter were absent.
    let result = db.query(
        "SELECT aidb_generate('Summarize', content) FROM documents WHERE text LIKE '%refund%'",
    );
    match result {
        Err(_) => {}
        Ok(rows) => assert!(
            rows.rows.len() < 2,
            "the filter was dropped instead of applied: {rows:?}"
        ),
    }
}

#[test]
fn the_keyed_cache_dedups_identical_calls_without_changing_the_answer() {
    let tmp = TempDb::new("opt-cache");
    let db = tmp.open();
    let first = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    let second = scalar(
        &db,
        "SELECT aidb_generate('Summarize', 'Refunds take 14 days.')",
    );
    assert_eq!(first, second, "a cached call is the same logical result");
    assert_eq!(generate_runs(&db), 2, "each call is still its own run");

    let cached = db
        .query(
            "SELECT id, prompt_tokens, completion_tokens, cost_usd FROM runs
             WHERE kind = 'generate' ORDER BY created_at_ms, rowid",
        )
        .expect("runs");
    assert_eq!(
        cell(&cached, 0, "prompt_tokens"),
        cell(&cached, 1, "prompt_tokens"),
        "the cached run reports the same accounting as the call it replays"
    );
    let hit = cell(&cached, 1, "id");
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM run_events WHERE run_id = '{hit}' AND kind = 'cache_hit'"
            )
        ),
        1
    );
    // A different prompt is a different key.
    let other = scalar(
        &db,
        "SELECT aidb_generate('Translate', 'Refunds take 14 days.')",
    );
    assert_ne!(other, first);
}

fn three_docs(tag: &str) -> (TempDb, aidb::Aidb) {
    let tmp = TempDb::new(tag);
    let db = tmp.open();
    for (title, body) in [
        ("A", "Refunds are issued within 14 days of purchase."),
        ("B", "Shipping takes three business days after dispatch."),
        ("C", "Returns need the original packaging and a receipt."),
    ] {
        insert_ready(&db, title, body);
    }
    (tmp, db)
}

#[test]
fn a_tight_call_budget_makes_the_optimizer_cascade_instead_of_exceeding_it() {
    let (_tmp, db) = three_docs("opt-budget-cascade");
    db.query("SELECT aidb_set_policy('{\"max_llm_calls\":1}')")
        .expect("policy");

    // The budget is visible in the plan, together with the rewrite that satisfies it.
    let plan = scalar(
        &db,
        "EXPLAIN SELECT aidb_generate('Summarize', content) FROM documents",
    );
    assert!(plan.contains("max_llm_calls=1"), "{plan}");
    assert!(
        plan.contains("CascadeEmbedTopKThenJudge"),
        "a row-wise plan cannot fit one call, so the optimizer must fall back:\n{plan}"
    );

    let before = generate_runs(&db);
    db.query("SELECT aidb_generate('Summarize', content) FROM documents")
        .expect("the cascade fits the budget");
    assert_eq!(
        generate_runs(&db) - before,
        1,
        "the fallback must stay inside max_llm_calls=1"
    );
}

#[test]
fn a_budget_too_small_for_the_work_is_a_hard_failure_not_a_partial_result() {
    let (_tmp, db) = three_docs("opt-budget-calls");
    // Classification is row-wise by contract, so no approximation can shrink it
    // to one call. The job must fail rather than quietly overspend.
    db.query("SELECT aidb_set_policy('{\"max_llm_calls\":1}')")
        .expect("policy");
    let err = db
        .query("SELECT aidb_classify('refund or shipping', content) FROM documents")
        .expect_err("the budget must stop the job");
    assert!(
        err.to_string().contains("budget exceeded"),
        "the failure must name the budget: {err}"
    );
    assert!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND status = 'succeeded'"
        ) <= 1,
        "no work beyond the budget may be committed"
    );
}

#[test]
fn a_max_usd_budget_fails_instead_of_being_exceeded() {
    let tmp = TempDb::new("opt-budget-usd");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.query("SELECT aidb_set_policy('{\"max_usd\":0.0}')")
        .expect("policy");
    let err = db
        .query("SELECT aidb_generate('Summarize', content) FROM documents")
        .expect_err("a zero dollar budget cannot pay for a model call");
    assert!(err.to_string().contains("budget exceeded"), "{err}");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND status = 'succeeded'"
        ),
        0,
        "a call that breaks the budget is not recorded as a success"
    );
}

#[test]
fn a_max_ms_budget_fails_instead_of_being_exceeded() {
    let tmp = TempDb::new("opt-budget-ms");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.query("SELECT aidb_set_policy('{\"max_ms\":0}')")
        .expect("policy");
    let err = db
        .query("SELECT aidb_generate('Summarize', content) FROM documents")
        .expect_err("a zero millisecond budget cannot run a model call");
    assert!(err.to_string().contains("budget exceeded"), "{err}");
}

#[test]
fn a_budget_that_fits_still_runs_and_records_its_cost() {
    let tmp = TempDb::new("opt-budget-ok");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.query("SELECT aidb_set_policy('{\"max_llm_calls\":8,\"max_usd\":10.0}')")
        .expect("policy");
    db.query("SELECT aidb_generate('Summarize', content) FROM documents")
        .expect("inside the budget");
    let row = db
        .query(
            "SELECT status, cost_usd, prompt_tokens FROM runs
             WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
        )
        .expect("run");
    assert_eq!(cell(&row, 0, "status"), "succeeded");
    assert!(
        cell(&row, 0, "cost_usd").parse::<f64>().expect("cost") >= 0.0,
        "cost is accounted for even when it fits"
    );
    assert!(
        cell(&row, 0, "prompt_tokens")
            .parse::<i64>()
            .expect("tokens")
            > 0
    );
}

#[test]
fn a_large_table_cascades_to_retrieval_and_reports_the_measured_recall() {
    let tmp = TempDb::new("opt-cascade");
    let db = tmp.open();
    for i in 0..20 {
        insert_doc(
            &db,
            &format!("Doc {i}"),
            &format!("Refunds are issued within 14 days of purchase. Note {i}."),
        );
    }
    db.drain_index(std::time::Duration::from_secs(60))
        .expect("drain");

    let plan = scalar(
        &db,
        "EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents",
    );
    assert!(
        plan.contains("approximation: CascadeEmbedTopKThenJudge"),
        "a table larger than k should cascade:\n{plan}"
    );
    assert!(
        plan.contains("sample_recall="),
        "the quality claim must be measured, not asserted:\n{plan}"
    );
    assert!(
        !plan.contains("sample_recall=unmeasured"),
        "recall must be sampled when an embedder is available:\n{plan}"
    );

    let before = generate_runs(&db);
    let out = db
        .query("SELECT aidb_generate('How do refunds work?', content) FROM documents")
        .expect("cascade generate");
    assert_eq!(
        out.rows.len(),
        1,
        "the cascade judges the retrieved top-k once"
    );
    assert_eq!(
        generate_runs(&db) - before,
        1,
        "the cascade must not call the model once per row"
    );
    // The cascade result keeps its provenance: it is retrieval plus judgement.
    let value: serde_json::Value =
        serde_json::from_str(&cell(&out, 0, "text")).expect("cited json");
    assert!(
        value["answer"].as_str().is_some_and(|a| !a.is_empty()),
        "{value}"
    );
    let sources = value["sources"].as_array().expect("sources");
    assert!(!sources.is_empty(), "{value}");
}

#[test]
fn a_small_table_does_not_cascade() {
    let tmp = TempDb::new("opt-no-cascade");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let plan = scalar(
        &db,
        "EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents",
    );
    assert!(
        !plan.contains("CascadeEmbedTopKThenJudge"),
        "one row is cheaper to judge directly:\n{plan}"
    );
    let out = db
        .query("SELECT aidb_generate('How do refunds work?', content) FROM documents")
        .expect("generate");
    assert_eq!(out.rows.len(), 1);
    assert!(
        !cell(&out, 0, "text").starts_with('{'),
        "a plain row-wise generate stays a string: {}",
        cell(&out, 0, "text")
    );
}

#[test]
fn the_retrieval_choice_is_a_physical_rewrite_of_the_same_search() {
    let tmp = TempDb::new("opt-hybrid");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    insert_ready(
        &db,
        "Codes",
        "The internal marker ZX19QPLUGH tracks this record.",
    );

    let keyword = scalar(&db, "EXPLAIN SELECT * FROM aidb_search('ZX19QPLUGH', 3)");
    let semantic = scalar(
        &db,
        "EXPLAIN SELECT * FROM aidb_search('How do refunds work?', 3)",
    );
    assert!(keyword.contains("HybridFtsVec"), "{keyword}");
    assert!(semantic.contains("HybridFtsVec"), "{semantic}");
    assert_ne!(
        keyword.lines().find(|l| l.contains("Similarity")),
        semantic.lines().find(|l| l.contains("Similarity")),
        "a keyword query and a semantic query should not bind the same algorithm"
    );

    // Same public function, different physical choice, both must find their doc.
    let by_code = column_values(
        &db.query("SELECT * FROM aidb_search('ZX19QPLUGH', 3)")
            .expect("keyword"),
        "content",
    );
    assert!(
        by_code.iter().any(|c| c.contains("ZX19QPLUGH")),
        "{by_code:?}"
    );
    let by_meaning = column_values(
        &db.query("SELECT * FROM aidb_search('How do refunds work?', 3)")
            .expect("semantic"),
        "content",
    );
    assert!(
        by_meaning
            .iter()
            .any(|c| c.to_lowercase().contains("refund")),
        "{by_meaning:?}"
    );
}

#[test]
fn optimizer_decisions_are_deterministic_for_deterministic_inputs() {
    let tmp = TempDb::new("opt-deterministic");
    let db = tmp.open();
    for i in 0..12 {
        insert_doc(
            &db,
            &format!("Doc {i}"),
            &format!("Refunds explained, part {i}."),
        );
    }
    db.drain_index(std::time::Duration::from_secs(60))
        .expect("drain");
    let sql = "EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents";
    let first = scalar(&db, sql);
    for _ in 0..3 {
        assert_eq!(scalar(&db, sql), first, "same inputs, same decisions");
    }
    // And the same file reopened decides the same way.
    drop(db);
    let reopened = tmp.open();
    assert_eq!(scalar(&reopened, sql), first);
}

#[test]
fn the_plan_names_its_rewrite_classes_so_a_developer_can_read_it() {
    let tmp = TempDb::new("opt-readable");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let plan = scalar(
        &db,
        "EXPLAIN SELECT aidb_generate('Summarize', content) FROM documents WHERE title = 'Refunds'",
    );
    assert!(
        plan.contains("equivalence: PushFilterBeforeExpensive"),
        "{plan}"
    );
    assert!(plan.contains("physical: CacheKeyedAiCall"), "{plan}");
    assert!(
        plan.contains("physical: BatchTupleIndependentLlm"),
        "{plan}"
    );
    assert!(plan.contains("Policy read_only="), "{plan}");
}
