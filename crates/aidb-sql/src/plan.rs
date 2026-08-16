//! Lower SQL to IR, bind, optimize, print and execute physical plans.

use aidb_ai::Embedder;
use aidb_core::{Error, QueryResult, Result, Retrieval, Value};
use aidb_ir::{BindContext, LlmContent, LogicalNode, LogicalOp, LogicalPlan, ModelRef, SpaceRef};
use aidb_opt::{cascade_candidate, optimize, OptimizeContext};
use aidb_storage::{sqlite_err, Connection};

pub fn bind_context(conn: &Connection) -> Result<BindContext> {
    let mut ctx = BindContext::engine();
    {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table', 'view')")
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sqlite_err)?;
        for name in rows {
            ctx.tables.push(name.map_err(sqlite_err)?);
        }
    }
    {
        let mut stmt = conn
            .prepare("SELECT name, kind, provider, provider_model, key_name FROM models")
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ModelRef {
                    name: row.get(0)?,
                    kind: row.get(1)?,
                    provider: row.get(2)?,
                    provider_model: row.get(3)?,
                    key_name: row.get(4)?,
                })
            })
            .map_err(sqlite_err)?;
        for model in rows {
            ctx.models.push(model.map_err(sqlite_err)?);
        }
    }
    if aidb_index::table_exists(conn, "embedding_spaces") {
        let mut stmt = conn
            .prepare("SELECT name, provider, provider_model, dimensions FROM embedding_spaces")
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SpaceRef {
                    name: row.get(0)?,
                    provider: row.get(1)?,
                    provider_model: row.get(2)?,
                    dimensions: row.get(3)?,
                })
            })
            .map_err(sqlite_err)?;
        for space in rows {
            ctx.spaces.push(space.map_err(sqlite_err)?);
        }
    }
    ctx.capabilities = aidb_tool::names(conn)?;
    Ok(ctx)
}

pub fn optimize_context(conn: &Connection) -> Result<OptimizeContext> {
    let ready_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM documents WHERE index_status = 'ready'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let has_vec: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'vec_chunks'",
            [],
            |_| Ok(true),
        )
        .optional()
        .map_err(sqlite_err)?
        .unwrap_or(false);
    let policy = aidb_tool::effective_policy(conn)?;
    Ok(OptimizeContext {
        ready_count,
        has_vec,
        has_fts: aidb_index::has_fts(conn),
        ..OptimizeContext {
            budget: crate::budget_from_policy(&policy),
            policy_summary: Some(policy.summary()),
            ..OptimizeContext::default()
        }
    })
}

pub fn explain_sql(conn: &Connection, sql: &str) -> Result<String> {
    explain_sql_with(conn, sql, None)
}

pub fn explain_sql_with(
    conn: &Connection,
    sql: &str,
    embedder: Option<&dyn Embedder>,
) -> Result<String> {
    let plan = lower_plan(sql)?;
    let bind = bind_context(conn)?;
    plan.bind(&bind)?;
    let mut ctx = optimize_context(conn)?;
    if let Ok(goal) = crate::parse_goal_sql(sql) {
        ctx.budget = ctx.budget.overlay(&aidb_opt::Budget {
            max_usd: goal.max_usd,
            max_ms: goal.max_ms,
            max_llm_calls: None,
        });
        if goal.read_only {
            if let Some(summary) = ctx.policy_summary.as_mut() {
                if !summary.contains("read_only=true") {
                    *summary = summary.replacen("read_only=false", "read_only=true", 1);
                }
            }
        }
        ctx.topk = goal.k;
    }
    if let Some(embedder) = embedder {
        prepare_cascade(conn, embedder, &plan, &mut ctx)?;
    } else if cascade_candidate(&plan, &ctx) {
        ctx.sample_recall = Some(1.0);
    }
    let optimized = optimize(plan, &ctx);
    Ok(optimized.render(&optimized.plan.to_physical(&bind).explain()))
}

pub fn lower_plan(sql: &str) -> Result<LogicalPlan> {
    if crate::looks_like_goal(sql) {
        return Ok(crate::parse_goal_sql(sql)?.to_logical());
    }
    if let Some(json) = crate::parse_aidb_agent(sql) {
        return Ok(crate::parse_agent(&json)?.to_logical());
    }
    if let Some(json) = crate::parse_aidb_workflow(sql) {
        return Ok(crate::parse_workflow(&json)?.to_logical());
    }
    crate::logical_plan(sql).ok_or_else(|| {
        Error::usage(
            "EXPLAIN only supports aidb_search, SEARCH, aidb_generate, AI_GENERATE, aidb_classify, aidb_workflow, aidb_agent, and TASK",
        )
    })
}

