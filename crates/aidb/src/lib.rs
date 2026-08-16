//! Public crate: open a file, execute SQL, query rows.

pub use aidb_ai::{register_custom_embedder, EmbedderConfig};
pub use aidb_core::{Error, QueryResult, Result, Retrieval, Value, SCHEMA_VERSION};
pub use aidb_run::{clear_last_run_id, subscribe_tokens, TokenEvent};
pub use aidb_storage::Store;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

mod agent;
mod experiment;
mod goal;
mod memory;
mod resume;
mod tool;
mod workflow;

use aidb_ai::{embedder, Embedder};
use aidb_index::Indexer;

pub struct Aidb {
    store: Arc<Store>,
    embedder: Arc<dyn Embedder>,
    indexer: Indexer,
}

pub fn open(path: impl AsRef<Path>) -> Result<Aidb> {
    open_with(path, EmbedderConfig::default())
}

pub fn open_with(path: impl AsRef<Path>, config: EmbedderConfig) -> Result<Aidb> {
    let store = Arc::new(Store::open(path)?);
    if let Some(existing) = store.meta_get("embedding_dimensions")? {
        let stored: usize = existing
            .parse()
            .map_err(|_| Error::schema("embedding_dimensions is not an integer"))?;
        if stored != config.dimensions {
            return Err(Error::schema(format!(
                "database embedding dimensions are {stored}, engine opened with {}",
                config.dimensions
            )));
        }
    }
    if let Some(stored) = store.meta_get("embedding_provider")? {
        if stored != config.provider {
            return Err(Error::schema(format!(
                "database embedding provider is {stored}, engine opened with {}",
                config.provider
            )));
        }
    }
    if let Some(stored) = store.meta_get("embedding_model")? {
        if stored != config.model {
            return Err(Error::schema(format!(
                "database embedding model is {stored}, engine opened with {}",
                config.model
            )));
        }
    }
    let embedder = embedder(&config)?;
    store.write(aidb_sql::register)?;
    store.write(aidb_run::recover_interrupted)?;
    let indexer = Indexer::start(Arc::clone(&store), Arc::clone(&embedder));
    let db = Aidb {
        store,
        embedder,
        indexer,
    };
    db.store.write(aidb_index::prune_orphan_vectors)?;
    db.store.write(aidb_index::enqueue_untracked)?;
    db.indexer.notify();
    resume_durable(&db)?;
    Ok(db)
}

fn resume_durable(db: &Aidb) -> Result<()> {
    let pending = db.store.write(aidb_run::running_durable)?;
    for (id, kind, spec_json) in pending {
        let paused = db
            .store
            .write(|conn| aidb_run::has_unresolved_pause(conn, &id))?;
        if paused {
            let status = db
                .store
                .write(|conn| aidb_run::pause_status(conn, &id))?
                .unwrap_or_else(|| "awaiting_approval".into());
            db.store
                .write(|conn| aidb_run::park_run(conn, &id, &status, None))?;
            continue;
        }
        match kind.as_str() {
            "agent" => agent::resume(db, &id, &spec_json)?,
            _ => {
                if let Err(err) = workflow::resume_one(db, &id, &spec_json) {
                    let message = err.to_string();
                    let _ = db
                        .store
                        .write(|conn| aidb_run::finish_run(conn, &id, "failed", Some(&message)));
                }
            }
        }
    }
    Ok(())
}

impl Aidb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        open(path)
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    pub fn execute(&self, sql: &str) -> Result<u64> {
        if aidb_sql::parse_aidb_insert_document(sql).is_some() {
            let _ = self.query(sql)?;
            return Ok(1);
        }
        if sql.trim_start().len() >= 12
            && sql.trim_start()[..12].eq_ignore_ascii_case("create model")
        {
            let spec = aidb_sql::parse_create_model(sql).ok_or_else(|| {
                Error::usage("invalid CREATE MODEL; store a key_name, never the secret")
            })?;
            return self
                .store
                .write(|conn| aidb_sql::execute_create_model(conn, &spec));
        }
        let changed = self.store.execute(sql)?;
        self.after_write()?;
        Ok(changed)
    }

    pub fn query(&self, sql: &str) -> Result<QueryResult> {
        if let Some(inner) = aidb_sql::strip_explain(sql) {
            if aidb_sql::parse_aidb_resume(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text("Resume [control]".into())]],
                });
            }
            if aidb_sql::parse_aidb_mcp_register(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "McpRegister [tool]  (writes capabilities)".into(),
                    )]],
                });
            }
            if aidb_sql::parse_aidb_mcp_connect(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "McpConnect [tool]  (stdio → capabilities)".into(),
                    )]],
                });
            }
            if aidb_sql::parse_aidb_mcp_disconnect(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "McpDisconnect [tool]  (keeps catalog rows)".into(),
                    )]],
                });
            }
            if let Some((name, _)) = aidb_sql::parse_aidb_tool(inner) {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(format!("Tool {name} [tool/runtime]"))]],
                });
            }
            if aidb_sql::parse_aidb_set_policy(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "SetPolicy [policy]  (writes aidb_meta, no secrets)".into(),
                    )]],
                });
            }
            if aidb_sql::parse_aidb_get_policy(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "GetPolicy [policy]  (reads aidb_meta)".into(),
                    )]],
                });
            }
            if aidb_sql::parse_aidb_secret_store(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "SecretStore [ai]  (env, then keychain or file: outside the db)".into(),
                    )]],
                });
            }
            if let Some(call) = aidb_sql::parse_aidb_session(inner) {
                let plan = match call {
                    aidb_sql::SessionCall::Get => {
                        "Session [control]  (current bind; later runs join this plan's thread)"
                            .to_string()
                    }
                    aidb_sql::SessionCall::Clear => {
                        "SessionClear [control]  (later runs are unscoped)".to_string()
                    }
                    aidb_sql::SessionCall::Bind(name) => {
                        format!(
                            "SessionBind {name} [control]  (later generate/agent runs join this plan's thread)"
                        )
                    }
                };
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(plan)]],
                });
            }
            if aidb_sql::parse_aidb_last_run_id(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "LastRunId [control]  (this connection's last inserted run id)".into(),
                    )]],
                });
            }
            if let Some((name, ..)) = aidb_sql::parse_aidb_create_space(inner) {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(format!(
                        "CreateSpace {name} [index]  (named vec table, default space unchanged)"
                    ))]],
                });
            }
            if aidb_sql::parse_aidb_memory_insert(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "MemoryInsert [data]  (documents + memory view)".into(),
                    )]],
                });
            }
            if aidb_sql::parse_create_model(inner).is_some() {
                return Ok(QueryResult {
                    columns: vec!["plan".into()],
                    rows: vec![vec![Value::Text(
                        "CreateModel [catalog]  (writes models, key name only)".into(),
                    )]],
                });
            }
            if aidb_sql::parse_aidb_memory_search(inner).is_some() {
                return self.explain_sql(inner);
            }
            if aidb_sql::looks_like_goal(inner)
                || aidb_sql::parse_aidb_workflow(inner).is_some()
                || aidb_sql::parse_aidb_agent(inner).is_some()
                || aidb_sql::logical_plan(inner).is_some()
            {
                return self.explain_sql(inner);
            }
        }
        if let Some(inner) = aidb_sql::parse_aidb_explain(sql) {
            return self.explain_sql(&inner);
        }
        if let Some((run_id, decision)) = aidb_sql::parse_aidb_resume(sql) {
            return resume::resume_sql(self, &run_id, &decision);
        }
        if let Some(json) = aidb_sql::parse_aidb_mcp_register(sql) {
            return tool::mcp_register(self, &json);
        }
        if let Some((transport, command)) = aidb_sql::parse_aidb_mcp_connect(sql) {
            return tool::mcp_connect(self, &transport, &command);
        }
        if aidb_sql::parse_aidb_mcp_disconnect(sql).is_some() {
            return tool::mcp_disconnect(self);
        }
        if let Some((name, args)) = aidb_sql::parse_aidb_tool(sql) {
            return tool::invoke_sql(self, &name, &args);
        }
        if let Some((json, name)) = aidb_sql::parse_aidb_set_policy(sql) {
            return tool::set_policy(self, &json, name.as_deref());
        }
        if aidb_sql::parse_aidb_get_policy(sql).is_some() {
            return tool::get_policy(self);
        }
        if let Some((name, provider, dimensions, model, distance)) =
            aidb_sql::parse_aidb_create_space(sql)
        {
            return self.store.write(|conn| {
                aidb_index::create_and_fill(
                    conn,
                    &name,
                    &provider,
                    dimensions,
                    model.as_deref(),
                    distance.as_deref(),
                    self.embedder.as_ref(),
                )
            });
        }
        if aidb_sql::looks_like_goal(sql) {
            return goal::run_sql(self, sql);
        }
        if let Some(json) = aidb_sql::parse_aidb_agent(sql) {
            return agent::run(self, &json);
        }
        if let Some(json) = aidb_sql::parse_aidb_experiment(sql) {
            return experiment::run(self, &json);
        }
        if let Some(json) = aidb_sql::parse_aidb_workflow(sql) {
            return workflow::run(self, &json);
        }
        if let Some(call) = aidb_sql::parse_aidb_generate(sql) {
            match call.from {
                Some(aidb_sql::GenerateFrom::Search {
                    query,
                    k,
                    filter,
                    space,
                }) => {
                    return self.store.write(|conn| {
                        aidb_sql::execute_rag_generate(
                            conn,
                            self.embedder.as_ref(),
                            &call.prompt,
                            &query,
                            k,
                            filter.as_deref(),
                            space.as_deref(),
                            call.schema.as_deref(),
                        )
                    });
                }
                Some(aidb_sql::GenerateFrom::Table { name, filter }) => {
                    return self.store.write(|conn| {
                        aidb_sql::execute_optimized_generate(
                            conn,
                            self.embedder.as_ref(),
                            &call.prompt,
                            &name,
                            filter.as_deref(),
                            call.schema.as_deref(),
                        )
                    });
                }
                None => {}
            }
        }
        if let Some(call) = aidb_sql::parse_aidb_classify(sql) {
            match call.from {
                Some(aidb_sql::GenerateFrom::Search {
                    query,
                    k,
                    filter,
                    space,
                }) => {
                    return self.store.write(|conn| {
                        aidb_sql::execute_rag_classify(
                            conn,
                            self.embedder.as_ref(),
                            &call.prompt,
                            &query,
                            k,
                            filter.as_deref(),
                            space.as_deref(),
                            call.schema.as_deref(),
                        )
                    });
                }
                Some(aidb_sql::GenerateFrom::Table { name, filter }) => {
                    return self.store.write(|conn| {
                        aidb_sql::execute_optimized_classify(
                            conn,
                            self.embedder.as_ref(),
                            &call.prompt,
                            &name,
                            filter.as_deref(),
                            call.schema.as_deref(),
                        )
                    });
                }
                None => {}
            }
        }
        if let Some(plan) = aidb_sql::logical_plan(sql) {
            if plan.is_search() {
                let hits = self
                    .store
                    .write(|conn| aidb_sql::execute_search(conn, self.embedder.as_ref(), &plan))?;
                return aidb_sql::project_selection(sql, hits);
            }
        }
        if let Some((scope, content)) = aidb_sql::parse_aidb_memory_insert(sql) {
            return memory::insert(self, &scope, &content);
        }
        if let Some((query, k, scope)) = aidb_sql::parse_aidb_memory_search(sql) {
            let hits = memory::search(self, &query, k, scope.as_deref())?;
            return aidb_sql::project_selection(sql, hits);
        }
        if let Some((title, content, metadata)) = aidb_sql::parse_aidb_insert_document(sql) {
            let title = if title.is_empty() { None } else { Some(title) };
            let id = self.store.write(|conn| {
                aidb_index::insert_document(conn, title.as_deref(), &content, &metadata)
            })?;
            self.after_write()?;
            return Ok(QueryResult {
                columns: vec!["id".into()],
                rows: vec![vec![Value::Text(id)]],
            });
        }
        self.store.query(sql)
    }

    pub fn journal_mode(&self) -> Result<String> {
        self.store.journal_mode()
    }

    pub fn drain_index(&self, timeout: Duration) -> Result<()> {
        self.indexer.drain(timeout)
    }

    pub fn resume(&self, run_id: &str, decision_json: &str) -> Result<QueryResult> {
        resume::resume_sql(self, run_id, decision_json)
    }

    /// Bindings entry point: query or execute, then drain the indexer after writes.
    pub fn sql(&self, sql: &str) -> Result<SqlOutput> {
        if is_query_sql(sql) {
            let result = self.query(sql)?;
            self.maybe_drain(sql)?;
            Ok(SqlOutput::Query(result))
        } else {
            let changed = self.execute(sql)?;
            self.maybe_drain(sql)?;
            Ok(SqlOutput::Execute(changed))
        }
    }

    fn maybe_drain(&self, sql: &str) -> Result<()> {
        let lower = sql.to_ascii_lowercase();
        if lower.contains("insert")
            || lower.contains("aidb_insert_document")
            || lower.contains("aidb_memory_insert")
        {
            self.drain_index(Duration::from_secs(60))?;
        }
        Ok(())
    }

    fn explain_sql(&self, sql: &str) -> Result<QueryResult> {
        let embedder = Arc::clone(&self.embedder);
        let text = self
            .store
            .write(|conn| aidb_sql::explain_sql_with(conn, sql, Some(embedder.as_ref())))?;
        Ok(QueryResult {
            columns: vec!["plan".into()],
            rows: vec![vec![Value::Text(text)]],
        })
    }

    pub(crate) fn after_write(&self) -> Result<()> {
        self.store.write(aidb_index::enqueue_untracked)?;
        self.indexer.notify();
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SqlOutput {
    Query(QueryResult),
    Execute(u64),
}

pub fn is_query_sql(sql: &str) -> bool {
    let trimmed = sql.trim_start();
    ["select", "pragma", "with", "explain", "search", "task"]
        .iter()
        .any(|kw| {
            trimmed
                .get(..kw.len())
                .is_some_and(|h| h.eq_ignore_ascii_case(kw))
        })
}

pub fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Integer(v) => serde_json::json!(v),
        Value::Real(v) => serde_json::json!(v),
        Value::Text(v) => serde_json::json!(v),
        Value::Blob(v) => {
            const DIGITS: &[u8; 16] = b"0123456789abcdef";
            let mut out = String::with_capacity(v.len() * 2 + 3);
            out.push_str("X'");
            for b in v {
                out.push(DIGITS[(b >> 4) as usize] as char);
                out.push(DIGITS[(b & 0x0f) as usize] as char);
            }
            out.push('\'');
            serde_json::json!(out)
        }
    }
}

