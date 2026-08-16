//! Phase 0 / 23 contracts at the command line. The CLI is a face over the same
//! file: it creates nothing of its own, and its exit codes are usable in scripts.

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;

fn sql(path: &std::path::Path, statement: &str) -> std::process::Output {
    cli(&["sql", &path.to_string_lossy(), statement])
}

fn ok_stdout(output: &std::process::Output, what: &str) -> String {
    assert!(
        output.status.success(),
        "{what} failed: {}",
        stderr_of(output)
    );
    stdout_of(output)
}

#[test]
fn aidb_sql_creates_the_database_and_answers_a_query() {
    let tmp = TempDb::new("cli-create");
    let path = tmp.path();
    assert!(!path.exists(), "the file does not exist yet");

    let out = sql(
        &path,
        "SELECT value FROM aidb_meta WHERE key = 'schema_version'",
    );
    let text = ok_stdout(&out, "schema version");
    assert!(path.exists(), "the CLI created the file");
    assert!(
        text.trim_end().ends_with(&aidb::SCHEMA_VERSION.to_string()),
        "unexpected output: {text:?}"
    );
    // Output is tab separated with a header, so it pipes into ordinary tools.
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("value"));
    assert_eq!(
        lines.next(),
        Some(aidb::SCHEMA_VERSION.to_string().as_str())
    );
}

#[test]
fn a_document_written_by_one_invocation_is_searchable_from_the_next() {
    let tmp = TempDb::new("cli-persist");
    let path = tmp.path();
    let inserted = ok_stdout(
        &sql(
            &path,
            "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')",
        ),
        "insert",
    );
    let id = inserted.lines().nth(1).expect("id row").trim().to_string();
    assert!(id.starts_with("doc_"), "{id}");

    // A separate process sees the committed document, already indexed.
    let status = ok_stdout(
        &sql(
            &path,
            &format!("SELECT index_status FROM documents WHERE id = '{id}'"),
        ),
        "status",
    );
    assert_eq!(status.lines().nth(1), Some("ready"));

    let hits = ok_stdout(
        &sql(&path, "SELECT * FROM aidb_search('how do refunds work', 5)"),
        "search",
    );
    assert!(hits.contains(&id), "{hits}");

    // And the Rust API opening the same file agrees.
    let db = tmp.open();
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM documents WHERE id = '{id}'")
        ),
        1
    );
}

#[test]
fn the_dialect_explain_and_goal_forms_all_work_from_the_command_line() {
    let tmp = TempDb::new("cli-surfaces");
    let path = tmp.path();
    sql(
        &path,
        "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')",
    );

    let dialect = ok_stdout(
        &sql(&path, "SELECT * FROM documents SEARCH 'refunds' LIMIT 3"),
        "dialect search",
    );
    assert!(dialect.contains("document_id"), "{dialect}");

    let explain = ok_stdout(
        &sql(&path, "EXPLAIN SELECT * FROM aidb_search('refunds', 3)"),
        "explain",
    );
    assert!(explain.contains("TopK"), "{explain}");

    let goal = ok_stdout(
        &sql(
            &path,
            "TASK summarize\nDATA documents\nCONSTRAINTS budget $1\nGOAL How do refunds work?",
        ),
        "goal",
    );
    assert!(!goal.trim().is_empty(), "a goal must produce output");

    // A write statement returns no rows but still succeeds.
    let write = sql(
        &path,
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)",
    );
    assert!(write.status.success(), "{}", stderr_of(&write));
    assert_eq!(stdout_of(&write), "");
}

#[test]
fn invalid_sql_exits_non_zero_and_explains_itself_on_stderr() {
    let tmp = TempDb::new("cli-bad-sql");
    let path = tmp.path();
    let out = sql(&path, "SELECT * FROM table_that_does_not_exist");
    assert!(
        !out.status.success(),
        "a broken query must fail the process"
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout_of(&out), "", "no partial rows on failure");
    let err = stderr_of(&out);
    assert!(err.starts_with("aidb: "), "{err}");
    assert!(err.contains("table_that_does_not_exist"), "{err}");
}

#[test]
fn a_failing_ai_operation_exits_non_zero() {
    let tmp = TempDb::new("cli-bad-ai");
    let path = tmp.path();
    let out = sql(
        &path,
        "SELECT * FROM aidb_search('refunds', 5, '{}', 'ghost')",
    );
    assert!(!out.status.success());
    assert!(
        stderr_of(&out).contains("unknown embedding space"),
        "{}",
        stderr_of(&out)
    );
}

#[test]
fn misuse_prints_usage_and_fails() {
    let tmp = TempDb::new("cli-usage");
    let path = tmp.path();
    let path_s = path.to_string_lossy();
    for args in [
        vec!["sql"],
        vec!["sql", path_s.as_ref()],
        vec!["runs"],
        vec!["serve"],
        vec!["frobnicate"],
        vec![],
    ] {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        let out = cli(&refs);
        assert!(!out.status.success(), "{refs:?} should have failed");
        assert!(
            stderr_of(&out).contains("Usage: aidb sql"),
            "{refs:?}: {}",
            stderr_of(&out)
        );
    }
    // Extra positional arguments are misuse too, not silently ignored.
    let out = cli(&["sql", &path.to_string_lossy(), "SELECT 1", "extra"]);
    assert!(!out.status.success());
}

