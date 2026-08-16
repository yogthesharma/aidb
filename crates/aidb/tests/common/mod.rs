//! Shared helpers for the integration suite. Deterministic, offline, no live credentials.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aidb::{Aidb, QueryResult, Value};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary database path that removes the file (and WAL sidecars) on drop.
pub struct TempDb {
    dir: PathBuf,
}

impl TempDb {
    pub fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aidb-it-{tag}-{nanos}-{n}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self { dir }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join("app.db")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Another file inside the same temp dir (for multi-file tests).
    pub fn sibling(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn open(&self) -> Aidb {
        let db = aidb::open(self.path()).expect("open aidb");
        // Session bind and last-run id are thread-local. Isolate tests that share a worker thread.
        let _ = db.query("SELECT aidb_session(NULL)");
        aidb::clear_last_run_id();
        db
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Insert a document through the public SQL surface and wait until it is searchable.
pub fn insert_ready(db: &Aidb, title: &str, content: &str) -> String {
    let id = insert_doc(db, title, content);
    db.drain_index(Duration::from_secs(30)).expect("drain");
    id
}

pub fn insert_doc(db: &Aidb, title: &str, content: &str) -> String {
    insert_doc_meta(db, title, content, "{}")
}

pub fn insert_doc_meta(db: &Aidb, title: &str, content: &str, metadata_json: &str) -> String {
    let rows = db
        .query(&format!(
            "SELECT aidb_insert_document('{}', '{}', '{}')",
            sql_escape(title),
            sql_escape(content),
            sql_escape(metadata_json)
        ))
        .expect("insert document with metadata");
    rows.rows[0][0].to_string()
}

pub fn sql_escape(text: &str) -> String {
    text.replace('\'', "''")
}

/// First column of the first row, as text. Panics when the query returned nothing.
pub fn scalar(db: &Aidb, sql: &str) -> String {
    let rows = db.query(sql).expect(sql);
    assert!(!rows.rows.is_empty(), "no rows for: {sql}");
    rows.rows[0][0].to_string()
}

pub fn scalar_i64(db: &Aidb, sql: &str) -> i64 {
    match db.query(sql).expect(sql).rows[0][0] {
        Value::Integer(v) => v,
        Value::Real(v) => v as i64,
        ref other => other.to_string().parse().expect("integer scalar"),
    }
}

pub fn count(db: &Aidb, sql: &str) -> i64 {
    scalar_i64(db, sql)
}

/// Column index by name; panics when the column is absent.
pub fn col(result: &QueryResult, name: &str) -> usize {
    result
        .columns
        .iter()
        .position(|c| c == name)
        .unwrap_or_else(|| panic!("column {name} not in {:?}", result.columns))
}

pub fn cell(result: &QueryResult, row: usize, name: &str) -> String {
    result.rows[row][col(result, name)].to_string()
}

pub fn column_values(result: &QueryResult, name: &str) -> Vec<String> {
    let idx = col(result, name);
    result.rows.iter().map(|r| r[idx].to_string()).collect()
}

/// Assert that an error happened and that its message mentions `needle`.
pub fn assert_err_contains<T>(result: aidb::Result<T>, needle: &str) {
    match result {
        Ok(_) => panic!("expected an error mentioning {needle:?}"),
        Err(err) => {
            let text = err.to_string();
            assert!(
                text.to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase()),
                "error {text:?} does not mention {needle:?}"
            );
        }
    }
}

/// Run the `aidb` CLI against a database file.
pub fn cli(args: &[&str]) -> std::process::Output {
    std::process::Command::new(cli_bin())
        .args(args)
        .output()
        .expect("spawn aidb cli")
}

/// Run the CLI with extra environment variables. Set them on the child, never in
/// the test process: the suite runs tests in parallel threads.
pub fn cli_with_env(args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut command = std::process::Command::new(cli_bin());
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("spawn aidb cli")
}

pub fn cli_bin() -> PathBuf {
    static CHECKED: OnceLock<PathBuf> = OnceLock::new();
    CHECKED
        .get_or_init(|| {
            let bin = target_bin("aidb");
            // `cargo test` does not refresh the uplifted binary, so a leftover build
            // from an earlier session can answer these tests. That fails much later
            // in confusing ways ("no such table: aidb_search"), so prove up front
            // that the binary is the engine the tests were built against.
            let probe = std::env::temp_dir().join(format!(
                "aidb-cli-probe-{}.db",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            let out = std::process::Command::new(&bin)
                .args([
                    "sql",
                    &probe.to_string_lossy(),
                    "SELECT value FROM aidb_meta WHERE key = 'schema_version'",
                ])
                .output()
                .expect("probe the aidb cli");
            let reported = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", probe.display()));
            }
            assert!(
                reported.ends_with(&aidb::SCHEMA_VERSION.to_string()),
                "the aidb binary at {} is stale (it reports {reported:?}, the engine is at schema {}). \
                 Run `cargo build --workspace` before `cargo test --workspace`.",
                bin.display(),
                aidb::SCHEMA_VERSION
            );
            bin
        })
        .clone()
}

/// A binary from the same target directory as this test. Integration tests live in
/// the `aidb` crate, so CARGO_BIN_EXE_ is not set for binaries of other crates.
pub fn target_bin(name: &str) -> PathBuf {
    let mut dir = std::env::current_exe().expect("current exe");
    dir.pop(); // deps/
    if dir.ends_with("deps") {
        dir.pop();
    }
    let candidate = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        candidate.exists(),
        "{name} binary not found at {}; run `cargo build --workspace` first",
        candidate.display()
    );
    candidate
}

/// Repository root, derived from this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop(); // crates/
    dir.pop();
    dir
}

pub fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