pub fn query_to_json(result: &QueryResult) -> serde_json::Value {
    let rows: Vec<Vec<serde_json::Value>> = result
        .rows
        .iter()
        .map(|row| row.iter().map(value_to_json).collect())
        .collect();
    serde_json::json!({
        "columns": result.columns,
        "rows": rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_db() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("aidb-phase1-{nanos}-{seq}.db"))
    }

    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn open_migrates_schema_and_sets_wal() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        let version = db
            .query("SELECT value FROM aidb_meta WHERE key = 'schema_version'")
            .expect("query");
        assert_eq!(version.rows[0][0].to_string(), SCHEMA_VERSION.to_string());
        assert_eq!(db.journal_mode().expect("journal").to_lowercase(), "wal");
        let again = Aidb::open(&path).expect("reopen");
        let version = again
            .query("SELECT value FROM aidb_meta WHERE key = 'schema_version'")
            .expect("query again");
        assert_eq!(version.rows[0][0].to_string(), SCHEMA_VERSION.to_string());
        drop(again);
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn insert_indexes_and_search_finds_document() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        let inserted = db
            .query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        assert_eq!(inserted.columns, ["id"]);
        db.drain_index(Duration::from_secs(5)).expect("index");

        let status = db
            .query("SELECT index_status FROM documents")
            .expect("status");
        assert_eq!(status.rows[0][0].to_string(), "ready");

        let hits = db
            .query("SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5)")
            .expect("search");
        assert!(!hits.rows.is_empty(), "expected a search hit");
        assert!(hits.rows[0][2]
            .to_string()
            .to_ascii_lowercase()
            .contains("refund"));
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn raw_insert_is_enqueued() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.execute(
            "INSERT INTO documents (id, title, content, content_hash, created_at_ms, updated_at_ms)
             VALUES ('doc_1', 'Shipping', 'Orders ship within two business days.', 'hash', 1, 1)",
        )
        .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");
        let status = db
            .query("SELECT id, index_status FROM documents")
            .expect("status");
        assert_eq!(status.rows[0][0].to_string(), "doc_1");
        assert_eq!(status.rows[0][1].to_string(), "ready");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn generate_writes_a_run() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.execute(
            "INSERT INTO models (name, kind, provider, provider_model, created_at_ms)
             VALUES ('fake-llm', 'llm', 'fake', 'aidb-fake', 1)",
        )
        .expect("model");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let generated = db
            .query("SELECT aidb_generate('Summarize this', content) FROM documents")
            .expect("generate");
        let text = generated.rows[0][0].to_string();
        assert!(text.contains("Summarize this"), "{text}");
        assert!(text.to_ascii_lowercase().contains("refund"), "{text}");

        let runs = db
            .query("SELECT kind, status, prompt_tokens, cost_usd FROM runs WHERE kind = 'generate'")
            .expect("runs");
        assert_eq!(runs.rows.len(), 1);
        assert_eq!(runs.rows[0][0].to_string(), "generate");
        assert_eq!(runs.rows[0][1].to_string(), "succeeded");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn search_writes_a_run() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");
        db.query("SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5)")
            .expect("search");

        let runs = db
            .query("SELECT kind, status, output_json FROM runs WHERE kind = 'search'")
            .expect("runs");
        assert_eq!(runs.rows.len(), 1);
        assert_eq!(runs.rows[0][0].to_string(), "search");
        assert_eq!(runs.rows[0][1].to_string(), "succeeded");
        assert!(
            runs.rows[0][2].to_string().contains("\"hits\":"),
            "{}",
            runs.rows[0][2]
        );
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn resume_index_after_chunk_checkpoint() {
        let path = temp_db();
        {
            let store = Store::open(&path).expect("store");
            store
                .execute(
                    "INSERT INTO documents
                        (id, title, content, metadata_json, content_hash, index_status, index_run_id, created_at_ms, updated_at_ms)
                     VALUES
                        ('doc_crash', 'Refunds', 'Refunds are issued within 14 days of purchase.', '{}', 'hash', 'indexing', 'run_crash', 1, 1);
                     INSERT INTO chunks (document_id, ordinal, content, created_at_ms)
                     VALUES ('doc_crash', 0, 'Refunds are issued within 14 days of purchase.', 1);
                     INSERT INTO runs (id, kind, status, document_id, created_at_ms, started_at_ms)
                     VALUES ('run_crash', 'index_document', 'running', 'doc_crash', 1, 1);
                     INSERT INTO checkpoints (run_id, node_id, seq, artifact_json, created_at_ms)
                     VALUES ('run_crash', 'chunk', 1, '{\"chunks\":1}', 1);",
                )
                .expect("seed crash state");
        }

        let db = Aidb::open(&path).expect("reopen");
        db.drain_index(Duration::from_secs(5)).expect("resume");

        let status = db
            .query("SELECT index_status FROM documents WHERE id = 'doc_crash'")
            .expect("status");
        assert_eq!(status.rows[0][0].to_string(), "ready");

        let vecs = db
            .query("SELECT COUNT(*) FROM vec_chunks WHERE document_id = 'doc_crash'")
            .expect("vec");
        assert_eq!(vecs.rows[0][0].to_string(), "1");

        let checkpoints = db
            .query("SELECT node_id FROM checkpoints WHERE run_id = 'run_crash' ORDER BY node_id")
            .expect("checkpoints");
        let nodes: Vec<String> = checkpoints
            .rows
            .iter()
            .map(|row| row[0].to_string())
            .collect();
        assert_eq!(nodes, ["chunk", "embed"]);

        let events = db
            .query("SELECT kind FROM run_events WHERE run_id = 'run_crash' ORDER BY seq")
            .expect("events");
        let kinds: Vec<String> = events.rows.iter().map(|row| row[0].to_string()).collect();
        assert!(kinds.contains(&"resume".into()), "{kinds:?}");
        assert!(kinds.contains(&"embed".into()), "{kinds:?}");

        let run = db
            .query("SELECT status FROM runs WHERE id = 'run_crash'")
            .expect("run");
        assert_eq!(run.rows[0][0].to_string(), "succeeded");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn recover_interrupted_generate_and_list_failed() {
        let path = temp_db();
        {
            let db = Aidb::open(&path).expect("open");
            db.execute(
                "INSERT INTO runs (id, kind, status, input_json, created_at_ms, started_at_ms)
                 VALUES ('run_gen', 'generate', 'running', '{}', 1, 1)",
            )
            .expect("seed running generate");
            drop(db);
        }

        let db = Aidb::open(&path).expect("reopen");
        let failed = db
            .query("SELECT id, status, error FROM runs WHERE status = 'failed'")
            .expect("failed");
        assert_eq!(failed.rows.len(), 1);
        assert_eq!(failed.rows[0][0].to_string(), "run_gen");
        assert_eq!(failed.rows[0][1].to_string(), "failed");
        assert_eq!(failed.rows[0][2].to_string(), "interrupted");

        let events = db
            .query("SELECT kind FROM run_events WHERE run_id = 'run_gen'")
            .expect("events");
        assert_eq!(events.rows[0][0].to_string(), "interrupted");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn explain_search_and_generate_print_physical_plans() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let search = db
            .query("EXPLAIN SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5)")
            .expect("explain search");
        let search_plan = search.rows[0][0].to_string();
        assert!(search_plan.contains("TopK k=5"), "{search_plan}");
        assert!(search_plan.contains("Similarity"), "{search_plan}");
        assert!(
            search_plan.contains("hybrid rrf (vec+fts)"),
            "{search_plan}"
        );
        assert!(search_plan.contains("HybridFtsVec"), "{search_plan}");
        assert!(search_plan.contains("Embed query="), "{search_plan}");
        assert!(search_plan.contains("Scan documents"), "{search_plan}");

        let generate = db
            .query("EXPLAIN SELECT aidb_generate('Summarize this', content) FROM documents")
            .expect("explain generate");
        let generate_plan = generate.rows[0][0].to_string();
        assert!(generate_plan.contains("Llm prompt="), "{generate_plan}");
        assert!(generate_plan.contains("column=content"), "{generate_plan}");
        assert!(generate_plan.contains("Scan documents"), "{generate_plan}");

        let via_fn = db
            .query("SELECT aidb_explain('SELECT aidb_search(''refunds'', 5)')")
            .expect("aidb_explain");
        assert!(
            via_fn.rows[0][0].to_string().contains("TopK k=5"),
            "{}",
            via_fn.rows[0][0]
        );
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn explain_unknown_table_fails_bind() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        let err = db
            .query("EXPLAIN SELECT aidb_generate('x', content) FROM no_such_table")
            .expect_err("bind");
        assert!(
            err.to_string().contains("unknown table: no_such_table"),
            "{err}"
        );
        drop(db);
        cleanup(&path);
    }

    const THEN_WORKFLOW: &str = r#"{"then":[{"search":{"query":"How do refunds work?","k":5}},{"generate":{"prompt":"Summarize this"}}]}"#;

    #[test]
    fn workflow_then_search_generate() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let explained = db
            .query(&format!("EXPLAIN SELECT aidb_workflow('{THEN_WORKFLOW}')"))
            .expect("explain");
        let plan = explained.rows[0][0].to_string();
        assert!(plan.contains("Then [control/seq]"), "{plan}");
        assert!(plan.contains("TopK k=5"), "{plan}");
        assert!(plan.contains("Llm prompt="), "{plan}");

        let ran = db
            .query(&format!("SELECT aidb_workflow('{THEN_WORKFLOW}')"))
            .expect("run");
        assert_eq!(ran.columns, ["run_id", "status", "output"]);
        assert_eq!(ran.rows[0][1].to_string(), "succeeded");
        assert!(
            ran.rows[0][2]
                .to_string()
                .to_ascii_lowercase()
                .contains("refund"),
            "{}",
            ran.rows[0][2]
        );

        let parent = ran.rows[0][0].to_string();
        let children = db
            .query(&format!(
                "SELECT kind, status FROM runs WHERE parent_id = '{parent}' ORDER BY kind"
            ))
            .expect("children");
        assert_eq!(children.rows.len(), 2);
        assert_eq!(children.rows[0][0].to_string(), "generate");
        assert_eq!(children.rows[1][0].to_string(), "search");

        let parent_kind = db
            .query(&format!(
                "SELECT kind, status FROM runs WHERE id = '{parent}'"
            ))
            .expect("parent");
        assert_eq!(parent_kind.rows[0][0].to_string(), "workflow");
        assert_eq!(parent_kind.rows[0][1].to_string(), "succeeded");

        let checkpoints = db
            .query(&format!(
                "SELECT node_id FROM checkpoints WHERE run_id = '{parent}' ORDER BY node_id"
            ))
            .expect("checkpoints");
        let nodes: Vec<String> = checkpoints
            .rows
            .iter()
            .map(|row| row[0].to_string())
            .collect();
        assert!(nodes.contains(&"w".into()), "{nodes:?}");
        assert!(nodes.contains(&"w.0".into()), "{nodes:?}");
        assert!(nodes.contains(&"w.1".into()), "{nodes:?}");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn workflow_branch_and_parallel() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let branch = r#"{"then":[{"search":{"query":"refunds","k":3}},{"branch":{"when":"hits > 0","then":{"generate":{"prompt":"Summarize this"}}}}]}"#;
        let ran = db
            .query(&format!("SELECT aidb_workflow('{branch}')"))
            .expect("branch");
        assert_eq!(ran.rows[0][1].to_string(), "succeeded");
        assert!(
            ran.rows[0][2].to_string().contains("Summarize this"),
            "{}",
            ran.rows[0][2]
        );

        let parallel = r#"{"parallel":[{"search":{"query":"refunds","k":2}},{"search":{"query":"purchase","k":2}}]}"#;
        let ran = db
            .query(&format!("SELECT aidb_workflow('{parallel}')"))
            .expect("parallel");
        assert_eq!(ran.rows[0][1].to_string(), "succeeded");
        let parent = ran.rows[0][0].to_string();
        let kids = db
            .query(&format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'search'"
            ))
            .expect("kids");
        assert_eq!(kids.rows[0][0].to_string(), "2");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn workflow_resumes_after_search_checkpoint() {
        let path = temp_db();
        {
            let db = Aidb::open(&path).expect("open");
            db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
                .expect("insert");
            db.drain_index(Duration::from_secs(5)).expect("index");
            drop(db);
        }
        {
            let store = Store::open(&path).expect("store");
            store
                .execute(
                    r#"INSERT INTO runs (id, kind, status, input_json, created_at_ms, started_at_ms)
                       VALUES ('run_wf', 'workflow', 'running', '{"then":[{"search":{"query":"refunds","k":5}},{"generate":{"prompt":"Summarize this"}}]}', 1, 1);
                       INSERT INTO checkpoints (run_id, node_id, seq, artifact_json, created_at_ms)
                       VALUES ('run_wf', 'w.0', 1, '{"hits":1,"text":"Refunds are issued within 14 days of purchase."}', 1);"#,
                )
                .expect("seed");
        }

        let db = Aidb::open(&path).expect("resume");
        let parent = db
            .query("SELECT status FROM runs WHERE id = 'run_wf'")
            .expect("parent");
        assert_eq!(parent.rows[0][0].to_string(), "succeeded");

        let generate = db
            .query("SELECT kind, status FROM runs WHERE parent_id = 'run_wf' AND kind = 'generate'")
            .expect("generate");
        assert_eq!(generate.rows.len(), 1);
        assert_eq!(generate.rows[0][1].to_string(), "succeeded");

        let events = db
            .query("SELECT kind FROM run_events WHERE run_id = 'run_wf' AND kind = 'step'")
            .expect("steps");
        assert!(!events.rows.is_empty());
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn explain_generate_pushes_filter_and_lists_rewrites() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        let plan = db
            .query("EXPLAIN SELECT aidb_generate('Summarize this', content) FROM documents WHERE index_status = 'ready'")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Llm prompt="), "{plan}");
        assert!(plan.contains("Filter index_status = 'ready'"), "{plan}");
        assert!(plan.contains("PushFilterBeforeExpensive"), "{plan}");
        assert!(plan.contains("CacheKeyedAiCall"), "{plan}");
        assert!(plan.contains("Budget max_llm_calls="), "{plan}");
        let llm_at = plan.find("Llm ").expect("llm");
        let filter_at = plan.find("Filter ").expect("filter");
        assert!(llm_at < filter_at, "filter should sit under llm:\n{plan}");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn cascade_is_cheaper_than_naive_per_row_llm() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("refunds");
        for i in 0..7 {
            db.query(&format!(
                "SELECT aidb_insert_document('Shipping {i}', 'Orders ship within two business days via ground service.', '{{}}')"
            ))
            .expect("shipping");
        }
        db.drain_index(Duration::from_secs(10)).expect("index");

        let plan = db
            .query("EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("CascadeEmbedTopKThenJudge"), "{plan}");
        assert!(plan.contains("Then [control/seq]"), "{plan}");

        let generated = db
            .query("SELECT aidb_generate('How do refunds work?', content) FROM documents")
            .expect("generate");
        assert_eq!(generated.rows.len(), 1);
        assert!(
            generated.rows[0][0]
                .to_string()
                .to_ascii_lowercase()
                .contains("refund"),
            "{}",
            generated.rows[0][0]
        );

        let llm_calls = db
            .query("SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND error IS NULL")
            .expect("calls");
        assert_eq!(
            llm_calls.rows[0][0].to_string(),
            "1",
            "cascade should beat 8 naive LLM calls"
        );
        let searches = db
            .query("SELECT COUNT(*) FROM runs WHERE kind = 'search'")
            .expect("search");
        assert_eq!(searches.rows[0][0].to_string(), "1");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn labeled_gold_job_is_cheaper_than_naive_and_records_spend() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        for i in 0..16 {
            db.query(&format!(
                "SELECT aidb_insert_document('Refunds {i}', 'How do refunds work? Refunds are issued within 14 days of purchase.', '{{}}')"
            ))
            .expect("refunds");
        }
        for i in 0..240 {
            db.query(&format!(
                "SELECT aidb_insert_document('Shipping {i}', 'Orders ship within two business days via ground service.', '{{}}')"
            ))
            .expect("shipping");
        }
        db.drain_index(Duration::from_secs(30)).expect("index");

        let plan = db
            .query("EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("CascadeEmbedTopKThenJudge"), "{plan}");
        assert!(plan.contains("sample_recall="), "{plan}");
        assert!(plan.contains("Budget max_llm_calls="), "{plan}");
        assert!(plan.contains("max_usd="), "{plan}");
        assert!(plan.contains("max_ms="), "{plan}");

        let generated = db
            .query("SELECT aidb_generate('How do refunds work?', content) FROM documents")
            .expect("generate");
        assert_eq!(generated.rows.len(), 1, "cascade should not emit 256 rows");
        assert!(
            generated.rows[0][0]
                .to_string()
                .to_ascii_lowercase()
                .contains("refund"),
            "{}",
            generated.rows[0][0]
        );

        let llm_calls = db
            .query("SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND error IS NULL")
            .expect("calls");
        assert_eq!(
            llm_calls.rows[0][0].to_string(),
            "1",
            "256-row gold job must beat naive per-row LLM"
        );

        let measured = db
            .query(
                "SELECT prompt_tokens, completion_tokens, cost_usd, output_json,
                        finished_at_ms - started_at_ms
                 FROM runs WHERE kind = 'generate'",
            )
            .expect("measured");
        assert!(measured.rows[0][0].to_string().parse::<i64>().unwrap() > 0);
        assert!(measured.rows[0][1].to_string().parse::<i64>().unwrap() > 0);
        assert!(measured.rows[0][2].to_string().parse::<f64>().unwrap() > 0.0);
        let output = measured.rows[0][3].to_string();
        assert!(output.contains("\"job\""), "{output}");
        assert!(output.contains("\"llm_calls\""), "{output}");
        assert!(output.contains("\"usd\""), "{output}");
        assert!(output.contains("\"ms\""), "{output}");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn usd_and_ms_budgets_are_enforced() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'How do refunds work? Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let usd = aidb_sql::with_budget(
            aidb_opt::Budget {
                max_usd: Some(0.0),
                max_ms: None,
                max_llm_calls: Some(64),
            },
            || db.query("SELECT aidb_generate('How do refunds work?', content) FROM documents"),
        )
        .expect_err("usd");
        assert!(usd.to_string().contains("max_usd"), "{usd}");

        let ms = aidb_sql::with_budget(
            aidb_opt::Budget {
                max_usd: None,
                max_ms: Some(0),
                max_llm_calls: Some(64),
            },
            || db.query("SELECT aidb_generate('How do refunds work?', content) FROM documents"),
        )
        .expect_err("ms");
        assert!(ms.to_string().contains("max_ms"), "{ms}");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn keyed_cache_skips_duplicate_llm_call() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_generate('ping', 'pong')")
            .expect("first");
        db.query("SELECT aidb_generate('ping', 'pong')")
            .expect("second");
        let hits = db
            .query("SELECT COUNT(*) FROM run_events WHERE kind = 'cache_hit'")
            .expect("cache");
        assert_eq!(hits.rows[0][0].to_string(), "1");
        drop(db);
        cleanup(&path);
    }

    const AGENT_JSON: &str = r#"{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":3}"#;

    #[test]
    fn agent_runs_as_parent_with_child_runs() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let plan = db
            .query(&format!("EXPLAIN SELECT aidb_agent('{AGENT_JSON}')"))
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Then [control/seq]"), "{plan}");
        assert!(plan.contains("TopK"), "{plan}");
        assert!(plan.contains("Loop"), "{plan}");

        let ran = db
            .query(&format!("SELECT aidb_agent('{AGENT_JSON}')"))
            .expect("agent");
        assert_eq!(ran.rows[0][1].to_string(), "succeeded");
        assert!(
            ran.rows[0][2]
                .to_string()
                .to_ascii_lowercase()
                .contains("refund"),
            "{}",
            ran.rows[0][2]
        );
        let parent = ran.rows[0][0].to_string();
        let kind = db
            .query(&format!("SELECT kind FROM runs WHERE id = '{parent}'"))
            .expect("kind");
        assert_eq!(kind.rows[0][0].to_string(), "agent");
        let kids = db
            .query(&format!(
                "SELECT kind FROM runs WHERE parent_id = '{parent}' ORDER BY kind"
            ))
            .expect("kids");
        let kinds: Vec<String> = kids.rows.iter().map(|r| r[0].to_string()).collect();
        assert!(kinds.contains(&"search".into()), "{kinds:?}");
        assert!(kinds.contains(&"generate".into()), "{kinds:?}");
        drop(db);
        cleanup(&path);
    }

    const HITL_WORKFLOW: &str = r#"{"then":[{"search":{"query":"How do refunds work?","k":5}},{"approve":{"message":"Send this answer?"}},{"generate":{"prompt":"Draft the reply"}}]}"#;

    #[test]
    fn workflow_pauses_for_approval_and_resumes() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let plan = db
            .query(&format!("EXPLAIN SELECT aidb_workflow('{HITL_WORKFLOW}')"))
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Then [control/seq]"), "{plan}");
        assert!(plan.contains("TopK"), "{plan}");
        assert!(plan.contains("Llm"), "{plan}");
        assert!(!plan.to_ascii_lowercase().contains("approv"), "{plan}");

        let paused = db
            .query(&format!("SELECT aidb_workflow('{HITL_WORKFLOW}')"))
            .expect("pause");
        assert_eq!(paused.rows[0][1].to_string(), "awaiting_approval");
        assert_eq!(paused.rows[0][2].to_string(), "Send this answer?");
        let parent = paused.rows[0][0].to_string();

        let waiting = db
            .query("SELECT id, status FROM runs WHERE status = 'awaiting_approval'")
            .expect("waiting");
        assert_eq!(waiting.rows.len(), 1);
        assert_eq!(waiting.rows[0][0].to_string(), parent);

        let kids = db
            .query(&format!(
                "SELECT kind FROM runs WHERE parent_id = '{parent}' ORDER BY kind"
            ))
            .expect("kids");
        let kinds: Vec<String> = kids.rows.iter().map(|r| r[0].to_string()).collect();
        assert!(kinds.contains(&"search".into()), "{kinds:?}");
        assert!(!kinds.contains(&"generate".into()), "{kinds:?}");

        let resumed = db
            .query(&format!(
                "SELECT aidb_resume('{parent}', '{{\"approved\":true}}')"
            ))
            .expect("resume");
        assert_eq!(resumed.rows[0][1].to_string(), "succeeded");
        assert!(
            resumed.rows[0][2]
                .to_string()
                .to_ascii_lowercase()
                .contains("refund"),
            "{}",
            resumed.rows[0][2]
        );
        let generate = db
            .query(&format!(
                "SELECT kind, status FROM runs WHERE parent_id = '{parent}' AND kind = 'generate'"
            ))
            .expect("generate");
        assert_eq!(generate.rows.len(), 1);
        assert_eq!(generate.rows[0][1].to_string(), "succeeded");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn workflow_reject_cancels_and_wait_resumes() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let rejected = db
            .query(&format!("SELECT aidb_workflow('{HITL_WORKFLOW}')"))
            .expect("pause");
        let reject_id = rejected.rows[0][0].to_string();
        let cancelled = db
            .query(&format!(
                "SELECT aidb_resume('{reject_id}', '{{\"approved\":false}}')"
            ))
            .expect("reject");
        assert_eq!(cancelled.rows[0][1].to_string(), "cancelled");
        let status = db
            .query(&format!("SELECT status FROM runs WHERE id = '{reject_id}'"))
            .expect("status");
        assert_eq!(status.rows[0][0].to_string(), "cancelled");

        let wait =
            r#"{"then":[{"search":{"query":"refunds","k":3}},{"wait":{"message":"later"}}]}"#;
        let parked = db
            .query(&format!("SELECT aidb_workflow('{wait}')"))
            .expect("wait");
        assert_eq!(parked.rows[0][1].to_string(), "suspended");
        let wait_id = parked.rows[0][0].to_string();
        let continued = db.resume(&wait_id, "{}").expect("resume wait");
        assert_eq!(continued.rows[0][1].to_string(), "succeeded");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn awaiting_approval_does_not_auto_resume_on_open() {
        let path = temp_db();
        let parent = {
            let db = Aidb::open(&path).expect("open");
            db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
                .expect("insert");
            db.drain_index(Duration::from_secs(5)).expect("index");
            let paused = db
                .query(&format!("SELECT aidb_workflow('{HITL_WORKFLOW}')"))
                .expect("pause");
            let id = paused.rows[0][0].to_string();
            drop(db);
            id
        };
        let db = Aidb::open(&path).expect("reopen");
        let status = db
            .query(&format!("SELECT status FROM runs WHERE id = '{parent}'"))
            .expect("still waiting");
        assert_eq!(status.rows[0][0].to_string(), "awaiting_approval");
        let generate = db
            .query(&format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'generate'"
            ))
            .expect("no generate");
        assert_eq!(generate.rows[0][0].to_string(), "0");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn hybrid_search_recovers_keyword_that_vec_misses() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        for i in 0..8 {
            db.query(&format!(
                "SELECT aidb_insert_document('Refunds {i}', 'How do refunds work? Refunds are issued within 14 days of purchase.', '{{}}')"
            ))
            .expect("refunds");
        }
        db.query("SELECT aidb_insert_document('Bin', 'Warehouse bin ZX19QPLUGH holds the discontinued adapter.', '{}')")
            .expect("sku");
        db.drain_index(Duration::from_secs(10)).expect("index");

        let query = "How do refunds work ZX19QPLUGH";
        let vec_only = db
            .store
            .write(|conn| aidb_index::knn(conn, db.embedder.as_ref(), query, 3, None, "vec_chunks"))
            .expect("knn");
        let vec_text: String = vec_only
            .rows
            .iter()
            .map(|row| row[2].to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();
        assert!(
            !vec_text.contains("zx19qplugh"),
            "vec-only should miss the SKU chunk:\n{vec_text}"
        );

        let plan = db
            .query(&format!(
                "EXPLAIN SELECT document_id, chunk_id, content, distance FROM aidb_search('{query}', 3)"
            ))
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("hybrid rrf (vec+fts)"), "{plan}");
        assert!(plan.contains("HybridFtsVec"), "{plan}");

        let hits = db
            .query(&format!(
                "SELECT document_id, chunk_id, content, distance FROM aidb_search('{query}', 3)"
            ))
            .expect("hybrid");
        let hybrid_text: String = hits
            .rows
            .iter()
            .map(|row| row[2].to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();
        assert!(
            hybrid_text.contains("zx19qplugh"),
            "hybrid should recover the SKU chunk:\n{hybrid_text}"
        );

        let keyword_plan = db
            .query("EXPLAIN SELECT document_id FROM aidb_search('ZX19QPLUGH', 5)")
            .expect("keyword explain")
            .rows[0][0]
            .to_string();
        assert!(keyword_plan.contains("fts5 match"), "{keyword_plan}");

        let algo = db
            .query("SELECT output_json FROM runs WHERE kind = 'search' ORDER BY created_at_ms DESC LIMIT 1")
            .expect("run");
        assert!(
            algo.rows[0][0].to_string().contains("algorithm"),
            "{}",
            algo.rows[0][0]
        );
        drop(db);
        cleanup(&path);
    }

    const MCP_GITHUB: &str = r#"{"tools":[{"name":"github.read","inputs":{"path":"string"},"outputs":{"content":"string"},"side_effect":"none","retry":"safe"}]}"#;
    const MCP_EMAIL: &str = r#"{"tools":[{"name":"send.email","inputs":{"to":"string"},"side_effect":"irreversible","retry":"forbidden"}]}"#;

    #[test]
    fn capabilities_catalog_and_mcp_register() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        let seeded = db
            .query("SELECT name, side_effect FROM capabilities ORDER BY name")
            .expect("seed");
        assert_eq!(seeded.rows[0][0].to_string(), "generate");
        assert_eq!(seeded.rows[0][1].to_string(), "none");
        assert_eq!(seeded.rows[1][0].to_string(), "search");

        let registered = db
            .query(&format!("SELECT aidb_mcp_register('{MCP_GITHUB}')"))
            .expect("mcp");
        assert_eq!(registered.rows[0][0].to_string(), "github.read");
        assert_eq!(registered.rows[0][2].to_string(), "mcp");

        let catalog = db
            .query("SELECT name, side_effect FROM capabilities WHERE name = 'github.read'")
            .expect("catalog");
        assert_eq!(catalog.rows[0][1].to_string(), "none");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn agent_records_tool_child_runs() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");
        db.query(&format!("SELECT aidb_mcp_register('{MCP_GITHUB}')"))
            .expect("mcp");

        let spec = r#"{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","github.read"],"max_steps":2}"#;
        let plan = db
            .query(&format!("EXPLAIN SELECT aidb_agent('{spec}')"))
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Tool github.read"), "{plan}");

        let ran = db
            .query(&format!("SELECT aidb_agent('{spec}')"))
            .expect("agent");
        assert_eq!(ran.rows[0][1].to_string(), "succeeded");
        let parent = ran.rows[0][0].to_string();
        let kids = db
            .query(&format!(
                "SELECT kind, input_json FROM runs WHERE parent_id = '{parent}' ORDER BY kind"
            ))
            .expect("kids");
        let kinds: Vec<String> = kids.rows.iter().map(|r| r[0].to_string()).collect();
        assert!(kinds.contains(&"search".into()), "{kinds:?}");
        assert!(kinds.contains(&"tool".into()), "{kinds:?}");
        let tool_input = kids
            .rows
            .iter()
            .find(|r| r[0].to_string() == "tool")
            .map(|r| r[1].to_string())
            .expect("tool input");
        assert!(tool_input.contains("github.read"), "{tool_input}");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn irreversible_tool_requires_approval() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query(&format!("SELECT aidb_mcp_register('{MCP_EMAIL}')"))
            .expect("mcp");
        let spec = r#"{"instructions":"Send the note. End with DONE.","goal":"Refunds","tools":["send.email"],"max_steps":1}"#;
        let paused = db
            .query(&format!("SELECT aidb_agent('{spec}')"))
            .expect("pause");
        assert_eq!(paused.rows[0][1].to_string(), "awaiting_approval");
        let parent = paused.rows[0][0].to_string();
        let kids = db
            .query(&format!(
                "SELECT COUNT(*) FROM runs WHERE parent_id = '{parent}' AND kind = 'tool'"
            ))
            .expect("no tool yet");
        assert_eq!(kids.rows[0][0].to_string(), "0");

        let resumed = db
            .query(&format!(
                "SELECT aidb_resume('{parent}', '{{\"approved\":true}}')"
            ))
            .expect("resume");
        assert_eq!(resumed.rows[0][1].to_string(), "succeeded");
        let tool = db
            .query(&format!(
                "SELECT status, input_json, output_json FROM runs WHERE parent_id = '{parent}' AND kind = 'tool'"
            ))
            .expect("tool");
        assert_eq!(tool.rows[0][0].to_string(), "succeeded");
        assert!(tool.rows[0][1].to_string().contains("send.email"));
        assert!(tool.rows[0][2].to_string().contains("\"sent\":false"));
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn denied_and_unknown_tools_fail() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query(&format!("SELECT aidb_mcp_register('{MCP_GITHUB}')"))
            .expect("mcp");
        db.execute("UPDATE capabilities SET enabled = 0 WHERE name = 'github.read'")
            .expect("disable");
        let denied = db.query(
            r#"SELECT aidb_agent('{"instructions":"x","goal":"y","tools":["github.read"],"max_steps":1}')"#,
        );
        assert!(denied.is_err(), "{denied:?}");
        assert!(
            denied.unwrap_err().to_string().contains("denied"),
            "expected denied"
        );

        let unknown = db.query(
            r#"SELECT aidb_agent('{"instructions":"x","goal":"y","tools":["not.a.tool"],"max_steps":1}')"#,
        );
        assert!(unknown.is_err(), "{unknown:?}");
        assert!(
            unknown
                .unwrap_err()
                .to_string()
                .contains("unknown capability"),
            "expected unknown"
        );

        db.execute("UPDATE capabilities SET enabled = 1 WHERE name = 'github.read'")
            .expect("enable");
        let blocked = aidb_tool::with_deny(&["github.read"], || {
            db.query(r#"SELECT aidb_tool('github.read', '{"path":"README.md"}')"#)
        });
        assert!(blocked.is_err(), "{blocked:?}");
        assert!(
            blocked.unwrap_err().to_string().contains("deny-list"),
            "expected deny-list"
        );
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn standalone_tool_and_http_stub() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query(&format!("SELECT aidb_mcp_register('{MCP_GITHUB}')"))
            .expect("github");
        db.query(
            r#"SELECT aidb_mcp_register('{"tools":[{"name":"http.get","inputs":{"url":"string"},"side_effect":"none"}]}')"#,
        )
        .expect("http");

        let read = db
            .query(r#"SELECT aidb_tool('github.read', '{"path":"README.md"}')"#)
            .expect("read");
        assert_eq!(read.rows[0][1].to_string(), "succeeded");
        assert!(read.rows[0][2].to_string().contains("README.md"));

        let get = db
            .query(r#"SELECT aidb_tool('http.get', '{"url":"aidb://docs"}')"#)
            .expect("get");
        assert_eq!(get.rows[0][1].to_string(), "succeeded");

        let parked = db
            .query(&format!("SELECT aidb_mcp_register('{MCP_EMAIL}')"))
            .expect("email");
        assert_eq!(parked.rows[0][0].to_string(), "send.email");
        let waiting = db
            .query(r#"SELECT aidb_tool('send.email', '{"to":"a@b.c","subject":"hi"}')"#)
            .expect("park");
        assert_eq!(waiting.rows[0][1].to_string(), "awaiting_approval");
        let id = waiting.rows[0][0].to_string();
        let done = db
            .query(&format!(
                "SELECT aidb_resume('{id}', '{{\"approved\":true}}')"
            ))
            .expect("resume tool");
        assert_eq!(done.rows[0][1].to_string(), "succeeded");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn memory_insert_is_a_document_and_searchable() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        let inserted = db
            .query("SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations. Explain things briefly.')")
            .expect("memory");
        let id = inserted.rows[0][0].to_string();
        assert!(id.starts_with("doc_"), "{id}");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let view = db
            .query("SELECT scope, content FROM memory WHERE scope = 'user:123'")
            .expect("view");
        assert_eq!(view.rows[0][0].to_string(), "user:123");
        assert!(view.rows[0][1].to_string().contains("concise"));

        let hits = db
            .query("SELECT document_id, content FROM aidb_search('How should I explain this?', 5)")
            .expect("search");
        assert!(
            hits.rows.iter().any(|row| row[0].to_string() == id),
            "{hits:?}"
        );

        let scoped = db
            .query("SELECT document_id FROM aidb_memory_search('How should I explain this?', 5, 'user:123')")
            .expect("scoped");
        assert_eq!(scoped.rows[0][0].to_string(), id);
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn parent_agent_spawns_child_agent_and_uses_memory() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations. Explain refunds briefly.')")
            .expect("memory");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("doc");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let spec = r#"{"instructions":"Coordinate. End with DONE.","goal":"How do refunds work?","tools":["search"],"memory":"user:123","max_steps":1,"agents":[{"instructions":"Answer from documents and memory. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":2}]}"#;
        let ran = db
            .query(&format!("SELECT aidb_agent('{spec}')"))
            .expect("parent");
        assert_eq!(ran.rows[0][1].to_string(), "succeeded");
        let parent = ran.rows[0][0].to_string();
        let agents = db
            .query("SELECT id, parent_id, kind, status FROM runs WHERE kind = 'agent' ORDER BY created_at_ms")
            .expect("agents");
        assert_eq!(agents.rows.len(), 2, "{agents:?}");
        assert_eq!(agents.rows[0][0].to_string(), parent);
        assert_eq!(agents.rows[0][1].to_string(), "");
        assert_eq!(agents.rows[1][1].to_string(), parent);
        assert_eq!(agents.rows[1][2].to_string(), "agent");
        assert_eq!(agents.rows[1][3].to_string(), "succeeded");
        let table = db
            .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'agents'")
            .expect("no agents table");
        assert!(table.rows.is_empty());
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn dialect_search_matches_aidb_search() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let via_fn = db
            .query("SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work?', 5)")
            .expect("fn");
        let via_search = db
            .query("SEARCH 'How do refunds work?' LIMIT 5")
            .expect("search");
        let via_from = db
            .query("SELECT * FROM documents SEARCH 'How do refunds work?' LIMIT 5")
            .expect("from");
        assert_eq!(via_fn.rows, via_search.rows);
        assert_eq!(via_fn.rows, via_from.rows);

        let fn_plan = db
            .query("EXPLAIN SELECT document_id FROM aidb_search('How do refunds work?', 5)")
            .expect("explain fn")
            .rows[0][0]
            .to_string();
        let dialect_plan = db
            .query("EXPLAIN SEARCH 'How do refunds work?' LIMIT 5")
            .expect("explain dialect")
            .rows[0][0]
            .to_string();
        assert!(fn_plan.contains("TopK k=5"), "{fn_plan}");
        assert_eq!(fn_plan, dialect_plan);
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn dialect_create_model_and_ai_generate() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.execute("CREATE MODEL gpt (kind = llm, provider = fake, provider_model = 'aidb-fake')")
            .expect("create");
        let row = db
            .query("SELECT name, kind, provider, provider_model FROM models WHERE name = 'gpt'")
            .expect("catalog");
        assert_eq!(row.rows[0][0].to_string(), "gpt");
        assert_eq!(row.rows[0][1].to_string(), "llm");
        assert_eq!(row.rows[0][2].to_string(), "fake");
        assert_eq!(row.rows[0][3].to_string(), "aidb-fake");

        let denied = db.execute(
            "CREATE MODEL bad (kind = llm, provider = openai, provider_model = 'x', api_key = 'sk')",
        );
        assert!(denied.is_err(), "{denied:?}");

        db.query("SELECT AI_GENERATE('ping', 'pong')").expect("gen");
        let runs = db
            .query("SELECT kind FROM runs WHERE kind = 'generate'")
            .expect("runs");
        assert!(!runs.rows.is_empty());
        drop(db);
        cleanup(&path);
    }

    const INCIDENT_TASK: &str = "\
TASK investigate_incident
DATA logs, deployments
CONSTRAINTS read_only, budget $1, timeout 5m
GOAL identify_root_cause";

    #[test]
    fn goal_language_compiles_to_ir_and_workflow_run() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Logs', 'Deploy failed after the checkout timeout. identify_root_cause in logs.', '{}')")
            .expect("doc");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let plan = db
            .query(&format!("EXPLAIN {INCIDENT_TASK}"))
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Then [control/seq]"), "{plan}");
        assert!(plan.contains("TopK k=5"), "{plan}");
        assert!(plan.contains("Llm"), "{plan}");
        assert!(plan.contains("Rewrites"), "{plan}");
        assert!(plan.contains("max_usd=1"), "{plan}");
        assert!(plan.contains("max_ms=300000"), "{plan}");

        let ran = db.query(INCIDENT_TASK).expect("run");
        assert_eq!(ran.rows[0][1].to_string(), "succeeded");
        let parent = ran.rows[0][0].to_string();
        let kind = db
            .query(&format!("SELECT kind FROM runs WHERE id = '{parent}'"))
            .expect("kind");
        assert_eq!(kind.rows[0][0].to_string(), "workflow");
        let kids = db
            .query(&format!(
                "SELECT kind FROM runs WHERE parent_id = '{parent}' ORDER BY kind"
            ))
            .expect("kids");
        let kinds: Vec<String> = kids.rows.iter().map(|r| r[0].to_string()).collect();
        assert!(kinds.contains(&"search".into()), "{kinds:?}");
        assert!(kinds.contains(&"generate".into()), "{kinds:?}");
        let table = db
            .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'goals'")
            .expect("no goals table");
        assert!(table.rows.is_empty());
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn goal_documents_data_is_rewritten_by_optimizer() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        for i in 0..8 {
            db.query(&format!(
                "SELECT aidb_insert_document('Refunds {i}', 'How do refunds work? Refunds are issued within 14 days of purchase.', '{{}}')"
            ))
            .expect("doc");
        }
        db.drain_index(Duration::from_secs(5)).expect("index");
        let plan = db
            .query("EXPLAIN TASK summarize\nDATA documents\nGOAL How do refunds work?")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Scan documents"), "{plan}");
        assert!(
            plan.contains("CascadeEmbedTopKThenJudge") || plan.contains("Llm"),
            "{plan}"
        );
        let ran = db
            .query("TASK summarize\nDATA documents\nGOAL How do refunds work?")
            .expect("run");
        assert_eq!(ran.rows[0][1].to_string(), "succeeded");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn generate_over_search_returns_citations() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("insert");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let plan = db
            .query("EXPLAIN SELECT aidb_generate('What is the refund policy?', content) FROM aidb_search('refund policy', 5)")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Llm prompt="), "{plan}");
        assert!(plan.contains("TopK k=5"), "{plan}");
        assert!(plan.contains("Similarity"), "{plan}");
        assert!(plan.contains("Embed query="), "{plan}");
        assert!(plan.contains("Scan documents"), "{plan}");

        let generated = db
            .query("SELECT aidb_generate('What is the refund policy?', content) FROM aidb_search('refund policy', 5)")
            .expect("generate");
        assert_eq!(generated.rows.len(), 1);
        let value: serde_json::Value =
            serde_json::from_str(&generated.rows[0][0].to_string()).expect("cited json");
        let answer = value["answer"].as_str().unwrap_or("");
        assert!(answer.to_ascii_lowercase().contains("refund"), "{value}");
        let sources = value["sources"].as_array().expect("sources");
        assert!(!sources.is_empty(), "{value}");
        assert!(
            sources[0]["document_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "{value}"
        );
        assert!(sources[0]["chunk_id"].as_str().is_some(), "{value}");
        assert!(sources[0]["score"].as_f64().is_some(), "{value}");

        let via_dialect = db
            .query("SELECT AI_GENERATE('What is the refund policy?', content) FROM aidb_search('refund policy', 5)")
            .expect("ai_generate");
        let dialect: serde_json::Value =
            serde_json::from_str(&via_dialect.rows[0][0].to_string()).expect("dialect json");
        assert!(!dialect["sources"].as_array().unwrap().is_empty());

        let output = db
            .query("SELECT output_json FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1")
            .expect("run")
            .rows[0][0]
            .to_string();
        assert!(output.contains("\"sources\""), "{output}");

        let table = db
            .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'citations'")
            .expect("no citations table");
        assert!(table.rows.is_empty());
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn plain_generate_stays_a_string() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        let text = db
            .query("SELECT aidb_generate('ping', 'pong')")
            .expect("generate")
            .rows[0][0]
            .to_string();
        assert!(text.contains("ping"), "{text}");
        assert!(text.contains("pong"), "{text}");
        assert!(
            serde_json::from_str::<serde_json::Value>(&text).is_err()
                || text.as_bytes().first() != Some(&b'{'),
            "plain generate must stay a string, got {text}"
        );
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn cascade_generate_includes_sources() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')")
            .expect("refunds");
        for i in 0..7 {
            db.query(&format!(
                "SELECT aidb_insert_document('Shipping {i}', 'Orders ship within two business days via ground service.', '{{}}')"
            ))
            .expect("shipping");
        }
        db.drain_index(Duration::from_secs(10)).expect("index");
        let generated = db
            .query("SELECT aidb_generate('How do refunds work?', content) FROM documents")
            .expect("generate");
        assert_eq!(generated.rows.len(), 1);
        let value: serde_json::Value =
            serde_json::from_str(&generated.rows[0][0].to_string()).expect("cited json");
        assert!(
            value["answer"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("refund"),
            "{value}"
        );
        assert!(!value["sources"].as_array().unwrap().is_empty(), "{value}");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn search_metadata_filter_keeps_matching_docs() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Support refunds', 'Refunds are issued within 14 days of purchase.', '{\"dept\":\"support\"}')")
            .expect("support");
        db.query("SELECT aidb_insert_document('Legal refunds', 'Refunds require a signed legal waiver before processing.', '{\"dept\":\"legal\"}')")
            .expect("legal");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let plan = db
            .query(r#"EXPLAIN SELECT document_id FROM aidb_search('refund policy', 5, '{"dept":"support"}')"#)
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Filter metadata"), "{plan}");
        assert!(plan.contains("MetadataFilter"), "{plan}");
        assert!(plan.contains("TopK k=5"), "{plan}");

        let filtered = db
            .query(r#"SELECT document_id, content FROM aidb_search('refund policy', 5, '{"dept":"support"}')"#)
            .expect("filtered");
        assert!(!filtered.rows.is_empty(), "{filtered:?}");
        let content_idx = filtered
            .columns
            .iter()
            .position(|c| c == "content")
            .unwrap_or(2);
        for row in &filtered.rows {
            let content = row[content_idx].to_string().to_ascii_lowercase();
            assert!(content.contains("14 days"), "{row:?}");
            assert!(!content.contains("waiver"), "{row:?}");
        }

        // The dialect returns the whole retrieval row, so compare it against the
        // function form asking for the same columns.
        let via_dialect = db
            .query("SEARCH 'refund policy' WHERE metadata.dept = 'support' LIMIT 5")
            .expect("dialect");
        let via_function = db
            .query(
                r#"SELECT document_id, chunk_id, content, distance
                   FROM aidb_search('refund policy', 5, '{"dept":"support"}')"#,
            )
            .expect("function");
        assert_eq!(via_dialect.rows, via_function.rows);
        assert_eq!(filtered.columns, vec!["document_id", "content"]);

        let unfiltered = db
            .query("SELECT document_id FROM aidb_search('refund policy', 5)")
            .expect("unfiltered");
        assert!(unfiltered.rows.len() >= 2, "{unfiltered:?}");

        db.query(
            "SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations.')",
        )
        .expect("mem a");
        db.query("SELECT aidb_memory_insert('user:999', 'Always write long legal essays.')")
            .expect("mem b");
        db.drain_index(Duration::from_secs(5)).expect("index mem");
        let scoped = db
            .query("SELECT document_id, content FROM aidb_memory_search('concise explanations', 5, 'user:123')")
            .expect("scoped");
        assert!(!scoped.rows.is_empty(), "{scoped:?}");
        let mem_content = scoped
            .columns
            .iter()
            .position(|c| c == "content")
            .unwrap_or(2);
        assert!(
            scoped
                .rows
                .iter()
                .all(|row| row[mem_content].to_string().contains("concise")),
            "{scoped:?}"
        );
        drop(db);
        cleanup(&path);
    }

    fn fake_mcp_bin() -> std::path::PathBuf {
        let exe = std::env::current_exe().expect("current_exe");
        let mut dir = exe.parent().expect("parent").to_path_buf();
        if dir.file_name().is_some_and(|name| name == "deps") {
            dir.pop();
        }
        let name = if cfg!(windows) {
            "fake-mcp.exe"
        } else {
            "fake-mcp"
        };
        let candidate = dir.join(name);
        if candidate.exists() {
            return candidate;
        }
        if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
            let profile = if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            };
            let alt = std::path::Path::new(&target).join(profile).join(name);
            if alt.exists() {
                return alt;
            }
        }
        candidate
    }

    #[test]
    fn live_mcp_stdio_writes_catalog_and_keeps_rows() {
        let bin = fake_mcp_bin();
        assert!(
            bin.exists(),
            "fake-mcp was not built at {} (cargo test --workspace builds it)",
            bin.display()
        );
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        let denied = db.query("SELECT aidb_mcp_connect('http', 'https://example.com/mcp')");
        assert!(denied.is_err(), "{denied:?}");

        let plan = db
            .query("EXPLAIN SELECT aidb_mcp_connect('stdio', './fake-mcp')")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("McpConnect"), "{plan}");

        let registered = db
            .query(&format!(
                "SELECT aidb_mcp_connect('stdio', '{}')",
                bin.display()
            ))
            .expect("connect");
        assert_eq!(registered.rows[0][0].to_string(), "echo.ping");
        assert_eq!(registered.rows[0][2].to_string(), "mcp");

        let catalog = db
            .query("SELECT name, source FROM capabilities WHERE source = 'mcp'")
            .expect("catalog");
        assert_eq!(catalog.rows[0][0].to_string(), "echo.ping");

        let invoked = db
            .query(r#"SELECT aidb_tool('echo.ping', '{"text":"hello"}')"#)
            .expect("tool");
        assert_eq!(invoked.rows[0][1].to_string(), "succeeded");
        assert!(
            invoked.rows[0][2].to_string().contains("hello"),
            "{invoked:?}"
        );

        let agent = db
            .query(r#"SELECT aidb_agent('Use the connected MCP tool', '["echo.ping"]')"#)
            .expect("agent");
        assert_eq!(agent.rows[0][1].to_string(), "succeeded");
        assert!(
            agent.rows[0][2].to_string().contains("pong")
                || agent.rows[0][2].to_string().contains("Use the connected"),
            "{agent:?}"
        );

        let left = db
            .query("SELECT aidb_mcp_disconnect()")
            .expect("disconnect");
        assert_eq!(left.rows[0][0].to_string(), "echo.ping");
        let kept = db
            .query("SELECT name, source FROM capabilities WHERE source = 'mcp'")
            .expect("kept");
        assert_eq!(kept.rows.len(), 1);
        assert_eq!(kept.rows[0][1].to_string(), "mcp");

        let runs = db
            .query("SELECT kind FROM runs WHERE kind = 'tool'")
            .expect("tool runs");
        assert!(!runs.rows.is_empty());
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn classify_writes_generate_run_and_registers_providers() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.execute("CREATE MODEL IF NOT EXISTS cls PROVIDER 'fake' KIND 'llm'")
            .expect("create cls");
        let cls = db
            .query("SELECT name, provider, provider_model FROM models WHERE name = 'cls'")
            .expect("cls");
        assert_eq!(cls.rows[0][1].to_string(), "fake");
        assert_eq!(cls.rows[0][2].to_string(), "aidb-fake");

        db.execute("CREATE MODEL IF NOT EXISTS cls PROVIDER 'anthropic' KIND 'llm'")
            .expect("if not exists keeps first");
        let kept = db
            .query("SELECT provider FROM models WHERE name = 'cls'")
            .expect("kept");
        assert_eq!(kept.rows[0][0].to_string(), "fake");

        let denied = db.execute("CREATE MODEL mystery PROVIDER hosted KIND llm");
        assert!(denied.is_err(), "{denied:?}");

        db.query("SELECT aidb_insert_document('Refunds', 'This refund was a negative surprise for the customer.', '{}')")
            .expect("doc");
        db.query("SELECT aidb_insert_document('Praise', 'The support team was a positive delight.', '{}')")
            .expect("doc2");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let plan = db
            .query("EXPLAIN SELECT aidb_classify('positive or negative', content) FROM documents")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("Llm"), "{plan}");

        let labels = db
            .query("SELECT aidb_classify('positive or negative', content) FROM documents")
            .expect("classify");
        assert_eq!(labels.rows.len(), 2);
        let texts: Vec<String> = labels.rows.iter().map(|row| row[0].to_string()).collect();
        assert!(texts.iter().any(|t| t == "negative"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "positive"), "{texts:?}");

        let runs = db
            .query("SELECT kind, status, input_json FROM runs WHERE kind = 'generate'")
            .expect("runs");
        assert_eq!(runs.rows.len(), 2);
        assert_eq!(runs.rows[0][0].to_string(), "generate");
        assert_eq!(runs.rows[0][1].to_string(), "succeeded");
        assert!(
            runs.rows[0][2]
                .to_string()
                .contains("\"task\":\"classify\""),
            "{}",
            runs.rows[0][2]
        );

        db.execute("CREATE MODEL IF NOT EXISTS claude PROVIDER 'anthropic' KIND 'llm'")
            .expect("anthropic catalog, no key");
        let claude = db
            .query("SELECT provider, provider_model FROM models WHERE name = 'claude'")
            .expect("claude");
        assert_eq!(claude.rows[0][0].to_string(), "anthropic");
        assert_eq!(claude.rows[0][1].to_string(), "claude-sonnet-4-20250514");

        let tables = db
            .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'classify'")
            .expect("no classify store");
        assert!(tables.rows.is_empty(), "{tables:?}");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn file_policy_denies_tools_and_survives_reopen() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query(&format!("SELECT aidb_mcp_register('{MCP_EMAIL}')"))
            .expect("mcp");
        db.query(&format!("SELECT aidb_mcp_register('{MCP_GITHUB}')"))
            .expect("github");

        let secrets =
            db.query(r#"SELECT aidb_set_policy('{"deny":["send.email"],"api_key":"sk"}')"#);
        assert!(secrets.is_err(), "{secrets:?}");
        assert!(
            secrets.unwrap_err().to_string().contains("secrets"),
            "expected secrets rejected"
        );

        let set = db
            .query(r#"SELECT aidb_set_policy('{"read_only":true,"deny":["send.email"],"max_usd":0.10}')"#)
            .expect("set");
        assert!(set.rows[0][0].to_string().contains("send.email"), "{set:?}");

        let plan = db
            .query("EXPLAIN SELECT aidb_set_policy('{\"deny\":[]}')")
            .expect("explain set")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("SetPolicy"), "{plan}");

        let search_plan = db
            .query("EXPLAIN SELECT document_id FROM aidb_search('refunds', 5)")
            .expect("explain search")
            .rows[0][0]
            .to_string();
        assert!(
            search_plan.contains("Policy read_only=true"),
            "{search_plan}"
        );
        assert!(search_plan.contains("deny=send.email"), "{search_plan}");
        assert!(search_plan.contains("max_usd=0.1"), "{search_plan}");

        let denied = db.query("SELECT aidb_agent('Email the customer', '[\"send.email\"]')");
        assert!(denied.is_err(), "{denied:?}");
        assert!(
            denied.unwrap_err().to_string().contains("deny-list"),
            "expected deny-list"
        );

        drop(db);
        let db = Aidb::open(&path).expect("reopen");
        let kept = db.query("SELECT aidb_get_policy()").expect("get");
        let json = kept.rows[0][0].to_string();
        assert!(json.contains("send.email"), "{json}");
        assert!(json.contains("\"read_only\":true"), "{json}");
        let still = db.query("SELECT aidb_agent('Email the customer', '[\"send.email\"]')");
        assert!(still.is_err(), "{still:?}");

        db.query(r#"SELECT aidb_set_policy('{"allow":["send.email"],"max_usd":0.10}')"#)
            .expect("allow");
        let paused = db
            .query("SELECT aidb_agent('Email the customer', '[\"send.email\"]')")
            .expect("hitl");
        assert_eq!(paused.rows[0][1].to_string(), "awaiting_approval");

        db.query(r#"SELECT aidb_set_policy('{"require_approval":["github.read"]}')"#)
            .expect("require");
        let parked = db
            .query(r#"SELECT aidb_tool('github.read', '{"path":"README.md"}')"#)
            .expect("park");
        assert_eq!(parked.rows[0][1].to_string(), "awaiting_approval");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn named_space_search_does_not_break_default() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Indemnity', 'The legal indemnity clause survives termination.', '{}')")
            .expect("doc");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let created = db
            .query("SELECT aidb_create_space('legal', 'fake', 32)")
            .expect("space");
        assert_eq!(created.rows[0][0].to_string(), "legal");
        assert_eq!(created.rows[0][5].to_string(), "vec_chunks_legal");
        let indexed: i64 = created.rows[0][6].to_string().parse().unwrap_or(0);
        assert!(indexed >= 1, "{created:?}");

        let plan = db
            .query("EXPLAIN SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal')")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("space=legal"), "{plan}");

        let legal = db
            .query("SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal')")
            .expect("legal search");
        assert!(!legal.rows.is_empty(), "{legal:?}");

        let default = db
            .query("SELECT document_id FROM aidb_search('indemnity', 5)")
            .expect("default search");
        assert!(!default.rows.is_empty(), "{default:?}");
        assert_eq!(legal.rows[0][0].to_string(), default.rows[0][0].to_string());

        let unknown =
            db.query("SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'missing')");
        assert!(unknown.is_err(), "{unknown:?}");

        let tables = db
            .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('vec_chunks', 'vec_chunks_legal', 'embedding_spaces') ORDER BY name")
            .expect("tables");
        let names: Vec<String> = tables.rows.iter().map(|row| row[0].to_string()).collect();
        assert!(names.contains(&"embedding_spaces".into()), "{names:?}");
        assert!(names.contains(&"vec_chunks".into()), "{names:?}");
        assert!(names.contains(&"vec_chunks_legal".into()), "{names:?}");
        drop(db);
        cleanup(&path);
    }

    #[test]
    fn space_owns_embedder_not_open() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.query("SELECT aidb_insert_document('Indemnity', 'The legal indemnity clause survives termination.', '{}')")
            .expect("doc");
        db.drain_index(Duration::from_secs(5)).expect("index");

        let created = db
            .query("SELECT aidb_create_space('legal', 'local', 384, 'BAAI/bge-small-en-v1.5', 'cosine')")
            .expect("space");
        assert_eq!(created.rows[0][1].to_string(), "local");
        assert_eq!(created.rows[0][2].to_string(), "BAAI/bge-small-en-v1.5");
        assert_eq!(created.rows[0][3].to_string(), "384");
        assert_eq!(created.rows[0][4].to_string(), "cosine");
        assert_eq!(created.rows[0][5].to_string(), "vec_chunks_legal");

        let catalog = db
            .query("SELECT name, provider, provider_model, dimensions, distance, vec_table FROM embedding_spaces")
            .expect("catalog");
        assert_eq!(catalog.rows[0][0].to_string(), "legal");
        assert_eq!(catalog.rows[0][1].to_string(), "local");

        let legal = db
            .query("SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal')")
            .expect("legal search");
        assert!(!legal.rows.is_empty(), "{legal:?}");

        let dim = db.query("SELECT aidb_create_space('bad', 'local', 32, 'bge-small')");
        assert!(dim.is_err(), "{dim:?}");
        let unknown = db.query("SELECT aidb_create_space('x', 'hosted-mystery', 32)");
        assert!(unknown.is_err(), "{unknown:?}");
        let custom_missing =
            db.query("SELECT aidb_create_space('mine', 'custom', 32, 'not-registered')");
        assert!(custom_missing.is_err(), "{custom_missing:?}");

        register_custom_embedder(
            "unit-custom",
            std::sync::Arc::new(aidb_ai::FakeEmbedder {
                model: "unit-custom".into(),
                dimensions: 32,
            }),
        );
        db.query("SELECT aidb_create_space('mine', 'custom', 32, 'unit-custom')")
            .expect("custom space");

        drop(db);
        cleanup(&path);
    }

    #[test]
    fn secret_store_keeps_key_names_out_of_the_file() {
        let path = temp_db();
        let db = Aidb::open(&path).expect("open");
        db.execute(
            "CREATE MODEL gpt (kind = llm, provider = openai, provider_model = 'gpt-4.1-mini', key_name = 'AIDB_PHASE25_MODEL_KEY')",
        )
        .expect("create");
        let row = db
            .query("SELECT name, provider, key_name FROM models WHERE name = 'gpt'")
            .expect("catalog");
        assert_eq!(row.rows[0][0].to_string(), "gpt");
        assert_eq!(row.rows[0][1].to_string(), "openai");
        assert_eq!(row.rows[0][2].to_string(), "AIDB_PHASE25_MODEL_KEY");

        let denied = db.execute(
            "CREATE MODEL bad (kind = llm, provider = openai, provider_model = 'x', api_key = 'sk-live')",
        );
        assert!(denied.is_err(), "{denied:?}");
        let secret_name = db.execute(
            "CREATE MODEL worse (kind = llm, provider = openai, key_name = 'sk-live-secret')",
        );
        assert!(secret_name.is_err(), "{secret_name:?}");
        assert!(
            secret_name
                .unwrap_err()
                .to_string()
                .contains("never the secret"),
            "expected key_name rejected"
        );

        let trigger = db.execute(
            "INSERT INTO models (name, kind, provider, provider_model, created_at_ms, key_name)
             VALUES ('raw', 'llm', 'openai', 'x', 1, 'sk-raw')",
        );
        assert!(trigger.is_err(), "{trigger:?}");

        let store = db.query("SELECT aidb_secret_store()").expect("store");
        assert!(!store.rows[0][0].to_string().is_empty());
        let plan = db
            .query("EXPLAIN SELECT aidb_secret_store()")
            .expect("explain")
            .rows[0][0]
            .to_string();
        assert!(plan.contains("SecretStore"), "{plan}");

        let missing = db.query("SELECT aidb_generate('ping', 'pong')");
        assert!(missing.is_err(), "{missing:?}");
        assert!(
            missing
                .unwrap_err()
                .to_string()
                .contains("AIDB_PHASE25_MODEL_KEY is not set"),
            "expected missing key name, not a corrupt file"
        );

        let meta = db.query("SELECT key, value FROM aidb_meta").expect("meta");
        for row in &meta.rows {
            let blob = format!("{} {}", row[0], row[1]);
            assert!(!blob.contains("sk-"), "{blob}");
            assert!(!blob.to_ascii_lowercase().contains("api_key="), "{blob}");
        }
        drop(db);

        let reopened = Aidb::open(&path).expect("reopen");
        let version = reopened
            .query("SELECT value FROM aidb_meta WHERE key = 'schema_version'")
            .expect("version");
        assert_eq!(version.rows[0][0].to_string(), SCHEMA_VERSION.to_string());
        let names = reopened
            .query("SELECT name, provider FROM models WHERE name = 'gpt'")
            .expect("still there");
        assert_eq!(names.rows[0][0].to_string(), "gpt");
        assert_eq!(names.rows[0][1].to_string(), "openai");
        let still = reopened.query("SELECT aidb_generate('ping', 'pong')");
        assert!(still.is_err(), "{still:?}");
        assert!(
            still
                .unwrap_err()
                .to_string()
                .contains("AIDB_PHASE25_MODEL_KEY is not set"),
            "reopen without the store is a usage error"
        );
        drop(reopened);
        cleanup(&path);
    }
}