#[test]
fn an_unopenable_path_is_a_clean_failure() {
    let tmp = TempDb::new("cli-bad-path");
    // A directory is not a database file.
    let out = cli(&["sql", &tmp.dir().to_string_lossy(), "SELECT 1"]);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.starts_with("aidb: "), "{err}");
    assert!(stdout_of(&out).is_empty());
}

#[test]
fn aidb_runs_lists_the_run_history_and_only_the_waiting_ones_with_a_flag() {
    let tmp = TempDb::new("cli-runs");
    let path = tmp.path();
    sql(&path, "SELECT aidb_generate('one word', 'hello there')");

    let listed = ok_stdout(&cli(&["runs", &path.to_string_lossy()]), "runs");
    let mut lines = listed.lines();
    assert_eq!(
        lines.next(),
        Some("id\tkind\tstatus\terror\tcreated_at_ms"),
        "{listed}"
    );
    let row = lines.next().expect("one run").to_string();
    assert!(row.contains("generate"), "{row}");
    assert!(row.contains("succeeded"), "{row}");

    // Nothing is waiting yet.
    let waiting = ok_stdout(
        &cli(&["runs", &path.to_string_lossy(), "--waiting"]),
        "waiting",
    );
    assert_eq!(waiting.lines().count(), 1, "header only: {waiting}");

    // Park a workflow on an approval, then it shows up.
    let spec = "{\"then\":[{\"approve\":{\"message\":\"Send this answer?\"}},\
        {\"generate\":{\"prompt\":\"Draft the reply\"}}]}";
    ok_stdout(
        &sql(&path, &format!("SELECT aidb_workflow('{spec}')")),
        "workflow",
    );
    let waiting = ok_stdout(
        &cli(&["runs", &path.to_string_lossy(), "--waiting"]),
        "waiting",
    );
    assert!(waiting.contains("awaiting_approval"), "{waiting}");
    assert!(waiting.contains("Send this answer?"), "{waiting}");

    // And it can be approved from the command line too.
    let run_id = waiting
        .lines()
        .nth(1)
        .and_then(|line| line.split('\t').next())
        .expect("run id")
        .to_string();
    ok_stdout(
        &sql(
            &path,
            &format!("SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"),
        ),
        "resume",
    );
    let after = ok_stdout(
        &cli(&["runs", &path.to_string_lossy(), "--waiting"]),
        "waiting after resume",
    );
    assert_eq!(after.lines().count(), 1, "nothing is waiting now: {after}");
}

#[test]
fn aidb_serve_answers_over_http_against_the_same_file() {
    let tmp = TempDb::new("cli-serve");
    let path = tmp.path();
    sql(
        &path,
        "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')",
    );

    let mut child = Command::new(cli_bin())
        .args(["serve", &path.to_string_lossy(), "--bind", "127.0.0.1:0"])
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn aidb serve");

    // The server announces where it listens; that line is the handshake.
    let stderr = child.stderr.take().expect("stderr");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    reader.read_line(&mut line).expect("banner");
    let addr = line
        .split("http://")
        .nth(1)
        .map(|rest| rest.trim().to_string())
        .unwrap_or_else(|| panic!("no address in banner: {line:?}"));
    assert!(line.contains("app.db"), "{line}");

    let result = std::panic::catch_unwind(|| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match ureq::get(&format!("http://{addr}/health")).call() {
                Ok(resp) => {
                    let body: serde_json::Value = resp.into_json().expect("json");
                    assert_eq!(body["ok"], true);
                    break;
                }
                Err(err) if Instant::now() < deadline => {
                    let _ = err;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(err) => panic!("health never came up: {err}"),
            }
        }

        // The served file is the same file the CLI wrote.
        let body: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
            .send_string("SELECT title FROM documents")
            .expect("sql over http")
            .into_json()
            .expect("json");
        assert_eq!(body["ok"], true);
        assert_eq!(body["rows"][0][0], "Refunds");

        // A write over HTTP lands in the same file.
        let inserted: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
            .send_string(
                "SELECT aidb_insert_document('Shipping', 'Shipping takes three days.', '{}')",
            )
            .expect("insert over http")
            .into_json()
            .expect("json");
        assert_eq!(inserted["ok"], true);
        inserted["rows"][0][0].as_str().expect("id").to_string()
    });

    let _ = child.kill();
    let _ = child.wait();
    let id = result.unwrap_or_else(|e| std::panic::resume_unwind(e));

    // After the server is gone, the CLI still reads what it wrote.
    let titles = ok_stdout(
        &sql(&path, "SELECT title FROM documents ORDER BY title"),
        "titles",
    );
    assert!(
        titles.contains("Refunds") && titles.contains("Shipping"),
        "{titles}"
    );
    let db = tmp.open();
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM documents WHERE id = '{id}'")
        ),
        1,
        "the Rust API sees the row the server wrote"
    );
}

#[test]
fn many_readers_can_query_the_same_file_at_once() {
    let tmp = TempDb::new("cli-readers");
    let path = tmp.path();
    sql(
        &path,
        "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')",
    );

    let children: Vec<_> = (0..6)
        .map(|_| {
            Command::new(cli_bin())
                .args([
                    "sql",
                    &path.to_string_lossy(),
                    "SELECT COUNT(*) FROM documents",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn reader")
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().expect("reader output");
        assert!(out.status.success(), "{}", stderr_of(&out));
        assert_eq!(stdout_of(&out).lines().nth(1), Some("1"));
    }
}
