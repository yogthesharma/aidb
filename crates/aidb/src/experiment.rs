//! Experiments: plan A vs plan B over labeled examples, priced from the file.
//!
//! The optimizer claims a rewrite is cheaper at acceptable quality. An experiment
//! makes that claim a row: it runs each named plan over the same dataset under the
//! same budget, grades the answers against gold, and leaves the numbers behind as
//! runs. There is no experiment store — the parent is the comparison, each child is
//! a plan, and `experiment_results` is a view over both.

use aidb_core::{new_id, Error, QueryResult, Result, Value};
use aidb_sql::{ExperimentSpec, PlanName};

use crate::Aidb;

struct Example {
    id: i64,
    question: String,
    expect_text: Option<String>,
    expect_documents: Vec<String>,
}

/// One example under one plan.
struct Score {
    correct: bool,
    /// `None` when the example names no gold documents: retrieval quality is then
    /// not a question this example can answer, and averaging it in would be a guess.
    recall: Option<f64>,
    llm_calls: i64,
}

pub(crate) fn run(db: &Aidb, spec_json: &str) -> Result<QueryResult> {
    let spec = aidb_sql::parse_experiment(spec_json)?;
    let examples = load_examples(db, &spec.dataset)?;
    if examples.is_empty() {
        return Err(Error::usage(format!(
            "dataset {} has no examples; INSERT INTO eval_examples first",
            spec.dataset
        )));
    }

    let id = new_id("run");
    db.store.write(|conn| {
        aidb_run::insert_kind_run(conn, &id, "experiment", spec_json, "running")?;
        aidb_run::append_event(conn, &id, "started", None)?;
        Ok(())
    })?;

    let mut summaries = Vec::new();
    for plan in &spec.plans {
        match run_plan(db, &id, &spec, *plan, &examples) {
            Ok(summary) => summaries.push(summary),
            Err(err) => {
                let message = err.to_string();
                let _ = db.store.write(|conn| {
                    aidb_run::finish_run(conn, &id, "failed", Some(&message))?;
                    aidb_run::append_event(conn, &id, "failed", Some(&message))?;
                    Ok(())
                });
                return Err(err);
            }
        }
    }

    let output = serde_json::json!({
        "dataset": spec.dataset,
        "examples": examples.len(),
        "k": spec.k,
        "plans": summaries.iter().map(|s| s.to_json()).collect::<Vec<_>>(),
        "best": best_of(&summaries),
    })
    .to_string();
    db.store.write(|conn| {
        aidb_run::complete_run(conn, &id, "succeeded", Some(&output), None)?;
        aidb_run::append_event(conn, &id, "succeeded", None)?;
        Ok(())
    })?;

    Ok(QueryResult {
        columns: vec!["run_id".into(), "status".into(), "output".into()],
        rows: vec![vec![
            Value::Text(id),
            Value::Text("succeeded".into()),
            Value::Text(output),
        ]],
    })
}

struct PlanSummary {
    plan: PlanName,
    run_id: String,
    status: String,
    error: Option<String>,
    examples: usize,
    correct: usize,
    accuracy: f64,
    recall: Option<f64>,
    llm_calls: i64,
    cost_usd: f64,
}

impl PlanSummary {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "plan": self.plan.as_str(),
            "run_id": self.run_id,
            "status": self.status,
            "error": self.error,
            "examples": self.examples,
            "correct": self.correct,
            "accuracy": self.accuracy,
            "recall": self.recall,
            "llm_calls": self.llm_calls,
            "cost_usd": self.cost_usd,
        })
    }
}

