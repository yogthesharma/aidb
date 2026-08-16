//! Provider contracts. Every adapter is reached the same way, deterministic fakes
//! carry the normal suite, and a provider that cannot run fails closed instead of
//! being replaced by a different one. No test here needs a credential.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aidb_ai::Embedder;
use aidb_core::{Error, Result};
use common::*;

/// A developer-supplied embedder: deterministic, and able to misbehave on demand.
struct TestEmbedder {
    model: String,
    dimensions: usize,
    calls: Arc<AtomicUsize>,
    behavior: Behavior,
}

#[derive(Clone, Copy, PartialEq)]
enum Behavior {
    Ok,
    Fails,
    WrongWidth,
    WrongCount,
}

impl Embedder for TestEmbedder {
    fn provider(&self) -> &str {
        "custom"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.behavior {
            Behavior::Fails => Err(Error::ai("test embedder is offline")),
            Behavior::WrongCount => Ok(Vec::new()),
            Behavior::WrongWidth => Ok(texts
                .iter()
                .map(|_| vec![0.5; self.dimensions + 1])
                .collect()),
            Behavior::Ok => Ok(texts
                .iter()
                .map(|t| aidb_ai::hashed_embedding(t, self.dimensions, "custom", &self.model))
                .collect()),
        }
    }
}

/// `expect_err` needs `Debug` on the success value, which trait objects do not have.
fn err_of<T>(result: Result<T>, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what} must fail"),
        Err(err) => err.to_string(),
    }
}

fn register(model: &str, dimensions: usize, behavior: Behavior) -> Arc<AtomicUsize> {
    let calls = Arc::new(AtomicUsize::new(0));
    aidb_ai::register_custom_embedder(
        model,
        Arc::new(TestEmbedder {
            model: model.to_string(),
            dimensions,
            calls: calls.clone(),
            behavior,
        }),
    );
    calls
}

#[test]
fn the_fake_embedder_is_deterministic_and_model_scoped() {
    let one = aidb_ai::hashed_embedding("refund policy", 32, "fake", "aidb-fake");
    let two = aidb_ai::hashed_embedding("refund policy", 32, "fake", "aidb-fake");
    assert_eq!(one, two, "the same text must embed identically every time");
    let other_model = aidb_ai::hashed_embedding("refund policy", 32, "fake", "aidb-fake-legal");
    assert_ne!(
        one, other_model,
        "two models must not produce comparable vectors"
    );
    let other_provider = aidb_ai::hashed_embedding("refund policy", 32, "local", "aidb-fake");
    assert_ne!(one, other_provider, "provider is part of the space too");
    assert_eq!(one.len(), 32);
    // Normalized, so cosine distance is meaningful.
    let norm: f32 = one.iter().map(|x| x * x).sum();
    assert!((norm - 1.0).abs() < 1e-4, "norm was {norm}");
}

#[test]
fn embedding_handles_batches_empty_input_and_very_long_text() {
    let e = TestEmbedder {
        model: "batch".into(),
        dimensions: 16,
        calls: Arc::new(AtomicUsize::new(0)),
        behavior: Behavior::Ok,
    };
    assert!(e.embed(&[]).expect("empty batch").is_empty());
    let batch: Vec<String> = (0..64).map(|i| format!("chunk number {i}")).collect();
    let out = e.embed(&batch).expect("batch");
    assert_eq!(out.len(), 64, "one vector per input, in order");
    assert_eq!(out[0], e.embed(&[batch[0].clone()]).expect("single")[0]);
    let huge = vec!["lorem ipsum ".repeat(50_000)];
    assert_eq!(e.embed(&huge).expect("huge")[0].len(), 16);
    // Text with no indexable words still yields a vector of the right width.
    let empty = e.embed(&[String::new()]).expect("empty text");
    assert_eq!(empty[0].len(), 16);
}

#[test]
fn a_custom_embedder_is_reachable_through_a_space_and_indexes_documents() {
    let calls = register("test-ok", 24, Behavior::Ok);
    let tmp = TempDb::new("prov-custom-ok");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    let created = db
        .query("SELECT aidb_create_space('mine', 'custom', 24, 'test-ok')")
        .expect("custom space");
    assert_eq!(cell(&created, 0, "provider"), "custom");
    assert_eq!(cell(&created, 0, "model"), "test-ok");
    assert!(
        calls.load(Ordering::SeqCst) > 0,
        "the adapter was actually used"
    );
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM vec_chunks_mine"),
        count(&db, "SELECT COUNT(*) FROM chunks")
    );
    let hits = db
        .query("SELECT * FROM aidb_search('refunds', 3, '{}', 'mine')")
        .expect("search");
    assert!(!hits.rows.is_empty());
}

