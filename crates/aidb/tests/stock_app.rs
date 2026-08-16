//! Section 29: the stock desk. An application whose entire backend is one AIDB
//! file — watchlist rows, filings, embeddings, answers, agent steps, approvals and
//! spend. These tests are the contract `examples/stock` depends on, plus the app
//! itself running end to end. Two of them are regressions for bugs the app found:
//! a projected retrieval returned the wrong columns, and an approved agent asked
//! for approval again instead of finishing.

mod common;

use std::process::Command;

use common::*;

const EMAIL_TOOL: &str = "{\"tools\":[{\"name\":\"send.email\",\
    \"inputs\":{\"to\":\"string\",\"subject\":\"string\",\"body\":\"string\"},\
    \"side_effect\":\"irreversible\",\"retry\":\"forbidden\"}]}";

const DESK_POLICY: &str = "{\"name\":\"desk\",\"allow\":[\"search\",\"generate\",\"send.email\"],\
    \"max_usd\":0.01,\"max_llm_calls\":12,\"require_approval\":[\"send.email\"]}";

const DIGEST: &str = "{\"instructions\":\"Draft the digest from the documents, then email it. \
    End with DONE.\",\"goal\":\"Morning digest for NVDA\",\
    \"tools\":[\"search\",\"generate\",\"send.email\"],\"max_steps\":4,\"k\":3,\"decide\":true}";

/// The desk as the application sets it up: its own tables, a tool catalog, a
/// policy, and a small research corpus tagged by ticker.
fn desk(tag: &str) -> (TempDb, aidb::Aidb) {
    let tmp = TempDb::new(tag);
    let db = tmp.open();
    db.execute(
        "CREATE TABLE watchlist (ticker TEXT PRIMARY KEY, name TEXT NOT NULL, added_at_ms INTEGER NOT NULL)",
    )
    .expect("watchlist");
    for (ticker, name) in [("AAPL", "Apple Inc."), ("NVDA", "NVIDIA Corporation")] {
        db.execute(&format!(
            "INSERT INTO watchlist (ticker, name, added_at_ms) VALUES ('{ticker}', '{name}', 1)"
        ))
        .expect("watch row");
    }
    db.query(&format!(
        "SELECT aidb_mcp_register('{}')",
        sql_escape(EMAIL_TOOL)
    ))
    .expect("tool catalog");
    db.query(&format!(
        "SELECT aidb_set_policy('{}')",
        sql_escape(DESK_POLICY)
    ))
    .expect("policy");

    for (title, content, ticker) in [
        (
            "NVDA 10-K excerpt",
            "Data center revenue was 47.5 billion dollars. Two direct customers accounted for 24 percent of total revenue.",
            "NVDA",
        ),
        (
            "NVDA earnings call",
            "Supply of the newest accelerator remains constrained through the first half.",
            "NVDA",
        ),
        (
            "AAPL earnings call",
            "Gross margin is expected to land between 46 and 47 percent for the December quarter.",
            "AAPL",
        ),
    ] {
        insert_doc_meta(
            &db,
            title,
            content,
            &format!("{{\"ticker\":\"{ticker}\",\"kind\":\"filing\"}}"),
        );
    }
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("index the corpus");
    (tmp, db)
}

#[test]
fn an_application_keeps_its_own_tables_beside_the_ai_state() {
    let (tmp, db) = desk("stock-tables");
    db.execute(
        "CREATE TABLE signals (id INTEGER PRIMARY KEY AUTOINCREMENT, ticker TEXT NOT NULL, \
         label TEXT NOT NULL, run_id TEXT, created_at_ms INTEGER NOT NULL)",
    )
    .expect("signals");
    let label = scalar(
        &db,
        "SELECT aidb_classify('bullish or bearish or neutral', 'Hyperscaler trims accelerator orders')",
    );
    let run_id = scalar(&db, "SELECT aidb_last_run_id()");
    db.execute(&format!(
        "INSERT INTO signals (ticker, label, run_id, created_at_ms) \
         VALUES ('NVDA', '{}', '{run_id}', 2)",
        sql_escape(&label)
    ))
    .expect("signal row");

    // Business data joins AI state in one query, because there is one store.
    let joined = db
        .query(
            "SELECT w.ticker, COUNT(s.id), r.kind FROM watchlist w \
             JOIN signals s ON s.ticker = w.ticker \
             JOIN runs r ON r.id = s.run_id \
             GROUP BY w.ticker",
        )
        .expect("join app data with runs");
    assert_eq!(joined.rows.len(), 1, "{joined:?}");
    assert_eq!(cell(&joined, 0, "ticker"), "NVDA");
    assert_eq!(cell(&joined, 0, "kind"), "generate");

    drop(db);
    let db = tmp.open();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM watchlist"), 2);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM signals"), 1);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM documents WHERE index_status = 'ready'"
        ),
        3
    );
    only_the_one_file(&tmp);
}

