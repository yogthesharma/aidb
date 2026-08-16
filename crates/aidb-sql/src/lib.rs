//! SQL surface helpers. Phase 1 intercepts search/insert. Phase 2 registers
//! `aidb_generate` as a real SQLite function so it works in `SELECT … FROM`.
//! Phase 4 lowers search/generate to IR and prints the physical plan.

mod dialect;
mod goal;
mod plan;
mod schema;

use rusqlite::functions::FunctionFlags;

use aidb_ai::llm_with_key;
use std::cell::RefCell;
use std::time::Instant;

use aidb_core::{content_hash, new_id, now_ms, Error, Result};
use aidb_ir::{LlmContent, LogicalPlan};
use aidb_storage::{current_connection, sqlite_err, Connection};

pub use dialect::{execute_create_model, parse_create_model, parse_search_dialect, CreateModel};
pub use goal::{looks_like_goal, parse_aidb_task, parse_goal, parse_goal_sql};
pub use plan::{
    bind_context, execute_naive_generate_in, execute_optimized_classify,
    execute_optimized_generate, execute_rag_classify, execute_rag_generate,
    execute_rag_generate_in, execute_rag_generate_traced, execute_retrieval_in, execute_search,
    explain_sql, explain_sql_with, RagOutcome,
};

pub fn register(conn: &Connection) -> Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8;
    conn.create_scalar_function("aidb_generate", 2, flags, |ctx| {
        let prompt: String = ctx.get(0)?;
        let content: String = ctx.get(1)?;
        generate(&prompt, &content).map_err(|err| rusqlite::Error::UserFunctionError(err.into()))
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("aidb_generate", 3, flags, |ctx| {
        let prompt: String = ctx.get(0)?;
        let content: String = ctx.get(1)?;
        let schema: String = ctx.get(2)?;
        generate_schema(&prompt, &content, &schema)
            .map_err(|err| rusqlite::Error::UserFunctionError(err.into()))
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("ai_generate", 2, flags, |ctx| {
        let prompt: String = ctx.get(0)?;
        let content: String = ctx.get(1)?;
        generate(&prompt, &content).map_err(|err| rusqlite::Error::UserFunctionError(err.into()))
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("ai_generate", 3, flags, |ctx| {
        let prompt: String = ctx.get(0)?;
        let content: String = ctx.get(1)?;
        let schema: String = ctx.get(2)?;
        generate_schema(&prompt, &content, &schema)
            .map_err(|err| rusqlite::Error::UserFunctionError(err.into()))
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("aidb_classify", 2, flags, |ctx| {
        let labels: String = ctx.get(0)?;
        let content: String = ctx.get(1)?;
        let conn =
            current_connection().map_err(|err| rusqlite::Error::UserFunctionError(err.into()))?;
        classify_text(conn, &labels, &content, None)
            .map_err(|err| rusqlite::Error::UserFunctionError(err.into()))
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("aidb_classify", 3, flags, |ctx| {
        let labels: String = ctx.get(0)?;
        let content: String = ctx.get(1)?;
        let schema: String = ctx.get(2)?;
        let conn =
            current_connection().map_err(|err| rusqlite::Error::UserFunctionError(err.into()))?;
        classify_text_in(conn, &labels, &content, None, Some(&schema))
            .map_err(|err| rusqlite::Error::UserFunctionError(err.into()))
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("aidb_explain", 1, flags, |ctx| {
        let sql: String = ctx.get(0)?;
        let conn =
            current_connection().map_err(|err| rusqlite::Error::UserFunctionError(err.into()))?;
        explain_sql_with(conn, &sql, None)
            .map_err(|err| rusqlite::Error::UserFunctionError(err.into()))
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("aidb_secret_store", 0, flags, |_ctx| {
        Ok(aidb_ai::secret_store_uri())
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("aidb_session", 0, flags, |_ctx| {
        Ok(aidb_run::active_session().unwrap_or_default())
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("aidb_session", 1, flags, |ctx| {
        let name: Option<String> = ctx.get(0)?;
        match name.as_deref() {
            None => {
                aidb_run::clear_session();
                Ok(String::new())
            }
            Some(name) => aidb_run::bind_session(name)
                .map_err(|err| rusqlite::Error::UserFunctionError(err.into())),
        }
    })
    .map_err(sqlite_err)?;
    conn.create_scalar_function("aidb_last_run_id", 0, flags, |_ctx| {
        Ok(aidb_run::last_run_id().unwrap_or_default())
    })
    .map_err(sqlite_err)?;
    Ok(())
}

thread_local! {
    static JOB: RefCell<JobMeter> = RefCell::new(JobMeter::default());
    static BUDGET_OVERRIDE: RefCell<Option<aidb_opt::Budget>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct JobMeter {
    llm_calls: u32,
    usd: f64,
    prompt_tokens: i64,
    completion_tokens: i64,
    started: Option<Instant>,
    k: Option<i64>,
    sample_recall: Option<f64>,
    budget: Option<aidb_opt::Budget>,
}

pub fn begin_llm_budget() {
    JOB.with(|j| {
        *j.borrow_mut() = JobMeter {
            started: Some(Instant::now()),
            ..JobMeter::default()
        };
    });
}

pub fn with_budget<T>(budget: aidb_opt::Budget, f: impl FnOnce() -> Result<T>) -> Result<T> {
    BUDGET_OVERRIDE.with(|slot| *slot.borrow_mut() = Some(budget));
    let result = f();
    BUDGET_OVERRIDE.with(|slot| *slot.borrow_mut() = None);
    result
}

pub fn job_set_cascade(k: i64, recall: f64) {
    JOB.with(|j| {
        let mut job = j.borrow_mut();
        job.k = Some(k);
        job.sample_recall = Some(recall);
    });
}

fn active_budget() -> aidb_opt::Budget {
    if let Some(over) = BUDGET_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return over;
    }
    JOB.with(|j| j.borrow().budget.clone())
        .unwrap_or_else(aidb_opt::Budget::from_env)
}

pub fn budget_from_policy(policy: &aidb_tool::Policy) -> aidb_opt::Budget {
    aidb_opt::Budget {
        max_usd: policy.max_usd,
        max_ms: policy.max_ms,
        max_llm_calls: policy.max_llm_calls.or(Some(64)),
    }
}

pub fn budget_from_conn(conn: &Connection) -> Result<aidb_opt::Budget> {
    Ok(budget_from_policy(&aidb_tool::effective_policy(conn)?))
}

fn resolve_budget(conn: &Connection) -> aidb_opt::Budget {
    BUDGET_OVERRIDE
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| budget_from_conn(conn).unwrap_or_else(|_| aidb_opt::Budget::from_env()))
}

fn job_snapshot() -> serde_json::Value {
    JOB.with(|j| {
        let job = j.borrow();
        let ms = job
            .started
            .map(|started| started.elapsed().as_millis() as u64)
            .unwrap_or(0);
        serde_json::json!({
            "llm_calls": job.llm_calls,
            "usd": job.usd,
            "ms": ms,
            "prompt_tokens": job.prompt_tokens,
            "completion_tokens": job.completion_tokens,
            "k": job.k,
            "sample_recall": job.sample_recall,
        })
    })
}

/// One retrieval hit attached to a generate that used search context.
#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    pub document_id: String,
    pub chunk_id: String,
    pub score: f64,
}

pub fn citations_from_hits(hits: &aidb_core::QueryResult) -> Vec<Citation> {
    let doc = column_index(hits, "document_id").unwrap_or(0);
    let chunk = column_index(hits, "chunk_id").unwrap_or(1);
    let score = column_index(hits, "distance").unwrap_or(3);
    let mut out = Vec::new();
    for row in &hits.rows {
        let Some(document_id) = row.get(doc).map(ToString::to_string) else {
            continue;
        };
        if document_id.is_empty() {
            continue;
        }
        let chunk_id = row.get(chunk).map(ToString::to_string).unwrap_or_default();
        let score = row.get(score).map(value_f64).unwrap_or(0.0);
        if out
            .iter()
            .any(|c: &Citation| c.document_id == document_id && c.chunk_id == chunk_id)
        {
            continue;
        }
        out.push(Citation {
            document_id,
            chunk_id,
            score,
        });
    }
    out
}

pub fn cite_answer(answer: &str, sources: &[Citation]) -> String {
    serde_json::json!({
        "answer": answer,
        "sources": citations_json(sources),
    })
    .to_string()
}

fn citations_json(sources: &[Citation]) -> serde_json::Value {
    serde_json::Value::Array(
        sources
            .iter()
            .map(|c| {
                serde_json::json!({
                    "document_id": c.document_id,
                    "chunk_id": c.chunk_id,
                    "score": c.score,
                })
            })
            .collect(),
    )
}

fn column_index(hits: &aidb_core::QueryResult, name: &str) -> Option<usize> {
    hits.columns.iter().position(|c| c == name)
}

fn value_f64(value: &aidb_core::Value) -> f64 {
    match value {
        aidb_core::Value::Real(v) => *v,
        aidb_core::Value::Integer(v) => *v as f64,
        aidb_core::Value::Text(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn present_generate(text: String, sources: Option<&[Citation]>) -> String {
    match sources {
        Some(sources) => cite_answer(&text, sources),
        None => text,
    }
}

fn apply_schema(schema: Option<&serde_json::Value>, text: String) -> Result<String, String> {
    let Some(schema) = schema else {
        return Ok(text);
    };
    let value = schema::extract_json(&text)?;
    schema::validate(schema, &value)?;
    Ok(value.to_string())
}

fn with_schema_prompt(prompt: &str, schema: Option<&str>) -> String {
    match schema {
        Some(json) => format!("{prompt}{}{json}", aidb_ai::JSON_SCHEMA_MARK),
        None => prompt.to_string(),
    }
}

fn fail_schema(
    conn: &Connection,
    run_id: &str,
    message: &str,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    cost_usd: Option<f64>,
) -> Result<String> {
    let err = format!("output did not match schema: {message}");
    aidb_run::complete_generate_run(
        conn,
        run_id,
        "failed",
        None,
        Some(&err),
        prompt_tokens,
        completion_tokens,
        cost_usd,
    )?;
    aidb_run::append_event(conn, run_id, "failed", Some(&err))?;
    Err(Error::usage(err))
}

fn generate_output_json(text: &str, cache: bool, sources: Option<&[Citation]>) -> String {
    let mut value = if cache {
        serde_json::json!({ "text": text, "cache": true, "job": job_snapshot() })
    } else {
        serde_json::json!({ "text": text, "job": job_snapshot() })
    };
    if let Some(sources) = sources {
        value["sources"] = citations_json(sources);
    }
    value.to_string()
}

fn generate(prompt: &str, content: &str) -> Result<String> {
    generate_text(current_connection()?, prompt, content, None)
}

fn generate_schema(prompt: &str, content: &str, schema: &str) -> Result<String> {
    generate_task(
        current_connection()?,
        prompt,
        content,
        None,
        None,
        false,
        Some(schema),
    )
}

pub fn generate_text(
    conn: &Connection,
    prompt: &str,
    content: &str,
    parent_id: Option<&str>,
) -> Result<String> {
    generate_with_sources(conn, prompt, content, parent_id, None)
}

pub fn classify_text(
    conn: &Connection,
    labels: &str,
    content: &str,
    parent_id: Option<&str>,
) -> Result<String> {
    classify_text_in(conn, labels, content, parent_id, None)
}

pub fn classify_text_in(
    conn: &Connection,
    labels: &str,
    content: &str,
    parent_id: Option<&str>,
    schema: Option<&str>,
) -> Result<String> {
    generate_task(conn, labels, content, parent_id, None, true, schema)
}

pub fn generate_with_sources(
    conn: &Connection,
    prompt: &str,
    content: &str,
    parent_id: Option<&str>,
    sources: Option<&[Citation]>,
) -> Result<String> {
    generate_task(conn, prompt, content, parent_id, sources, false, None)
}

pub fn generate_with_schema(
    conn: &Connection,
    prompt: &str,
    content: &str,
    parent_id: Option<&str>,
    sources: Option<&[Citation]>,
    schema: Option<&str>,
) -> Result<String> {
    generate_task(conn, prompt, content, parent_id, sources, false, schema)
}

fn generate_task(
    conn: &Connection,
    prompt: &str,
    content: &str,
    parent_id: Option<&str>,
    sources: Option<&[Citation]>,
    classify: bool,
    schema: Option<&str>,
) -> Result<String> {
    let schema_value = match schema {
        Some(json) => Some(schema::parse_schema(json)?),
        None => None,
    };
    let budget = resolve_budget(conn);
    JOB.with(|j| {
        let mut job = j.borrow_mut();
        if job.started.is_none() {
            job.started = Some(Instant::now());
        }
        job.budget = Some(budget);
    });
    let ctx = bind_context(conn)?;
    let plan = LogicalPlan::generate(prompt, LlmContent::Literal(content.to_string()), None, None);
    plan.bind(&ctx)?;
    let physical = plan.to_physical(&ctx);
    let (provider, model) = physical
        .llm_binding()
        .unwrap_or_else(|| ("fake".into(), "aidb-fake".into()));
    let name = ctx.llm_catalog_name().unwrap_or("");
    let key_name = ctx.llm_key_name();
    let run_id = new_id("run");
    let input = if classify {
        serde_json::json!({
            "task": "classify",
            "labels": prompt,
            "content": content,
            "schema": schema_value,
        })
        .to_string()
    } else {
        serde_json::json!({
            "prompt": prompt,
            "content": content,
            "schema": schema_value,
        })
        .to_string()
    };
    let model_name = if name.is_empty() { None } else { Some(name) };

    aidb_run::insert_generate_run(
        conn, &run_id, model_name, &input, None, "running", None, None, None, None, parent_id,
    )?;
    aidb_run::append_event(conn, &run_id, "started", None)?;

    ensure_llm_cache(conn)?;
    let mut cache_input = format!(
        "{provider}\0{model}\0{}\0{prompt}\0{content}",
        if classify { "classify" } else { "generate" },
    );
    if let Some(schema) = schema {
        cache_input.push('\0');
        cache_input.push_str(schema);
    }
    let cache_key = content_hash(&cache_input);
    if let Some((text, prompt_tokens, completion_tokens, cost_usd)) = cache_get(conn, &cache_key)? {
        check_ms(&active_budget())?;
        match apply_schema(schema_value.as_ref(), text) {
            Ok(text) => {
                let output = generate_output_json(&text, true, sources);
                aidb_run::complete_generate_run(
                    conn,
                    &run_id,
                    "succeeded",
                    Some(&output),
                    None,
                    prompt_tokens,
                    completion_tokens,
                    cost_usd,
                )?;
                aidb_run::put_checkpoint(conn, &run_id, "generate", Some(&output))?;
                aidb_run::append_event(conn, &run_id, "cache_hit", Some(&cache_key))?;
                return Ok(present_generate(text, sources));
            }
            Err(message) => {
                return fail_schema(
                    conn,
                    &run_id,
                    &message,
                    prompt_tokens,
                    completion_tokens,
                    cost_usd,
                );
            }
        }
    }

    charge_llm_call()?;
    aidb_core::crash_point("before_llm");
    let llm_prompt = with_schema_prompt(prompt, schema);
    match llm_with_key(&provider, &model, key_name).and_then(|client| {
        if classify && schema.is_none() {
            client.classify(prompt, content)
        } else {
            client.complete_streaming(&llm_prompt, content, &mut |delta| {
                aidb_run::append_token(conn, &run_id, delta)?;
                aidb_core::crash_point("after_token");
                Ok(())
            })
        }
    }) {
        Ok(completion) => {
            aidb_core::crash_point("after_llm");
            if let Err(err) = charge_completion(
                completion.cost_usd,
                completion.prompt_tokens,
                completion.completion_tokens,
            ) {
                aidb_run::complete_generate_run(
                    conn,
                    &run_id,
                    "failed",
                    None,
                    Some(&err.to_string()),
                    completion.prompt_tokens,
                    completion.completion_tokens,
                    completion.cost_usd,
                )?;
                aidb_run::append_event(conn, &run_id, "failed", Some(&err.to_string()))?;
                return Err(err);
            }
            let output = generate_output_json(&completion.text, false, sources);
            match apply_schema(schema_value.as_ref(), completion.text.clone()) {
                Ok(text) => {
                    let output = generate_output_json(&text, false, sources);
                    aidb_run::complete_generate_run(
                        conn,
                        &run_id,
                        "succeeded",
                        Some(&output),
                        None,
                        completion.prompt_tokens,
                        completion.completion_tokens,
                        completion.cost_usd,
                    )?;
                    aidb_run::put_checkpoint(conn, &run_id, "generate", Some(&output))?;
                    aidb_run::append_event(conn, &run_id, "generated", None)?;
                    cache_put(
                        conn,
                        &cache_key,
                        &text,
                        completion.prompt_tokens,
                        completion.completion_tokens,
                        completion.cost_usd,
                    )?;
                    Ok(present_generate(text, sources))
                }
                Err(message) => {
                    let err = format!("output did not match schema: {message}");
                    let mut value = serde_json::from_str::<serde_json::Value>(&output)
                        .unwrap_or_else(|_| serde_json::json!({ "text": completion.text }));
                    value["schema_error"] = serde_json::Value::String(err.clone());
                    let output = value.to_string();
                    aidb_run::complete_generate_run(
                        conn,
                        &run_id,
                        "failed",
                        Some(&output),
                        Some(&err),
                        completion.prompt_tokens,
                        completion.completion_tokens,
                        completion.cost_usd,
                    )?;
                    aidb_run::append_event(conn, &run_id, "failed", Some(&err))?;
                    Err(Error::usage(err))
                }
            }
        }
        Err(err) => {
            aidb_run::complete_generate_run(
                conn,
                &run_id,
                "failed",
                None,
                Some(&err.to_string()),
                None,
                None,
                None,
            )?;
            aidb_run::append_event(conn, &run_id, "failed", Some(&err.to_string()))?;
            Err(err)
        }
    }
}

fn charge_llm_call() -> Result<()> {
    let budget = active_budget();
    check_ms(&budget)?;
    JOB.with(|j| {
        let mut job = j.borrow_mut();
        if job.started.is_none() {
            job.started = Some(Instant::now());
        }
        let n = job.llm_calls.saturating_add(1);
        if let Some(max) = budget.max_llm_calls {
            if n > max {
                return Err(Error::usage(format!(
                    "budget exceeded: {n} LLM calls > max_llm_calls={max}"
                )));
            }
        }
        job.llm_calls = n;
        Ok(())
    })
}

fn charge_completion(
    cost: Option<f64>,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
) -> Result<()> {
    let budget = active_budget();
    check_ms(&budget)?;
    JOB.with(|j| {
        let mut job = j.borrow_mut();
        job.usd += cost.unwrap_or(0.0);
        job.prompt_tokens += prompt_tokens.unwrap_or(0);
        job.completion_tokens += completion_tokens.unwrap_or(0);
        if let Some(max) = budget.max_usd {
            if job.usd > max + f64::EPSILON {
                return Err(Error::usage(format!(
                    "budget exceeded: ${:.8} > max_usd={max}",
                    job.usd
                )));
            }
        }
        Ok(())
    })
}

fn check_ms(budget: &aidb_opt::Budget) -> Result<()> {
    let Some(max) = budget.max_ms else {
        return Ok(());
    };
    JOB.with(|j| {
        let job = j.borrow();
        let Some(started) = job.started else {
            return Ok(());
        };
        let elapsed = started.elapsed().as_millis() as u64;
        if max == 0 || elapsed > max {
            return Err(Error::usage(format!(
                "budget exceeded: {elapsed}ms > max_ms={max}"
            )));
        }
        Ok(())
    })
}

fn ensure_llm_cache(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS aidb_llm_cache (
            cache_key TEXT PRIMARY KEY,
            output TEXT NOT NULL,
            prompt_tokens INTEGER,
            completion_tokens INTEGER,
            cost_usd REAL,
            created_at_ms INTEGER NOT NULL
        )",
    )
    .map_err(sqlite_err)
}

#[allow(clippy::type_complexity)]
fn cache_get(
    conn: &Connection,
    key: &str,
) -> Result<Option<(String, Option<i64>, Option<i64>, Option<f64>)>> {
    match conn.query_row(
        "SELECT output, prompt_tokens, completion_tokens, cost_usd
         FROM aidb_llm_cache WHERE cache_key = ?1",
        [key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ) {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(sqlite_err(err)),
    }
}

fn cache_put(
    conn: &Connection,
    key: &str,
    output: &str,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    cost_usd: Option<f64>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO aidb_llm_cache
            (cache_key, output, prompt_tokens, completion_tokens, cost_usd, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            key,
            output,
            prompt_tokens,
            completion_tokens,
            cost_usd,
            now_ms()
        ],
    )
    .map_err(sqlite_err)?;
    Ok(())
}

pub fn logical_plan(sql: &str) -> Option<LogicalPlan> {
    if looks_like_goal(sql) {
        return parse_goal_sql(sql).ok().map(|spec| spec.to_logical());
    }
    if let Some(json) = parse_aidb_agent(sql) {
        return parse_agent(&json).ok().map(|spec| spec.to_logical());
    }
    if let Some(json) = parse_aidb_workflow(sql) {
        return parse_workflow(&json).ok().map(|wf| wf.to_logical());
    }
    if let Some(call) = parse_aidb_generate(sql).or_else(|| parse_aidb_classify(sql)) {
        return Some(match call.from {
            Some(GenerateFrom::Search {
                query,
                k,
                filter,
                space,
            }) => LogicalPlan::generate_over_search_in(
                call.prompt,
                query,
                k,
                filter.as_deref(),
                space.as_deref(),
            ),
            Some(GenerateFrom::Table { name, filter }) => {
                let llm_content = if is_ident(&call.content) {
                    LlmContent::Column(call.content)
                } else {
                    LlmContent::Literal(call.content)
                };
                LogicalPlan::generate_naive(call.prompt, llm_content, &name, filter.as_deref())
            }
            None => {
                LogicalPlan::generate(call.prompt, LlmContent::Literal(call.content), None, None)
            }
        });
    }
    if let Some((query, k, scope)) = parse_aidb_memory_search(sql) {
        let filter = match scope.as_deref() {
            Some(scope) => memory_metadata(scope),
            None => serde_json::json!({ "kind": "memory" }).to_string(),
        };
        return Some(LogicalPlan::search_filtered(query, k, Some(&filter)));
    }
    if let Some((query, k, filter)) = parse_search_dialect(sql) {
        return Some(LogicalPlan::search_filtered(query, k, filter.as_deref()));
    }
    if let Some((query, k, filter, space)) = parse_aidb_search(sql) {
        return Some(LogicalPlan::search_in(
            query,
            k,
            filter.as_deref(),
            space.as_deref(),
        ));
    }
    None
}

pub fn strip_explain(sql: &str) -> Option<&str> {
    let trimmed = sql.trim();
    let trimmed = trimmed.strip_suffix(';').unwrap_or(trimmed).trim();
    if trimmed.len() < 7 || !trimmed[..7].eq_ignore_ascii_case("explain") {
        return None;
    }
    let rest = trimmed[7..].trim_start();
    if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case("query") {
        return None;
    }
    Some(rest)
}

pub fn parse_aidb_explain(sql: &str) -> Option<String> {
    let (_, _, args) = parse_call(sql, "aidb_explain")?;
    if args.len() != 1 {
        return None;
    }
    Some(args[0].clone())
}

pub fn parse_aidb_agent(sql: &str) -> Option<String> {
    let (_, _, args) = parse_call(sql, "aidb_agent")?;
    match args.len() {
        1 => Some(args[0].clone()),
        2 => Some(agent_from_parts(&args[0], &args[1])),
        _ => None,
    }
}

fn agent_from_parts(text: &str, tools: &str) -> String {
    let tools = serde_json::from_str::<serde_json::Value>(tools)
        .unwrap_or_else(|_| serde_json::json!([tools]));
    serde_json::json!({
        "instructions": text,
        "goal": text,
        "tools": tools,
        "max_steps": 2
    })
    .to_string()
}

pub fn parse_aidb_experiment(sql: &str) -> Option<String> {
    let (_, _, args) = parse_call(sql, "aidb_experiment")?;
    match args.len() {
        1 => Some(args[0].clone()),
        2 => Some(serde_json::json!({ "dataset": args[0], "plans": [args[1]] }).to_string()),
        _ => None,
    }
}

/// The plans an experiment can compare. Named, because "plan A vs plan B" has to be
/// a value you can store and query, and because an experiment must run the plan the
/// engine really uses — not a lookalike written for the benchmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanName {
    /// Row-wise generate over every ready document: one model call per row. This is
    /// the plan the optimizer rewrites away, so the experiment runs it unrewritten.
    Naive,
    /// Retrieve the top k, then answer from what came back. The rewrite, priced.
    Cascade,
    /// Retrieval only, no model call: the floor on cost and the ceiling on speed.
    Search,
}

impl PlanName {
    pub fn parse(name: &str) -> Result<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "naive" => Ok(Self::Naive),
            "cascade" => Ok(Self::Cascade),
            "search" | "retrieval" => Ok(Self::Search),
            other => Err(Error::usage(format!(
                "unknown plan: {other}; known plans are naive, cascade, search"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Naive => "naive",
            Self::Cascade => "cascade",
            Self::Search => "search",
        }
    }

    /// Whether the plan produces an answer. Retrieval does not, so it belongs in the
    /// comparison as the price floor but never as the plan that won it.
    pub fn answers(self) -> bool {
        !matches!(self, Self::Search)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentSpec {
    pub dataset: String,
    pub plans: Vec<PlanName>,
    pub k: i64,
    pub prompt: String,
}

pub fn parse_experiment(json: &str) -> Result<ExperimentSpec> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|err| Error::usage(format!("experiment JSON: {err}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::usage("experiment spec must be a JSON object"))?;
    let dataset = obj
        .get("dataset")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::usage("experiment.dataset is required"))?
        .to_string();
    let plans = match obj.get("plans") {
        Some(serde_json::Value::Array(items)) if !items.is_empty() => {
            let mut plans = Vec::new();
            for item in items {
                let name = item
                    .as_str()
                    .ok_or_else(|| Error::usage("experiment.plans must be strings"))?;
                let plan = PlanName::parse(name)?;
                if !plans.contains(&plan) {
                    plans.push(plan);
                }
            }
            plans
        }
        Some(serde_json::Value::String(name)) => vec![PlanName::parse(name)?],
        None => vec![PlanName::Naive, PlanName::Cascade],
        _ => {
            return Err(Error::usage(
                "experiment.plans must be a list of plan names",
            ))
        }
    };
    if plans.len() < 2 {
        return Err(Error::usage(
            "an experiment compares plans: name at least two, for example [\"naive\",\"cascade\"]",
        ));
    }
    let k = obj
        .get("k")
        .and_then(|v| v.as_i64())
        .unwrap_or(5)
        .clamp(1, 4096);
    let prompt = obj
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("Answer only from the sources.")
        .to_string();
    Ok(ExperimentSpec {
        dataset,
        plans,
        k,
        prompt,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub instructions: String,
    pub goal: String,
    pub tools: Vec<String>,
    pub max_steps: u32,
    pub k: i64,
    pub memory: Option<String>,
    pub agents: Vec<AgentSpec>,
    /// When true, each step is a schema-valid choose-and-act instead of the
    /// listed tools in order. Recipe agents stay the default.
    pub decide: bool,
    /// Optional thread name. Stamped on the agent run and inherited by children.
    /// Not a second store — `session_turns` is a view over `runs`.
    pub session: Option<String>,
}

impl AgentSpec {
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t == name)
    }

    pub fn to_logical(&self) -> aidb_ir::LogicalPlan {
        use aidb_ir::Workflow;
        if self.decide {
            return Workflow::Decide {
                tools: self.tools.clone(),
                max: self.max_steps,
            }
            .to_logical();
        }
        let mut steps = Vec::new();
        if self.has_tool("search") {
            steps.push(Workflow::Search {
                query: self.goal.clone(),
                k: self.k,
            });
        }
        for tool in &self.tools {
            if tool != "search" && tool != "generate" {
                steps.push(Workflow::Tool { name: tool.clone() });
            }
        }
        if self.has_tool("generate") {
            steps.push(Workflow::Loop {
                body: Box::new(Workflow::Generate {
                    prompt: self.instructions.clone(),
                    content: None,
                }),
                until: "done".into(),
                max: self.max_steps,
            });
        }
        for child in &self.agents {
            steps.push(Workflow::Then(child_steps(child)));
        }
        if steps.is_empty() {
            steps.push(Workflow::Generate {
                prompt: self.instructions.clone(),
                content: Some(self.goal.clone()),
            });
        }
        Workflow::Then(steps).to_logical()
    }

    pub fn decide_ops(&self) -> Vec<String> {
        let mut ops = self.tools.clone();
        if !ops.iter().any(|name| name == "stop") {
            ops.push("stop".into());
        }
        ops
    }

    pub fn decide_schema(&self) -> String {
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "enum": self.decide_ops() },
                "args": { "type": "object" }
            },
            "required": ["op"]
        })
        .to_string()
    }
}

pub fn parse_agent(json: &str) -> Result<AgentSpec> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|err| aidb_core::Error::usage(format!("agent JSON: {err}")))?;
    parse_agent_value(&value, false)
}

fn parse_agent_value(value: &serde_json::Value, nested: bool) -> Result<AgentSpec> {
    let obj = value
        .as_object()
        .ok_or_else(|| aidb_core::Error::usage("agent spec must be a JSON object"))?;
    let instructions = obj
        .get("instructions")
        .and_then(|v| v.as_str())
        .ok_or_else(|| aidb_core::Error::usage("agent.instructions is required"))?
        .to_string();
    let goal = obj
        .get("goal")
        .and_then(|v| v.as_str())
        .ok_or_else(|| aidb_core::Error::usage("agent.goal is required"))?
        .to_string();
    let tools = match obj.get("tools") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(ToOwned::to_owned))
            .collect(),
        _ => vec!["search".into(), "generate".into()],
    };
    let max_steps = obj
        .get("max_steps")
        .and_then(|v| v.as_u64())
        .unwrap_or(4)
        .clamp(1, 16) as u32;
    let k = obj.get("k").and_then(|v| v.as_i64()).unwrap_or(5).max(1);
    let memory = obj
        .get("memory")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let agents = match obj.get("agents") {
        Some(serde_json::Value::Array(items)) if !nested => items
            .iter()
            .map(|item| parse_agent_value(item, true))
            .collect::<Result<Vec<_>>>()?,
        _ => Vec::new(),
    };
    let decide = obj.get("decide").and_then(|v| v.as_bool()).unwrap_or(false);
    let session = match obj.get("session").and_then(|v| v.as_str()) {
        Some(name) if !name.trim().is_empty() => Some(aidb_run::validate_session_id(name)?),
        _ => None,
    };
    Ok(AgentSpec {
        instructions,
        goal,
        tools,
        max_steps,
        k,
        memory,
        agents,
        decide,
        session,
    })
}

fn child_steps(spec: &AgentSpec) -> Vec<aidb_ir::Workflow> {
    use aidb_ir::Workflow;
    let mut steps = Vec::new();
    if spec.has_tool("search") {
        steps.push(Workflow::Search {
            query: spec.goal.clone(),
            k: spec.k,
        });
    }
    if spec.has_tool("generate") {
        steps.push(Workflow::Generate {
            prompt: spec.instructions.clone(),
            content: Some(spec.goal.clone()),
        });
    }
    if steps.is_empty() {
        steps.push(Workflow::Generate {
            prompt: spec.instructions.clone(),
            content: Some(spec.goal.clone()),
        });
    }
    steps
}

pub fn parse_aidb_workflow(sql: &str) -> Option<String> {
    let (_, _, args) = parse_call(sql, "aidb_workflow")?;
    if args.len() != 1 {
        return None;
    }
    Some(args[0].clone())
}

pub fn parse_aidb_resume(sql: &str) -> Option<(String, String)> {
    let (_, _, args) = parse_call(sql, "aidb_resume")?;
    if args.len() != 2 {
        return None;
    }
    Some((args[0].clone(), args[1].clone()))
}

pub fn parse_aidb_mcp_connect(sql: &str) -> Option<(String, String)> {
    let (_, _, args) = parse_call(sql, "aidb_mcp_connect")?;
    if args.len() != 2 {
        return None;
    }
    Some((args[0].clone(), args[1].clone()))
}

pub fn parse_aidb_mcp_disconnect(sql: &str) -> Option<()> {
    let (_, _, args) = parse_call(sql, "aidb_mcp_disconnect")?;
    if args.is_empty() {
        Some(())
    } else {
        None
    }
}

pub fn parse_aidb_mcp_register(sql: &str) -> Option<String> {
    let (_, _, args) = parse_call(sql, "aidb_mcp_register")?;
    if args.len() != 1 {
        return None;
    }
    Some(args[0].clone())
}

pub fn parse_aidb_tool(sql: &str) -> Option<(String, String)> {
    let (_, _, args) = parse_call(sql, "aidb_tool")?;
    if args.len() != 2 {
        return None;
    }
    Some((args[0].clone(), args[1].clone()))
}

pub fn parse_aidb_set_policy(sql: &str) -> Option<(String, Option<String>)> {
    let (_, _, args) = parse_call(sql, "aidb_set_policy")?;
    match args.len() {
        1 => Some((args[0].clone(), None)),
        2 => Some((args[1].clone(), Some(args[0].clone()))),
        _ => None,
    }
}

pub fn parse_aidb_get_policy(sql: &str) -> Option<()> {
    let (_, _, args) = parse_call(sql, "aidb_get_policy")?;
    if args.is_empty() {
        Some(())
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCall {
    Get,
    Bind(String),
    Clear,
}

pub fn parse_aidb_session(sql: &str) -> Option<SessionCall> {
    let (_, _, args) = parse_call(sql, "aidb_session")?;
    match args.len() {
        0 => Some(SessionCall::Get),
        1 if args[0].eq_ignore_ascii_case("null") => Some(SessionCall::Clear),
        1 => Some(SessionCall::Bind(args[0].clone())),
        _ => None,
    }
}

pub fn parse_aidb_last_run_id(sql: &str) -> Option<()> {
    let (_, _, args) = parse_call(sql, "aidb_last_run_id")?;
    if args.is_empty() {
        Some(())
    } else {
        None
    }
}

pub fn parse_aidb_secret_store(sql: &str) -> Option<()> {
    let (_, _, args) = parse_call(sql, "aidb_secret_store")?;
    if args.is_empty() {
        Some(())
    } else {
        None
    }
}

pub fn parse_aidb_memory_insert(sql: &str) -> Option<(String, String)> {
    let (_, _, args) = parse_call(sql, "aidb_memory_insert")?;
    if args.len() != 2 {
        return None;
    }
    Some((args[0].clone(), args[1].clone()))
}

pub fn parse_aidb_memory_search(sql: &str) -> Option<(String, i64, Option<String>)> {
    let (_, _, args) = parse_call(sql, "aidb_memory_search")?;
    if args.len() < 2 || args.len() > 3 {
        return None;
    }
    let k = args[1].parse().unwrap_or(5).max(1);
    let scope = args.get(2).cloned().filter(|s| !s.is_empty());
    Some((args[0].clone(), k, scope))
}

pub fn memory_metadata(scope: &str) -> String {
    serde_json::json!({ "kind": "memory", "scope": scope }).to_string()
}

pub fn parse_workflow(json: &str) -> Result<aidb_ir::Workflow> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|err| aidb_core::Error::usage(format!("workflow JSON: {err}")))?;
    parse_workflow_value(&value)
}

fn parse_workflow_value(value: &serde_json::Value) -> Result<aidb_ir::Workflow> {
    use aidb_core::Error;
    use aidb_ir::Workflow;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::usage("workflow node must be a JSON object"))?;
    if let Some(steps) = obj.get("then") {
        return Ok(Workflow::Then(parse_workflow_list(steps)?));
    }
    if let Some(steps) = obj.get("parallel") {
        return Ok(Workflow::Parallel(parse_workflow_list(steps)?));
    }
    if let Some(branch) = obj.get("branch") {
        let branch = branch
            .as_object()
            .ok_or_else(|| Error::usage("branch must be an object"))?;
        let when = branch
            .get("when")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::usage("branch.when is required"))?
            .to_string();
        let then = parse_workflow_value(
            branch
                .get("then")
                .ok_or_else(|| Error::usage("branch.then is required"))?,
        )?;
        let else_ = match branch.get("else") {
            Some(value) => Some(Box::new(parse_workflow_value(value)?)),
            None => None,
        };
        return Ok(Workflow::Branch {
            when,
            then: Box::new(then),
            else_,
        });
    }
    if let Some(loop_) = obj.get("loop") {
        let loop_ = loop_
            .as_object()
            .ok_or_else(|| Error::usage("loop must be an object"))?;
        let body = parse_workflow_value(
            loop_
                .get("body")
                .ok_or_else(|| Error::usage("loop.body is required"))?,
        )?;
        let until = loop_
            .get("until")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let max = loop_
            .get("max")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .clamp(1, 32) as u32;
        return Ok(Workflow::Loop {
            body: Box::new(body),
            until,
            max,
        });
    }
    if let Some(search) = obj.get("search") {
        let search = search
            .as_object()
            .ok_or_else(|| Error::usage("search must be an object"))?;
        let query = search
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::usage("search.query is required"))?
            .to_string();
        let k = search.get("k").and_then(|v| v.as_i64()).unwrap_or(5);
        return Ok(Workflow::Search { query, k });
    }
    if let Some(generate) = obj.get("generate") {
        let generate = generate
            .as_object()
            .ok_or_else(|| Error::usage("generate must be an object"))?;
        let prompt = generate
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::usage("generate.prompt is required"))?
            .to_string();
        let content = generate
            .get("content")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        return Ok(Workflow::Generate { prompt, content });
    }
    if let Some(tool) = obj.get("tool") {
        let name = match tool {
            serde_json::Value::String(name) => name.clone(),
            serde_json::Value::Object(obj) => obj
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::usage("tool.name is required"))?
                .to_string(),
            _ => return Err(Error::usage("tool must be a name or an object")),
        };
        if name.trim().is_empty() {
            return Err(Error::usage("tool.name is required"));
        }
        return Ok(Workflow::Tool { name });
    }
    if let Some(approve) = obj.get("approve") {
        return Ok(Workflow::Pause {
            status: "awaiting_approval".into(),
            message: pause_message(approve, "approval required"),
        });
    }
    if let Some(wait) = obj.get("wait") {
        return Ok(Workflow::Pause {
            status: "suspended".into(),
            message: pause_message(wait, "suspended"),
        });
    }
    Err(Error::usage(
        "workflow node must be then, parallel, branch, loop, search, generate, tool, approve, or wait",
    ))
}

fn pause_message(value: &serde_json::Value, default: &str) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Object(obj) => obj
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string(),
        _ => default.to_string(),
    }
}

fn parse_workflow_list(value: &serde_json::Value) -> Result<Vec<aidb_ir::Workflow>> {
    let arr = value
        .as_array()
        .ok_or_else(|| aidb_core::Error::usage("expected a JSON array of steps"))?;
    arr.iter().map(parse_workflow_value).collect()
}

pub fn parse_aidb_search(sql: &str) -> Option<(String, i64, Option<String>, Option<String>)> {
    let (_, _, args) = parse_call(sql, "aidb_search")?;
    if args.len() < 2 || args.len() > 4 {
        return None;
    }
    let k: i64 = args[1].parse().ok()?;
    let filter = args
        .get(2)
        .cloned()
        .and_then(|raw| aidb_ir::normalize_metadata_filter(Some(&raw)));
    let space = args.get(3).cloned().and_then(|raw| {
        let raw = raw.trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("null") || raw == "default" {
            None
        } else {
            Some(raw.to_string())
        }
    });
    Some((args[0].clone(), k.max(1), filter, space))
}

#[allow(clippy::type_complexity)]
pub fn parse_aidb_create_space(
    sql: &str,
) -> Option<(String, String, i64, Option<String>, Option<String>)> {
    let (_, _, args) = parse_call(sql, "aidb_create_space")?;
    if args.len() < 3 || args.len() > 5 {
        return None;
    }
    let dimensions: i64 = args[2].parse().ok()?;
    let model = args.get(3).cloned().filter(|m| !m.is_empty());
    let distance = args.get(4).cloned().filter(|d| !d.is_empty());
    Some((
        args[0].clone(),
        args[1].clone(),
        dimensions,
        model,
        distance,
    ))
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenerateFrom {
    Table {
        name: String,
        filter: Option<String>,
    },
    Search {
        query: String,
        k: i64,
        filter: Option<String>,
        space: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmCall {
    pub prompt: String,
    pub content: String,
    pub schema: Option<String>,
    pub from: Option<GenerateFrom>,
}

pub fn parse_aidb_generate(sql: &str) -> Option<LlmCall> {
    parse_llm_call(sql, &["aidb_generate", "ai_generate"])
}

pub fn parse_aidb_classify(sql: &str) -> Option<LlmCall> {
    parse_llm_call(sql, &["aidb_classify"])
}

fn parse_llm_call(sql: &str, names: &[&str]) -> Option<LlmCall> {
    for name in names {
        if let Some((_, end, args)) = parse_call(sql, name) {
            if args.len() == 2 || args.len() == 3 {
                return Some(LlmCall {
                    prompt: args[0].clone(),
                    content: args[1].clone(),
                    schema: args.get(2).cloned(),
                    from: parse_generate_from(&sql[end..]),
                });
            }
        }
    }
    None
}

/// Apply the statement's column list to a retrieval result.
///
/// `SELECT document_id, content FROM aidb_search(...)` has to hand back two
/// columns in that order: a caller reading by position must not get `chunk_id`
/// where it asked for `content`. `*`, expressions and aliases are not projected
/// — they get every column the retrieval produced.
pub fn project_selection(
    sql: &str,
    result: aidb_core::QueryResult,
) -> Result<aidb_core::QueryResult> {
    let Some(names) = select_columns(sql) else {
        return Ok(result);
    };
    let mut picked = Vec::with_capacity(names.len());
    for name in &names {
        let index = result
            .columns
            .iter()
            .position(|column| column.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                Error::usage(format!(
                    "unknown column {name}; retrieval returns {}",
                    result.columns.join(", ")
                ))
            })?;
        picked.push(index);
    }
    Ok(aidb_core::QueryResult {
        columns: picked.iter().map(|&i| result.columns[i].clone()).collect(),
        rows: result
            .rows
            .iter()
            .map(|row| picked.iter().map(|&i| row[i].clone()).collect())
            .collect(),
    })
}

/// The bare column names in `SELECT <list> FROM`, or `None` when the list is
/// `*` or holds anything that is not a plain column name.
fn select_columns(sql: &str) -> Option<Vec<String>> {
    let trimmed = sql.trim_start();
    if !trimmed
        .get(..6)
        .is_some_and(|word| word.eq_ignore_ascii_case("select"))
    {
        return None;
    }
    let list = split_before_from(&trimmed[6..])?;
    let mut names = Vec::new();
    for item in list.split(',') {
        let item = item.trim();
        if item.is_empty() || !is_bare_column(item) {
            return None;
        }
        names.push(item.to_string());
    }
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

fn split_before_from(sql: &str) -> Option<&str> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
            }
            _ if depth == 0 && bytes[i].is_ascii_whitespace() && starts_with_from(&sql[i..]) => {
                return Some(&sql[..i]);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn starts_with_from(sql: &str) -> bool {
    let rest = sql.trim_start();
    rest.get(..4)
        .is_some_and(|word| word.eq_ignore_ascii_case("from"))
        && rest[4..]
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '(')
}

fn is_bare_column(item: &str) -> bool {
    !item.is_empty()
        && !item.starts_with(|c: char| c.is_ascii_digit())
        && item.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn parse_aidb_insert_document(sql: &str) -> Option<(String, String, String)> {
    let (_, _, args) = parse_call(sql, "aidb_insert_document")?;
    if args.len() != 3 {
        return None;
    }
    Some((args[0].clone(), args[1].clone(), args[2].clone()))
}

fn parse_generate_from(sql: &str) -> Option<GenerateFrom> {
    let rest = sql.trim_start();
    if rest.len() < 4 || !rest[..4].eq_ignore_ascii_case("from") {
        return None;
    }
    let after = rest[4..].trim_start();
    if let Some((_, _, args)) = parse_call(after, "aidb_search") {
        if (2..=4).contains(&args.len()) {
            if let Ok(k) = args[1].parse::<i64>() {
                let filter = args
                    .get(2)
                    .cloned()
                    .and_then(|raw| aidb_ir::normalize_metadata_filter(Some(&raw)));
                let space = args.get(3).cloned().and_then(|raw| {
                    let raw = raw.trim();
                    if raw.is_empty() || raw.eq_ignore_ascii_case("null") || raw == "default" {
                        None
                    } else {
                        Some(raw.to_string())
                    }
                });
                return Some(GenerateFrom::Search {
                    query: args[0].clone(),
                    k: k.max(1),
                    filter,
                    space,
                });
            }
        }
    }
    let table: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if table.is_empty() {
        return None;
    }
    let after_table = after[table.len()..].trim_start();
    let filter = if after_table.len() >= 5 && after_table[..5].eq_ignore_ascii_case("where") {
        let pred = after_table[5..].trim().trim_end_matches(';').trim();
        if pred.is_empty() {
            None
        } else {
            Some(pred.to_string())
        }
    } else {
        None
    };
    Some(GenerateFrom::Table {
        name: table,
        filter,
    })
}

fn is_ident(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(crate) fn parse_call(sql: &str, name: &str) -> Option<(usize, usize, Vec<String>)> {
    let lower = sql.to_ascii_lowercase();
    let needle = format!("{name}(");
    let start = lower.find(&needle)?;
    let mut rest = &sql[start + needle.len()..];
    let mut args = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.starts_with(')') {
            let end = sql.len() - rest.len() + 1;
            return Some((start, end, args));
        }
        if !args.is_empty() {
            rest = rest.strip_prefix(',')?.trim_start();
        }
        let (value, next) = parse_arg(rest)?;
        args.push(value);
        rest = next;
    }
}

fn parse_arg(sql: &str) -> Option<(String, &str)> {
    let mut rest = sql.trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        if quote.is_ascii_digit()
            || (quote == '-' && rest[1..].starts_with(|c: char| c.is_ascii_digit()))
        {
            // Keep the sign: an out-of-range number has to reach the operator so it
            // can name the real problem instead of looking like unknown SQL.
            let number: String = rest
                .chars()
                .take(1)
                .chain(rest[1..].chars().take_while(|c| c.is_ascii_digit()))
                .collect();
            return Some((number.clone(), rest[number.len()..].trim_start()));
        }
        if quote.is_ascii_alphabetic() || quote == '_' {
            let ident: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            return Some((ident.clone(), rest[ident.len()..].trim_start()));
        }
        return None;
    }
    rest = &rest[1..];
    let mut out = String::new();
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == quote as u8 {
            if i + 1 < bytes.len() && bytes[i + 1] == quote as u8 {
                out.push(quote);
                i += 2;
                continue;
            }
            return Some((out, rest[i + 1..].trim_start()));
        }
        out.push(rest[i..].chars().next()?);
        i += rest[i..].chars().next()?.len_utf8();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search() {
        let (query, k, filter, space) = parse_aidb_search(
            "SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5);",
        )
        .expect("parse");
        assert_eq!(query, "How do refunds work?");
        assert_eq!(k, 5);
        assert_eq!(filter, None);
        assert_eq!(space, None);
    }

    #[test]
    fn parses_search_with_metadata_filter() {
        let (query, k, filter, space) = parse_aidb_search(
            r#"SELECT document_id FROM aidb_search('refund policy', 5, '{"dept":"support"}');"#,
        )
        .expect("parse");
        assert_eq!(query, "refund policy");
        assert_eq!(k, 5);
        assert_eq!(filter.as_deref(), Some(r#"{"dept":"support"}"#));
        assert_eq!(space, None);
    }

    #[test]
    fn parses_search_with_space() {
        let (query, k, filter, space) = parse_aidb_search(
            "SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal');",
        )
        .expect("parse");
        assert_eq!(query, "indemnity");
        assert_eq!(k, 5);
        assert_eq!(filter, None);
        assert_eq!(space.as_deref(), Some("legal"));
        let (name, provider, dims, model, distance) =
            parse_aidb_create_space("SELECT aidb_create_space('legal', 'fake', 32);")
                .expect("space");
        assert_eq!(name, "legal");
        assert_eq!(provider, "fake");
        assert_eq!(dims, 32);
        assert_eq!(model, None);
        assert_eq!(distance, None);
        let five = parse_aidb_create_space(
            "SELECT aidb_create_space('legal', 'local', 384, 'BAAI/bge-small-en-v1.5', 'cosine');",
        )
        .expect("local space");
        assert_eq!(five.1, "local");
        assert_eq!(five.2, 384);
        assert_eq!(five.3.as_deref(), Some("BAAI/bge-small-en-v1.5"));
        assert_eq!(five.4.as_deref(), Some("cosine"));
    }

    #[test]
    fn parses_insert() {
        let (title, content, meta) = parse_aidb_insert_document(
            "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days…', '{}');",
        )
        .expect("parse");
        assert_eq!(title, "Refunds");
        assert_eq!(content, "Refunds are issued within 14 days…");
        assert_eq!(meta, "{}");
    }

    #[test]
    fn parses_classify_from_documents() {
        let call = parse_aidb_classify(
            "SELECT aidb_classify('positive or negative', content) FROM documents LIMIT 3;",
        )
        .expect("parse");
        assert_eq!(call.prompt, "positive or negative");
        assert_eq!(call.content, "content");
        assert_eq!(call.schema, None);
        assert_eq!(
            call.from,
            Some(GenerateFrom::Table {
                name: "documents".into(),
                filter: None,
            })
        );
        let plan =
            logical_plan("SELECT aidb_classify('positive or negative', content) FROM documents")
                .expect("plan");
        assert!(matches!(plan.root.op, aidb_ir::LogicalOp::Llm { .. }));
    }

    #[test]
    fn parses_generate_from_documents() {
        let call = parse_aidb_generate(
            "SELECT aidb_generate('Summarize this', content) FROM documents WHERE id = 'doc_1';",
        )
        .expect("parse");
        assert_eq!(call.prompt, "Summarize this");
        assert_eq!(call.content, "content");
        assert_eq!(call.schema, None);
        assert_eq!(
            call.from,
            Some(GenerateFrom::Table {
                name: "documents".into(),
                filter: Some("id = 'doc_1'".into()),
            })
        );
    }

    #[test]
    fn parses_generate_with_a_schema() {
        let call = parse_aidb_generate(
            r#"SELECT aidb_generate('Extract', content, '{"type":"object","required":["ticker"]}') FROM documents;"#,
        )
        .expect("parse");
        assert_eq!(call.prompt, "Extract");
        assert_eq!(call.content, "content");
        assert_eq!(
            call.schema.as_deref(),
            Some(r#"{"type":"object","required":["ticker"]}"#)
        );
        assert_eq!(
            call.from,
            Some(GenerateFrom::Table {
                name: "documents".into(),
                filter: None,
            })
        );
    }

    #[test]
    fn logical_plan_prefers_generate_over_embedded_search() {
        let plan = logical_plan(
            "SELECT aidb_generate('What is the refund policy?', content) FROM aidb_search('refund policy', 5)",
        )
        .expect("plan");
        assert!(matches!(plan.root.op, aidb_ir::LogicalOp::Llm { .. }));
        assert!(!plan.is_search());
        assert_eq!(plan.search_args(), Some(("refund policy".into(), 5)));
    }

    #[test]
    fn parses_generate_from_search() {
        let call = parse_aidb_generate(
            "SELECT aidb_generate('What is the refund policy?', content) FROM aidb_search('refund policy', 5);",
        )
        .expect("parse");
        assert_eq!(call.prompt, "What is the refund policy?");
        assert_eq!(call.content, "content");
        assert_eq!(call.schema, None);
        assert_eq!(
            call.from,
            Some(GenerateFrom::Search {
                query: "refund policy".into(),
                k: 5,
                filter: None,
                space: None,
            })
        );
    }

    #[test]
    fn cite_answer_is_first_class_json() {
        let json = cite_answer(
            "Refunds take 14 days.",
            &[Citation {
                document_id: "doc_123".into(),
                chunk_id: "8".into(),
                score: 0.91,
            }],
        );
        let value: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["answer"], "Refunds take 14 days.");
        assert_eq!(value["sources"][0]["document_id"], "doc_123");
        assert_eq!(value["sources"][0]["chunk_id"], "8");
        assert_eq!(value["sources"][0]["score"], 0.91);
    }

    #[test]
    fn reads_a_plain_column_list_and_ignores_everything_else() {
        assert_eq!(
            select_columns("SELECT document_id, content FROM aidb_search('q', 5)"),
            Some(vec!["document_id".into(), "content".into()])
        );
        // A literal that contains the word `from` is not a FROM clause.
        assert_eq!(
            select_columns("SELECT content FROM aidb_search('who is from paris', 5)"),
            Some(vec!["content".into()])
        );
        for sql in [
            "SELECT * FROM aidb_search('q', 5)",
            "SELECT substr(content, 1, 5) FROM aidb_search('q', 5)",
            "SELECT content AS body FROM aidb_search('q', 5)",
            "SELECT COUNT(*) FROM aidb_search('q', 5)",
            "SELECT content",
            "SEARCH 'q' LIMIT 5",
        ] {
            assert_eq!(select_columns(sql), None, "{sql}");
        }
    }

    #[test]
    fn projects_a_retrieval_by_name_and_names_a_column_that_is_not_there() {
        let hits = aidb_core::QueryResult {
            columns: vec![
                "document_id".into(),
                "chunk_id".into(),
                "content".into(),
                "distance".into(),
            ],
            rows: vec![vec![
                aidb_core::Value::Text("doc_1".into()),
                aidb_core::Value::Integer(7),
                aidb_core::Value::Text("Refunds take 14 days.".into()),
                aidb_core::Value::Real(0.9),
            ]],
        };
        let projected = project_selection(
            "SELECT content, document_id FROM aidb_search('refunds', 1)",
            hits.clone(),
        )
        .expect("project");
        assert_eq!(projected.columns, vec!["content", "document_id"]);
        assert_eq!(
            projected.rows[0][0],
            aidb_core::Value::Text("Refunds take 14 days.".into())
        );

        assert_eq!(
            project_selection("SELECT * FROM aidb_search('refunds', 1)", hits.clone())
                .expect("star")
                .columns,
            hits.columns
        );
        let err = project_selection("SELECT ticker FROM aidb_search('refunds', 1)", hits)
            .expect_err("unknown column");
        assert!(err.to_string().contains("unknown column ticker"), "{err}");
    }

    #[test]
    fn strips_explain_but_not_query_plan() {
        assert_eq!(
            strip_explain("EXPLAIN SELECT aidb_search('q', 5);"),
            Some("SELECT aidb_search('q', 5)")
        );
        assert!(strip_explain("EXPLAIN QUERY PLAN SELECT 1").is_none());
    }

    #[test]
    fn parses_agent_spec() {
        let json = parse_aidb_agent(
            r#"SELECT aidb_agent('{"instructions":"Be brief.","goal":"refunds","tools":["search"],"max_steps":2}');"#,
        )
        .expect("parse call");
        let spec = parse_agent(&json).expect("parse json");
        assert_eq!(spec.goal, "refunds");
        assert!(spec.has_tool("search"));
        assert_eq!(spec.max_steps, 2);
        assert!(!spec.decide);
        let decide = parse_agent(
            r#"{"instructions":"Be brief.","goal":"NVDA","tools":["search","generate"],"decide":true}"#,
        )
        .expect("decide spec");
        assert!(decide.decide);
        let plan = decide.to_logical();
        assert!(matches!(
            plan.root.op,
            aidb_ir::LogicalOp::Decide { max: 4, .. }
        ));
        let session = parse_agent(
            r#"{"instructions":"Be brief.","goal":"NVDA","tools":["search"],"session":"desk:nvda"}"#,
        )
        .expect("session spec");
        assert_eq!(session.session.as_deref(), Some("desk:nvda"));
        assert!(parse_aidb_session("SELECT aidb_session();").is_some());
        assert!(matches!(
            parse_aidb_session("SELECT aidb_session('desk:nvda');"),
            Some(SessionCall::Bind(name)) if name == "desk:nvda"
        ));
        assert!(parse_aidb_last_run_id("SELECT aidb_last_run_id();").is_some());
    }

    #[test]
    fn parses_memory_and_child_agents() {
        let (scope, content) = parse_aidb_memory_insert(
            "SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations.');",
        )
        .expect("insert");
        assert_eq!(scope, "user:123");
        assert!(content.contains("concise"));
        let (query, k, scope) = parse_aidb_memory_search(
            "SELECT aidb_memory_search('How should I explain this?', 5, 'user:123');",
        )
        .expect("search");
        assert_eq!(query, "How should I explain this?");
        assert_eq!(k, 5);
        assert_eq!(scope.as_deref(), Some("user:123"));
        let spec = parse_agent(
            r#"{"instructions":"Lead.","goal":"refunds","tools":["search"],"memory":"user:123","agents":[{"instructions":"Answer.","goal":"refunds","tools":["generate"]}]}"#,
        )
        .expect("parent");
        assert_eq!(spec.memory.as_deref(), Some("user:123"));
        assert_eq!(spec.agents.len(), 1);
        assert!(spec.agents[0].agents.is_empty());
    }

    #[test]
    fn parses_workflow_then() {
        let json = parse_aidb_workflow(
            r#"SELECT aidb_workflow('{"then":[{"search":{"query":"refunds","k":5}},{"generate":{"prompt":"Summarize this"}}]}');"#,
        )
        .expect("parse call");
        let wf = parse_workflow(&json).expect("parse json");
        assert!(matches!(wf, aidb_ir::Workflow::Then(steps) if steps.len() == 2));
    }

    #[test]
    fn parses_a_tool_step_so_the_tool_operator_is_reachable_from_sql() {
        let json = parse_aidb_workflow(
            r#"SELECT aidb_workflow('{"then":[{"search":{"query":"refunds","k":1}},{"tool":{"name":"http.get"}}]}');"#,
        )
        .expect("parse call");
        match parse_workflow(&json).expect("parse json") {
            aidb_ir::Workflow::Then(steps) => assert!(
                matches!(&steps[1], aidb_ir::Workflow::Tool { name } if name == "http.get"),
                "{:?}",
                steps[1]
            ),
            other => panic!("{other:?}"),
        }
        // The shorthand spelling is the same node.
        assert_eq!(
            parse_workflow(r#"{"tool":"http.get"}"#).expect("shorthand"),
            aidb_ir::Workflow::Tool {
                name: "http.get".into()
            }
        );
        assert!(parse_workflow(r#"{"tool":{}}"#).is_err());
        assert!(parse_workflow(r#"{"tool":""}"#).is_err());
    }

    #[test]
    fn parses_approve_and_resume() {
        let json = parse_aidb_workflow(
            r#"SELECT aidb_workflow('{"then":[{"search":{"query":"refunds","k":5}},{"approve":{"message":"send?"}},{"generate":{"prompt":"Draft"}}]}');"#,
        )
        .expect("parse call");
        let wf = parse_workflow(&json).expect("parse json");
        match wf {
            aidb_ir::Workflow::Then(steps) => {
                assert!(matches!(
                    &steps[1],
                    aidb_ir::Workflow::Pause { status, message }
                        if status == "awaiting_approval" && message == "send?"
                ));
            }
            other => panic!("{other:?}"),
        }
        let (id, decision) =
            parse_aidb_resume(r#"SELECT aidb_resume('run_abc', '{"approved":true}');"#)
                .expect("resume");
        assert_eq!(id, "run_abc");
        assert_eq!(decision, r#"{"approved":true}"#);
    }

    #[test]
    fn parses_mcp_connect_disconnect_and_short_agent() {
        let (transport, command) =
            parse_aidb_mcp_connect("SELECT aidb_mcp_connect('stdio', './fake-mcp');")
                .expect("connect");
        assert_eq!(transport, "stdio");
        assert_eq!(command, "./fake-mcp");
        assert!(parse_aidb_mcp_disconnect("SELECT aidb_mcp_disconnect();").is_some());
        let json = parse_aidb_agent(
            r#"SELECT aidb_agent('Use the connected MCP tool', '["echo.ping"]');"#,
        )
        .expect("agent");
        let spec = parse_agent(&json).expect("spec");
        assert_eq!(spec.goal, "Use the connected MCP tool");
        assert_eq!(spec.tools, ["echo.ping"]);
    }

    #[test]
    fn parses_mcp_register_and_tool() {
        let json = parse_aidb_mcp_register(
            r#"SELECT aidb_mcp_register('{"tools":[{"name":"github.read"}]}');"#,
        )
        .expect("mcp");
        assert!(json.contains("github.read"), "{json}");
        let (name, args) =
            parse_aidb_tool(r#"SELECT aidb_tool('github.read', '{"path":"README.md"}');"#)
                .expect("tool");
        assert_eq!(name, "github.read");
        assert_eq!(args, r#"{"path":"README.md"}"#);
    }

    #[test]
    fn parses_set_and_get_policy() {
        let (json, name) = parse_aidb_set_policy(
            r#"SELECT aidb_set_policy('{"read_only":true,"deny":["send.email"],"max_usd":0.10}');"#,
        )
        .expect("set");
        assert!(json.contains("send.email"), "{json}");
        assert_eq!(name, None);
        let (json, name) = parse_aidb_set_policy(
            r#"SELECT aidb_set_policy('strict', '{"deny":["send.email"]}');"#,
        )
        .expect("named");
        assert_eq!(name.as_deref(), Some("strict"));
        assert!(json.contains("send.email"), "{json}");
        assert!(parse_aidb_get_policy("SELECT aidb_get_policy();").is_some());
        assert!(parse_aidb_secret_store("SELECT aidb_secret_store();").is_some());
    }
}
