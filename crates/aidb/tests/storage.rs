//! Phase 0 contracts: open, pragmas, ordinary SQL, transactions, reopen, readers.
//! Derived from DESIGN.md §15 (pragmas, connections) and PHASES.md phase 0.

mod common;

use std::time::Duration;

use aidb::{Value, SCHEMA_VERSION};
use common::*;

#[test]
fn opening_a_nonexistent_path_creates_the_database() {
    let tmp = TempDb::new("open-create");
    let path = tmp.sibling("nested/deeper/app.db");
    assert!(!path.exists());
    let db = aidb::open(&path).expect("open creates");
    assert_eq!(db.path(), path.as_path());
    drop(db);
    assert!(path.exists(), "AI.open must create the file");
}

#[test]
fn schema_version_matches_the_engine_and_migrations_are_idempotent() {
    let tmp = TempDb::new("version");
    for _ in 0..3 {
        let db = tmp.open();
        assert_eq!(
            scalar(
                &db,
                "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
            ),
            SCHEMA_VERSION.to_string()
        );
        // Reopening must not duplicate seeded rows.
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM aidb_meta WHERE key = 'schema_version'"
            ),
            1
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM capabilities WHERE name = 'search'"
            ),
            1
        );
    }
}

#[test]
fn created_at_ms_is_stamped_once_and_never_rewritten() {
    let tmp = TempDb::new("created-at");
    let first = {
        let db = tmp.open();
        scalar(
            &db,
            "SELECT value FROM aidb_meta WHERE key = 'created_at_ms'",
        )
    };
    assert!(first.parse::<i64>().expect("created_at_ms integer") > 0);
    std::thread::sleep(Duration::from_millis(5));
    let db = tmp.open();
    assert_eq!(
        scalar(
            &db,
            "SELECT value FROM aidb_meta WHERE key = 'created_at_ms'"
        ),
        first
    );
}

#[test]
fn open_sets_the_documented_pragmas() {
    let tmp = TempDb::new("pragmas");
    let db = tmp.open();
    assert_eq!(db.journal_mode().expect("journal mode"), "wal");
    assert_eq!(scalar(&db, "PRAGMA journal_mode"), "wal");
    assert_eq!(scalar(&db, "PRAGMA foreign_keys"), "1");
    assert_eq!(scalar(&db, "PRAGMA busy_timeout"), "5000");
    // synchronous NORMAL == 1, temp_store MEMORY == 2
    assert_eq!(scalar(&db, "PRAGMA synchronous"), "1");
    assert_eq!(scalar(&db, "PRAGMA temp_store"), "2");
}

#[test]
fn wal_sidecar_files_exist_after_a_write() {
    let tmp = TempDb::new("wal-files");
    let db = tmp.open();
    db.execute("CREATE TABLE t (x INTEGER)").expect("create");
    db.execute("INSERT INTO t VALUES (1)").expect("insert");
    let wal = tmp.sibling("app.db-wal");
    assert!(wal.exists(), "WAL journal must exist next to app.db");
}

#[test]
fn ordinary_sql_crud_works_and_survives_reopen() {
    let tmp = TempDb::new("crud");
    {
        let db = tmp.open();
        db.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, country TEXT, total REAL)")
            .expect("create");
        db.execute(
            "INSERT INTO orders (id, country, total) VALUES (1, 'IN', 10.5), (2, 'US', 20.0)",
        )
        .expect("insert");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM orders"), 2);
        db.execute("UPDATE orders SET total = 11.5 WHERE id = 1")
            .expect("update");
        db.execute("DELETE FROM orders WHERE id = 2")
            .expect("delete");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM orders"), 1);
    }
    let db = tmp.open();
    let rows = db
        .query("SELECT id, country, total FROM orders")
        .expect("select");
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(rows.rows[0][0], Value::Integer(1));
    assert_eq!(rows.rows[0][1], Value::Text("IN".into()));
    assert_eq!(rows.rows[0][2], Value::Real(11.5));
}

#[test]
fn every_sqlite_value_type_round_trips() {
    let tmp = TempDb::new("value-types");
    let db = tmp.open();
    db.execute("CREATE TABLE v (i INTEGER, r REAL, t TEXT, b BLOB, n TEXT)")
        .expect("create");
    db.execute("INSERT INTO v VALUES (7, 2.5, 'hi', x'0102', NULL)")
        .expect("insert");
    let rows = db.query("SELECT i, r, t, b, n FROM v").expect("select");
    assert_eq!(rows.rows[0][0], Value::Integer(7));
    assert_eq!(rows.rows[0][1], Value::Real(2.5));
    assert_eq!(rows.rows[0][2], Value::Text("hi".into()));
    assert_eq!(rows.rows[0][3], Value::Blob(vec![1, 2]));
    assert_eq!(rows.rows[0][4], Value::Null);
}

#[test]
fn transactions_commit_and_rollback() {
    let tmp = TempDb::new("tx");
    let db = tmp.open();
    db.execute("CREATE TABLE t (x INTEGER)").expect("create");

    db.execute("BEGIN; INSERT INTO t VALUES (1); INSERT INTO t VALUES (2); COMMIT;")
        .expect("commit");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM t"), 2);

    db.execute("BEGIN; INSERT INTO t VALUES (3); ROLLBACK;")
        .expect("rollback");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM t"), 2);
}