/// A plan that fails is a result, not an exception: "naive cannot answer inside this
/// budget" is exactly what an experiment is for. Only a broken *setup* aborts.
fn run_plan(
    db: &Aidb,
    parent_id: &str,
    spec: &ExperimentSpec,
    plan: PlanName,
    examples: &[Example],
) -> Result<PlanSummary> {
    let run_id = new_id("run");
    let input = serde_json::json!({
        "plan": plan.as_str(),
        "dataset": spec.dataset,
        "k": spec.k,
        "prompt": spec.prompt,
    })
    .to_string();
    db.store.write(|conn| {
        aidb_run::insert_kind_run_parent(
            conn,
            &run_id,
            "experiment",
            &input,
            "running",
            Some(parent_id),
        )?;
        aidb_run::append_event(conn, &run_id, "started", None)?;
        Ok(())
    })?;

    let mut correct = 0usize;
    let mut graded = 0usize;
    let mut recalls = Vec::new();
    let mut llm_calls = 0i64;
    let mut failure = None;
    for example in examples {
        match score_example(db, &run_id, spec, plan, example) {
            Ok(score) => {
                graded += 1;
                if score.correct {
                    correct += 1;
                }
                if let Some(recall) = score.recall {
                    recalls.push(recall);
                }
                llm_calls += score.llm_calls;
            }
            Err(err) => {
                failure = Some(format!("example {}: {err}", example.id));
                break;
            }
        }
    }

    let accuracy = if graded == 0 {
        0.0
    } else {
        correct as f64 / graded as f64
    };
    let recall = if recalls.is_empty() {
        None
    } else {
        Some(recalls.iter().sum::<f64>() / recalls.len() as f64)
    };
    let status = if failure.is_some() {
        "failed"
    } else {
        "succeeded"
    };
    let (prompt_tokens, completion_tokens, cost_usd) =
        db.store.write(|conn| aidb_run::rollup_of(conn, &run_id))?;
    let output = serde_json::json!({
        "plan": plan.as_str(),
        "dataset": spec.dataset,
        "k": spec.k,
        "examples": graded,
        "correct": correct,
        "accuracy": accuracy,
        "recall": recall,
        "llm_calls": llm_calls,
    })
    .to_string();
    db.store.write(|conn| {
        aidb_run::complete_rollup_run(
            conn,
            &run_id,
            status,
            Some(&output),
            failure.as_deref(),
            Some(prompt_tokens),
            Some(completion_tokens),
            Some(cost_usd),
        )?;
        aidb_run::append_event(conn, &run_id, status, failure.as_deref())?;
        Ok(())
    })?;

    Ok(PlanSummary {
        plan,
        run_id,
        status: status.to_string(),
        error: failure,
        examples: graded,
        correct,
        accuracy,
        recall,
        llm_calls,
        cost_usd,
    })
}

fn score_example(
    db: &Aidb,
    plan_run: &str,
    spec: &ExperimentSpec,
    plan: PlanName,
    example: &Example,
) -> Result<Score> {
    let prompt = format!("{}\n\nQuestion: {}", spec.prompt, example.question);
    match plan {
        // No retrieval, so nothing can be missed: recall is 1.0 by construction, and
        // the model call per document is the price of that.
        PlanName::Naive => {
            let answers = db
                .store
                .write(|conn| aidb_sql::execute_naive_generate_in(conn, &prompt, Some(plan_run)))?;
            let calls = answers.len() as i64;
            let correct = match example.expect_text.as_deref() {
                Some(gold) => answers.iter().any(|a| says(a, gold)),
                // Nothing but gold documents to grade on, and this plan had every
                // document in context, so the example cannot tell the plans apart.
                None => !answers.is_empty(),
            };
            Ok(Score {
                correct,
                // Reported only where the example asks about retrieval, so the
                // average covers the same examples cascade's does.
                recall: (!example.expect_documents.is_empty()).then_some(1.0),
                llm_calls: calls,
            })
        }
        PlanName::Cascade => {
            let out = db.store.write(|conn| {
                aidb_sql::execute_rag_generate_traced(
                    conn,
                    db.embedder.as_ref(),
                    &prompt,
                    &example.question,
                    spec.k,
                    None,
                    None,
                    Some(plan_run),
                    None,
                )
            })?;
            let found: Vec<String> = out.sources.iter().map(|c| c.document_id.clone()).collect();
            let correct = match example.expect_text.as_deref() {
                Some(gold) => says(&out.answer, gold),
                None => cites_gold(&found, &example.expect_documents),
            };
            Ok(Score {
                correct,
                recall: recall_of(&found, &example.expect_documents),
                llm_calls: 1,
            })
        }
        // Retrieval alone answers nothing, so it is graded on whether the gold text
        // was in what it returned. Cheap, and the experiment says how cheap.
        PlanName::Search => {
            let hits = db.store.write(|conn| {
                aidb_sql::execute_retrieval_in(
                    conn,
                    db.embedder.as_ref(),
                    &example.question,
                    spec.k,
                    Some(plan_run),
                )
            })?;
            let found: Vec<String> = aidb_sql::citations_from_hits(&hits)
                .iter()
                .map(|c| c.document_id.clone())
                .collect();
            let text = hit_text(&hits);
            let correct = match example.expect_text.as_deref() {
                Some(gold) => says(&text, gold),
                None => cites_gold(&found, &example.expect_documents),
            };
            Ok(Score {
                correct,
                recall: recall_of(&found, &example.expect_documents),
                llm_calls: 0,
            })
        }
    }
}

