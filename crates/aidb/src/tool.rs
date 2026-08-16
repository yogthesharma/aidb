//! SQL entry points for the capability catalog. MCP registers rows; it is not the API.

use aidb_core::{QueryResult, Result};

use crate::Aidb;

pub(crate) fn mcp_register(db: &Aidb, json: &str) -> Result<QueryResult> {
    db.store.write(|conn| aidb_tool::register_mcp(conn, json))
}

pub(crate) fn mcp_connect(db: &Aidb, transport: &str, command: &str) -> Result<QueryResult> {
    db.store
        .write(|conn| aidb_tool::connect_mcp(conn, transport, command))
}

pub(crate) fn mcp_disconnect(db: &Aidb) -> Result<QueryResult> {
    db.store.write(aidb_tool::disconnect_mcp)
}

pub(crate) fn set_policy(db: &Aidb, json: &str, name: Option<&str>) -> Result<QueryResult> {
    db.store
        .write(|conn| aidb_tool::set_policy_sql(conn, json, name))
}

pub(crate) fn get_policy(db: &Aidb) -> Result<QueryResult> {
    db.store.write(aidb_tool::get_policy_sql)
}

pub(crate) fn invoke_sql(db: &Aidb, name: &str, args_json: &str) -> Result<QueryResult> {
    db.store.write(|conn| {
        let cap = aidb_tool::require(conn, name)?;
        let policy = aidb_tool::authorize_in(conn, &cap, None)?;
        if policy.requires_approval(&cap) {
            let id = aidb_tool::park_irreversible(conn, name, args_json, None)?;
            return Ok(aidb_tool::tool_row(
                id,
                "awaiting_approval",
                format!("approve irreversible tool {name}"),
            ));
        }
        let (id, output) = aidb_tool::invoke(conn, name, args_json, None)?;
        Ok(aidb_tool::tool_row(id, "succeeded", output))
    })
}

pub(crate) fn resume_tool(db: &Aidb, run_id: &str) -> Result<QueryResult> {
    let output = db
        .store
        .write(|conn| aidb_tool::finish_approved(conn, run_id))?;
    Ok(aidb_tool::tool_row(run_id.to_string(), "succeeded", output))
}
