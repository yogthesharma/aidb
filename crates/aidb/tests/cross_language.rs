//! Section 21: one file, one engine. Rust creates the database, TypeScript writes
//! into it, Python reads and writes, the CLI reads, and Rust verifies the result.
//! Documents, vectors, runs, memory, models and capabilities all have to survive
//! the trip, because there is only ever one store behind all four faces.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

/// The native faces are built artifacts. When they are missing, say so loudly
/// rather than pretending the contract was checked.
fn faces_available() -> Result<(PathBuf, PathBuf), String> {
    let root = repo_root();
    let ts = root.join("bindings/typescript/src/index.mjs");
    let py = root.join("bindings/python");
    let addon_present = std::fs::read_dir(root.join("bindings/typescript"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().ends_with(".node"))
        })
        .unwrap_or(false);
    let module_present = std::fs::read_dir(py.join("aidb"))
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("aidb_native")
            })
        })
        .unwrap_or(false);
    if !addon_present || !module_present {
        return Err(
            "native faces are not staged; run: cargo build -p aidb-node -p aidb-python && \
             node bindings/typescript/scripts/stage-native.mjs && \
             python3 bindings/python/scripts/stage_native.py"
                .into(),
        );
    }
    if Command::new("node").arg("--version").output().is_err() {
        return Err("node is not installed".into());
    }
    if Command::new("python3").arg("--version").output().is_err() {
        return Err("python3 is not installed".into());
    }
    Ok((ts, py))
}