/// Regression: `SELECT document_id, content FROM aidb_search(...)` used to return
/// every retrieval column, so a caller reading by position got `chunk_id` where it
/// asked for `content`. Our own memory face did exactly that.
#[test]
fn a_projected_retrieval_returns_the_columns_the_caller_asked_for() {
    let (_tmp, db) = desk("stock-projection");
    let hits = db
        .query("SELECT document_id, content FROM aidb_search('customer concentration', 3)")
        .expect("projected search");
    assert_eq!(hits.columns, vec!["document_id", "content"]);
    assert!(!hits.rows.is_empty());
    for row in &hits.rows {
        assert_eq!(row.len(), 2, "{row:?}");
        assert!(
            row[0].to_string().starts_with("doc_"),
            "column one must be the document id: {row:?}"
        );
        assert!(
            row[1].to_string().len() > 20,
            "column two must be the chunk text, not its id: {row:?}"
        );
    }

    // Order follows the request, not the retrieval's internal layout.
    let flipped = db
        .query("SELECT content, document_id FROM aidb_search('customer concentration', 3)")
        .expect("flipped");
    assert_eq!(flipped.columns, vec!["content", "document_id"]);
    assert_eq!(flipped.rows[0][1], hits.rows[0][0]);

    // `*` still means the whole retrieval row.
    let all = db
        .query("SELECT * FROM aidb_search('customer concentration', 3)")
        .expect("star");
    assert_eq!(
        all.columns,
        vec!["document_id", "chunk_id", "content", "distance"]
    );

    // Expressions are not projected: the caller gets every column rather than a
    // silently wrong one.
    let expression = db
        .query(
            "SELECT document_id, substr(content, 1, 10) AS content FROM aidb_search('supply', 2)",
        )
        .expect("expression list");
    assert_eq!(expression.columns.len(), 4);

    // A column that does not exist is an error, not four columns.
    assert_err_contains(
        db.query("SELECT document_id, ticker FROM aidb_search('supply', 2)"),
        "unknown column ticker",
    );

    // Memory is the same surface, and the same contract.
    db.query("SELECT aidb_memory_insert('user:1', 'Prefers two sentence answers.')")
        .expect("memory");
    db.drain_index(std::time::Duration::from_secs(30))
        .expect("index memory");
    let memory = db
        .query("SELECT document_id, content FROM aidb_memory_search('answer length', 3, 'user:1')")
        .expect("memory search");
    assert_eq!(memory.columns, vec!["document_id", "content"]);
    assert_eq!(
        memory.rows[0][1].to_string(),
        "Prefers two sentence answers."
    );
}

#[test]
fn an_answer_scoped_to_one_ticker_only_cites_that_ticker() {
    let (_tmp, db) = desk("stock-citations");
    let answer = scalar(
        &db,
        "SELECT aidb_generate('Answer from the sources', content) \
         FROM aidb_search('what is the margin guidance', 3, '{\"ticker\":\"AAPL\"}')",
    );
    let value: serde_json::Value = serde_json::from_str(&answer).expect("cited answer");
    let sources = value["sources"].as_array().expect("sources");
    assert!(!sources.is_empty(), "a desk answer has to be citable");
    for source in sources {
        let doc = source["document_id"].as_str().expect("document_id");
        assert_eq!(
            scalar(
                &db,
                &format!(
                    "SELECT json_extract(metadata_json, '$.ticker') FROM documents WHERE id = '{doc}'"
                )
            ),
            "AAPL",
            "a filtered answer must not cite another ticker"
        );
    }
}

