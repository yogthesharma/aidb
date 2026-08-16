//! Phase 25 contracts. The file holds key *names*. A secret value must never be
//! written into `app.db`, not by the catalog, not by a policy, not by a run, and
//! not by an error message.

mod common;

use common::*;

/// Stand-in for a credential. Never a real key; only used to prove it stays out.
const CANARY: &str = "sk-canary-do-not-persist-1234567890";

fn file_bytes(tmp: &TempDb) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(tmp.dir()).expect("dir") {
        let entry = entry.expect("entry");
        if entry.file_name().to_string_lossy().starts_with("app.db") {
            all.extend(std::fs::read(entry.path()).expect("read db file"));
        }
    }
    all
}

fn dump_every_table(db: &aidb::Aidb) -> String {
    let tables = column_values(
        &db.query(
            "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')
             AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'vec_chunks%'
             AND name NOT LIKE 'chunks_fts%'",
        )
        .expect("tables"),
        "name",
    );
    let mut dumped = String::new();
    for table in tables {
        let rows = db
            .query(&format!("SELECT * FROM {table}"))
            .unwrap_or_else(|e| panic!("dump {table}: {e}"));
        for row in &rows.rows {
            for value in row {
                dumped.push_str(&value.to_string());
                dumped.push(' ');
            }
        }
    }
    dumped
}

#[test]
fn the_default_secret_source_is_the_environment() {
    let tmp = TempDb::new("secret-default");
    let db = tmp.open();
    let store = scalar(&db, "SELECT aidb_secret_store()");
    assert!(
        store == "env" || store.starts_with("file:") || store.starts_with("keychain"),
        "unexpected store: {store}"
    );
    // Whatever it is, it is not the database.
    assert!(!store.contains("app.db"), "{store}");
    let plan = scalar(&db, "EXPLAIN SELECT aidb_secret_store()");
    assert!(plan.contains("outside the db"), "{plan}");
}

#[test]
fn a_secret_store_lives_outside_the_database_file() {
    let tmp = TempDb::new("secret-store-file");
    // The optional store is an ordinary file elsewhere, holding NAME=value pairs.
    let keys = tmp.sibling("keys.env");
    std::fs::write(&keys, format!("AIDB_TEST_CANARY_KEY={CANARY}\n")).expect("write keys");

    // A child process configured with that store can still only record the name.
    let out = cli_with_env(
        &[
            "sql",
            &tmp.path().to_string_lossy(),
            "CREATE MODEL gpt PROVIDER openai KIND llm KEY_NAME 'AIDB_TEST_CANARY_KEY'",
        ],
        &[("AIDB_SECRET_STORE", &format!("file:{}", keys.display()))],
    );
    assert!(out.status.success(), "{}", stderr_of(&out));

    let reported = cli_with_env(
        &[
            "sql",
            &tmp.path().to_string_lossy(),
            "SELECT aidb_secret_store()",
        ],
        &[("AIDB_SECRET_STORE", &format!("file:{}", keys.display()))],
    );
    let reported = stdout_of(&reported);
    assert!(reported.contains("file:"), "{reported}");
    assert!(
        !reported.contains(CANARY),
        "the store URI leaked the secret"
    );

    let db = tmp.open();
    assert_eq!(
        scalar(&db, "SELECT key_name FROM models WHERE name = 'gpt'"),
        "AIDB_TEST_CANARY_KEY",
        "the catalog stores the name"
    );
    assert!(
        !String::from_utf8_lossy(&file_bytes(&tmp)).contains(CANARY),
        "the secret value reached app.db"
    );
    // Removing the store does not corrupt anything; the model is still catalogued.
    std::fs::remove_file(&keys).expect("remove store");
    let db = tmp.open();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM models WHERE name = 'gpt'"),
        1,
        "a missing store is a missing key, not a broken file"
    );
}

