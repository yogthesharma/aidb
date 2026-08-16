//! Goal language frontend. Compiles to IR, persists as a workflow run.

use aidb_core::{new_id, QueryResult, Result};
use aidb_ir::GoalSpec;
use aidb_opt::Budget;

use crate::{workflow, Aidb};

pub(crate) fn run_sql(db: &Aidb, sql: &str) -> Result<QueryResult> {
    let spec = aidb_sql::parse_goal_sql(sql)?;
    run(db, spec)
}

pub(crate) fn run(db: &Aidb, spec: GoalSpec) -> Result<QueryResult> {
    let file = db.store.write(aidb_sql::budget_from_conn)?;
    let goal = Budget {
        max_usd: spec.max_usd,
        max_ms: spec.max_ms,
        max_llm_calls: None,
    };
    let budget = file.overlay(&goal);
    aidb_sql::with_budget(budget, || run_inner(db, spec))
}

fn run_inner(db: &Aidb, spec: GoalSpec) -> Result<QueryResult> {
    if spec.emits_generate_over_documents() {
        return run_generate(db, &spec);
    }
    workflow::run_compiled(db, &spec.source, spec.to_workflow())
}

fn run_generate(db: &Aidb, spec: &GoalSpec) -> Result<QueryResult> {
    let plan = spec.to_logical();
    let parent_id = new_id("run");
    db.store.write(|conn| {
        let ctx = aidb_sql::bind_context(conn)?;
        plan.bind(&ctx)?;
        aidb_run::insert_workflow_run(conn, &parent_id, &spec.source, "running")?;
        aidb_run::append_event(conn, &parent_id, "started", None)?;
        Ok(())
    })?;
    let result = db.store.write(|conn| {
        aidb_sql::execute_optimized_generate(
            conn,
            db.embedder.as_ref(),
            &spec.prompt(),
            "documents",
            None,
            None,
        )
    });
    match result {
        Ok(rows) => {
            let output = rows
                .rows
                .first()
                .and_then(|r| r.first())
                .map(ToString::to_string)
                .unwrap_or_default();
            db.store.write(|conn| {
                aidb_run::complete_run(conn, &parent_id, "succeeded", Some(&output), None)?;
                aidb_run::append_event(conn, &parent_id, "succeeded", None)?;
                Ok(())
            })?;
            Ok(aidb_core::QueryResult {
                columns: vec!["run_id".into(), "status".into(), "output".into()],
                rows: vec![vec![
                    aidb_core::Value::Text(parent_id),
                    aidb_core::Value::Text("succeeded".into()),
                    aidb_core::Value::Text(output),
                ]],
            })
        }
        Err(err) => {
            let message = err.to_string();
            let _ = db.store.write(|conn| {
                aidb_run::finish_run(conn, &parent_id, "failed", Some(&message))?;
                aidb_run::append_event(conn, &parent_id, "failed", Some(&message))?;
                Ok(())
            });
            Err(err)
        }
    }
}
