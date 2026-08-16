//! Phase 31 contracts: an experiment turns "the rewrite is cheaper" into rows you
//! can query. The dataset is data in the file, the comparison is a run, each plan is
//! a child of it, and every number in `experiment_results` comes from those runs.

mod common;

use common::*;

/// A corpus where retrieval matters: one document answers the question and the rest
/// are near-identical filler, so the naive plan pays for every row.
fn corpus(db: &aidb::Aidb) -> String {
    for i in 1..=8 {
        insert_ready(
            db,
            &format!("Shipping {i}"),
            &format!("Standard shipping takes five business days for parcel {i}."),
        );
    }
    insert_ready(
        db,
        "Refunds",
        "A refund lands on the original card within 14 days of approval.",
    )
}

fn example(db: &aidb::Aidb, dataset: &str, question: &str, expect_text: &str, gold: &[&str]) {
    let documents = format!(
        "[{}]",
        gold.iter()
            .map(|id| format!("\"{id}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    db.execute(&format!(
        "INSERT INTO eval_examples (dataset, question, expect_text, expect_documents)
         VALUES ('{dataset}', '{}', '{}', '{documents}')",
        sql_escape(question),
        sql_escape(expect_text),
    ))
    .expect("insert example");
}

fn experiment(db: &aidb::Aidb, spec: &str) -> String {
    let result = db
        .query(&format!("SELECT aidb_experiment('{}')", sql_escape(spec)))
        .expect("experiment");
    assert_eq!(cell(&result, 0, "status"), "succeeded");
    cell(&result, 0, "run_id")
}

fn results(db: &aidb::Aidb, experiment_id: &str) -> aidb::QueryResult {
    db.query(&format!(
        "SELECT plan, status, examples, accuracy, recall, llm_calls, cost_usd, latency_ms, run_id, error
           FROM experiment_results
          WHERE experiment_id = '{experiment_id}'
          ORDER BY plan"
    ))
    .expect("experiment_results")
}

fn number(result: &aidb::QueryResult, row: usize, name: &str) -> f64 {
    cell(result, row, name)
        .parse()
        .unwrap_or_else(|_| panic!("{name} is not a number: {}", cell(result, row, name)))
}

fn row_of(result: &aidb::QueryResult, plan: &str) -> usize {
    column_values(result, "plan")
        .iter()
        .position(|p| p == plan)
        .unwrap_or_else(|| panic!("no row for plan {plan}"))
}

#[test]
fn an_experiment_prices_the_rewrite_instead_of_claiming_it() {
    let tmp = TempDb::new("exp-prices");
    let db = tmp.open();
    let gold = corpus(&db);
    example(
        &db,
        "refunds_gold",
        "how long does a refund take",
        "14 days",
        &[&gold],
    );

    let id = experiment(
        &db,
        "{\"dataset\":\"refunds_gold\",\"plans\":[\"naive\",\"cascade\"],\"k\":3}",
    );
    let rows = results(&db, &id);
    assert_eq!(rows.rows.len(), 2, "one row per plan");

    let naive = row_of(&rows, "naive");
    let cascade = row_of(&rows, "cascade");
    assert_eq!(cell(&rows, naive, "status"), "succeeded");
    assert_eq!(cell(&rows, cascade, "status"), "succeeded");

    // Same answer quality…
    assert_eq!(number(&rows, naive, "accuracy"), 1.0);
    assert_eq!(number(&rows, cascade, "accuracy"), 1.0);
    // …for one model call instead of one per document, and that shows up as money.
    assert_eq!(number(&rows, cascade, "llm_calls"), 1.0);
    assert_eq!(number(&rows, naive, "llm_calls"), 9.0);
    assert!(
        number(&rows, cascade, "cost_usd") < number(&rows, naive, "cost_usd"),
        "cascade must be cheaper: {:?}",
        (
            number(&rows, cascade, "cost_usd"),
            number(&rows, naive, "cost_usd")
        )
    );

    // And the file says which plan won, so nobody has to eyeball the table.
    let verdict = scalar(
        &db,
        &format!("SELECT json_extract(output_json, '$.best.plan') FROM runs WHERE id = '{id}'"),
    );
    assert_eq!(verdict, "cascade");
}

#[test]
fn a_plan_that_cannot_answer_inside_the_budget_is_a_result_not_an_exception() {
    let tmp = TempDb::new("exp-budget");
    let db = tmp.open();
    let gold = corpus(&db);
    example(
        &db,
        "refunds_gold",
        "how long does a refund take",
        "14 days",
        &[&gold],
    );
    db.query("SELECT aidb_set_policy('{\"max_llm_calls\":1}')")
        .expect("policy");

    // The experiment itself succeeds: "naive does not fit in this budget" is the
    // finding, and a finding belongs in the file rather than in an error the caller
    // has to catch.
    let id = experiment(
        &db,
        "{\"dataset\":\"refunds_gold\",\"plans\":[\"naive\",\"cascade\"],\"k\":3}",
    );
    let rows = results(&db, &id);
    let naive = row_of(&rows, "naive");
    let cascade = row_of(&rows, "cascade");
    assert_eq!(cell(&rows, naive, "status"), "failed");
    assert!(
        cell(&rows, naive, "error").contains("max_llm_calls=1"),
        "the row has to say why: {}",
        cell(&rows, naive, "error")
    );
    assert_eq!(cell(&rows, cascade, "status"), "succeeded");
    assert_eq!(number(&rows, cascade, "accuracy"), 1.0);

    // Both plans were held to the same budget: the one that failed is the one that
    // asked for more, not the one that ran second.
    assert!(scalar(&db, "SELECT aidb_get_policy()").contains("\"max_llm_calls\":1"));
}

#[test]
fn retrieval_only_is_the_price_floor_and_never_the_winner() {
    let tmp = TempDb::new("exp-floor");
    let db = tmp.open();
    let gold = corpus(&db);
    example(
        &db,
        "refunds_gold",
        "how long does a refund take",
        "14 days",
        &[&gold],
    );

    let id = experiment(
        &db,
        "{\"dataset\":\"refunds_gold\",\"plans\":[\"naive\",\"cascade\",\"search\"],\"k\":3}",
    );
    let rows = results(&db, &id);
    let search = row_of(&rows, "search");
    assert_eq!(number(&rows, search, "llm_calls"), 0.0);
    assert_eq!(number(&rows, search, "cost_usd"), 0.0);

    // Free, and it answers nothing, so it is never the plan the experiment picks.
    let verdict = scalar(
        &db,
        &format!("SELECT json_extract(output_json, '$.best.plan') FROM runs WHERE id = '{id}'"),
    );
    assert_ne!(verdict, "search");
}

#[test]
fn recall_reports_the_gold_the_plan_actually_found() {
    let tmp = TempDb::new("exp-recall");
    let db = tmp.open();
    corpus(&db);
    // Gold that retrieval cannot reach from this question: the answer lives in a
    // document about something else entirely.
    let unreachable = insert_ready(
        &db,
        "Warranty",
        "The warranty covers manufacturing defects for 36 months from delivery.",
    );
    example(
        &db,
        "hard_gold",
        "how long does a refund take",
        "36 months",
        &[&unreachable],
    );

    let id = experiment(
        &db,
        "{\"dataset\":\"hard_gold\",\"plans\":[\"naive\",\"cascade\"],\"k\":3}",
    );
    let rows = results(&db, &id);
    let naive = row_of(&rows, "naive");
    let cascade = row_of(&rows, "cascade");

    // Retrieval missed it, so the cheap plan is wrong and the experiment says so.
    assert_eq!(number(&rows, cascade, "recall"), 0.0);
    assert_eq!(number(&rows, cascade, "accuracy"), 0.0);
    // The expensive plan read every document, so it found the answer. That is the
    // trade the numbers exist to make visible.
    assert_eq!(number(&rows, naive, "recall"), 1.0);
    assert_eq!(number(&rows, naive, "accuracy"), 1.0);

    let verdict = scalar(
        &db,
        &format!("SELECT json_extract(output_json, '$.best.plan') FROM runs WHERE id = '{id}'"),
    );
    assert_eq!(verdict, "naive", "accuracy outranks cost");
}

#[test]
fn every_number_in_the_comparison_comes_from_the_runs_that_produced_it() {
    let tmp = TempDb::new("exp-rollup");
    let db = tmp.open();
    let gold = corpus(&db);
    example(
        &db,
        "refunds_gold",
        "how long does a refund take",
        "14 days",
        &[&gold],
    );

    let id = experiment(
        &db,
        "{\"dataset\":\"refunds_gold\",\"plans\":[\"naive\",\"cascade\"],\"k\":3}",
    );

    // The comparison is a run, each plan is its child, and the plan's spend is the
    // spend of its own children. No experiment table, no second store.
    assert_eq!(
        scalar(&db, &format!("SELECT kind FROM runs WHERE id = '{id}'")),
        "experiment"
    );
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM runs WHERE parent_id = '{id}' AND kind = 'experiment'")
        ),
        2
    );

    let rows = results(&db, &id);
    for row in 0..rows.rows.len() {
        let plan_run = cell(&rows, row, "run_id");
        let children = scalar(
            &db,
            &format!(
                "SELECT ROUND(COALESCE(SUM(cost_usd), 0.0), 12) FROM runs WHERE parent_id = '{plan_run}'"
            ),
        );
        let rolled = scalar(
            &db,
            &format!("SELECT ROUND(COALESCE(cost_usd, 0.0), 12) FROM runs WHERE id = '{plan_run}'"),
        );
        assert_eq!(
            children,
            rolled,
            "plan {} must cost exactly what its children cost",
            cell(&rows, row, "plan")
        );
        assert!(number(&rows, row, "latency_ms") >= 0.0);
    }

    // The model calls the naive plan reported are generate runs under it, so its
    // count can be audited from the file rather than trusted.
    let naive = row_of(&rows, "naive");
    let naive_run = cell(&rows, naive, "run_id");
    assert_eq!(
        count(
            &db,
            &format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{naive_run}' AND kind = 'generate'"
            )
        ) as f64,
        number(&rows, naive, "llm_calls")
    );
}

