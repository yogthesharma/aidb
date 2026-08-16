//! Section 25: every `examples/sql/phase*.sql` file is a user-visible proof, so
//! every one of them has to still run. The assertions here are about the demo
//! working end to end; the behaviour each demo shows is asserted in the
//! phase-specific suites.

mod common;

use std::time::Duration;

use common::*;

/// Split a file into the statements a user would paste into `aidb sql`, dropping
/// the commented walkthrough at the top. Terminators inside string literals stay
/// part of the statement.
fn statements(sql: &str) -> Vec<String> {
    let body: String = sql
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    for ch in body.chars() {
        match ch {
            '\'' => {
                in_string = !in_string;
                current.push(ch);
            }
            ';' if !in_string => {
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    // The goal-language demo is a multi-line statement with no terminator.
    out.push(current);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn example_files() -> Vec<(String, String)> {
    let dir = repo_root().join("examples/sql");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .map(|path| {
            let name = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read_to_string(&path).expect("read example");
            (name, text)
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// The demos assume the walkthrough above them has been run, which for the
/// retrieval examples means a couple of indexed documents.
fn seed(db: &aidb::Aidb) {
    insert_doc_meta(
        db,
        "Refunds",
        "Refunds are issued within 14 days of purchase. How do refunds work ZX19QPLUGH.",
        "{\"dept\":\"support\"}",
    );
    insert_doc_meta(
        db,
        "Shipping",
        "Orders ship within two business days.",
        "{\"dept\":\"logistics\"}",
    );
    db.drain_index(Duration::from_secs(30)).expect("drain");
}

/// Run a demo statement the way `aidb sql` runs it: retrieval and EXPLAIN read,
/// everything else writes.
fn run(db: &aidb::Aidb, statement: &str) -> aidb::Result<()> {
    let head = statement.trim_start().to_ascii_lowercase();
    let reads = ["select", "pragma", "with", "explain", "search", "task"]
        .iter()
        .any(|kw| head.starts_with(kw));
    if reads {
        db.query(statement).map(|_| ())
    } else {
        db.execute(statement).map(|_| ())
    }
}

/// The MCP demo spawns a server by relative path; point it at the built binary.
fn resolve(statement: &str) -> String {
    if statement.contains("./fake-mcp") {
        return statement.replace(
            "./fake-mcp",
            &target_bin("fake-mcp").to_string_lossy().replace('\'', "''"),
        );
    }
    statement.to_string()
}

#[test]
fn every_phase_demo_still_runs_against_a_fresh_database() {
    let files = example_files();
    assert!(files.len() >= 25, "found only {} examples", files.len());
    for (name, text) in files {
        let statements = statements(&text);
        assert!(
            !statements.is_empty(),
            "{name} has no runnable statement, only prose"
        );
        let tmp = TempDb::new("examples");
        let db = tmp.open();
        seed(&db);
        for statement in statements {
            let statement = resolve(&statement);
            run(&db, &statement).unwrap_or_else(|e| panic!("{name}: {statement}\n  failed: {e}"));
        }
    }
}

#[test]
fn every_demo_tells_the_reader_how_to_run_it() {
    for (name, text) in example_files() {
        assert!(
            text.contains("aidb-cli")
                || text.contains("curl")
                || text.contains("aidb ")
                || text.contains("AI.open"),
            "{name} does not show how to run it"
        );
        assert!(
            text.lines().any(|line| line.trim_start().starts_with("--")),
            "{name} has no explanation for a reader"
        );
    }
}

#[test]
fn the_explain_demos_print_a_plan_rather_than_rows() {
    let tmp = TempDb::new("examples-explain");
    let db = tmp.open();
    seed(&db);
    for (name, text) in example_files() {
        for statement in statements(&text) {
            if !statement.to_uppercase().starts_with("EXPLAIN") {
                continue;
            }
            let plan = db
                .query(&statement)
                .unwrap_or_else(|e| panic!("{name}: {statement}: {e}"));
            assert!(!plan.rows.is_empty(), "{name}: EXPLAIN printed nothing");
            let rendered = plan
                .rows
                .iter()
                .map(|row| row[0].to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                rendered.to_lowercase().contains("plan")
                    || rendered.contains("Search")
                    || rendered.contains("Generate")
                    || rendered.contains("Scan")
                    || rendered.contains("Decide"),
                "{name}: plan is not readable: {rendered}"
            );
        }
    }
}

#[test]
fn the_search_demos_actually_return_the_seeded_documents() {
    let tmp = TempDb::new("examples-search");
    let db = tmp.open();
    seed(&db);
    // phase10/phase18: the retrieval demos are only proofs if they find something.
    let hybrid = db
        .query("SELECT document_id, chunk_id, content, distance FROM aidb_search('How do refunds work ZX19QPLUGH', 3)")
        .expect("hybrid demo");
    assert!(!hybrid.rows.is_empty());
    let filtered = db
        .query("SELECT document_id, chunk_id, content, distance FROM aidb_search('refund policy', 5, '{\"dept\":\"support\"}')")
        .expect("filter demo");
    assert!(!filtered.rows.is_empty());
    for content in column_values(&filtered, "content") {
        assert!(content.contains("Refund"), "{content}");
    }
    // phase15: the dialect demo answers the same way as the function form.
    let dialect = db
        .query("SEARCH 'How do refunds work?' LIMIT 5")
        .expect("dialect");
    let function = db
        .query("SELECT * FROM aidb_search('How do refunds work?', 5)")
        .expect("function");
    assert_eq!(
        column_values(&dialect, "chunk_id"),
        column_values(&function, "chunk_id")
    );
}

#[test]
fn the_demos_are_runnable_through_the_cli_exactly_as_written() {
    // The walkthrough is a CLI transcript, so at least one demo has to hold up as
    // one: same file, same statements, printed rows, exit code zero.
    let tmp = TempDb::new("examples-cli");
    let path = tmp.path();
    let db = path.to_string_lossy().into_owned();
    let inserted = cli(&[
        "sql",
        &db,
        "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');",
    ]);
    assert!(inserted.status.success(), "{}", stderr_of(&inserted));
    assert!(
        stdout_of(&inserted).contains("doc_"),
        "{}",
        stdout_of(&inserted)
    );

    let meta = cli(&["sql", &db, "SELECT key, value FROM aidb_meta ORDER BY key;"]);
    assert!(meta.status.success(), "{}", stderr_of(&meta));
    assert!(stdout_of(&meta).contains("schema_version"));

    let runs = cli(&[
        "sql",
        &db,
        "SELECT id, kind, status, error, created_at_ms FROM runs ORDER BY created_at_ms DESC;",
    ]);
    assert!(runs.status.success(), "{}", stderr_of(&runs));
    assert!(stdout_of(&runs).contains("index_document"));
}
