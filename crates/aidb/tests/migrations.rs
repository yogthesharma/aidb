//! Migration contracts: v001 → v009 is additive, idempotent, and preserves data.
//! Historical migrations are never rewritten; these tests only read them.

mod common;

use aidb::SCHEMA_VERSION;
use common::*;
use rusqlite::Connection;

const V001: &str = include_str!("../../../schema/v001.sql");
const V002: &str = include_str!("../../../schema/v002.sql");
const V003: &str = include_str!("../../../schema/v003.sql");
const V004: &str = include_str!("../../../schema/v004.sql");
const V005: &str = include_str!("../../../schema/v005.sql");
const V006: &str = include_str!("../../../schema/v006.sql");
const V007: &str = include_str!("../../../schema/v007.sql");
const V008: &str = include_str!("../../../schema/v008.sql");
const V009: &str = include_str!("../../../schema/v009.sql");

fn steps() -> Vec<(u32, &'static str)> {
    vec![
        (2, V002),
        (3, V003),
        (4, V004),
        (5, V005),
        (6, V006),
        (7, V007),
        (8, V008),
        (9, V009),
    ]
}

/// Build a database at exactly `version` by applying the historical files by hand.
fn seed_at(path: &std::path::Path, version: u32) -> Connection {
    let conn = Connection::open(path).expect("open raw");
    conn.execute_batch(V001).expect("v001");
    for (v, sql) in steps() {
        if v > version {
            break;
        }
        conn.execute_batch(sql).expect("step");
        conn.execute(
            "INSERT INTO aidb_meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [v.to_string()],
        )
        .expect("stamp");
    }
    conn
}

fn seed_payload(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO documents (id, content, content_hash, index_status, created_at_ms, updated_at_ms)
             VALUES ('doc_legacy', 'legacy refunds policy', 'h1', 'ready', 10, 10);
         INSERT INTO chunks (id, document_id, ordinal, content, created_at_ms)
             VALUES (1, 'doc_legacy', 0, 'legacy refunds policy', 10);
         INSERT INTO models (name, kind, provider, provider_model, dimensions, created_at_ms)
             VALUES ('legacy-embed', 'embedding', 'fake', 'aidb-fake', 32, 10);
         INSERT INTO runs (id, kind, status, document_id, created_at_ms)
             VALUES ('run_legacy', 'index_document', 'succeeded', 'doc_legacy', 10);
         INSERT INTO run_events (run_id, seq, kind, created_at_ms)
             VALUES ('run_legacy', 1, 'enqueued', 10);
         INSERT INTO checkpoints (run_id, node_id, seq, artifact_json, created_at_ms)
             VALUES ('run_legacy', 'chunk', 1, '{\"chunks\":1}', 10);",
    )
    .expect("seed payload");
}

#[test]
fn a_v001_database_upgrades_to_the_current_schema_and_keeps_its_data() {
    let tmp = TempDb::new("mig-v001");
    let path = tmp.path();
    {
        let conn = seed_at(&path, 1);
        seed_payload(&conn);
    }

    let db = aidb::open(&path).expect("migrate v001");
    assert_eq!(
        scalar(
            &db,
            "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
        ),
        SCHEMA_VERSION.to_string()
    );
    assert_eq!(
        scalar(&db, "SELECT content FROM documents WHERE id = 'doc_legacy'"),
        "legacy refunds policy"
    );
    assert_eq!(count(&db, "SELECT COUNT(*) FROM chunks"), 1);
    assert_eq!(
        scalar(
            &db,
            "SELECT provider FROM models WHERE name = 'legacy-embed'"
        ),
        "fake"
    );
    assert_eq!(
        scalar(&db, "SELECT status FROM runs WHERE id = 'run_legacy'"),
        "succeeded"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT kind FROM run_events WHERE run_id = 'run_legacy'"
        ),
        "enqueued"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT artifact_json FROM checkpoints WHERE run_id = 'run_legacy'"
        ),
        "{\"chunks\":1}"
    );
}