pub fn execute_search(
    conn: &Connection,
    embedder: &dyn Embedder,
    plan: &LogicalPlan,
) -> Result<QueryResult> {
    let bind = bind_context(conn)?;
    plan.bind(&bind)?;
    let mut ctx = optimize_context(conn)?;
    if let Some((_, k)) = plan.search_args() {
        ctx.topk = k;
    }
    let space = plan.space();
    if let Some(name) = space.as_deref() {
        ctx.has_vec = aidb_index::has_vec_table(conn, &format!("vec_chunks_{name}"));
    }
    let optimized = optimize(plan.clone(), &ctx);
    let (query, k) = optimized
        .plan
        .search_args()
        .ok_or_else(|| Error::usage("internal: search plan is missing query or k"))?;
    let space = optimized.plan.space().or(space);
    let mode = retrieval_of(&optimized.plan.root)
        .unwrap_or_else(|| Retrieval::choose(&query, ctx.has_vec, ctx.has_fts));
    let filter = optimized.plan.metadata_filter();
    aidb_index::search_in(
        conn,
        embedder,
        &query,
        k,
        None,
        Some(mode),
        filter.as_deref(),
        space.as_deref(),
    )
}

fn retrieval_of(node: &LogicalNode) -> Option<Retrieval> {
    if matches!(node.op, LogicalOp::Similarity) {
        if let Some(mode) = node.hints.retrieval {
            return Some(mode);
        }
    }
    node.children.iter().find_map(retrieval_of)
}

pub fn execute_optimized_generate(
    conn: &Connection,
    embedder: &dyn Embedder,
    prompt: &str,
    from: &str,
    filter: Option<&str>,
    schema: Option<&str>,
) -> Result<QueryResult> {
    execute_optimized_llm(conn, embedder, prompt, from, filter, false, schema)
}

pub fn execute_optimized_classify(
    conn: &Connection,
    embedder: &dyn Embedder,
    labels: &str,
    from: &str,
    filter: Option<&str>,
    schema: Option<&str>,
) -> Result<QueryResult> {
    execute_optimized_llm(conn, embedder, labels, from, filter, true, schema)
}