#[test]
fn a_provider_that_errors_fails_the_operation_and_leaves_nothing_searchable() {
    register("test-offline", 24, Behavior::Fails);
    let tmp = TempDb::new("prov-error");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    assert_err_contains(
        db.query("SELECT aidb_create_space('broken', 'custom', 24, 'test-offline')"),
        "test embedder is offline",
    );
    // The failure is not papered over with a different provider, and the space is
    // not left behind half-built.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM embedding_spaces WHERE name = 'broken'"
        ),
        0
    );
    assert_err_contains(
        db.query("SELECT * FROM aidb_search('refunds', 3, '{}', 'broken')"),
        "unknown embedding space",
    );
}

#[test]
fn a_provider_returning_the_wrong_width_is_rejected_not_stored() {
    register("test-wide", 24, Behavior::WrongWidth);
    let tmp = TempDb::new("prov-width");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    assert_err_contains(
        db.query("SELECT aidb_create_space('wide', 'custom', 24, 'test-wide')"),
        "dimension mismatch",
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM embedding_spaces"), 0);
}

#[test]
fn a_provider_returning_too_few_vectors_is_rejected() {
    register("test-short", 24, Behavior::WrongCount);
    let tmp = TempDb::new("prov-count");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    assert_err_contains(
        db.query("SELECT aidb_create_space('short', 'custom', 24, 'test-short')"),
        "wrong number of vectors",
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM embedding_spaces"), 0);
}

#[test]
fn a_custom_embedder_registered_at_the_wrong_width_fails_closed() {
    register("test-16", 16, Behavior::Ok);
    let tmp = TempDb::new("prov-declared-width");
    let db = tmp.open();
    assert_err_contains(
        db.query("SELECT aidb_create_space('mismatch', 'custom', 32, 'test-16')"),
        "16 dimensions, space has 32",
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM embedding_spaces"), 0);
}

#[test]
fn the_default_embedder_config_is_the_deterministic_fake() {
    // The normal suite must never reach for a network provider by default.
    let config = aidb::EmbedderConfig::default();
    assert_eq!(config.provider, "fake");
    assert_eq!(config.dimensions, 32);
    assert!(config.key_name.is_none(), "no key is needed offline");
    let (provider, model) = aidb_ai::default_llm();
    assert_eq!(provider, "fake");
    assert_eq!(model, "aidb-fake");
}

#[test]
fn the_model_catalog_accepts_the_supported_providers_and_rejects_the_rest() {
    let tmp = TempDb::new("prov-catalog");
    let db = tmp.open();
    for sql in [
        "CREATE MODEL cheap PROVIDER fake KIND llm",
        "CREATE MODEL gpt PROVIDER openai KIND llm MODEL 'gpt-4.1-mini'",
        "CREATE MODEL claude PROVIDER anthropic KIND llm",
        "CREATE MODEL kimi PROVIDER kimi KIND llm",
    ] {
        db.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let rows = db
        .query("SELECT name, provider, provider_model, kind FROM models ORDER BY name")
        .expect("models");
    assert_eq!(
        column_values(&rows, "name"),
        vec!["cheap", "claude", "gpt", "kimi"]
    );
    assert_eq!(
        cell(&rows, 1, "provider_model"),
        "claude-sonnet-4-20250514",
        "a provider has a documented default model"
    );
    assert_eq!(cell(&rows, 3, "provider"), "kimi");
    assert_eq!(
        cell(&rows, 3, "provider_model"),
        "kimi-k2.5",
        "kimi is OpenAI-compatible with a Moonshot default model"
    );

    assert_err_contains(
        db.execute("CREATE MODEL mystery PROVIDER hosted_mystery KIND llm"),
        "unknown model provider",
    );
    assert_err_contains(
        db.execute("CREATE MODEL weird PROVIDER fake KIND oracle"),
        "kind must be llm, embedding, or rerank",
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM models WHERE name IN ('mystery', 'weird')"
        ),
        0
    );
}

#[test]
fn the_catalog_stores_a_key_name_and_refuses_a_secret_value() {
    let tmp = TempDb::new("prov-key-name");
    let db = tmp.open();
    db.execute("CREATE MODEL gpt PROVIDER openai KIND llm KEY_NAME 'PROD_OPENAI_KEY'")
        .expect("key name");
    assert_eq!(
        scalar(&db, "SELECT key_name FROM models WHERE name = 'gpt'"),
        "PROD_OPENAI_KEY"
    );
    // A secret-looking value is refused, and `api_key = ...` is not even a field.
    assert_err_contains(
        db.execute("CREATE MODEL bad PROVIDER openai KIND llm KEY_NAME 'sk-not-a-real-key'"),
        "never the secret",
    );
    assert!(db
        .execute("CREATE MODEL bad2 (kind = llm, provider = openai, api_key = 'sk-nope')")
        .is_err());
    let dumped = db
        .query("SELECT name, provider, provider_model, key_name FROM models")
        .expect("models")
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!dumped.contains("sk-"), "{dumped}");
}

#[test]
fn a_model_whose_key_is_missing_fails_closed_and_records_a_failed_run() {
    let tmp = TempDb::new("prov-missing-key");
    let db = tmp.open();
    // Bind a provider that needs a credential to a key name that is certainly not
    // in the environment. Generation must fail rather than fall back to the fake.
    db.execute("CREATE MODEL gpt PROVIDER openai KIND llm KEY_NAME 'AIDB_TEST_ABSENT_KEY'")
        .expect("model");
    let before = count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'generate'");
    let out = db.query("SELECT aidb_generate('Summarize this', 'Refunds take 14 days')");
    assert_err_contains(out, "AIDB_TEST_ABSENT_KEY is not set");

    let row = db
        .query("SELECT status, error FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1")
        .expect("run");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'generate'"),
        before + 1,
        "the attempt is still observable as a run"
    );
    assert_eq!(cell(&row, 0, "status"), "failed");
    assert!(cell(&row, 0, "error").contains("AIDB_TEST_ABSENT_KEY"));
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND status = 'succeeded'"
        ),
        0,
        "a missing key must not be answered by another provider"
    );
}