#[test]
fn every_intermediate_version_upgrades_cleanly() {
    for start in 1..=SCHEMA_VERSION {
        let tmp = TempDb::new(&format!("mig-from-{start}"));
        let path = tmp.path();
        {
            let conn = seed_at(&path, start);
            seed_payload(&conn);
        }
        let db = aidb::open(&path).unwrap_or_else(|e| panic!("migrate from v{start}: {e}"));
        assert_eq!(
            scalar(
                &db,
                "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
            ),
            SCHEMA_VERSION.to_string(),
            "upgrade from v{start}"
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM documents WHERE id = 'doc_legacy'"
            ),
            1,
            "documents preserved upgrading from v{start}"
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM runs WHERE id = 'run_legacy'"),
            1,
            "runs preserved upgrading from v{start}"
        );
    }
}

#[test]
fn v002_adds_the_human_in_the_loop_run_statuses() {
    let tmp = TempDb::new("mig-hitl");
    let path = tmp.path();
    {
        let conn = seed_at(&path, 1);
        // v001 rejects the HITL statuses.
        assert!(conn
            .execute(
                "INSERT INTO runs (id, kind, status, created_at_ms)
                 VALUES ('r', 'workflow', 'awaiting_approval', 1)",
                [],
            )
            .is_err());
    }
    let db = aidb::open(&path).expect("migrate");
    db.execute(
        "INSERT INTO runs (id, kind, status, created_at_ms)
         VALUES ('r_wait', 'workflow', 'awaiting_approval', 1),
                ('r_susp', 'workflow', 'suspended', 2)",
    )
    .expect("HITL statuses accepted after v002");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE status IN ('awaiting_approval', 'suspended')"
        ),
        2
    );
}

#[test]
fn v003_adds_the_capability_catalog_and_the_tool_run_kind() {
    let tmp = TempDb::new("mig-caps");
    let db = tmp.open();
    let caps = db
        .query("SELECT name, side_effect, retry, source, enabled FROM capabilities ORDER BY name")
        .expect("capabilities");
    let names = column_values(&caps, "name");
    assert!(names.contains(&"search".to_string()));
    assert!(names.contains(&"generate".to_string()));
    assert_eq!(cell(&caps, 0, "source"), "builtin");
    db.execute(
        "INSERT INTO runs (id, kind, status, created_at_ms) VALUES ('t1', 'tool', 'succeeded', 1)",
    )
    .expect("tool run kind accepted after v003");
    assert!(db
        .execute(
            "INSERT INTO runs (id, kind, status, created_at_ms) VALUES ('x', 'nope', 'succeeded', 1)"
        )
        .is_err());
    assert!(db
        .execute(
            "INSERT INTO capabilities (name, side_effect, retry, created_at_ms)
             VALUES ('bad', 'maybe', 'safe', 1)"
        )
        .is_err());
}

#[test]
fn v004_memory_is_a_view_over_documents_not_a_second_table() {
    let tmp = TempDb::new("mig-memory");
    let db = tmp.open();
    assert_eq!(
        scalar(&db, "SELECT type FROM sqlite_master WHERE name = 'memory'"),
        "view",
        "memory must stay a view over documents"
    );
    let id = insert_doc_meta(
        &db,
        "note",
        "Prefers concise technical explanations.",
        "{\"kind\":\"memory\",\"scope\":\"user:123\"}",
    );
    let rows = db
        .query("SELECT id, scope, content FROM memory")
        .expect("memory view");
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(cell(&rows, 0, "id"), id);
    assert_eq!(cell(&rows, 0, "scope"), "user:123");
    // Deleting the underlying document removes the memory row: one store, not two.
    db.execute(&format!("DELETE FROM documents WHERE id = '{id}'"))
        .expect("delete");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM memory"), 0);
}

#[test]
fn v006_models_carry_a_key_name_and_reject_secret_looking_values() {
    let tmp = TempDb::new("mig-keyname");
    let db = tmp.open();
    db.execute(
        "INSERT INTO models (name, kind, provider, provider_model, created_at_ms, key_name)
         VALUES ('gpt', 'llm', 'openai', 'gpt-4.1-mini', 1, 'OPENAI_API_KEY')",
    )
    .expect("key name is allowed");
    assert_eq!(
        scalar(&db, "SELECT key_name FROM models WHERE name = 'gpt'"),
        "OPENAI_API_KEY"
    );
    for secret in ["sk-abcdef", "sk-ant-123", "Bearer abc"] {
        let denied = db.execute(&format!(
            "INSERT INTO models (name, kind, provider, provider_model, created_at_ms, key_name)
             VALUES ('m_{}', 'llm', 'openai', 'x', 1, '{}')",
            secret.len(),
            secret
        ));
        assert!(
            denied.is_err(),
            "models.key_name must never accept a secret-looking value: {secret}"
        );
    }
}