/// Regression: the digest agent's model says DONE while drafting, then the email
/// tool runs after it. The DONE used to be overwritten by the tool output, so an
/// approved run looped and parked again — the approval queue never drained.
#[test]
fn an_approved_digest_finishes_instead_of_asking_again() {
    let (tmp, db) = desk("stock-approve");
    let parked = db
        .query(&format!("SELECT aidb_agent('{}')", sql_escape(DIGEST)))
        .expect("digest");
    let run_id = cell(&parked, 0, "run_id");
    assert_eq!(cell(&parked, 0, "status"), "awaiting_approval");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'tool'"),
        0,
        "nothing may be sent before a human answers"
    );

    // The parked run is visible to a different process, which is how the app's
    // approval queue works.
    drop(db);
    let db = tmp.open();
    let waiting = db
        .query("SELECT id, kind FROM runs WHERE status IN ('awaiting_approval', 'suspended')")
        .expect("queue");
    assert_eq!(waiting.rows.len(), 1, "{waiting:?}");
    assert_eq!(cell(&waiting, 0, "id"), run_id);

    let resumed = db
        .query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
        ))
        .expect("approve");
    assert_eq!(
        cell(&resumed, 0, "status"),
        "succeeded",
        "an approved digest must finish, not park again"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE status IN ('awaiting_approval', 'suspended')"
        ),
        0,
        "the approval queue has to drain"
    );

    // The tool ran exactly once, and it is a child of the agent.
    let tools = db
        .query("SELECT parent_id, status, output_json FROM runs WHERE kind = 'tool'")
        .expect("tool runs");
    assert_eq!(tools.rows.len(), 1, "{tools:?}");
    assert_eq!(cell(&tools, 0, "parent_id"), run_id);
    assert_eq!(cell(&tools, 0, "status"), "succeeded");
    assert!(
        cell(&tools, 0, "output_json").contains("\"queued\":true"),
        "{tools:?}"
    );
}

#[test]
fn a_rejected_digest_never_sends_and_cannot_be_resumed_twice() {
    let (_tmp, db) = desk("stock-reject");
    let run_id = cell(
        &db.query(&format!("SELECT aidb_agent('{}')", sql_escape(DIGEST)))
            .expect("digest"),
        0,
        "run_id",
    );
    let rejected = db
        .query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":false}}')"
        ))
        .expect("reject");
    assert_eq!(cell(&rejected, 0, "status"), "cancelled");
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM runs WHERE kind = 'tool'"),
        0,
        "a rejected digest must not send anything"
    );
    assert_err_contains(
        db.query(&format!(
            "SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"
        )),
        "not awaiting_approval",
    );
}

#[test]
fn the_desk_can_price_every_ai_call_from_the_file() {
    let (_tmp, db) = desk("stock-spend");
    let agent = db
        .query(
            "SELECT aidb_agent('{\"instructions\":\"Brief the desk. End with DONE.\",\
             \"goal\":\"Risks for NVDA\",\"tools\":[\"search\",\"generate\"],\"max_steps\":2,\"k\":3}')",
        )
        .expect("brief");
    let run_id = cell(&agent, 0, "run_id");

    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'generate' AND cost_usd IS NULL"
        ),
        0,
        "every model call has to carry its price"
    );
    let total: f64 = scalar(&db, "SELECT COALESCE(SUM(cost_usd), 0) FROM runs")
        .parse()
        .expect("total spend");
    assert!(total > 0.0, "the file has to know what the desk spent");

    // Spend is attributable: the agent's children add up to what the agent cost.
    let children: f64 = scalar(
        &db,
        &format!("SELECT COALESCE(SUM(cost_usd), 0) FROM runs WHERE parent_id = '{run_id}'"),
    )
    .parse()
    .expect("child spend");
    assert!(children > 0.0, "the brief's steps must record their cost");
    assert!(children <= total + f64::EPSILON);
}

// --------------------------------------------------------------- the real app

/// The example app is a real face: node, the napi addon, one file. When the addon
/// is not staged, say so rather than pretending the app was exercised.
fn app_available() -> Result<std::path::PathBuf, String> {
    let app = repo_root().join("examples/stock/stock.mjs");
    if !app.exists() {
        return Err(format!("{} is missing", app.display()));
    }
    let addon_present = std::fs::read_dir(repo_root().join("bindings/typescript"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().ends_with(".node"))
        })
        .unwrap_or(false);
    if !addon_present {
        return Err(
            "napi addon is not staged; run: cargo build -p aidb-node && \
                    node bindings/typescript/scripts/stage-native.mjs"
                .into(),
        );
    }
    if Command::new("node").arg("--version").output().is_err() {
        return Err("node is not installed".into());
    }
    Ok(app)
}