#[test]
fn classification_runs_through_the_same_run_model_as_generation() {
    let tmp = TempDb::new("prov-classify");
    let db = tmp.open();
    let out = db
        .query("SELECT aidb_classify('billing or shipping', 'My invoice is wrong')")
        .expect("classify");
    assert_eq!(out.rows[0][0].to_string(), "billing");

    let row = db
        .query(
            "SELECT kind, status, input_json, output_json, prompt_tokens, completion_tokens, cost_usd
             FROM runs ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
        )
        .expect("run");
    assert_eq!(
        cell(&row, 0, "kind"),
        "generate",
        "classification is a generate run, not a separate subsystem"
    );
    assert_eq!(cell(&row, 0, "status"), "succeeded");
    let input: serde_json::Value =
        serde_json::from_str(&cell(&row, 0, "input_json")).expect("input");
    assert_eq!(input["task"], "classify");
    assert_eq!(input["labels"], "billing or shipping");
    assert!(
        cell(&row, 0, "prompt_tokens")
            .parse::<i64>()
            .expect("tokens")
            > 0
    );
    assert!(cell(&row, 0, "cost_usd").parse::<f64>().expect("cost") > 0.0);
}

#[test]
fn a_label_set_that_matches_nothing_still_returns_one_of_the_labels() {
    let tmp = TempDb::new("prov-classify-edge");
    let db = tmp.open();
    let out = db
        .query("SELECT aidb_classify('billing or shipping', 'zzz unrelated text')")
        .expect("classify");
    let text = out.rows[0][0].to_string();
    assert!(
        text == "billing" || text == "shipping",
        "a classifier must answer inside its label set, got {text}"
    );
    // Empty content is not a crash.
    let empty = db
        .query("SELECT aidb_classify('billing or shipping', '')")
        .expect("classify empty");
    assert!(!empty.rows[0][0].to_string().is_empty());
}

#[test]
fn an_unknown_llm_provider_is_an_error_not_a_substitution() {
    // The adapter factory is the single place providers are resolved.
    let err = err_of(
        aidb_ai::llm("hosted_mystery", "x"),
        "an unknown llm provider",
    );
    assert!(err.contains("unknown llm provider"), "{err}");
    let err = err_of(
        aidb_ai::embedder(&aidb::EmbedderConfig {
            provider: "hosted_mystery".into(),
            model: "x".into(),
            dimensions: 8,
            key_name: None,
        }),
        "an unknown embedding provider",
    );
    assert!(err.contains("unknown embedding provider"), "{err}");
}

#[test]
fn the_local_provider_never_reaches_for_a_hosted_one() {
    // A local space is usable with no credential at all, and a missing local model
    // is an error rather than a hosted substitution.
    let tmp = TempDb::new("prov-local");
    let db = tmp.open();
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.query("SELECT aidb_create_space('bge', 'local', 384, 'bge-small')")
        .expect("local space");
    let hits = db
        .query("SELECT * FROM aidb_search('refunds', 3, '{}', 'bge')")
        .expect("local search");
    assert!(!hits.rows.is_empty());
    let err = err_of(
        aidb_ai::embedder(&aidb::EmbedderConfig {
            provider: "local".into(),
            model: "mystery-embed".into(),
            dimensions: 384,
            key_name: None,
        }),
        "an unknown local model",
    );
    assert!(err.contains("unknown local embedding model"), "{err}");
    assert!(
        !err.to_ascii_lowercase().contains("openai"),
        "a local failure must never mention a hosted fallback: {err}"
    );
}