#[test]
fn v008_adds_experiments_without_a_store_of_their_own() {
    let tmp = TempDb::new("mig-experiments");
    let path = tmp.path();
    {
        let conn = seed_at(&path, 7);
        seed_payload(&conn);
        // v007 has no such run kind, so an old engine could not have written one.
        assert!(conn
            .execute(
                "INSERT INTO runs (id, kind, status, created_at_ms)
                 VALUES ('e', 'experiment', 'succeeded', 1)",
                [],
            )
            .is_err());
    }

    let db = aidb::open(&path).expect("migrate");
    // The runs table was rewritten to take the new kind, and the old rows came with it.
    assert_eq!(
        scalar(&db, "SELECT status FROM runs WHERE id = 'run_legacy'"),
        "succeeded"
    );
    db.execute(
        "INSERT INTO runs (id, kind, status, created_at_ms)
         VALUES ('e1', 'experiment', 'succeeded', 1)",
    )
    .expect("experiment run kind accepted after v008");

    // Results are a view over those runs, not a table beside them.
    assert_eq!(
        scalar(
            &db,
            "SELECT type FROM sqlite_master WHERE name = 'experiment_results'"
        ),
        "view",
        "experiment_results must stay a view over runs"
    );

    // A dataset is data, and an example without gold cannot be graded.
    db.execute(
        "INSERT INTO eval_examples (dataset, question, expect_text)
         VALUES ('d', 'how long does a refund take', '14 days')",
    )
    .expect("labeled example");
    assert!(
        db.execute("INSERT INTO eval_examples (dataset, question) VALUES ('d', 'ungradeable')")
            .is_err(),
        "an example needs gold text or gold documents"
    );
}

#[test]
fn v009_adds_session_views_without_a_store_of_their_own() {
    let tmp = TempDb::new("mig-sessions");
    let path = tmp.path();
    {
        let conn = seed_at(&path, 8);
        seed_payload(&conn);
        assert!(
            conn.query_row(
                "SELECT session_id FROM runs WHERE id = 'run_legacy'",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .is_err(),
            "v008 has no session_id column"
        );
    }

    let db = aidb::open(&path).expect("migrate");
    assert_eq!(
        scalar(&db, "SELECT status FROM runs WHERE id = 'run_legacy'"),
        "succeeded"
    );
    assert_eq!(
        scalar(&db, "SELECT session_id FROM runs WHERE id = 'run_legacy'"),
        "",
        "legacy rows keep a NULL session_id"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT type FROM sqlite_master WHERE name = 'sessions'"
        ),
        "view",
        "sessions must stay a view over runs"
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT type FROM sqlite_master WHERE name = 'session_turns'"
        ),
        "view"
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'sessions'"
        ),
        0,
        "no sessions table"
    );
    assert_eq!(
        scalar(&db, "SELECT type FROM sqlite_master WHERE name = 'memory'"),
        "view",
        "memory stays documents"
    );
}

#[test]
fn migrating_twice_in_a_row_is_a_no_op() {
    let tmp = TempDb::new("mig-twice");
    let path = tmp.path();
    {
        let conn = seed_at(&path, 1);
        seed_payload(&conn);
    }
    let counts = |db: &aidb::Aidb| {
        (
            count(db, "SELECT COUNT(*) FROM documents"),
            count(db, "SELECT COUNT(*) FROM chunks"),
            count(db, "SELECT COUNT(*) FROM runs"),
            count(db, "SELECT COUNT(*) FROM run_events"),
            count(db, "SELECT COUNT(*) FROM capabilities"),
            count(db, "SELECT COUNT(*) FROM aidb_meta"),
        )
    };
    let first = {
        let db = aidb::open(&path).expect("first migrate");
        counts(&db)
    };
    let second = {
        let db = aidb::open(&path).expect("second migrate");
        counts(&db)
    };
    assert_eq!(first, second, "reopening must not duplicate rows");
}

#[test]
fn a_corrupt_schema_version_is_a_clean_error() {
    let tmp = TempDb::new("mig-corrupt");
    {
        let db = tmp.open();
        db.execute("UPDATE aidb_meta SET value = 'seven' WHERE key = 'schema_version'")
            .expect("corrupt");
    }
    assert_err_contains(aidb::open(tmp.path()), "not an integer");
}
