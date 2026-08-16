//! Three rewrite classes. Quality is measured, not a predicted 0.95.

use aidb_core::Retrieval;
use aidb_ir::{LlmContent, LogicalNode, LogicalOp, LogicalPlan, Schema};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteClass {
    Equivalence,
    Approximation,
    Physical,
}

impl RewriteClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Equivalence => "equivalence",
            Self::Approximation => "approximation",
            Self::Physical => "physical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    pub class: RewriteClass,
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Budget {
    pub max_usd: Option<f64>,
    pub max_ms: Option<u64>,
    pub max_llm_calls: Option<u32>,
}

impl Budget {
    pub fn from_env() -> Self {
        Self {
            max_usd: std::env::var("AIDB_MAX_USD")
                .ok()
                .and_then(|v| v.parse().ok()),
            max_ms: std::env::var("AIDB_MAX_MS")
                .ok()
                .and_then(|v| v.parse().ok()),
            max_llm_calls: std::env::var("AIDB_MAX_LLM_CALLS")
                .ok()
                .and_then(|v| v.parse().ok())
                .or(Some(64)),
        }
    }

    pub fn overlay(&self, over: &Self) -> Self {
        Self {
            max_usd: min_opt(self.max_usd, over.max_usd),
            max_ms: min_opt(self.max_ms, over.max_ms),
            max_llm_calls: min_opt(self.max_llm_calls, over.max_llm_calls),
        }
    }
}

fn min_opt<T: Copy + PartialOrd>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if x < y { x } else { y }),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_usd: None,
            max_ms: None,
            max_llm_calls: Some(64),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OptimizeContext {
    pub ready_count: i64,
    pub has_vec: bool,
    pub has_fts: bool,
    pub topk: i64,
    pub max_k: i64,
    pub quality_floor: f64,
    pub sample_recall: Option<f64>,
    pub widened_from: Option<i64>,
    pub budget: Budget,
    pub policy_summary: Option<String>,
}

