//! Execute a compiled workflow: child runs + checkpoint after each operator.
//! Approve / wait are run states, not IR nodes.

use aidb_core::{new_id, QueryResult, Result, Value};
use aidb_ir::Workflow;

use crate::Aidb;

enum StepResult {
    Output(String),
    Paused { status: String, message: String },
}

pub(crate) fn run(db: &Aidb, spec_json: &str) -> Result<QueryResult> {
    let workflow = aidb_sql::parse_workflow(spec_json)?;
    let session = serde_json::from_str::<serde_json::Value>(spec_json)
        .ok()
        .and_then(|value| {
            value
                .get("session")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(ToOwned::to_owned)
        });
    aidb_run::with_session(session.as_deref(), || run_compiled(db, spec_json, workflow))
}

pub(crate) fn run_compiled(db: &Aidb, stored: &str, workflow: Workflow) -> Result<QueryResult> {
    let plan = workflow.to_logical();
    let parent_id = new_id("run");
    db.store.write(|conn| {
        let ctx = aidb_sql::bind_context(conn)?;
        plan.bind(&ctx)?;
        aidb_run::insert_workflow_run(conn, &parent_id, stored, "running")?;
        aidb_run::append_event(conn, &parent_id, "started", None)?;
        Ok(())
    })?;
    let mut state = FlowState::new(parent_id.clone());
    finish(db, parent_id, exec_node(db, &mut state, &workflow, "w"))
}

pub(crate) fn resume_one(db: &Aidb, id: &str, spec_json: &str) -> Result<QueryResult> {
    let workflow = if aidb_sql::looks_like_goal(spec_json) {
        aidb_sql::parse_goal(spec_json)?.to_workflow()
    } else {
        aidb_sql::parse_workflow(spec_json)?
    };
    finish(
        db,
        id.to_string(),
        exec_node(db, &mut FlowState::new(id.to_string()), &workflow, "w"),
    )
}