fn execute_optimized_llm(
    conn: &Connection,
    embedder: &dyn Embedder,
    prompt: &str,
    from: &str,
    filter: Option<&str>,
    classify: bool,
    schema: Option<&str>,
) -> Result<QueryResult> {
    crate::begin_llm_budget();
    let plan =
        LogicalPlan::generate_naive(prompt, LlmContent::Column("content".into()), from, filter);
    let bind = bind_context(conn)?;
    plan.bind(&bind)?;
    let mut ctx = optimize_context(conn)?;
    prepare_cascade(conn, embedder, &plan, &mut ctx)?;
    let optimized = optimize(plan, &ctx);
    if !classify && optimized.is_cascade() {
        let (query, k) = optimized
            .plan
            .search_args()
            .unwrap_or_else(|| (prompt.to_string(), ctx.topk));
        let hits = aidb_index::search(conn, embedder, &query, k)?;
        return cited_generate(conn, prompt, &hits, schema);
    }

    let sql = match filter {
        Some(pred) if !pred.is_empty() => {
            format!("SELECT content FROM {from} WHERE {pred}")
        }
        _ => format!("SELECT content FROM {from}"),
    };
    let rows = query_texts(conn, &sql)?;
    let mut out_rows = Vec::new();
    for content in rows {
        let text = if classify {
            crate::classify_text_in(conn, prompt, &content, None, schema)?
        } else {
            crate::generate_with_schema(conn, prompt, &content, None, None, schema)?
        };
        out_rows.push(vec![Value::Text(text)]);
    }
    Ok(QueryResult {
        columns: vec!["text".into()],
        rows: out_rows,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn execute_rag_generate(
    conn: &Connection,
    embedder: &dyn Embedder,
    prompt: &str,
    query: &str,
    k: i64,
    filter: Option<&str>,
    space: Option<&str>,
    schema: Option<&str>,
) -> Result<QueryResult> {
    execute_rag_generate_in(
        conn, embedder, prompt, query, k, filter, space, None, schema,
    )
}

/// Retrieve, then answer from what came back — the same path `aidb_generate … FROM
/// aidb_search(…)` takes, with the retrieval and the model call recorded under a
/// parent so their cost rolls up to it.
#[allow(clippy::too_many_arguments)]
pub fn execute_rag_generate_in(
    conn: &Connection,
    embedder: &dyn Embedder,
    prompt: &str,
    query: &str,
    k: i64,
    filter: Option<&str>,
    space: Option<&str>,
    parent: Option<&str>,
    schema: Option<&str>,
) -> Result<QueryResult> {
    let out = execute_rag_generate_traced(
        conn, embedder, prompt, query, k, filter, space, parent, schema,
    )?;
    Ok(QueryResult {
        columns: vec!["text".into()],
        rows: vec![vec![Value::Text(out.answer)]],
    })
}

/// What the cascade did, not just what it said: the answer plus the citations it was
/// given. A caller that grades retrieval (an experiment) needs both, and it must get
/// them from this path rather than a lookalike of it.
pub struct RagOutcome {
    pub answer: String,
    pub sources: Vec<crate::Citation>,
}

#[allow(clippy::too_many_arguments)]
pub fn execute_rag_generate_traced(
    conn: &Connection,
    embedder: &dyn Embedder,
    prompt: &str,
    query: &str,
    k: i64,
    filter: Option<&str>,
    space: Option<&str>,
    parent: Option<&str>,
    schema: Option<&str>,
) -> Result<RagOutcome> {
    crate::begin_llm_budget();
    let plan = LogicalPlan::generate_over_search_filtered(prompt, query, k, filter);
    let bind = bind_context(conn)?;
    plan.bind(&bind)?;
    let mut ctx = optimize_context(conn)?;
    ctx.topk = k.max(1);
    let optimized = optimize(plan, &ctx);
    let (query, k) = optimized
        .plan
        .search_args()
        .unwrap_or_else(|| (query.to_string(), k));
    let mode = retrieval_of(&optimized.plan.root)
        .unwrap_or_else(|| Retrieval::choose(&query, ctx.has_vec, ctx.has_fts));
    let filter = optimized.plan.metadata_filter();
    let hits = aidb_index::search_in(
        conn,
        embedder,
        &query,
        k,
        parent,
        Some(mode),
        filter.as_deref(),
        space,
    )?;
    let sources = crate::citations_from_hits(&hits);
    let answer = crate::generate_with_schema(
        conn,
        prompt,
        &hits_to_text(&hits),
        parent,
        Some(&sources),
        schema,
    )?;
    Ok(RagOutcome { answer, sources })
}

/// The row-wise plan the optimizer exists to rewrite away: one model call per ready
/// document, no retrieval. Run on purpose (and unrewritten) so an experiment can
/// price what the rewrite saves. Returns one answer per document.
pub fn execute_naive_generate_in(
    conn: &Connection,
    prompt: &str,
    parent: Option<&str>,
) -> Result<Vec<String>> {
    crate::begin_llm_budget();
    let sql = "SELECT content FROM documents WHERE index_status = 'ready'";
    let mut out = Vec::new();
    for content in query_texts(conn, sql)? {
        out.push(crate::generate_text(conn, prompt, &content, parent)?);
    }
    Ok(out)
}

/// Retrieval on its own, recorded as a run: no model call, so no model cost.
pub fn execute_retrieval_in(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
    parent: Option<&str>,
) -> Result<QueryResult> {
    crate::begin_llm_budget();
    aidb_index::search_in(conn, embedder, query, k, parent, None, None, None)
}

#[allow(clippy::too_many_arguments)]
pub fn execute_rag_classify(
    conn: &Connection,
    embedder: &dyn Embedder,
    labels: &str,
    query: &str,
    k: i64,
    filter: Option<&str>,
    space: Option<&str>,
    schema: Option<&str>,
) -> Result<QueryResult> {
    crate::begin_llm_budget();
    let plan = LogicalPlan::generate_over_search_filtered(labels, query, k, filter);
    let bind = bind_context(conn)?;
    plan.bind(&bind)?;
    let mut ctx = optimize_context(conn)?;
    ctx.topk = k.max(1);
    let optimized = optimize(plan, &ctx);
    let (query, k) = optimized
        .plan
        .search_args()
        .unwrap_or_else(|| (query.to_string(), k));
    let mode = retrieval_of(&optimized.plan.root)
        .unwrap_or_else(|| Retrieval::choose(&query, ctx.has_vec, ctx.has_fts));
    let filter = optimized.plan.metadata_filter();
    let hits = aidb_index::search_in(
        conn,
        embedder,
        &query,
        k,
        None,
        Some(mode),
        filter.as_deref(),
        space,
    )?;
    let text = hits_to_text(&hits);
    let out = crate::classify_text_in(conn, labels, &text, None, schema)?;
    Ok(QueryResult {
        columns: vec!["text".into()],
        rows: vec![vec![Value::Text(out)]],
    })
}

fn cited_generate(
    conn: &Connection,
    prompt: &str,
    hits: &QueryResult,
    schema: Option<&str>,
) -> Result<QueryResult> {
    let sources = crate::citations_from_hits(hits);
    let text = hits_to_text(hits);
    let out = crate::generate_with_schema(conn, prompt, &text, None, Some(&sources), schema)?;
    Ok(QueryResult {
        columns: vec!["text".into()],
        rows: vec![vec![Value::Text(out)]],
    })
}

fn prepare_cascade(
    conn: &Connection,
    embedder: &dyn Embedder,
    plan: &LogicalPlan,
    ctx: &mut OptimizeContext,
) -> Result<()> {
    if !cascade_candidate(plan, ctx) {
        return Ok(());
    }
    let query = plan
        .search_args()
        .map(|(q, _)| q)
        .or_else(|| llm_prompt(plan))
        .unwrap_or_default();
    let start_k = ctx.topk;
    let mut samples = Vec::new();
    for k in aidb_opt::widen_candidates(start_k, ctx.max_k) {
        let recall = sample_recall(conn, embedder, &query, k)?;
        samples.push((k, recall));
        if recall + f64::EPSILON >= ctx.quality_floor {
            break;
        }
    }
    match aidb_opt::pick_cascade_k(ctx.quality_floor, &samples) {
        Some((k, recall)) => {
            if k != start_k {
                ctx.widened_from = Some(start_k);
            }
            ctx.topk = k;
            ctx.sample_recall = Some(recall);
            crate::job_set_cascade(k, recall);
        }
        None => {
            if let Some((k, recall)) = samples.last().copied() {
                ctx.topk = k;
                ctx.sample_recall = Some(recall);
            }
        }
    }
    Ok(())
}

pub fn sample_recall(
    conn: &Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: i64,
) -> Result<f64> {
    let sample: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, content FROM documents
                 WHERE index_status = 'ready'
                 ORDER BY created_at_ms
                 LIMIT 8",
            )
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)?
    };
    if sample.is_empty() {
        return Ok(1.0);
    }
    let tokens = significant_tokens(query);
    let gold: Vec<&str> = sample
        .iter()
        .filter(|(_, content)| shares_token(content, &tokens))
        .map(|(id, _)| id.as_str())
        .collect();
    if gold.is_empty() {
        return Ok(1.0);
    }
    let hits = aidb_index::hits(conn, embedder, query, k)?;
    let doc_idx = hits
        .columns
        .iter()
        .position(|c| c == "document_id")
        .unwrap_or(0);
    let hit_ids: Vec<String> = hits
        .rows
        .iter()
        .filter_map(|row| row.get(doc_idx).map(ToString::to_string))
        .collect();
    let hits_on_gold = gold
        .iter()
        .filter(|id| hit_ids.iter().any(|hit| hit == *id))
        .count();
    Ok(hits_on_gold as f64 / gold.len() as f64)
}

fn llm_prompt(plan: &LogicalPlan) -> Option<String> {
    fn walk(node: &aidb_ir::LogicalNode) -> Option<String> {
        if let aidb_ir::LogicalOp::Llm { prompt, .. } = &node.op {
            return Some(prompt.clone());
        }
        node.children.iter().find_map(walk)
    }
    walk(&plan.root)
}

fn query_texts(conn: &Connection, sql: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql).map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sqlite_err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)
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

fn significant_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| w.len() > 3)
        .collect()
}

fn shares_token(content: &str, tokens: &[String]) -> bool {
    let lower = content.to_ascii_lowercase();
    tokens.iter().any(|t| lower.contains(t.as_str()))
}

trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err),
        }
    }
}