impl Default for OptimizeContext {
    fn default() -> Self {
        Self {
            ready_count: 0,
            has_vec: false,
            has_fts: false,
            topk: 5,
            max_k: 32,
            quality_floor: 0.5,
            sample_recall: None,
            widened_from: None,
            budget: Budget::default(),
            policy_summary: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Optimized {
    pub plan: LogicalPlan,
    pub rewrites: Vec<Rewrite>,
    pub budget: Budget,
    pub policy_summary: Option<String>,
}

impl Optimized {
    pub fn render(&self, physical: &str) -> String {
        let mut out = physical.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if !self.rewrites.is_empty() {
            out.push_str("\nRewrites\n");
            for rewrite in &self.rewrites {
                out.push_str(&format!(
                    "  {}: {} — {}\n",
                    rewrite.class.as_str(),
                    rewrite.name,
                    rewrite.detail
                ));
            }
        }
        let calls = self
            .budget
            .max_llm_calls
            .map(|n| n.to_string())
            .unwrap_or_else(|| "none".into());
        let usd = self
            .budget
            .max_usd
            .map(|n| n.to_string())
            .unwrap_or_else(|| "none".into());
        let ms = self
            .budget
            .max_ms
            .map(|n| n.to_string())
            .unwrap_or_else(|| "none".into());
        out.push_str(&format!(
            "\nBudget max_llm_calls={calls} max_usd={usd} max_ms={ms}\n"
        ));
        if let Some(policy) = &self.policy_summary {
            out.push_str(&format!("Policy {policy}\n"));
        }
        out
    }

    pub fn is_cascade(&self) -> bool {
        self.rewrites
            .iter()
            .any(|r| r.name == "CascadeEmbedTopKThenJudge")
    }
}

pub fn widen_candidates(start_k: i64, max_k: i64) -> Vec<i64> {
    let mut k = start_k.max(1);
    let max_k = max_k.max(k);
    let mut out = vec![k];
    while k < max_k {
        k = (k.saturating_mul(2)).min(max_k);
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

pub fn pick_cascade_k(floor: f64, samples: &[(i64, f64)]) -> Option<(i64, f64)> {
    samples
        .iter()
        .copied()
        .find(|(_, recall)| *recall + f64::EPSILON >= floor)
}

fn cascade_detail(ctx: &OptimizeContext) -> String {
    let recall = ctx
        .sample_recall
        .map(|r| format!("{r:.2}"))
        .unwrap_or_else(|| "unmeasured".into());
    match ctx.widened_from {
        Some(from) if from != ctx.topk => format!(
            "k={} ready={} sample_recall={} widened_from={}",
            ctx.topk, ctx.ready_count, recall, from
        ),
        _ => format!(
            "k={} ready={} sample_recall={}",
            ctx.topk, ctx.ready_count, recall
        ),
    }
}

pub fn cascade_candidate(plan: &LogicalPlan, ctx: &OptimizeContext) -> bool {
    let over_budget = ctx
        .budget
        .max_llm_calls
        .is_some_and(|max| ctx.ready_count > i64::from(max));
    ctx.has_vec
        && (ctx.ready_count > ctx.topk || over_budget)
        && has_llm_over_documents(&plan.root)
        && !has_similarity(&plan.root)
}

pub fn optimize(plan: LogicalPlan, ctx: &OptimizeContext) -> Optimized {
    let mut rewrites = Vec::new();
    let mut root = push_filter_before_expensive(plan.root, &mut rewrites);
    root = push_metadata_filter(root, &mut rewrites);
    if cascade_candidate(&LogicalPlan { root: root.clone() }, ctx) {
        let recall = ctx.sample_recall.unwrap_or(1.0);
        if recall + f64::EPSILON >= ctx.quality_floor {
            root = apply_cascade(root, ctx);
            rewrites.push(Rewrite {
                class: RewriteClass::Approximation,
                name: "CascadeEmbedTopKThenJudge".into(),
                detail: cascade_detail(ctx),
            });
        } else {
            rewrites.push(Rewrite {
                class: RewriteClass::Approximation,
                name: "CascadeEmbedTopKThenJudge".into(),
                detail: format!(
                    "rejected sample_recall={recall:.2} < floor={:.2}; using gold plan",
                    ctx.quality_floor
                ),
            });
        }
    }
    mark_hybrid_retrieval(&mut root, ctx, &mut rewrites);
    mark_metadata_filter(&root, &mut rewrites);
    mark_physical(&mut root, &mut rewrites);
    Optimized {
        plan: LogicalPlan { root },
        rewrites,
        budget: ctx.budget.clone(),
        policy_summary: ctx.policy_summary.clone(),
    }
}

fn push_filter_before_expensive(node: LogicalNode, rewrites: &mut Vec<Rewrite>) -> LogicalNode {
    let children: Vec<LogicalNode> = node
        .children
        .into_iter()
        .map(|child| push_filter_before_expensive(child, rewrites))
        .collect();
    let mut node = LogicalNode { children, ..node };

    if let LogicalOp::Filter { predicate } = &node.op {
        if node.children.len() == 1 {
            let child = &node.children[0];
            if matches!(child.op, LogicalOp::Llm { .. })
                && child.contract.tuple_independent
                && matches!(child.contract.side_effect, aidb_ir::SideEffect::None)
                && !child.contract.listwise
                && predicate_uses_only_child_schema(predicate, child)
            {
                let filter_pred = predicate.clone();
                let mut llm = node.children.remove(0);
                let llm_children = std::mem::take(&mut llm.children);
                let pushed = LogicalNode::new(
                    format!("{}.push", node.id),
                    LogicalOp::Filter {
                        predicate: filter_pred.clone(),
                    },
                    llm.schema_in.clone(),
                    llm.schema_in.clone(),
                    llm_children,
                );
                llm.children = vec![pushed];
                rewrites.push(Rewrite {
                    class: RewriteClass::Equivalence,
                    name: "PushFilterBeforeExpensive".into(),
                    detail: format!("moved `{filter_pred}` before Llm"),
                });
                return llm;
            }
        }
    }
    node
}

fn push_metadata_filter(node: LogicalNode, rewrites: &mut Vec<Rewrite>) -> LogicalNode {
    let children: Vec<LogicalNode> = node
        .children
        .into_iter()
        .map(|child| push_metadata_filter(child, rewrites))
        .collect();
    let mut node = LogicalNode { children, ..node };
    if let LogicalOp::Filter { predicate } = &node.op {
        if is_metadata_predicate(predicate) && node.children.len() == 1 {
            let child_is_retrieval = matches!(
                node.children[0].op,
                LogicalOp::TopK { .. } | LogicalOp::Similarity | LogicalOp::Llm { .. }
            );
            if child_is_retrieval {
                let pred = predicate.clone();
                let hint = node.hints.metadata_filter.clone();
                let child = node.children.remove(0);
                rewrites.push(Rewrite {
                    class: RewriteClass::Equivalence,
                    name: "PushFilterBeforeExpensive".into(),
                    detail: format!("moved `{pred}` before retrieval"),
                });
                return insert_metadata_before_embed(child, pred, hint);
            }
        }
    }
    node
}

fn insert_metadata_before_embed(
    node: LogicalNode,
    predicate: String,
    hint: Option<String>,
) -> LogicalNode {
    if matches!(node.op, LogicalOp::Embed { .. }) {
        let children = node.children.clone();
        let mut filter = LogicalNode::new(
            "opt.meta",
            LogicalOp::Filter {
                predicate: predicate.clone(),
            },
            Schema::documents(),
            Schema::documents(),
            children,
        );
        filter.hints.metadata_filter =
            hint.or_else(|| predicate.strip_prefix("metadata ").map(ToOwned::to_owned));
        let mut embed = node;
        embed.children = vec![filter];
        return embed;
    }
    let children = node
        .children
        .into_iter()
        .map(|child| insert_metadata_before_embed(child, predicate.clone(), hint.clone()))
        .collect();
    LogicalNode { children, ..node }
}

fn is_metadata_predicate(predicate: &str) -> bool {
    let trimmed = predicate.trim_start();
    trimmed.starts_with("metadata ") || trimmed.starts_with("metadata.")
}

fn mark_metadata_filter(node: &LogicalNode, rewrites: &mut Vec<Rewrite>) {
    if let Some(filter) = aidb_ir::normalize_metadata_filter(node.hints.metadata_filter.as_deref())
    {
        if !rewrites.iter().any(|r| r.name == "MetadataFilter") {
            rewrites.push(Rewrite {
                class: RewriteClass::Physical,
                name: "MetadataFilter".into(),
                detail: filter,
            });
        }
    }
    for child in &node.children {
        mark_metadata_filter(child, rewrites);
    }
}

fn predicate_uses_only_child_schema(predicate: &str, llm: &LogicalNode) -> bool {
    let idents = idents(predicate);
    if idents.is_empty() {
        return false;
    }
    let out: Vec<&str> = llm
        .schema_out
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let input = llm
        .children
        .first()
        .map(|child| {
            child
                .schema_out
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    !idents.iter().any(|id| out.contains(&id.as_str()))
        && idents
            .iter()
            .all(|id| input.contains(&id.as_str()) || is_sql_noise(id))
}

fn is_sql_noise(ident: &str) -> bool {
    matches!(
        ident.to_ascii_lowercase().as_str(),
        "and" | "or" | "not" | "in" | "is" | "null" | "like" | "between"
    )
}

fn idents(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            if !cur.is_empty() {
                if cur
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
            quote = Some(ch);
            continue;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else if !cur.is_empty() {
            if cur
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if !cur.is_empty()
        && cur
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        out.push(cur);
    }
    out
}

fn apply_cascade(root: LogicalNode, ctx: &OptimizeContext) -> LogicalNode {
    let prompt = find_llm_prompt(&root).unwrap_or_else(|| "query".into());
    let search = LogicalPlan::search(&prompt, ctx.topk).root;
    let llm = LogicalNode::new(
        "opt.llm",
        LogicalOp::Llm {
            prompt,
            content: LlmContent::Column("context".into()),
        },
        Schema::search_hits(),
        Schema::generated_text(),
        Vec::new(),
    );
    LogicalNode::new(
        "opt.then",
        LogicalOp::Then,
        Schema::default(),
        Schema::generated_text(),
        vec![search, llm],
    )
}

fn mark_hybrid_retrieval(
    node: &mut LogicalNode,
    ctx: &OptimizeContext,
    rewrites: &mut Vec<Rewrite>,
) {
    if matches!(node.op, LogicalOp::Similarity) && node.hints.retrieval.is_none() {
        let query = find_embed_query(node).unwrap_or_default();
        let retrieval = Retrieval::choose(&query, ctx.has_vec, ctx.has_fts);
        node.hints.retrieval = Some(retrieval);
        if !rewrites.iter().any(|r| r.name == "HybridFtsVec") {
            rewrites.push(Rewrite {
                class: RewriteClass::Physical,
                name: "HybridFtsVec".into(),
                detail: retrieval.algorithm().into(),
            });
        }
    }
    for child in &mut node.children {
        mark_hybrid_retrieval(child, ctx, rewrites);
    }
}

fn find_embed_query(node: &LogicalNode) -> Option<String> {
    if let LogicalOp::Embed {
        input: aidb_ir::EmbedInput::Query(query),
    } = &node.op
    {
        return Some(query.clone());
    }
    node.children.iter().find_map(find_embed_query)
}

fn mark_physical(node: &mut LogicalNode, rewrites: &mut Vec<Rewrite>) {
    if matches!(node.op, LogicalOp::Llm { .. }) && !node.hints.cache_keyed {
        node.hints.cache_keyed = true;
        node.hints.batch = true;
        if !rewrites.iter().any(|r| r.name == "CacheKeyedAiCall") {
            rewrites.push(Rewrite {
                class: RewriteClass::Physical,
                name: "CacheKeyedAiCall".into(),
                detail: "key=model+prompt+content".into(),
            });
            rewrites.push(Rewrite {
                class: RewriteClass::Physical,
                name: "BatchTupleIndependentLlm".into(),
                detail: "dedup identical (prompt, content) via keyed cache".into(),
            });
        }
    }
    for child in &mut node.children {
        mark_physical(child, rewrites);
    }
}

fn has_llm_over_documents(node: &LogicalNode) -> bool {
    if matches!(node.op, LogicalOp::Llm { .. }) && has_scan_documents(node) {
        return true;
    }
    node.children.iter().any(has_llm_over_documents)
}

fn has_scan_documents(node: &LogicalNode) -> bool {
    match &node.op {
        LogicalOp::Scan { table } if table == "documents" => true,
        _ => node.children.iter().any(has_scan_documents),
    }
}

fn has_similarity(node: &LogicalNode) -> bool {
    matches!(node.op, LogicalOp::Similarity | LogicalOp::TopK { .. })
        || node.children.iter().any(has_similarity)
}

fn find_llm_prompt(node: &LogicalNode) -> Option<String> {
    if let LogicalOp::Llm { prompt, .. } = &node.op {
        return Some(prompt.clone());
    }
    node.children.iter().find_map(find_llm_prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidb_ir::LlmContent;

    #[test]
    fn pushes_filter_before_llm() {
        let plan = LogicalPlan::generate_naive(
            "Summarize this",
            LlmContent::Column("content".into()),
            "documents",
            Some("index_status = 'ready'"),
        );
        assert!(matches!(plan.root.op, LogicalOp::Filter { .. }));
        let optimized = optimize(plan, &OptimizeContext::default());
        assert!(matches!(optimized.plan.root.op, LogicalOp::Llm { .. }));
        assert!(matches!(
            optimized.plan.root.children[0].op,
            LogicalOp::Filter { .. }
        ));
        assert!(optimized
            .rewrites
            .iter()
            .any(|r| r.name == "PushFilterBeforeExpensive"));
    }

    #[test]
    fn does_not_push_filter_on_llm_output() {
        let plan = LogicalPlan::generate_naive(
            "Summarize this",
            LlmContent::Column("content".into()),
            "documents",
            Some("text LIKE '%refund%'"),
        );
        let optimized = optimize(plan, &OptimizeContext::default());
        assert!(matches!(optimized.plan.root.op, LogicalOp::Filter { .. }));
        assert!(!optimized
            .rewrites
            .iter()
            .any(|r| r.name == "PushFilterBeforeExpensive"));
    }

    #[test]
    fn cascades_when_many_ready_docs() {
        let plan = LogicalPlan::generate_naive(
            "How do refunds work?",
            LlmContent::Column("content".into()),
            "documents",
            None,
        );
        let ctx = OptimizeContext {
            ready_count: 20,
            has_vec: true,
            topk: 5,
            sample_recall: Some(1.0),
            ..OptimizeContext::default()
        };
        let optimized = optimize(plan, &ctx);
        assert!(matches!(optimized.plan.root.op, LogicalOp::Then));
        assert!(optimized.is_cascade());
        assert!(optimized.plan.search_args().is_some());
    }

    #[test]
    fn cascade_falls_back_when_sample_misses_floor() {
        let plan = LogicalPlan::generate_naive(
            "How do refunds work?",
            LlmContent::Column("content".into()),
            "documents",
            None,
        );
        let ctx = OptimizeContext {
            ready_count: 20,
            has_vec: true,
            topk: 5,
            sample_recall: Some(0.1),
            quality_floor: 0.5,
            ..OptimizeContext::default()
        };
        let optimized = optimize(plan, &ctx);
        assert!(!matches!(optimized.plan.root.op, LogicalOp::Then));
        assert!(optimized
            .rewrites
            .iter()
            .any(|r| r.detail.contains("rejected")));
    }

    #[test]
    fn pushes_metadata_filter_before_retrieval() {
        let search = LogicalPlan::search("refund policy", 5);
        let mut filter = LogicalNode::new(
            "nF",
            LogicalOp::Filter {
                predicate: r#"metadata {"dept":"support"}"#.into(),
            },
            search.root.schema_out.clone(),
            search.root.schema_out.clone(),
            vec![search.root],
        );
        filter.hints.metadata_filter = Some(r#"{"dept":"support"}"#.into());
        let plan = LogicalPlan { root: filter };
        let optimized = optimize(plan, &OptimizeContext::default());
        assert!(matches!(optimized.plan.root.op, LogicalOp::TopK { .. }));
        assert_eq!(
            optimized.plan.metadata_filter().as_deref(),
            Some(r#"{"dept":"support"}"#)
        );
        assert!(optimized
            .rewrites
            .iter()
            .any(|r| r.name == "PushFilterBeforeExpensive" && r.detail.contains("retrieval")));
        assert!(optimized
            .rewrites
            .iter()
            .any(|r| r.name == "MetadataFilter"));
    }

    #[test]
    fn hybrid_rewrite_picks_fts_for_keyword_query() {
        let plan = LogicalPlan::search("ZX19QPLUGH", 5);
        let ctx = OptimizeContext {
            has_vec: true,
            has_fts: true,
            ..OptimizeContext::default()
        };
        let optimized = optimize(plan, &ctx);
        assert!(optimized
            .rewrites
            .iter()
            .any(|r| r.name == "HybridFtsVec" && r.detail.contains("fts5")));
        let sim = find_sim(&optimized.plan.root).expect("similarity");
        assert_eq!(sim.hints.retrieval, Some(Retrieval::Fts));
    }

    #[test]
    fn hybrid_rewrite_blends_semantic_query() {
        let plan = LogicalPlan::search("How do refunds work?", 5);
        let ctx = OptimizeContext {
            has_vec: true,
            has_fts: true,
            ..OptimizeContext::default()
        };
        let optimized = optimize(plan, &ctx);
        assert!(optimized
            .rewrites
            .iter()
            .any(|r| r.detail.contains("hybrid rrf")));
    }

    #[test]
    fn widens_k_until_recall_meets_floor() {
        assert_eq!(widen_candidates(5, 20), vec![5, 10, 20]);
        assert_eq!(
            pick_cascade_k(0.5, &[(5, 0.25), (10, 0.6)]),
            Some((10, 0.6))
        );
        assert_eq!(pick_cascade_k(0.5, &[(5, 0.1), (10, 0.2), (20, 0.3)]), None);
    }

    fn find_sim(node: &LogicalNode) -> Option<&LogicalNode> {
        if matches!(node.op, LogicalOp::Similarity) {
            return Some(node);
        }
        node.children.iter().find_map(find_sim)
    }
}