fn finish(db: &Aidb, parent_id: String, result: Result<StepResult>) -> Result<QueryResult> {
    match result {
        Ok(StepResult::Output(output)) => {
            db.store.write(|conn| {
                aidb_run::complete_run(conn, &parent_id, "succeeded", Some(&output), None)?;
                aidb_run::append_event(conn, &parent_id, "succeeded", None)?;
                Ok(())
            })?;
            Ok(workflow_row(parent_id, "succeeded", output))
        }
        Ok(StepResult::Paused { status, message }) => {
            db.store.write(|conn| {
                aidb_run::park_run(conn, &parent_id, &status, Some(&message))?;
                aidb_run::append_event(conn, &parent_id, &status, Some(&message))?;
                Ok(())
            })?;
            Ok(workflow_row(parent_id, &status, message))
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

struct FlowState {
    parent_id: String,
    last_text: String,
    last_hits: i64,
    last_json: String,
    /// Set when an `approve` pause in this graph was resolved, so the next
    /// irreversible tool may run without asking again.
    approved: bool,
}

impl FlowState {
    fn new(parent_id: String) -> Self {
        Self {
            parent_id,
            last_text: String::new(),
            last_hits: 0,
            last_json: "{}".into(),
            approved: false,
        }
    }
}

fn exec_node(
    db: &Aidb,
    state: &mut FlowState,
    workflow: &Workflow,
    id: &str,
) -> Result<StepResult> {
    if let Some(artifact) = load_checkpoint(db, state, id)? {
        if is_unresolved_pause(&artifact) {
            return Ok(StepResult::Paused {
                status: pause_status(&artifact),
                message: pause_message(&artifact),
            });
        }
        if is_resolved_approval(&artifact) {
            state.approved = true;
        }
        return Ok(StepResult::Output(artifact));
    }
    let output = match workflow {
        Workflow::Then(steps) => {
            let mut out = state.last_json.clone();
            for (i, step) in steps.iter().enumerate() {
                match exec_node(db, state, step, &format!("{id}.{i}"))? {
                    StepResult::Output(value) => out = value,
                    paused @ StepResult::Paused { .. } => return Ok(paused),
                }
            }
            out
        }
        Workflow::Parallel(steps) => {
            let mut parts = Vec::new();
            for (i, step) in steps.iter().enumerate() {
                match exec_node(db, state, step, &format!("{id}.{i}"))? {
                    StepResult::Output(value) => parts.push(value),
                    paused @ StepResult::Paused { .. } => return Ok(paused),
                }
            }
            format!("[{}]", parts.join(","))
        }
        Workflow::Branch { when, then, else_ } => {
            if eval_pred(when, state) {
                match exec_node(db, state, then, &format!("{id}.then"))? {
                    StepResult::Output(value) => value,
                    paused @ StepResult::Paused { .. } => return Ok(paused),
                }
            } else if let Some(else_step) = else_ {
                match exec_node(db, state, else_step, &format!("{id}.else"))? {
                    StepResult::Output(value) => value,
                    paused @ StepResult::Paused { .. } => return Ok(paused),
                }
            } else {
                state.last_json.clone()
            }
        }
        Workflow::Loop { body, until, max } => {
            let mut out = state.last_json.clone();
            for i in 0..*max {
                let iter_id = format!("{id}.{i}");
                if let Some(artifact) = load_checkpoint(db, state, &iter_id)? {
                    if is_unresolved_pause(&artifact) {
                        return Ok(StepResult::Paused {
                            status: pause_status(&artifact),
                            message: pause_message(&artifact),
                        });
                    }
                    out = artifact;
                    continue;
                }
                if !until.is_empty() && eval_pred(until, state) {
                    break;
                }
                match exec_node(db, state, body, &iter_id)? {
                    StepResult::Output(value) => out = value,
                    paused @ StepResult::Paused { .. } => return Ok(paused),
                }
            }
            out
        }
        Workflow::Search { query, k } => {
            let hits = db.store.write(|conn| {
                aidb_index::search_with_parent(
                    conn,
                    db.embedder.as_ref(),
                    query,
                    *k,
                    Some(&state.parent_id),
                )
            })?;
            let text = hits_to_text(&hits);
            state.last_hits = hits.rows.len() as i64;
            state.last_text = text.clone();
            serde_json::json!({ "hits": hits.rows.len(), "text": text }).to_string()
        }
        Workflow::Generate { prompt, content } => {
            let content = content.clone().unwrap_or_else(|| state.last_text.clone());
            let text = db.store.write(|conn| {
                aidb_sql::generate_text(conn, prompt, &content, Some(&state.parent_id))
            })?;
            state.last_text = text.clone();
            serde_json::json!({ "text": text }).to_string()
        }
        Workflow::Tool { name } => {
            let args = if state.last_json.is_empty() {
                "{}".into()
            } else {
                state.last_json.clone()
            };
            let output = db.store.write(|conn| {
                let cap = aidb_tool::require(conn, name)?;
                let policy = aidb_tool::authorize_in(conn, &cap, None)?;
                if policy.requires_approval(&cap) {
                    if !state.approved {
                        return Err(aidb_core::Error::usage(format!(
                            "capability {name} is irreversible and needs approval"
                        )));
                    }
                    let (_, output) =
                        aidb_tool::invoke_approved(conn, name, &args, Some(&state.parent_id))?;
                    return Ok(output);
                }
                let (_, output) = aidb_tool::invoke(conn, name, &args, Some(&state.parent_id))?;
                Ok(output)
            })?;
            state.approved = false;
            state.last_text = output.clone();
            state.last_json = output.clone();
            output
        }
        Workflow::Decide { .. } => {
            return Err(aidb_core::Error::usage(
                "decide is an agent loop, not a workflow node",
            ));
        }
        Workflow::Pause { status, message } => {
            let artifact = serde_json::json!({
                "paused": true,
                "status": status,
                "message": message,
                "text": state.last_text,
                "hits": state.last_hits,
            })
            .to_string();
            save_checkpoint(db, state, id, &artifact)?;
            return Ok(StepResult::Paused {
                status: status.clone(),
                message: message.clone(),
            });
        }
    };
    save_checkpoint(db, state, id, &output)?;
    Ok(StepResult::Output(output))
}

fn load_checkpoint(db: &Aidb, state: &mut FlowState, id: &str) -> Result<Option<String>> {
    let artifact = db
        .store
        .write(|conn| aidb_run::get_checkpoint(conn, &state.parent_id, id))?;
    if let Some(artifact) = &artifact {
        restore(state, artifact);
    }
    Ok(artifact)
}

fn save_checkpoint(db: &Aidb, state: &mut FlowState, id: &str, artifact: &str) -> Result<()> {
    restore(state, artifact);
    aidb_core::crash_point("before_checkpoint");
    db.store.write(|conn| {
        aidb_run::put_checkpoint(conn, &state.parent_id, id, Some(artifact))?;
        aidb_run::append_event(conn, &state.parent_id, "step", Some(id))?;
        Ok(())
    })?;
    aidb_core::crash_point("after_checkpoint");
    Ok(())
}

fn restore(state: &mut FlowState, artifact: &str) {
    state.last_json = artifact.to_string();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(artifact) {
        if let Some(hits) = value.get("hits").and_then(|v| v.as_i64()) {
            state.last_hits = hits;
        }
        if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
            state.last_text = text.to_string();
        }
    }
}

fn is_unresolved_pause(artifact: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(artifact)
        .ok()
        .and_then(|v| v.get("paused").and_then(|p| p.as_bool()))
        == Some(true)
}

fn is_resolved_approval(artifact: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(artifact) else {
        return false;
    };
    value.get("status").and_then(|s| s.as_str()) == Some("awaiting_approval")
        && value.get("resumed").and_then(|v| v.as_bool()) == Some(true)
}

fn pause_status(artifact: &str) -> String {
    serde_json::from_str::<serde_json::Value>(artifact)
        .ok()
        .and_then(|v| {
            v.get("status")
                .and_then(|s| s.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "awaiting_approval".into())
}

fn pause_message(artifact: &str) -> String {
    serde_json::from_str::<serde_json::Value>(artifact)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|s| s.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| artifact.to_string())
}

fn eval_pred(pred: &str, state: &FlowState) -> bool {
    let pred = pred.trim();
    if pred.eq_ignore_ascii_case("true") {
        return true;
    }
    if pred.eq_ignore_ascii_case("false") {
        return false;
    }
    if pred.eq_ignore_ascii_case("done") {
        return state.last_text.to_ascii_lowercase().contains("done");
    }
    if pred.eq_ignore_ascii_case("empty") {
        return state.last_hits == 0 && state.last_text.is_empty();
    }
    if let Some(needle) = pred.strip_prefix("contains:") {
        return state
            .last_text
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase());
    }
    if let Some(rest) = pred.strip_prefix("hits") {
        let rest = rest.trim();
        let parse_n = |s: &str| s.trim().parse::<i64>().unwrap_or(0);
        if let Some(n) = rest.strip_prefix(">=") {
            return state.last_hits >= parse_n(n);
        }
        if let Some(n) = rest.strip_prefix("<=") {
            return state.last_hits <= parse_n(n);
        }
        if let Some(n) = rest.strip_prefix('>') {
            return state.last_hits > parse_n(n);
        }
        if let Some(n) = rest.strip_prefix('<') {
            return state.last_hits < parse_n(n);
        }
        if let Some(n) = rest.strip_prefix('=') {
            return state.last_hits == parse_n(n);
        }
    }
    false
}

fn hits_to_text(hits: &QueryResult) -> String {
    let content_idx = hits
        .columns
        .iter()
        .position(|c| c == "content")
        .unwrap_or(2);
    hits.rows
        .iter()
        .filter_map(|row| row.get(content_idx).map(ToString::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

fn workflow_row(run_id: String, status: &str, output: String) -> QueryResult {
    QueryResult {
        columns: vec!["run_id".into(), "status".into(), "output".into()],
        rows: vec![vec![
            Value::Text(run_id),
            Value::Text(status.into()),
            Value::Text(output),
        ]],
    }
}