fn node(script: &str) -> String {
    let out = Command::new("node")
        .args(["--input-type=module", "-e", script])
        .output()
        .expect("spawn node");
    assert!(
        out.status.success(),
        "node failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn python(script: &str) -> String {
    let out = Command::new("python3")
        .args(["-c", script])
        .output()
        .expect("spawn python3");
    assert!(
        out.status.success(),
        "python failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn rust_typescript_python_and_the_cli_all_share_one_file() {
    let (ts_entry, py_root) = match faces_available() {
        Ok(paths) => paths,
        Err(reason) => {
            eprintln!("skipping cross-language test: {reason}");
            return;
        }
    };
    let tmp = TempDb::new("cross");
    let path = tmp.path();
    let path_str = path.to_string_lossy().into_owned();

    // 1. Rust creates the file and seeds every kind of state.
    let rust_doc = {
        let db = tmp.open();
        let id = insert_ready(
            &db,
            "Refunds",
            "Refunds are issued within 14 days of purchase.",
        );
        db.execute("CREATE MODEL cheap PROVIDER fake KIND llm")
            .expect("model");
        db.query(
            "SELECT aidb_mcp_register('{\"tools\":[{\"name\":\"github.read\",\
             \"side_effect\":\"none\",\"retry\":\"safe\"}]}')",
        )
        .expect("capability");
        db.query("SELECT aidb_memory_insert('user:1', 'Prefers short answers.')")
            .expect("memory");
        id
    };

    // 2. TypeScript opens the same path and writes a document and a memory.
    let ts_ids = node(&format!(
        r#"
        import {{ AI }} from {entry:?};
        const db = await AI.open({path:?});
        const doc = await db.documents.insert({{
          title: "Shipping",
          content: "Shipping takes three business days after dispatch.",
        }});
        const mem = await db.memory.insert({{ userId: "2", content: "Prefers diagrams." }});
        // TypeScript can read what Rust wrote, including the vectors.
        const hits = await db.search("how do refunds work", {{ limit: 5 }});
        if (!hits.rows.length) throw new Error("typescript saw no vectors from rust");
        const models = await db.query("SELECT COUNT(*) FROM models WHERE name = 'cheap'");
        if (Number(models.rows[0][0]) !== 1) throw new Error("typescript lost the model");
        await db.close();
        console.log(JSON.stringify({{ doc: doc.id, mem: mem.id }}));
        "#,
        entry = ts_entry.to_string_lossy(),
        path = path_str,
    ));
    let ts_ids: serde_json::Value = serde_json::from_str(&ts_ids).expect("node output json");
    let ts_doc = ts_ids["doc"].as_str().expect("ts doc id").to_string();

    // 3. Python opens the same path, reads both documents, and writes a third.
    let py_ids = python(&format!(
        r#"
import json, sys
sys.path.insert(0, {py_root:?})
from aidb import AI
db = AI.open({path:?})
titles = [r[0] for r in db.query(
    "SELECT title FROM documents WHERE COALESCE(json_extract(metadata_json, '$.kind'), '') != 'memory'"
    " ORDER BY title"
)["rows"]]
assert titles == ["Refunds", "Shipping"], titles
hits = db.search("shipping times", limit=5)
assert hits["rows"], "python saw no vectors from typescript"
caps = db.query("SELECT COUNT(*) FROM capabilities WHERE name = 'github.read'")
assert int(caps["rows"][0][0]) == 1, caps
mems = db.query("SELECT COUNT(*) FROM documents WHERE json_extract(metadata_json, '$.kind') = 'memory'")
assert int(mems["rows"][0][0]) == 2, mems
doc = db.documents.insert(title="Warranty", content="The warranty covers defects for one year.")
agent = db.agent.run(instructions="Answer from documents. End with DONE.", goal="How do refunds work?", max_steps=3)
assert agent["status"] == "succeeded", agent
db.close()
print(json.dumps({{"doc": doc["id"], "agent": agent["run_id"]}}))
        "#,
        py_root = py_root.to_string_lossy(),
        path = path_str,
    ));
    let py_ids: serde_json::Value = serde_json::from_str(&py_ids).expect("python output json");
    let py_doc = py_ids["doc"].as_str().expect("py doc id").to_string();
    let py_agent = py_ids["agent"].as_str().expect("agent run id").to_string();

    // 4. The CLI reads the same file and sees all three documents and the runs.
    let titles = cli(&[
        "sql",
        &path_str,
        "SELECT title FROM documents ORDER BY title",
    ]);
    assert!(titles.status.success(), "{}", stderr_of(&titles));
    let titles = stdout_of(&titles);
    for expected in ["Refunds", "Shipping", "Warranty"] {
        assert!(titles.contains(expected), "cli missed {expected}: {titles}");
    }
    let runs = cli(&["runs", &path_str]);
    assert!(runs.status.success(), "{}", stderr_of(&runs));
    let runs = stdout_of(&runs);
    assert!(runs.contains(&py_agent), "cli missed the agent run: {runs}");

    // 5. Rust reopens and verifies the whole shared state.
    let db = tmp.open();
    for id in [&rust_doc, &ts_doc, &py_doc] {
        assert_eq!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM documents WHERE id = '{id}'")
            ),
            1,
            "document {id} is missing"
        );
        assert_eq!(
            scalar(
                &db,
                &format!("SELECT index_status FROM documents WHERE id = '{id}'")
            ),
            "ready",
            "document {id} was never indexed"
        );
        assert!(
            count(
                &db,
                &format!("SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{id}'")
            ) > 0,
            "document {id} has no vectors"
        );
    }
    // Memory written by two different faces lives in the same view.
    assert_eq!(count(&db, "SELECT COUNT(*) FROM memory"), 2);
    // Catalogs written by Rust are still there.
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM models WHERE name = 'cheap'"),
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM capabilities WHERE name = 'github.read'"
        ),
        1
    );
    // Every face's work landed in the one run table.
    assert_eq!(
        scalar(
            &db,
            &format!("SELECT kind FROM runs WHERE id = '{py_agent}'")
        ),
        "agent"
    );
    assert!(
        count(
            &db,
            "SELECT COUNT(*) FROM runs WHERE kind = 'index_document'"
        ) >= 3,
        "each document has an index run"
    );
    // A search from Rust reaches documents inserted by the other faces.
    let hits = column_values(
        &db.query("SELECT * FROM aidb_search('warranty defects', 5)")
            .expect("search"),
        "document_id",
    );
    assert!(hits.contains(&py_doc), "{hits:?}");

    // And the only durable artifact is the one file (plus its WAL sidecars).
    let strays: Vec<String> = std::fs::read_dir(tmp.dir())
        .expect("dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with("app.db"))
        .collect();
    assert!(strays.is_empty(), "extra files appeared: {strays:?}");
}
