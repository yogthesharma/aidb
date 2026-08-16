//! Phase 19: spawn fake-mcp, write catalog rows, invoke through the catalog.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aidb_storage::Store;

fn temp_db() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("aidb-mcp-{nanos}.db"))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
}

#[test]
fn stdio_connect_lists_tools_and_disconnect_keeps_rows() {
    let bin = env!("CARGO_BIN_EXE_fake-mcp");
    let path = temp_db();
    let store = Store::open(&path).expect("open");
    let registered = store
        .write(|conn| aidb_tool::connect_mcp(conn, "stdio", bin))
        .expect("connect");
    assert_eq!(registered.rows[0][0].to_string(), "echo.ping");
    assert_eq!(registered.rows[0][2].to_string(), "mcp");

    let (run_id, output) = store
        .write(|conn| aidb_tool::invoke(conn, "echo.ping", r#"{"text":"hello"}"#, None))
        .expect("invoke");
    assert!(run_id.starts_with("run_"), "{run_id}");
    assert!(output.contains("\"pong\":true"), "{output}");
    assert!(output.contains("hello"), "{output}");

    let runs = store
        .query("SELECT kind, status FROM runs WHERE kind = 'tool'")
        .expect("runs");
    assert_eq!(runs.rows[0][0].to_string(), "tool");
    assert_eq!(runs.rows[0][1].to_string(), "succeeded");

    let left = store.write(aidb_tool::disconnect_mcp).expect("disconnect");
    assert_eq!(left.rows[0][0].to_string(), "echo.ping");
    assert_eq!(left.rows[0][1].to_string(), "mcp");

    let catalog = store
        .query("SELECT name, source FROM capabilities WHERE source = 'mcp'")
        .expect("kept");
    assert_eq!(catalog.rows.len(), 1);
    assert_eq!(catalog.rows[0][0].to_string(), "echo.ping");
    cleanup(&path);
}