fn says(answer: &str, gold: &str) -> bool {
    let gold = gold.trim();
    !gold.is_empty() && answer.to_lowercase().contains(&gold.to_lowercase())
}

fn cites_gold(found: &[String], gold: &[String]) -> bool {
    !gold.is_empty() && gold.iter().all(|id| found.iter().any(|f| f == id))
}

fn recall_of(found: &[String], gold: &[String]) -> Option<f64> {
    if gold.is_empty() {
        return None;
    }
    let hit = gold
        .iter()
        .filter(|id| found.iter().any(|f| f == *id))
        .count();
    Some(hit as f64 / gold.len() as f64)
}

fn hit_text(hits: &QueryResult) -> String {
    let index = hits
        .columns
        .iter()
        .position(|c| c == "content")
        .unwrap_or(2);
    hits.rows
        .iter()
        .filter_map(|row| row.get(index).map(ToString::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Most correct first, then cheapest, then by name so the verdict never depends on
/// which plan happened to run first. Retrieval-only plans are excluded: they are
/// free because they answer nothing, and free is not a way to win.
fn best_of(summaries: &[PlanSummary]) -> serde_json::Value {
    let mut ranked: Vec<&PlanSummary> = summaries
        .iter()
        .filter(|s| s.status == "succeeded" && s.plan.answers())
        .collect();
    ranked.sort_by(|a, b| {
        b.accuracy
            .total_cmp(&a.accuracy)
            .then(a.cost_usd.total_cmp(&b.cost_usd))
            .then(a.plan.as_str().cmp(b.plan.as_str()))
    });
    match ranked.first() {
        Some(best) => serde_json::json!({
            "plan": best.plan.as_str(),
            "why": "highest accuracy, then lowest cost, among plans that answer",
            "accuracy": best.accuracy,
            "cost_usd": best.cost_usd,
        }),
        None => serde_json::Value::Null,
    }
}

fn load_examples(db: &Aidb, dataset: &str) -> Result<Vec<Example>> {
    db.store.write(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, question, expect_text, expect_documents
                   FROM eval_examples
                  WHERE dataset = ?1
                  ORDER BY id",
            )
            .map_err(aidb_storage::sqlite_err)?;
        let rows = stmt
            .query_map([dataset], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(aidb_storage::sqlite_err)?;
        let mut out = Vec::new();
        for row in rows {
            let (id, question, expect_text, expect_documents) =
                row.map_err(aidb_storage::sqlite_err)?;
            out.push(Example {
                id,
                question,
                expect_text,
                expect_documents: document_ids(expect_documents.as_deref())?,
            });
        }
        Ok(out)
    })
}

fn document_ids(json: Option<&str>) -> Result<Vec<String>> {
    let Some(json) = json.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|err| Error::usage(format!("eval_examples.expect_documents: {err}")))?;
    match value {
        serde_json::Value::Array(items) => Ok(items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect()),
        serde_json::Value::String(one) => Ok(vec![one]),
        _ => Err(Error::usage(
            "eval_examples.expect_documents must be a JSON array of document ids",
        )),
    }
}