#[test]
fn the_same_experiment_twice_reports_the_same_comparison() {
    let tmp = TempDb::new("exp-determinism");
    let db = tmp.open();
    let gold = corpus(&db);
    example(
        &db,
        "refunds_gold",
        "how long does a refund take",
        "14 days",
        &[&gold],
    );
    let spec = "{\"dataset\":\"refunds_gold\",\"plans\":[\"naive\",\"cascade\"],\"k\":3}";

    let first = results(&db, &experiment(&db, spec));
    let second = results(&db, &experiment(&db, spec));
    for name in [
        "plan",
        "status",
        "accuracy",
        "recall",
        "llm_calls",
        "cost_usd",
    ] {
        assert_eq!(
            column_values(&first, name),
            column_values(&second, name),
            "{name} drifted between identical experiments"
        );
    }
}

#[test]
fn an_experiment_outlives_the_process_that_ran_it() {
    let tmp = TempDb::new("exp-durable");
    let path = tmp.path();
    let id = {
        let db = tmp.open();
        let gold = corpus(&db);
        example(
            &db,
            "refunds_gold",
            "how long does a refund take",
            "14 days",
            &[&gold],
        );
        experiment(
            &db,
            "{\"dataset\":\"refunds_gold\",\"plans\":[\"naive\",\"cascade\"],\"k\":3}",
        )
    };

    let reopened = aidb::open(&path).expect("reopen");
    let rows = results(&reopened, &id);
    assert_eq!(rows.rows.len(), 2);
    assert_eq!(
        column_values(&rows, "status"),
        vec!["succeeded".to_string(), "succeeded".to_string()]
    );
    // The dataset is in the file too, so the comparison can be repeated later.
    assert_eq!(
        count(
            &reopened,
            "SELECT COUNT(*) FROM eval_examples WHERE dataset = 'refunds_gold'"
        ),
        1
    );
}

