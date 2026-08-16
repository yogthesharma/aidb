//! Agent = model + instructions + tools + memory + loop.
//! Tools come from the capability catalog. No agents table.

use aidb_core::{new_id, QueryResult, Result, Value};
use aidb_sql::AgentSpec;

use crate::Aidb;

enum AgentResult {
    Output(String),
    Paused { status: String, message: String },
}

pub(crate) fn run(db: &Aidb, spec_json: &str) -> Result<QueryResult> {
    run_inner(db, spec_json, None)
}

pub(crate) fn resume(db: &Aidb, parent_id: &str, spec_json: &str) -> Result<()> {
    let spec = aidb_sql::parse_agent(spec_json)?;
    let _ = finish(db, parent_id.to_string(), exec_agent(db, parent_id, &spec));
    Ok(())
}

fn run_inner(db: &Aidb, spec_json: &str, parent_id: Option<&str>) -> Result<QueryResult> {
    let spec = aidb_sql::parse_agent(spec_json)?;
    aidb_run::with_session(spec.session.as_deref(), || {
        let plan = spec.to_logical();
        let id = new_id("run");
        db.store.write(|conn| {
            let ctx = aidb_sql::bind_context(conn)?;
            plan.bind(&ctx)?;
            authorize_tools(conn, &spec)?;
            aidb_run::insert_agent_run_parent(conn, &id, spec_json, "running", parent_id)?;
            aidb_run::append_event(conn, &id, "started", None)?;
            Ok(())
        })?;
        let result = exec_agent(db, &id, &spec);
        finish(db, id, result)
    })
}