#[test]
fn no_table_in_the_file_will_hold_a_secret_looking_value() {
    let tmp = TempDb::new("secret-tables");
    let db = tmp.open();
    // Exercise every writer we have: catalog, policy, documents, runs, events,
    // checkpoints, capabilities, spaces, memory.
    db.query("SELECT aidb_set_policy('{\"read_only\":false,\"max_usd\":1.0}')")
        .expect("policy");
    insert_ready(
        &db,
        "Refunds",
        "Refunds are issued within 14 days of purchase.",
    );
    db.query("SELECT aidb_memory_insert('user:1', 'Prefers short answers.')")
        .expect("memory");
    db.query("SELECT aidb_create_space('legal', 'fake', 64, 'aidb-fake-legal')")
        .expect("space");
    db.query(
        "SELECT aidb_generate('Answer from the sources', content) FROM aidb_search('refunds', 3)",
    )
    .expect("generate");
    db.query("SELECT aidb_mcp_register('{\"tools\":[{\"name\":\"github.read\",\"side_effect\":\"none\"}]}')")
        .expect("capability");
    // Catalogue a hosted model last: from here on the bound LLM needs a key, which
    // is exactly the state in which a leak would matter.
    db.execute("CREATE MODEL gpt PROVIDER openai KIND llm KEY_NAME 'OPENAI_API_KEY'")
        .expect("model");
    assert_err_contains(
        db.query("SELECT aidb_generate('Summarize', 'text')"),
        "OPENAI_API_KEY is not set",
    );

    let dumped = dump_every_table(&db);
    for forbidden in ["sk-", "api_key", CANARY] {
        assert!(
            !dumped.contains(forbidden),
            "{forbidden:?} appears in the file contents"
        );
    }
    // Key names are allowed, and are what a reader should find.
    assert!(dumped.contains("OPENAI_API_KEY"), "the name is expected");
}

#[test]
fn the_catalog_refuses_a_secret_shaped_key_name_at_every_door() {
    let tmp = TempDb::new("secret-refuse");
    let db = tmp.open();
    // The dialect refuses it.
    assert_err_contains(
        db.execute(&format!(
            "CREATE MODEL gpt PROVIDER openai KIND llm KEY_NAME '{CANARY}'"
        )),
        "never the secret",
    );
    // A raw INSERT into the catalog refuses it too, so no back door.
    let raw = db.execute(&format!(
        "INSERT INTO models (name, kind, provider, provider_model, dimensions, key_name, created_at_ms)
         VALUES ('raw', 'llm', 'openai', 'gpt-4.1-mini', NULL, '{CANARY}', 0)"
    ));
    assert!(raw.is_err(), "a direct insert must not smuggle a secret in");
    assert_eq!(count(&db, "SELECT COUNT(*) FROM models"), 0);
    assert!(!String::from_utf8_lossy(&file_bytes(&tmp)).contains(CANARY));
}

#[test]
fn a_policy_cannot_carry_a_credential() {
    let tmp = TempDb::new("secret-policy");
    let db = tmp.open();
    assert_err_contains(
        db.query(&format!(
            "SELECT aidb_set_policy('{{\"api_key\":\"{CANARY}\"}}')"
        )),
        "secrets",
    );
    assert!(!String::from_utf8_lossy(&file_bytes(&tmp)).contains(CANARY));
}

#[test]
fn a_missing_key_names_the_variable_and_nothing_else() {
    let tmp = TempDb::new("secret-missing");
    let db = tmp.open();
    db.execute("CREATE MODEL gpt PROVIDER openai KIND llm KEY_NAME 'AIDB_TEST_ABSENT_KEY'")
        .expect("model");
    let err = match db.query("SELECT aidb_generate('Summarize', 'text')") {
        Ok(_) => panic!("a missing key must fail closed"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("AIDB_TEST_ABSENT_KEY is not set"), "{err}");
    assert!(!err.contains("sk-"), "{err}");

    // The failed run records the same message, which is a name, not a value.
    let recorded = scalar(
        &db,
        "SELECT error FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
    );
    assert!(recorded.contains("AIDB_TEST_ABSENT_KEY"), "{recorded}");
    // No stored row anywhere carries a credential-shaped value. (The schema text
    // itself mentions the `sk-` prefix, because a trigger rejects it, so this has
    // to look at the data rather than the raw bytes.)
    let dumped = dump_every_table(&db);
    assert!(!dumped.contains("sk-"), "{dumped}");
}

#[test]
fn a_key_name_must_look_like_a_name() {
    let tmp = TempDb::new("secret-names");
    let db = tmp.open();
    for bad in [
        "sk-live-abc",
        "OPENAI_API_KEY=sk-abc",
        "has space",
        "9starts_with_digit",
    ] {
        assert!(
            db.execute(&format!(
                "CREATE MODEL m PROVIDER openai KIND llm KEY_NAME '{bad}'"
            ))
            .is_err(),
            "{bad:?} should be rejected"
        );
    }
    for good in ["OPENAI_API_KEY", "prod-openai", "_internal.key"] {
        db.execute(&format!(
            "CREATE MODEL m PROVIDER openai KIND llm KEY_NAME '{good}'"
        ))
        .unwrap_or_else(|e| panic!("{good:?} should be accepted: {e}"));
    }
}