fn app(script: &std::path::Path, db: &str, args: &[&str]) -> String {
    let mut command = Command::new("node");
    command.arg(script).args(args).args(["--db", db]);
    let out = command.output().expect("spawn the stock app");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "stock {args:?} failed: {}{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

#[test]
fn the_stock_desk_runs_end_to_end_on_one_file() {
    let script = match app_available() {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("skipping the stock desk app test: {reason}");
            return;
        }
    };
    let tmp = TempDb::new("stock-app");
    let path = tmp.path().to_string_lossy().into_owned();

    app(&script, &path, &["init"]);
    let ingested = app(&script, &path, &["ingest"]);
    assert!(ingested.contains("7 documents"), "{ingested}");
    // Re-ingesting the same filings is a no-op: the desk keys them itself, because
    // two inserts of the same text would otherwise be two documents.
    let again = app(&script, &path, &["ingest"]);
    assert!(again.contains("7 already present"), "{again}");

    let answer = app(
        &script,
        &path,
        &["ask", "how concentrated is data center revenue", "--k", "3"],
    );
    assert!(answer.contains("sources:"), "{answer}");
    assert!(
        answer.contains("NVDA"),
        "an answer has to cite the desk's own filings: {answer}"
    );

    let scoped = app(
        &script,
        &path,
        &[
            "ask",
            "what is the margin guidance",
            "--ticker",
            "AAPL",
            "--k",
            "3",
        ],
    );
    assert!(
        !scoped.contains("NVDA"),
        "a ticker-scoped answer must not cite another ticker: {scoped}"
    );

    app(
        &script,
        &path,
        &["remember", "u1", "Prefers two sentence answers."],
    );
    let brief = app(&script, &path, &["brief", "NVDA"]);
    assert!(brief.contains("succeeded"), "{brief}");
    assert!(
        brief.contains("search") && brief.contains("generate"),
        "{brief}"
    );

    let digest = app(&script, &path, &["digest", "NVDA"]);
    assert!(digest.contains("awaiting_approval"), "{digest}");
    let queue = app(&script, &path, &["waiting"]);
    let run_id = queue
        .split_whitespace()
        .next()
        .expect("a parked run id")
        .to_string();
    assert!(run_id.starts_with("run_"), "{queue}");

    app(
        &script,
        &path,
        &["sentiment", "NVDA", "Hyperscaler trims orders"],
    );
    let approved = app(&script, &path, &["approve", &run_id]);
    assert!(approved.contains("succeeded"), "{approved}");
    assert!(
        app(&script, &path, &["waiting"]).contains("nothing is waiting"),
        "the app's approval queue has to drain"
    );

    let status = app(&script, &path, &["status"]);
    assert!(status.contains("7 indexed"), "{status}");
    assert!(status.contains("3 tickers"), "{status}");

    // Now check the file itself, not the app's own reporting.
    let db = tmp.open();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM watchlist"), 3);
    assert!(count(&db, "SELECT COUNT(*) FROM signals") >= 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM memory"), 1);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM documents WHERE index_status = 'ready' \
             AND COALESCE(json_extract(metadata_json, '$.kind'), '') != 'memory'"
        ),
        7
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'agent' AND status = 'succeeded'"
        ),
        2,
        "the brief and the approved digest both have to finish"
    );
    let tools = db
        .query("SELECT status, output_json FROM runs WHERE kind = 'tool'")
        .expect("tool runs");
    assert_eq!(
        tools.rows.len(),
        1,
        "the email must run once, after approval"
    );
    assert!(cell(&tools, 0, "output_json").contains("\"queued\":true"));
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE status IN ('awaiting_approval', 'suspended')"
        ),
        0
    );
    let policy: serde_json::Value =
        serde_json::from_str(&scalar(&db, "SELECT aidb_get_policy()")).expect("policy json");
    assert_eq!(policy["name"], "desk");
    assert_eq!(policy["require_approval"][0], "send.email");
    assert!(
        scalar(&db, "SELECT COALESCE(SUM(cost_usd), 0) FROM runs")
            .parse::<f64>()
            .expect("spend")
            > 0.0
    );

    only_the_one_file(&tmp);
}

/// The whole application is one file plus SQLite's own sidecars: no sidecar index,
/// no trace directory, no vector store.
fn only_the_one_file(tmp: &TempDb) {
    let strays: Vec<String> = std::fs::read_dir(tmp.dir())
        .expect("temp dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with("app.db"))
        .collect();
    assert!(
        strays.is_empty(),
        "extra durable state appeared: {strays:?}"
    );
}