fn finish(db: &Aidb, parent_id: String, result: Result<AgentResult>) -> Result<QueryResult> {
    match result {
        Ok(AgentResult::Output(output)) => {
            db.store.write(|conn| {
                aidb_run::complete_run(conn, &parent_id, "succeeded", Some(&output), None)?;
                aidb_run::append_event(conn, &parent_id, "succeeded", None)?;
                Ok(())
            })?;
            Ok(QueryResult {
                columns: vec!["run_id".into(), "status".into(), "output".into()],
                rows: vec![vec![
                    Value::Text(parent_id),
                    Value::Text("succeeded".into()),
                    Value::Text(output),
                ]],
            })
        }
        Ok(AgentResult::Paused { status, message }) => {
            db.store.write(|conn| {
                aidb_run::park_run(conn, &parent_id, &status, Some(&message))?;
                aidb_run::append_event(conn, &parent_id, &status, Some(&message))?;
                Ok(())
            })?;
            Ok(QueryResult {
                columns: vec!["run_id".into(), "status".into(), "output".into()],
                rows: vec![vec![
                    Value::Text(parent_id),
                    Value::Text(status),
                    Value::Text(message),
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

fn authorize_tools(conn: &aidb_storage::Connection, spec: &AgentSpec) -> Result<()> {
    for name in &spec.tools {
        let cap = aidb_tool::require(conn, name)?;
        aidb_tool::authorize_in(conn, &cap, Some(&spec.tools))?;
    }
    Ok(())
}

fn exec_agent(db: &Aidb, parent_id: &str, spec: &AgentSpec) -> Result<AgentResult> {
    let mut memory = if let Some(scope) = spec.memory.as_deref() {
        crate::memory::load_scope(db, scope)?
    } else {
        String::new()
    };
    let mut last = memory.clone();
    if spec.decide {
        return exec_decide(db, parent_id, spec, &mut memory, &mut last);
    }
    for step in 0..spec.max_steps {
        // Only the model can end the loop, and it can do so before the last tool in
        // the recipe runs. A tool that runs after it (an email, say) must not erase
        // the signal, or an approved run would ask for approval again next step.
        let mut done = false;
        for tool in &spec.tools {
            let node_id = format!("a.{step}.{tool}");
            match run_tool(db, parent_id, spec, tool, &node_id, &last, &memory, None)? {
                ToolStep::Paused { status, message } => {
                    return Ok(AgentResult::Paused { status, message });
                }
                ToolStep::Text(text) => {
                    if !text.is_empty() {
                        done = done || is_done(&text);
                        last = text;
                        if !memory.is_empty() {
                            memory.push('\n');
                        }
                        memory.push_str(&last);
                    }
                }
            }
        }
        if done || is_done(&last) {
            break;
        }
    }
    if let Some(paused) = run_children(db, parent_id, spec, &memory)? {
        return Ok(paused);
    }
    Ok(AgentResult::Output(if last.is_empty() {
        memory
    } else {
        last
    }))
}

fn exec_decide(
    db: &Aidb,
    parent_id: &str,
    spec: &AgentSpec,
    memory: &mut String,
    last: &mut String,
) -> Result<AgentResult> {
    let mut taken: Vec<String> = Vec::new();
    for step in 0..spec.max_steps {
        let decide_id = format!("a.{step}.decide");
        let choice = match run_decide(db, parent_id, spec, &decide_id, last, memory, &taken)? {
            DecideStep::Paused { status, message } => {
                return Ok(AgentResult::Paused { status, message });
            }
            DecideStep::Choice(choice) => choice,
        };
        if choice.op == "stop" {
            break;
        }
        if !spec.tools.iter().any(|t| t == &choice.op) {
            return Err(aidb_core::Error::usage(format!(
                "decide chose {}, which is not in the tool list",
                choice.op
            )));
        }
        let node_id = format!("a.{step}.{}", choice.op);
        match run_tool(
            db,
            parent_id,
            spec,
            &choice.op,
            &node_id,
            last,
            memory,
            Some(&choice.args),
        )? {
            ToolStep::Paused { status, message } => {
                return Ok(AgentResult::Paused { status, message });
            }
            ToolStep::Text(text) => {
                if !taken.iter().any(|t| t == &choice.op) {
                    taken.push(choice.op.clone());
                }
                if !text.is_empty() {
                    *last = text;
                    if !memory.is_empty() {
                        memory.push('\n');
                    }
                    memory.push_str(last);
                }
            }
        }
    }
    if let Some(paused) = run_children(db, parent_id, spec, memory)? {
        return Ok(paused);
    }
    Ok(AgentResult::Output(if last.is_empty() {
        memory.clone()
    } else {
        last.clone()
    }))
}

struct Choice {
    op: String,
    args: String,
}

enum DecideStep {
    Choice(Choice),
    Paused { status: String, message: String },
}

fn run_decide(
    db: &Aidb,
    parent_id: &str,
    spec: &AgentSpec,
    node_id: &str,
    last: &str,
    memory: &str,
    taken: &[String],
) -> Result<DecideStep> {
    if let Some(artifact) = db
        .store
        .write(|conn| aidb_run::get_checkpoint(conn, parent_id, node_id))?
    {
        if is_unresolved_pause(&artifact) {
            return Ok(DecideStep::Paused {
                status: pause_status(&artifact),
                message: pause_message(&artifact),
            });
        }
        return Ok(DecideStep::Choice(choice_from_artifact(&artifact)?));
    }

    let schema = spec.decide_schema();
    let taken_line = if taken.is_empty() {
        String::new()
    } else {
        taken.join(", ")
    };
    let content = format!(
        "Goal: {}\nInstructions: {}\nLast:\n{}\nMemory:\n{}\nTaken: {taken_line}",
        spec.goal, spec.instructions, last, memory
    );
    let text = db.store.write(|conn| {
        aidb_sql::generate_with_schema(
            conn,
            "Choose the next operator as JSON.",
            &content,
            Some(parent_id),
            None,
            Some(&schema),
        )
    })?;
    let choice = parse_choice(&text, spec)?;
    let artifact = serde_json::json!({
        "op": choice.op,
        "args": serde_json::from_str::<serde_json::Value>(&choice.args)
            .unwrap_or(serde_json::json!({})),
        "text": text,
        "tool": "decide",
    })
    .to_string();
    db.store.write(|conn| {
        aidb_run::put_checkpoint(conn, parent_id, node_id, Some(&artifact))?;
        aidb_run::append_event(conn, parent_id, "step", Some(node_id))?;
        Ok(())
    })?;
    Ok(DecideStep::Choice(choice))
}

fn parse_choice(text: &str, spec: &AgentSpec) -> Result<Choice> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|err| aidb_core::Error::usage(format!("decide output is not JSON: {err}")))?;
    let op = value
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| aidb_core::Error::usage("decide output is missing op"))?
        .to_string();
    if op != "stop" && !spec.tools.iter().any(|t| t == &op) {
        return Err(aidb_core::Error::usage(format!(
            "decide chose {op}, which is not in the tool list"
        )));
    }
    let args = value
        .get("args")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(Choice {
        op,
        args: if args.is_object() {
            args.to_string()
        } else {
            "{}".into()
        },
    })
}

fn choice_from_artifact(artifact: &str) -> Result<Choice> {
    let value: serde_json::Value = serde_json::from_str(artifact)
        .unwrap_or_else(|_| serde_json::json!({ "op": "stop", "args": {} }));
    let op = value
        .get("op")
        .and_then(|v| v.as_str())
        .unwrap_or("stop")
        .to_string();
    let args = value
        .get("args")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(Choice {
        op,
        args: args.to_string(),
    })
}

fn run_children(
    db: &Aidb,
    parent_id: &str,
    spec: &AgentSpec,
    memory: &str,
) -> Result<Option<AgentResult>> {
    for (i, child) in spec.agents.iter().enumerate() {
        let node_id = format!("a.child.{i}");
        if let Some(artifact) = db
            .store
            .write(|conn| aidb_run::get_checkpoint(conn, parent_id, &node_id))?
        {
            if is_unresolved_pause(&artifact) {
                return Ok(Some(AgentResult::Paused {
                    status: pause_status(&artifact),
                    message: pause_message(&artifact),
                }));
            }
            continue;
        }
        let mut child_spec = child.clone();
        if child_spec.memory.is_none() {
            child_spec.memory = spec.memory.clone();
        }
        if child_spec.memory.is_none() && !memory.is_empty() {
            child_spec.memory = spec.memory.clone();
        }
        let child_json = child_spec_json(&child_spec);
        let result = run_inner(db, &child_json, Some(parent_id))?;
        let status = result.rows[0][1].to_string();
        let output = result.rows[0][2].to_string();
        let child_id = result.rows[0][0].to_string();
        if status == "awaiting_approval" || status == "suspended" {
            let artifact = serde_json::json!({
                "paused": true,
                "status": status,
                "message": output,
                "child_id": child_id
            })
            .to_string();
            db.store.write(|conn| {
                aidb_run::put_checkpoint(conn, parent_id, &node_id, Some(&artifact))?;
                Ok(())
            })?;
            return Ok(Some(AgentResult::Paused {
                status,
                message: output,
            }));
        }
        let artifact = serde_json::json!({
            "text": output,
            "child_id": child_id,
            "status": status
        })
        .to_string();
        db.store.write(|conn| {
            aidb_run::put_checkpoint(conn, parent_id, &node_id, Some(&artifact))?;
            aidb_run::append_event(conn, parent_id, "child", Some(&child_id))?;
            Ok(())
        })?;
    }
    Ok(None)
}

fn child_spec_json(spec: &AgentSpec) -> String {
    serde_json::json!({
        "instructions": spec.instructions,
        "goal": spec.goal,
        "tools": spec.tools,
        "max_steps": spec.max_steps,
        "k": spec.k,
        "memory": spec.memory,
        "decide": spec.decide,
        "session": spec.session,
    })
    .to_string()
}

enum ToolStep {
    Text(String),
    Paused { status: String, message: String },
}

#[allow(clippy::too_many_arguments)]
fn run_tool(
    db: &Aidb,
    parent_id: &str,
    spec: &AgentSpec,
    tool: &str,
    node_id: &str,
    last: &str,
    memory: &str,
    chosen_args: Option<&str>,
) -> Result<ToolStep> {
    if let Some(artifact) = db
        .store
        .write(|conn| aidb_run::get_checkpoint(conn, parent_id, node_id))?
    {
        if is_unresolved_pause(&artifact) {
            return Ok(ToolStep::Paused {
                status: pause_status(&artifact),
                message: pause_message(&artifact),
            });
        }
        if needs_invoke_after_approval(&artifact) {
            let args = args_from_artifact(&artifact).unwrap_or_else(|| {
                chosen_args
                    .map(str::to_string)
                    .unwrap_or_else(|| tool_args(spec, tool, last))
            });
            let text = dispatch(db, parent_id, spec, tool, &args)?;
            let done =
                serde_json::json!({ "text": text, "tool": tool, "approved": true }).to_string();
            db.store.write(|conn| {
                aidb_run::put_checkpoint(conn, parent_id, node_id, Some(&done))?;
                aidb_run::append_event(conn, parent_id, "step", Some(node_id))?;
                Ok(())
            })?;
            return Ok(ToolStep::Text(text));
        }
        return Ok(ToolStep::Text(artifact_text(&artifact)));
    }

    let args = chosen_args
        .map(str::to_string)
        .unwrap_or_else(|| tool_args(spec, tool, last));

    let (cap, policy) = db.store.write(|conn| {
        let cap = aidb_tool::require(conn, tool)?;
        let policy = aidb_tool::authorize_in(conn, &cap, Some(&spec.tools))?;
        Ok((cap, policy))
    })?;

    if policy.requires_approval(&cap) {
        let message = format!("approve irreversible tool {tool}");
        let artifact = serde_json::json!({
            "paused": true,
            "status": "awaiting_approval",
            "message": message,
            "tool": tool,
            "args": serde_json::from_str::<serde_json::Value>(&args).unwrap_or(serde_json::json!({}))
        })
        .to_string();
        db.store.write(|conn| {
            aidb_run::put_checkpoint(conn, parent_id, node_id, Some(&artifact))?;
            Ok(())
        })?;
        return Ok(ToolStep::Paused {
            status: "awaiting_approval".into(),
            message,
        });
    }

    let text = match tool {
        "search" => search_step(db, parent_id, spec, &args)?,
        "generate" => {
            let content = if memory.is_empty() {
                spec.goal.clone()
            } else {
                format!("Goal: {}\nMemory:\n{}", spec.goal, memory)
            };
            db.store.write(|conn| {
                aidb_sql::generate_text(conn, &spec.instructions, &content, Some(parent_id))
            })?
        }
        _ => dispatch(db, parent_id, spec, tool, &args)?,
    };

    let artifact = serde_json::json!({ "text": text, "tool": tool, "args": serde_json::from_str::<serde_json::Value>(&args).unwrap_or(serde_json::json!({})) }).to_string();
    aidb_core::crash_point("before_agent_step_checkpoint");
    db.store.write(|conn| {
        aidb_run::put_checkpoint(conn, parent_id, node_id, Some(&artifact))?;
        aidb_run::append_event(conn, parent_id, "step", Some(node_id))?;
        Ok(())
    })?;
    aidb_core::crash_point("after_agent_step_checkpoint");
    Ok(ToolStep::Text(text))
}

fn search_step(db: &Aidb, parent_id: &str, spec: &AgentSpec, args: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}));
    let query = value
        .get("query")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(spec.goal.as_str());
    let k = value
        .get("k")
        .and_then(|v| v.as_i64())
        .unwrap_or(spec.k)
        .clamp(1, 4096);
    let filter = match value.get("filter") {
        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(obj) if obj.is_object() => Some(obj.to_string()),
        _ => None,
    };
    let space = value
        .get("space")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let hits = db.store.write(|conn| {
        aidb_index::search_in(
            conn,
            db.embedder.as_ref(),
            query,
            k,
            Some(parent_id),
            None,
            filter.as_deref(),
            space,
        )
    })?;
    Ok(hits_to_text(&hits))
}

fn args_from_artifact(artifact: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(artifact)
        .ok()
        .and_then(|v| v.get("args").cloned())
        .map(|v| v.to_string())
}

fn dispatch(
    db: &Aidb,
    parent_id: &str,
    spec: &AgentSpec,
    tool: &str,
    args: &str,
) -> Result<String> {
    db.store.write(|conn| {
        let cap = aidb_tool::require(conn, tool)?;
        let policy = aidb_tool::authorize_in(conn, &cap, Some(&spec.tools))?;
        let approved = policy.requires_approval(&cap);
        let (_, output) = if approved {
            aidb_tool::invoke_approved(conn, tool, args, Some(parent_id))?
        } else {
            aidb_tool::invoke(conn, tool, args, Some(parent_id))?
        };
        Ok(output)
    })
}

fn tool_args(spec: &AgentSpec, tool: &str, last: &str) -> String {
    match tool {
        "github.read" => serde_json::json!({ "path": spec.goal, "query": spec.goal }).to_string(),
        "send.email" => serde_json::json!({
            "to": "user@example.com",
            "subject": spec.goal,
            "body": last
        })
        .to_string(),
        "http.get" => serde_json::json!({ "url": "aidb://docs" }).to_string(),
        _ => serde_json::json!({ "goal": spec.goal, "last": last }).to_string(),
    }
}

fn is_done(text: &str) -> bool {
    text.to_ascii_lowercase().contains("done")
}

fn artifact_text(artifact: &str) -> String {
    serde_json::from_str::<serde_json::Value>(artifact)
        .ok()
        .and_then(|v| {
            v.get("text")
                .and_then(|t| t.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| artifact.to_string())
}

fn is_unresolved_pause(artifact: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(artifact)
        .ok()
        .and_then(|v| v.get("paused").and_then(|p| p.as_bool()))
        == Some(true)
}

fn needs_invoke_after_approval(artifact: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(artifact) else {
        return false;
    };
    value.get("resumed").and_then(|v| v.as_bool()) == Some(true)
        && value.get("text").and_then(|v| v.as_str()).is_none()
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
        .unwrap_or_default()
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
