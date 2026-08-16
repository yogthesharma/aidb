//! Logical and physical plans. Phase 4: typed operators, contracts, bind.
//! Phase 5: workflow combinators compile to this IR.

mod goal;
mod workflow;

use std::fmt;

pub use goal::GoalSpec;
pub use workflow::Workflow;

use aidb_core::{Error, Result, Retrieval};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Determinism {
    Strict,
    Approximate,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffect {
    None,
    Reversible,
    Irreversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    Safe,
    Conditional,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cache {
    Always,
    Keyed,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Data,
    Ai,
    Tool,
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalBackend {
    Sqlite,
    AiRuntime,
    ToolRuntime,
    Control,
}

impl PhysicalBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::AiRuntime => "ai",
            Self::ToolRuntime => "tool",
            Self::Control => "control",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub determinism: Determinism,
    pub side_effect: SideEffect,
    pub tuple_independent: bool,
    pub listwise: bool,
    pub retry: Retry,
    pub cache: Cache,
    pub backend: Backend,
}

impl Contract {
    pub fn strict_data() -> Self {
        Self {
            determinism: Determinism::Strict,
            side_effect: SideEffect::None,
            tuple_independent: true,
            listwise: false,
            retry: Retry::Safe,
            cache: Cache::Always,
            backend: Backend::Data,
        }
    }

    pub fn strict_data_relational() -> Self {
        Self {
            tuple_independent: false,
            ..Self::strict_data()
        }
    }

    pub fn approx_ai() -> Self {
        Self {
            determinism: Determinism::Approximate,
            side_effect: SideEffect::None,
            tuple_independent: true,
            listwise: false,
            retry: Retry::Safe,
            cache: Cache::Keyed,
            backend: Backend::Ai,
        }
    }

    pub fn llm() -> Self {
        Self {
            determinism: Determinism::None,
            ..Self::approx_ai()
        }
    }

    pub fn tool_get() -> Self {
        Self {
            determinism: Determinism::None,
            side_effect: SideEffect::None,
            tuple_independent: true,
            listwise: false,
            retry: Retry::Safe,
            cache: Cache::Keyed,
            backend: Backend::Tool,
        }
    }

    pub fn control() -> Self {
        Self {
            determinism: Determinism::Strict,
            side_effect: SideEffect::None,
            tuple_independent: false,
            listwise: false,
            retry: Retry::Safe,
            cache: Cache::Never,
            backend: Backend::Control,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColType {
    Text,
    Integer,
    Real,
    Blob,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub ty: ColType,
}

impl Column {
    pub fn text(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ColType::Text,
        }
    }

    pub fn integer(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ColType::Integer,
        }
    }

    pub fn real(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ColType::Real,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schema {
    pub columns: Vec<Column>,
}

impl Schema {
    pub fn new(columns: Vec<Column>) -> Self {
        Self { columns }
    }

    pub fn documents() -> Self {
        Self::new(vec![
            Column::text("id"),
            Column::text("title"),
            Column::text("content"),
            Column::text("index_status"),
        ])
    }

    pub fn search_hits() -> Self {
        Self::new(vec![
            Column::text("document_id"),
            Column::integer("chunk_id"),
            Column::text("content"),
            Column::real("distance"),
        ])
    }

    pub fn embedding() -> Self {
        Self::new(vec![Column {
            name: "embedding".into(),
            ty: ColType::Blob,
        }])
    }

    pub fn generated_text() -> Self {
        Self::new(vec![Column::text("text")])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hints {
    pub backend_preference: Option<Backend>,
    pub cache_keyed: bool,
    pub batch: bool,
    pub retrieval: Option<Retrieval>,
    pub metadata_filter: Option<String>,
    pub space: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedInput {
    Query(String),
    Column(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmContent {
    Literal(String),
    Column(String),
}

impl fmt::Display for LlmContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(text) => write!(f, "literal={text:?}"),
            Self::Column(name) => write!(f, "column={name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalOp {
    Scan {
        table: String,
    },
    Filter {
        predicate: String,
    },
    Join {
        left: String,
        right: String,
        on: String,
    },
    Aggregate {
        expr: String,
    },
    Embed {
        input: EmbedInput,
    },
    Similarity,
    TopK {
        k: i64,
    },
    Llm {
        prompt: String,
        content: LlmContent,
    },
    Tool {
        name: String,
    },
    Then,
    Parallel,
    Branch {
        predicate: String,
    },
    Loop {
        until: String,
        max: u32,
    },
    /// Agent loop: the model chooses search / generate / a catalog tool / stop.
    Decide {
        tools: Vec<String>,
        max: u32,
    },
}

impl LogicalOp {
    pub fn contract(&self) -> Contract {
        match self {
            Self::Scan { .. } | Self::Filter { .. } | Self::TopK { .. } => Contract::strict_data(),
            Self::Join { .. } | Self::Aggregate { .. } => Contract::strict_data_relational(),
            Self::Embed { .. } => Contract::approx_ai(),
            Self::Similarity => Contract {
                backend: Backend::Data,
                ..Contract::approx_ai()
            },
            Self::Llm { .. } => Contract::llm(),
            Self::Tool { .. } => Contract::tool_get(),
            Self::Then
            | Self::Parallel
            | Self::Branch { .. }
            | Self::Loop { .. }
            | Self::Decide { .. } => Contract::control(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogicalNode {
    pub id: String,
    pub op: LogicalOp,
    pub schema_in: Schema,
    pub schema_out: Schema,
    pub contract: Contract,
    pub hints: Hints,
    pub children: Vec<LogicalNode>,
}

impl LogicalNode {
    pub fn new(
        id: impl Into<String>,
        op: LogicalOp,
        schema_in: Schema,
        schema_out: Schema,
        children: Vec<LogicalNode>,
    ) -> Self {
        let contract = op.contract();
        Self {
            id: id.into(),
            op,
            schema_in,
            schema_out,
            contract,
            hints: Hints::default(),
            children,
        }
    }

    fn bind(&self, ctx: &BindContext) -> Result<()> {
        match &self.op {
            LogicalOp::Scan { table } => {
                if !ctx.has_table(table) {
                    return Err(Error::schema(format!("unknown table: {table}")));
                }
            }
            LogicalOp::Embed { .. } | LogicalOp::Similarity | LogicalOp::TopK { .. } => {
                if !ctx.has_function("aidb_search") {
                    return Err(Error::schema("function aidb_search is not registered"));
                }
            }
            LogicalOp::Llm { .. } => {
                if !ctx.has_function("aidb_generate") {
                    return Err(Error::schema("function aidb_generate is not registered"));
                }
            }
            LogicalOp::Tool { name } => {
                if !ctx.has_capability(name) && !ctx.has_function(name) {
                    return Err(Error::schema(format!("unknown capability: {name}")));
                }
            }
            LogicalOp::Decide { tools, .. } => {
                if !ctx.has_function("aidb_generate") {
                    return Err(Error::schema("function aidb_generate is not registered"));
                }
                if tools.iter().any(|t| t == "search") && !ctx.has_function("aidb_search") {
                    return Err(Error::schema("function aidb_search is not registered"));
                }
                for name in tools {
                    if name == "search" || name == "generate" || name == "stop" {
                        continue;
                    }
                    if !ctx.has_capability(name) && !ctx.has_function(name) {
                        return Err(Error::schema(format!("unknown capability: {name}")));
                    }
                }
            }
            LogicalOp::Filter { .. }
            | LogicalOp::Join { .. }
            | LogicalOp::Aggregate { .. }
            | LogicalOp::Then
            | LogicalOp::Parallel
            | LogicalOp::Branch { .. }
            | LogicalOp::Loop { .. } => {}
        }
        for child in &self.children {
            child.bind(ctx)?;
        }
        Ok(())
    }

    fn to_physical(&self, ctx: &BindContext) -> PhysicalNode {
        let backend = match self.contract.backend {
            Backend::Data => PhysicalBackend::Sqlite,
            Backend::Ai => PhysicalBackend::AiRuntime,
            Backend::Tool => PhysicalBackend::ToolRuntime,
            Backend::Control => PhysicalBackend::Control,
        };
        let algorithm = match &self.op {
            LogicalOp::Scan { .. } => "seqscan".into(),
            LogicalOp::Filter { .. } => "predicate".into(),
            LogicalOp::Join { .. } => "hashjoin".into(),
            LogicalOp::Aggregate { .. } => "hashagg".into(),
            LogicalOp::Embed { .. } => {
                let (provider, model) = ctx.embedding_for(self.hints.space.as_deref());
                match self.hints.space.as_deref() {
                    Some(space) => format!("embed {provider}:{model} space={space}"),
                    None => format!("embed {provider}:{model}"),
                }
            }
            LogicalOp::Similarity => {
                let algo = self.hints.retrieval.unwrap_or(Retrieval::Vec).algorithm();
                match self.hints.space.as_deref() {
                    Some(space) => format!("{algo} space={space}"),
                    None => algo.into(),
                }
            }
            LogicalOp::TopK { .. } => "limit".into(),
            LogicalOp::Llm { .. } => {
                let (provider, model) = ctx.llm();
                let base = format!("llm {provider}:{model}");
                if self.hints.cache_keyed {
                    format!("cached {base}")
                } else {
                    base
                }
            }
            LogicalOp::Tool { name } => format!("tool {name}"),
            LogicalOp::Then => "seq".into(),
            LogicalOp::Parallel => "seq".into(),
            LogicalOp::Branch { .. } => "predicate".into(),
            LogicalOp::Loop { .. } | LogicalOp::Decide { .. } => "bounded".into(),
        };
        PhysicalNode {
            id: self.id.clone(),
            op: self.op.clone(),
            algorithm,
            backend,
            schema_out: self.schema_out.clone(),
            contract: self.contract.clone(),
            children: self
                .children
                .iter()
                .map(|child| child.to_physical(ctx))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogicalPlan {
    pub root: LogicalNode,
}

impl LogicalPlan {
    pub fn search(query: impl Into<String>, k: i64) -> Self {
        Self::search_filtered(query, k, None)
    }

    pub fn search_filtered(
        query: impl Into<String>,
        k: i64,
        metadata_filter: Option<&str>,
    ) -> Self {
        Self::search_in(query, k, metadata_filter, None)
    }

    pub fn search_in(
        query: impl Into<String>,
        k: i64,
        metadata_filter: Option<&str>,
        space: Option<&str>,
    ) -> Self {
        let scan = LogicalNode::new(
            "n0",
            LogicalOp::Scan {
                table: "documents".into(),
            },
            Schema::default(),
            Schema::documents(),
            Vec::new(),
        );
        let ready = LogicalNode::new(
            "n1",
            LogicalOp::Filter {
                predicate: "index_status = 'ready'".into(),
            },
            Schema::documents(),
            Schema::documents(),
            vec![scan],
        );
        let docs = match normalize_metadata_filter(metadata_filter) {
            Some(json) => {
                let mut meta = LogicalNode::new(
                    "n1b",
                    LogicalOp::Filter {
                        predicate: format!("metadata {json}"),
                    },
                    Schema::documents(),
                    Schema::documents(),
                    vec![ready],
                );
                meta.hints.metadata_filter = Some(json);
                meta
            }
            None => ready,
        };
        let embed = LogicalNode::new(
            "n2",
            LogicalOp::Embed {
                input: EmbedInput::Query(query.into()),
            },
            Schema::documents(),
            Schema::embedding(),
            vec![docs],
        );
        let mut similarity = LogicalNode::new(
            "n3",
            LogicalOp::Similarity,
            Schema::embedding(),
            Schema::search_hits(),
            vec![embed],
        );
        if let Some(space) = space.filter(|s| !s.is_empty() && *s != "default") {
            let space = space.to_string();
            similarity.hints.space = Some(space.clone());
            if let Some(child) = similarity.children.get_mut(0) {
                child.hints.space = Some(space);
            }
        }
        let topk = LogicalNode::new(
            "n4",
            LogicalOp::TopK { k: k.max(1) },
            Schema::search_hits(),
            Schema::search_hits(),
            vec![similarity],
        );
        Self { root: topk }
    }

    pub fn generate(
        prompt: impl Into<String>,
        content: LlmContent,
        scan_table: Option<&str>,
        filter: Option<&str>,
    ) -> Self {
        let children = match scan_table {
            Some(table) => {
                let scan = LogicalNode::new(
                    "n0",
                    LogicalOp::Scan {
                        table: table.to_string(),
                    },
                    Schema::default(),
                    Schema::documents(),
                    Vec::new(),
                );
                match filter {
                    Some(predicate) if !predicate.is_empty() => {
                        vec![LogicalNode::new(
                            "n1",
                            LogicalOp::Filter {
                                predicate: predicate.to_string(),
                            },
                            Schema::documents(),
                            Schema::documents(),
                            vec![scan],
                        )]
                    }
                    _ => vec![scan],
                }
            }
            None => Vec::new(),
        };
        let llm = LogicalNode::new(
            "n2",
            LogicalOp::Llm {
                prompt: prompt.into(),
                content,
            },
            if children.is_empty() {
                Schema::default()
            } else {
                Schema::documents()
            },
            Schema::generated_text(),
            children,
        );
        Self { root: llm }
    }

    /// Generate over retrieval: Scan → Filter → Embed → Similarity → TopK → Llm.
    /// Sources come from the TopK hits. Not a citations table.
    pub fn generate_over_search(
        prompt: impl Into<String>,
        query: impl Into<String>,
        k: i64,
    ) -> Self {
        Self::generate_over_search_filtered(prompt, query, k, None)
    }

    pub fn generate_over_search_filtered(
        prompt: impl Into<String>,
        query: impl Into<String>,
        k: i64,
        metadata_filter: Option<&str>,
    ) -> Self {
        Self::generate_over_search_in(prompt, query, k, metadata_filter, None)
    }

    pub fn generate_over_search_in(
        prompt: impl Into<String>,
        query: impl Into<String>,
        k: i64,
        metadata_filter: Option<&str>,
        space: Option<&str>,
    ) -> Self {
        let search = Self::search_in(query, k, metadata_filter, space).root;
        let llm = LogicalNode::new(
            "n5",
            LogicalOp::Llm {
                prompt: prompt.into(),
                content: LlmContent::Column("content".into()),
            },
            Schema::search_hits(),
            Schema::generated_text(),
            vec![search],
        );
        Self { root: llm }
    }

    /// Naive generate-over-table: Filter after Llm so the optimizer can push it.
    pub fn generate_naive(
        prompt: impl Into<String>,
        content: LlmContent,
        scan_table: &str,
        filter: Option<&str>,
    ) -> Self {
        let scan = LogicalNode::new(
            "n0",
            LogicalOp::Scan {
                table: scan_table.to_string(),
            },
            Schema::default(),
            Schema::documents(),
            Vec::new(),
        );
        let llm = LogicalNode::new(
            "n2",
            LogicalOp::Llm {
                prompt: prompt.into(),
                content,
            },
            Schema::documents(),
            Schema::generated_text(),
            vec![scan],
        );
        match filter {
            Some(predicate) if !predicate.is_empty() => Self {
                root: LogicalNode::new(
                    "n1",
                    LogicalOp::Filter {
                        predicate: predicate.to_string(),
                    },
                    Schema::generated_text(),
                    Schema::generated_text(),
                    vec![llm],
                ),
            },
            _ => Self { root: llm },
        }
    }

    pub fn bind(&self, ctx: &BindContext) -> Result<()> {
        self.root.bind(ctx)
    }

    pub fn to_physical(&self, ctx: &BindContext) -> PhysicalPlan {
        PhysicalPlan {
            root: self.root.to_physical(ctx),
        }
    }

    pub fn search_args(&self) -> Option<(String, i64)> {
        search_args_from_op(&self.root)
    }

    pub fn metadata_filter(&self) -> Option<String> {
        metadata_filter_of(&self.root)
    }

    pub fn space(&self) -> Option<String> {
        space_of(&self.root)
    }

    pub fn is_search(&self) -> bool {
        matches!(self.root.op, LogicalOp::TopK { .. })
    }

    pub fn is_workflow(&self) -> bool {
        matches!(
            self.root.op,
            LogicalOp::Then
                | LogicalOp::Parallel
                | LogicalOp::Branch { .. }
                | LogicalOp::Loop { .. }
                | LogicalOp::Decide { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalNode {
    pub id: String,
    pub op: LogicalOp,
    pub algorithm: String,
    pub backend: PhysicalBackend,
    pub schema_out: Schema,
    pub contract: Contract,
    pub children: Vec<PhysicalNode>,
}

impl PhysicalNode {
    fn write_explain(&self, out: &mut String, indent: usize) {
        for _ in 0..indent {
            out.push_str("  ");
        }
        out.push_str(&self.explain_line());
        out.push('\n');
        for child in &self.children {
            child.write_explain(out, indent + 1);
        }
    }

    fn explain_line(&self) -> String {
        let tag = format!("{}/{}", self.backend.as_str(), self.algorithm);
        match &self.op {
            LogicalOp::Scan { table } => format!("Scan {table} [{tag}]"),
            LogicalOp::Filter { predicate } => format!("Filter {predicate} [{tag}]"),
            LogicalOp::Join { left, right, on } => {
                format!("Join {left} {right} on {on} [{tag}]")
            }
            LogicalOp::Aggregate { expr } => format!("Aggregate {expr} [{tag}]"),
            LogicalOp::Embed { input } => match input {
                EmbedInput::Query(query) => format!("Embed query={query:?} [{tag}]"),
                EmbedInput::Column(name) => format!("Embed column={name} [{tag}]"),
            },
            LogicalOp::Similarity => format!("Similarity [{tag}]"),
            LogicalOp::TopK { k } => format!("TopK k={k} [{tag}]"),
            LogicalOp::Llm { prompt, content } => {
                format!("Llm prompt={prompt:?} {content} [{tag}]")
            }
            LogicalOp::Tool { name } => format!("Tool {name} [{tag}]"),
            LogicalOp::Then => format!("Then [{tag}]"),
            LogicalOp::Parallel => format!("Parallel [{tag}]"),
            LogicalOp::Branch { predicate } => format!("Branch when={predicate:?} [{tag}]"),
            LogicalOp::Loop { until, max } => {
                format!("Loop until={until:?} max={max} [{tag}]")
            }
            LogicalOp::Decide { tools, max } => {
                format!("Decide tools={} max={max} [{tag}]", tools.join(","))
            }
        }
    }

    fn walk(&self, visit: &mut impl FnMut(&PhysicalNode)) {
        visit(self);
        for child in &self.children {
            child.walk(visit);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalPlan {
    pub root: PhysicalNode,
}

impl PhysicalPlan {
    pub fn explain(&self) -> String {
        let mut out = String::new();
        self.root.write_explain(&mut out, 0);
        out
    }

    pub fn search_args(&self) -> Option<(String, i64)> {
        search_args_from_op(&self.root)
    }

    pub fn llm_binding(&self) -> Option<(String, String)> {
        let mut found = None;
        self.root.walk(&mut |node| {
            if matches!(node.op, LogicalOp::Llm { .. }) {
                if let Some(rest) = node.algorithm.strip_prefix("llm ") {
                    if let Some((provider, model)) = rest.split_once(':') {
                        found = Some((provider.to_string(), model.to_string()));
                    }
                }
            }
        });
        found
    }
}

pub fn normalize_metadata_filter(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() || raw == "{}" || raw.eq_ignore_ascii_case("null") {
        return None;
    }
    Some(raw.to_string())
}

fn metadata_filter_of(node: &LogicalNode) -> Option<String> {
    if let Some(raw) = normalize_metadata_filter(node.hints.metadata_filter.as_deref()) {
        return Some(raw);
    }
    node.children.iter().find_map(metadata_filter_of)
}

fn space_of(node: &LogicalNode) -> Option<String> {
    if let Some(space) = node.hints.space.as_deref().filter(|s| !s.is_empty()) {
        return Some(space.to_string());
    }
    node.children.iter().find_map(space_of)
}

fn search_args_from_op<T: HasSearchArgs>(node: &T) -> Option<(String, i64)> {
    let mut query = None;
    let mut k = None;
    node.collect_search(&mut query, &mut k);
    Some((query?, k?))
}

trait HasSearchArgs {
    fn collect_search(&self, query: &mut Option<String>, k: &mut Option<i64>);
}

impl HasSearchArgs for LogicalNode {
    fn collect_search(&self, query: &mut Option<String>, k: &mut Option<i64>) {
        match &self.op {
            LogicalOp::Embed {
                input: EmbedInput::Query(text),
            } => *query = Some(text.clone()),
            LogicalOp::TopK { k: value } => *k = Some(*value),
            _ => {}
        }
        for child in &self.children {
            child.collect_search(query, k);
        }
    }
}

impl HasSearchArgs for PhysicalNode {
    fn collect_search(&self, query: &mut Option<String>, k: &mut Option<i64>) {
        match &self.op {
            LogicalOp::Embed {
                input: EmbedInput::Query(text),
            } => *query = Some(text.clone()),
            LogicalOp::TopK { k: value } => *k = Some(*value),
            _ => {}
        }
        for child in &self.children {
            child.collect_search(query, k);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub name: String,
    pub kind: String,
    pub provider: String,
    pub provider_model: String,
    pub key_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceRef {
    pub name: String,
    pub provider: String,
    pub provider_model: String,
    pub dimensions: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BindContext {
    pub tables: Vec<String>,
    pub functions: Vec<String>,
    pub models: Vec<ModelRef>,
    pub capabilities: Vec<String>,
    pub spaces: Vec<SpaceRef>,
}

impl BindContext {
    pub fn engine() -> Self {
        Self {
            tables: Vec::new(),
            functions: vec![
                "aidb_search".into(),
                "aidb_generate".into(),
                "ai_generate".into(),
                "aidb_classify".into(),
                "aidb_explain".into(),
                "aidb_insert_document".into(),
                "aidb_workflow".into(),
                "aidb_agent".into(),
                "aidb_experiment".into(),
                "aidb_resume".into(),
                "aidb_mcp_register".into(),
                "aidb_mcp_connect".into(),
                "aidb_mcp_disconnect".into(),
                "aidb_tool".into(),
                "aidb_memory_insert".into(),
                "aidb_memory_search".into(),
                "aidb_task".into(),
                "aidb_set_policy".into(),
                "aidb_get_policy".into(),
                "aidb_create_space".into(),
                "aidb_secret_store".into(),
            ],
            models: Vec::new(),
            capabilities: Vec::new(),
            spaces: Vec::new(),
        }
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.tables.iter().any(|table| table == name)
    }

    pub fn has_function(&self, name: &str) -> bool {
        self.functions.iter().any(|function| function == name)
    }

    pub fn has_capability(&self, name: &str) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability == name)
    }

    pub fn embedding(&self) -> (String, String) {
        self.embedding_for(None)
    }

    pub fn embedding_for(&self, space: Option<&str>) -> (String, String) {
        if let Some(name) = space.filter(|s| !s.is_empty() && *s != "default") {
            if let Some(space) = self.spaces.iter().find(|s| s.name == name) {
                return (space.provider.clone(), space.provider_model.clone());
            }
        }
        self.models
            .iter()
            .find(|model| model.kind == "embedding")
            .map(|model| (model.provider.clone(), model.provider_model.clone()))
            .unwrap_or_else(|| ("fake".into(), "aidb-fake".into()))
    }

    pub fn llm(&self) -> (String, String) {
        self.llm_model()
            .map(|model| (model.provider.clone(), model.provider_model.clone()))
            .unwrap_or_else(default_llm_from_env)
    }

    pub fn llm_catalog_name(&self) -> Option<&str> {
        self.llm_model().map(|model| model.name.as_str())
    }

    pub fn llm_key_name(&self) -> Option<&str> {
        self.llm_model()
            .and_then(|model| model.key_name.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    fn llm_model(&self) -> Option<&ModelRef> {
        self.models.iter().find(|model| model.kind == "llm")
    }
}

fn default_llm_from_env() -> (String, String) {
    match std::env::var("AIDB_LLM").ok().as_deref() {
        Some("openai") => (
            "openai".into(),
            std::env::var("AIDB_LLM_MODEL").unwrap_or_else(|_| "gpt-4.1-mini".into()),
        ),
        Some("anthropic") => (
            "anthropic".into(),
            std::env::var("AIDB_LLM_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".into()),
        ),
        _ => ("fake".into(), "aidb-fake".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_plan_is_scan_filter_embed_similarity_topk() {
        let plan = LogicalPlan::search("How do refunds work?", 5);
        let ops = collect_ops(&plan.root);
        assert_eq!(ops, ["TopK", "Similarity", "Embed", "Filter", "Scan",]);
        assert_eq!(plan.search_args(), Some(("How do refunds work?".into(), 5)));
        assert_eq!(plan.root.contract.determinism, Determinism::Strict);
    }

    #[test]
    fn generate_over_search_is_llm_over_retrieval() {
        let plan =
            LogicalPlan::generate_over_search("What is the refund policy?", "refund policy", 5);
        let ops = collect_ops(&plan.root);
        assert_eq!(
            ops,
            ["Llm", "TopK", "Similarity", "Embed", "Filter", "Scan",]
        );
        assert_eq!(plan.search_args(), Some(("refund policy".into(), 5)));
        assert!(!plan.is_search());
    }

    #[test]
    fn search_filtered_adds_metadata_filter() {
        let plan = LogicalPlan::search_filtered("refund policy", 5, Some(r#"{"dept":"support"}"#));
        let ops = collect_ops(&plan.root);
        assert_eq!(
            ops,
            ["TopK", "Similarity", "Embed", "Filter", "Filter", "Scan",]
        );
        assert_eq!(
            plan.metadata_filter().as_deref(),
            Some(r#"{"dept":"support"}"#)
        );
        assert!(matches!(
            &plan.root.children[0].children[0].children[0].op,
            LogicalOp::Filter { predicate } if predicate.contains("dept")
        ));
    }

    #[test]
    fn bind_rejects_unknown_table() {
        let plan = LogicalPlan::generate(
            "Summarize this",
            LlmContent::Column("content".into()),
            Some("missing"),
            None,
        );
        let err = plan.bind(&BindContext::engine()).unwrap_err();
        assert!(err.to_string().contains("unknown table: missing"), "{err}");
    }

    #[test]
    fn physical_explain_is_indented() {
        let mut ctx = BindContext::engine();
        ctx.tables.push("documents".into());
        let plan = LogicalPlan::search("refunds", 5)
            .to_physical(&ctx)
            .explain();
        assert!(plan.contains("TopK k=5 [sqlite/limit]"), "{plan}");
        assert!(
            plan.contains("Similarity [sqlite/sqlite-vec knn]"),
            "{plan}"
        );
        assert!(
            plan.contains("Embed query=\"refunds\" [ai/embed fake:aidb-fake]"),
            "{plan}"
        );
        assert!(plan.contains("  Scan documents [sqlite/seqscan]"), "{plan}");
    }

    fn collect_ops(node: &LogicalNode) -> Vec<&'static str> {
        let mut ops = vec![op_name(&node.op)];
        for child in &node.children {
            ops.extend(collect_ops(child));
        }
        ops
    }

    fn op_name(op: &LogicalOp) -> &'static str {
        match op {
            LogicalOp::Scan { .. } => "Scan",
            LogicalOp::Filter { .. } => "Filter",
            LogicalOp::Join { .. } => "Join",
            LogicalOp::Aggregate { .. } => "Aggregate",
            LogicalOp::Embed { .. } => "Embed",
            LogicalOp::Similarity => "Similarity",
            LogicalOp::TopK { .. } => "TopK",
            LogicalOp::Llm { .. } => "Llm",
            LogicalOp::Tool { .. } => "Tool",
            LogicalOp::Then => "Then",
            LogicalOp::Parallel => "Parallel",
            LogicalOp::Branch { .. } => "Branch",
            LogicalOp::Loop { .. } => "Loop",
            LogicalOp::Decide { .. } => "Decide",
        }
    }
}