#[test]
fn an_experiment_killed_mid_flight_does_not_stay_running_forever() {
    let tmp = TempDb::new("exp-interrupted");
    let path = tmp.path();
    {
        let db = tmp.open();
        db.execute(
            "INSERT INTO runs (id, kind, status, input_json, created_at_ms, started_at_ms)
             VALUES ('run_killed', 'experiment', 'running', '{\"dataset\":\"x\"}', 1, 1)",
        )
        .expect("a run the process never finished");
    }

    let reopened = aidb::open(&path).expect("reopen");
    assert_eq!(
        scalar(&reopened, "SELECT status FROM runs WHERE id = 'run_killed'"),
        "failed"
    );
    assert_eq!(
        scalar(&reopened, "SELECT error FROM runs WHERE id = 'run_killed'"),
        "interrupted"
    );
}

#[test]
fn a_comparison_the_file_cannot_grade_is_refused_before_it_spends_anything() {
    let tmp = TempDb::new("exp-refusals");
    let db = tmp.open();
    let gold = corpus(&db);
    let spent = || count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'generate'");
    let before = spent();

    // An example with no gold at all cannot be graded, so the file rejects it.
    assert!(
        db.execute("INSERT INTO eval_examples (dataset, question) VALUES ('ungradeable', 'why')")
            .is_err(),
        "eval_examples must require gold"
    );

    // A dataset nobody has filled in.
    assert_err_contains(
        db.query(
            "SELECT aidb_experiment('{\"dataset\":\"missing\",\"plans\":[\"naive\",\"cascade\"]}')",
        ),
        "no examples",
    );

    example(
        &db,
        "refunds_gold",
        "how long does a refund take",
        "14 days",
        &[&gold],
    );
    // A plan the engine does not have, named rather than guessed at.
    assert_err_contains(
        db.query("SELECT aidb_experiment('{\"dataset\":\"refunds_gold\",\"plans\":[\"naive\",\"magic\"]}')"),
        "unknown plan",
    );
    // One plan is not a comparison.
    assert_err_contains(
        db.query(
            "SELECT aidb_experiment('{\"dataset\":\"refunds_gold\",\"plans\":[\"cascade\"]}')",
        ),
        "at least two",
    );
    assert_eq!(
        before,
        spent(),
        "a refused experiment must not call a model"
    );
}