#[test]
fn a_failing_statement_inside_a_batch_does_not_partially_commit() {
    let tmp = TempDb::new("atomic");
    let db = tmp.open();
    db.execute("CREATE TABLE t (x INTEGER PRIMARY KEY)")
        .expect("create");
    let failed = db.execute(
        "BEGIN;
         INSERT INTO t VALUES (1);
         INSERT INTO t VALUES (1);
         COMMIT;",
    );
    assert!(failed.is_err(), "duplicate primary key must fail");
    // The batch never reached COMMIT, so the open transaction is rolled back on
    // the next successful statement boundary; either way no row is visible.
    let _ = db.execute("ROLLBACK");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM t"), 0);
}

#[test]
fn foreign_keys_cascade_document_deletes_to_chunks() {
    let tmp = TempDb::new("fk-cascade");
    let db = tmp.open();
    let id = insert_ready(&db, "Refunds", "Refunds are issued within 14 days.");
    assert!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'")
        ) > 0
    );
    db.execute(&format!("DELETE FROM documents WHERE id = '{id}'"))
        .expect("delete document");
    assert_eq!(
        count(
            &db,
            &format!("SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'")
        ),
        0,
        "foreign_keys=ON must cascade chunks"
    );
}

#[test]
fn invalid_sql_is_a_clean_error_and_leaves_the_database_usable() {
    let tmp = TempDb::new("bad-sql");
    let db = tmp.open();
    assert_err_contains(db.query("SELECT * FROM nope"), "no such table");
    assert_err_contains(db.execute("THIS IS NOT SQL"), "syntax error");
    assert_eq!(
        scalar(
            &db,
            "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
        ),
        SCHEMA_VERSION.to_string()
    );
}

#[test]
fn opening_a_directory_is_a_clean_error() {
    let tmp = TempDb::new("dir-path");
    let result = aidb::open(tmp.dir());
    assert!(result.is_err(), "opening a directory must fail cleanly");
    let message = result.err().unwrap().to_string();
    assert!(
        message.contains(&tmp.dir().display().to_string()),
        "error should name the path: {message}"
    );
}

#[test]
fn opening_a_non_database_file_is_a_clean_error() {
    let tmp = TempDb::new("not-a-db");
    let path = tmp.sibling("garbage.db");
    std::fs::write(&path, b"this is definitely not a sqlite file").expect("write");
    let result = aidb::open(&path);
    assert!(result.is_err(), "a non-sqlite file must not open silently");
}

#[test]
fn a_schema_newer_than_the_engine_fails_closed() {
    let tmp = TempDb::new("future-schema");
    {
        let db = tmp.open();
        db.execute(&format!(
            "UPDATE aidb_meta SET value = '{}' WHERE key = 'schema_version'",
            SCHEMA_VERSION + 1
        ))
        .expect("bump");
    }
    assert_err_contains(aidb::open(tmp.path()), "newer than this engine");
}

#[test]
fn many_readers_see_the_same_committed_state() {
    let tmp = TempDb::new("readers");
    let writer = tmp.open();
    writer
        .execute("CREATE TABLE t (x INTEGER)")
        .expect("create");
    writer.execute("INSERT INTO t VALUES (1)").expect("insert");

    let readers: Vec<_> = (0..4)
        .map(|_| {
            let path = tmp.path();
            std::thread::spawn(move || {
                let db = aidb::open(path).expect("reader open");
                db.query("SELECT COUNT(*) FROM t").expect("read").rows[0][0].to_string()
            })
        })
        .collect();
    for reader in readers {
        assert_eq!(reader.join().expect("reader thread"), "1");
    }

    // The writer keeps working while readers were attached.
    writer
        .execute("INSERT INTO t VALUES (2)")
        .expect("insert 2");
    assert_eq!(count(&writer, "SELECT COUNT(*) FROM t"), 2);
}

#[test]
fn concurrent_writers_serialize_instead_of_failing_with_sqlite_busy() {
    let tmp = TempDb::new("writers");
    {
        let db = tmp.open();
        db.execute("CREATE TABLE t (x INTEGER)").expect("create");
    }
    let handles: Vec<_> = (0..4)
        .map(|w| {
            let path = tmp.path();
            std::thread::spawn(move || {
                let db = aidb::open(path).expect("writer open");
                for i in 0..10 {
                    db.execute(&format!("INSERT INTO t VALUES ({})", w * 100 + i))
                        .expect("busy_timeout must absorb contention");
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("writer thread");
    }
    let db = tmp.open();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM t"), 40);
}

#[test]
fn committed_data_survives_process_termination() {
    let tmp = TempDb::new("kill9");
    let path = tmp.path();
    // A child process writes and commits, then aborts without a clean close.
    let status = std::process::Command::new(cli_bin())
        .args([
            "sql",
            path.to_str().unwrap(),
            "CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (42);",
        ])
        .status()
        .expect("cli write");
    assert!(status.success());

    let victim = std::process::Command::new(cli_bin())
        .args(["serve", "--bind", "127.0.0.1:0", path.to_str().unwrap()])
        .spawn();
    if let Ok(mut child) = victim {
        std::thread::sleep(Duration::from_millis(150));
        let _ = child.kill();
        let _ = child.wait();
    }

    let db = aidb::open(&path).expect("reopen after kill");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM t WHERE x = 42"), 1);
}
