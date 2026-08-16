//! Shared memory is documents + a `memory` view. Not a second store.

use aidb_core::{QueryResult, Result, Value};

use crate::Aidb;

pub(crate) fn insert(db: &Aidb, scope: &str, content: &str) -> Result<QueryResult> {
    let metadata = aidb_sql::memory_metadata(scope);
    let id = db
        .store
        .write(|conn| aidb_index::insert_document(conn, Some(scope), content, &metadata))?;
    db.after_write()?;
    Ok(QueryResult {
        columns: vec!["id".into()],
        rows: vec![vec![Value::Text(id)]],
    })
}

pub(crate) fn search(db: &Aidb, query: &str, k: i64, scope: Option<&str>) -> Result<QueryResult> {
    let filter = match scope {
        Some(scope) => aidb_sql::memory_metadata(scope),
        None => serde_json::json!({ "kind": "memory" }).to_string(),
    };
    let plan = aidb_ir::LogicalPlan::search_filtered(query, k, Some(&filter));
    db.store
        .write(|conn| aidb_sql::execute_search(conn, db.embedder.as_ref(), &plan))
}

pub(crate) fn load_scope(db: &Aidb, scope: &str) -> Result<String> {
    let rows = db.store.query(&format!(
        "SELECT content FROM memory WHERE scope = {} ORDER BY created_at_ms",
        sql_string(scope)
    ))?;
    Ok(rows
        .rows
        .iter()
        .filter_map(|row| row.first().map(ToString::to_string))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
