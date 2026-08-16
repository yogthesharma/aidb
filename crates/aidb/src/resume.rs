//! Resume parked runs. Approval / wait are statuses, not IR nodes.

use aidb_core::{Error, QueryResult, Result, Value};

use crate::{agent, workflow, Aidb};

pub(crate) fn resume_sql(db: &Aidb, run_id: &str, decision_json: &str) -> Result<QueryResult> {
    let decision: serde_json::Value = serde_json::from_str(decision_json)
        .map_err(|err| Error::usage(format!("resume JSON: {err}")))?;
    let row = db
        .store
        .write(|conn| aidb_run::get_run(conn, run_id))?
        .ok_or_else(|| Error::usage(format!("unknown run: {run_id}")))?;

    if !aidb_run::is_waiting_status(&row.status) {
        return Err(Error::usage(format!(
            "run {run_id} is {}, not awaiting_approval or suspended",
            row.status
        )));
    }

    let approved = decision.get("approved").and_then(|v| v.as_bool());
    if row.status == "awaiting_approval" && approved.is_none() {
        return Err(Error::usage(
            "awaiting_approval requires {\"approved\":true} or {\"approved\":false}",
        ));
    }
    if approved == Some(false) {
        db.store.write(|conn| {
            aidb_run::complete_run(conn, run_id, "cancelled", None, Some("rejected"))?;
            aidb_run::append_event(conn, run_id, "cancelled", Some(decision_json))?;
            Ok(())
        })?;
        return Ok(QueryResult {
            columns: vec!["run_id".into(), "status".into(), "output".into()],
            rows: vec![vec![
                Value::Text(run_id.into()),
                Value::Text("cancelled".into()),
                Value::Text("rejected".into()),
            ]],
        });
    }

    db.store.write(|conn| {
        aidb_run::resolve_pauses(conn, run_id, decision_json)?;
        aidb_run::set_running(conn, run_id)?;
        aidb_run::append_event(conn, run_id, "resumed", Some(decision_json))?;
        Ok(())
    })?;

    match row.kind.as_str() {
        "tool" => crate::tool::resume_tool(db, run_id),
        "agent" => {
            agent::resume(db, run_id, &row.input_json)?;
            let status = db
                .store
                .write(|conn| Ok(aidb_run::get_run(conn, run_id)?.map(|r| r.status)))?;
            Ok(QueryResult {
                columns: vec!["run_id".into(), "status".into(), "output".into()],
                rows: vec![vec![
                    Value::Text(run_id.into()),
                    Value::Text(status.unwrap_or_else(|| "succeeded".into())),
                    Value::Text(String::new()),
                ]],
            })
        }
        _ => workflow::resume_one(db, run_id, &row.input_json),
    }
}
