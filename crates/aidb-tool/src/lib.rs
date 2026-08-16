//! Capability catalog and tool runtime. MCP is an adapter that writes rows.

mod mcp;
mod policy;

use std::cell::RefCell;

pub use policy::{
    effective as effective_policy, get_sql as get_policy_sql, set_sql as set_policy_sql, Policy,
    META_KEY as POLICY_META_KEY,
};

use aidb_core::{new_id, now_ms, Error, QueryResult, Result, Value};
use aidb_storage::{sqlite_err, Connection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub name: String,
    pub inputs: String,
    pub outputs: String,
    pub side_effect: String,
    pub retry: String,
    pub source: String,
    pub enabled: bool,
}

impl Capability {
    pub fn needs_approval(&self) -> bool {
        self.side_effect == "irreversible"
    }
}

thread_local! {
    static DENY_OVERRIDE: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

pub fn with_deny<T>(names: &[&str], f: impl FnOnce() -> Result<T>) -> Result<T> {
    DENY_OVERRIDE.with(|slot| {
        *slot.borrow_mut() = Some(names.iter().map(|s| (*s).to_string()).collect());
    });
    let result = f();
    DENY_OVERRIDE.with(|slot| *slot.borrow_mut() = None);
    result
}

pub fn deny_list() -> Vec<String> {
    Policy::from_env().deny
}

pub(crate) fn deny_override() -> Option<Vec<String>> {
    DENY_OVERRIDE.with(|slot| slot.borrow().clone())
}

pub fn get(conn: &Connection, name: &str) -> Result<Option<Capability>> {
    match conn.query_row(
        "SELECT name, inputs, outputs, side_effect, retry, source, enabled
         FROM capabilities WHERE name = ?1",
        [name],
        row_cap,
    ) {
        Ok(cap) => Ok(Some(cap)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(sqlite_err(err)),
    }
}

pub fn list(conn: &Connection) -> Result<Vec<Capability>> {
    let mut stmt = conn
        .prepare(
            "SELECT name, inputs, outputs, side_effect, retry, source, enabled
             FROM capabilities ORDER BY name",
        )
        .map_err(sqlite_err)?;
    let rows = stmt.query_map([], row_cap).map_err(sqlite_err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)
}

pub fn names(conn: &Connection) -> Result<Vec<String>> {
    Ok(list(conn)?.into_iter().map(|c| c.name).collect())
}

pub fn upsert(conn: &Connection, cap: &Capability) -> Result<()> {
    conn.execute(
        "INSERT INTO capabilities
            (name, inputs, outputs, side_effect, retry, source, enabled, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(name) DO UPDATE SET
            inputs = excluded.inputs,
            outputs = excluded.outputs,
            side_effect = excluded.side_effect,
            retry = excluded.retry,
            source = excluded.source,
            enabled = excluded.enabled",
        rusqlite::params![
            cap.name,
            cap.inputs,
            cap.outputs,
            cap.side_effect,
            cap.retry,
            cap.source,
            if cap.enabled { 1 } else { 0 },
            now_ms()
        ],
    )
    .map_err(sqlite_err)?;
    Ok(())
}

pub fn set_enabled(conn: &Connection, name: &str, enabled: bool) -> Result<()> {
    let n = conn
        .execute(
            "UPDATE capabilities SET enabled = ?1 WHERE name = ?2",
            rusqlite::params![if enabled { 1 } else { 0 }, name],
        )
        .map_err(sqlite_err)?;
    if n == 0 {
        return Err(Error::usage(format!("unknown capability: {name}")));
    }
    Ok(())
}

pub fn require(conn: &Connection, name: &str) -> Result<Capability> {
    get(conn, name)?.ok_or_else(|| Error::usage(format!("unknown capability: {name}")))
}

pub fn authorize(cap: &Capability, allow: Option<&[String]>) -> Result<()> {
    authorize_policy(cap, allow, &runtime_policy(None))
}

pub fn authorize_in(
    conn: &Connection,
    cap: &Capability,
    allow: Option<&[String]>,
) -> Result<Policy> {
    let policy = effective_policy(conn)?;
    authorize_policy(cap, allow, &policy)?;
    Ok(policy)
}

fn runtime_policy(file: Option<Policy>) -> Policy {
    let mut policy = file
        .unwrap_or_else(Policy::empty)
        .overlay(&Policy::from_env());
    if let Some(deny) = deny_override() {
        policy.deny = {
            let mut out = policy.deny;
            for name in deny {
                if !out.iter().any(|n| n == &name) {
                    out.push(name);
                }
            }
            out
        };
    }
    policy
}

pub fn authorize_policy(cap: &Capability, allow: Option<&[String]>, policy: &Policy) -> Result<()> {
    if !cap.enabled {
        return Err(Error::usage(format!(
            "capability {} is denied (disabled)",
            cap.name
        )));
    }
    if policy.deny.iter().any(|d| d == &cap.name) {
        return Err(Error::usage(format!(
            "capability {} is on the deny-list",
            cap.name
        )));
    }
    if policy.read_only && cap.side_effect != "none" {
        return Err(Error::usage(format!(
            "policy is read_only; capability {} has side effects",
            cap.name
        )));
    }
    if let Some(allow) = &policy.allow {
        if !allow.iter().any(|t| t == &cap.name) {
            return Err(Error::usage(format!(
                "capability {} is not on the allow-list",
                cap.name
            )));
        }
    }
    if let Some(allow) = allow {
        if !allow.iter().any(|t| t == &cap.name) {
            return Err(Error::usage(format!(
                "capability {} is not on the allow-list",
                cap.name
            )));
        }
    }
    Ok(())
}

pub fn parse_mcp_register(json: &str) -> Result<Vec<Capability>> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|err| Error::usage(format!("mcp register JSON: {err}")))?;
    let tools = if let Some(arr) = value.get("tools").and_then(|v| v.as_array()) {
        arr.clone()
    } else if value.get("name").and_then(|v| v.as_str()).is_some() {
        vec![value.clone()]
    } else {
        return Err(Error::usage(
            "mcp register needs {\"tools\":[...]} or a single tool object",
        ));
    };
    let mut out = Vec::new();
    for tool in tools {
        let obj = tool
            .as_object()
            .ok_or_else(|| Error::usage("mcp tool must be an object"))?;
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::usage("mcp tool.name is required"))?
            .to_string();
        if name == "search" || name == "generate" {
            return Err(Error::usage(format!(
                "cannot overwrite builtin capability: {name}"
            )));
        }
        let inputs = obj
            .get("inputs")
            .or_else(|| obj.get("inputSchema"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let outputs = obj
            .get("outputs")
            .or_else(|| obj.get("outputSchema"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let side_effect = obj
            .get("side_effect")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| mcp_side_effect(obj));
        if !matches!(side_effect.as_str(), "none" | "reversible" | "irreversible") {
            return Err(Error::usage(format!(
                "invalid side_effect for {name}: {side_effect}"
            )));
        }
        let retry = obj.get("retry").and_then(|v| v.as_str()).unwrap_or("safe");
        if !matches!(retry, "safe" | "conditional" | "forbidden") {
            return Err(Error::usage(format!("invalid retry for {name}: {retry}")));
        }
        out.push(Capability {
            name,
            inputs: inputs.to_string(),
            outputs: outputs.to_string(),
            side_effect,
            retry: retry.into(),
            source: "mcp".into(),
            enabled: obj.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        });
    }
    if out.is_empty() {
        return Err(Error::usage("mcp register listed no tools"));
    }
    Ok(out)
}

fn mcp_side_effect(obj: &serde_json::Map<String, serde_json::Value>) -> String {
    let annotations = obj.get("annotations");
    if annotations
        .and_then(|v| v.get("destructiveHint"))
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        "irreversible".into()
    } else {
        "none".into()
    }
}

pub fn connect_mcp(conn: &Connection, transport: &str, command: &str) -> Result<QueryResult> {
    if transport != "stdio" {
        return Err(Error::usage(
            "aidb_mcp_connect only supports stdio (local process)",
        ));
    }
    mcp::disconnect();
    let listed = mcp::connect(command)?;
    register_mcp(conn, &listed.to_string())
}

pub fn disconnect_mcp(conn: &Connection) -> Result<QueryResult> {
    mcp::disconnect();
    let remaining = list(conn)?
        .into_iter()
        .filter(|cap| cap.source == "mcp")
        .map(|cap| {
            vec![
                Value::Text(cap.name),
                Value::Text(cap.source),
                Value::Integer(i64::from(cap.enabled)),
            ]
        })
        .collect();
    Ok(QueryResult {
        columns: vec!["name".into(), "source".into(), "enabled".into()],
        rows: remaining,
    })
}

pub fn register_mcp(conn: &Connection, json: &str) -> Result<QueryResult> {
    let caps = parse_mcp_register(json)?;
    let mut rows = Vec::new();
    for cap in caps {
        upsert(conn, &cap)?;
        rows.push(vec![
            Value::Text(cap.name),
            Value::Text(cap.side_effect),
            Value::Text(cap.source),
        ]);
    }
    Ok(QueryResult {
        columns: vec!["name".into(), "side_effect".into(), "source".into()],
        rows,
    })
}

pub fn execute_handler(name: &str, args_json: &str) -> Result<String> {
    if mcp::has_tool(name) {
        return mcp::call(name, args_json);
    }
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or_else(|_| serde_json::json!({ "raw": args_json }));
    match name {
        "search" | "generate" => Err(Error::usage(format!(
            "{name} is a builtin operator, not a catalog tool invoke"
        ))),
        "http.get" => http_get_stub(&args),
        "github.read" => {
            let path = args
                .get("path")
                .or_else(|| args.get("query"))
                .and_then(|v| v.as_str())
                .unwrap_or("README.md");
            Ok(serde_json::json!({
                "source": "mcp",
                "path": path,
                "content": format!("Repository stub for {path}. No network.")
            })
            .to_string())
        }
        "send.email" => {
            let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("");
            let subject = args.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            Ok(serde_json::json!({
                "queued": true,
                "sent": false,
                "to": to,
                "subject": subject
            })
            .to_string())
        }
        _ => Ok(serde_json::json!({
            "ok": true,
            "name": name,
            "args": args
        })
        .to_string()),
    }
}

fn http_get_stub(args: &serde_json::Value) -> Result<String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::usage("http.get requires args.url"))?;
    if !url.starts_with("aidb://") {
        return Err(Error::usage(
            "http.get only accepts aidb:// URLs in-process (no network)",
        ));
    }
    Ok(serde_json::json!({
        "url": url,
        "status": 200,
        "body": format!("stub GET {url}")
    })
    .to_string())
}

pub fn invoke(
    conn: &Connection,
    name: &str,
    args_json: &str,
    parent_id: Option<&str>,
) -> Result<(String, String)> {
    invoke_inner(conn, name, args_json, parent_id, false)
}

pub fn invoke_approved(
    conn: &Connection,
    name: &str,
    args_json: &str,
    parent_id: Option<&str>,
) -> Result<(String, String)> {
    invoke_inner(conn, name, args_json, parent_id, true)
}

fn invoke_inner(
    conn: &Connection,
    name: &str,
    args_json: &str,
    parent_id: Option<&str>,
    approved: bool,
) -> Result<(String, String)> {
    let cap = require(conn, name)?;
    let policy = authorize_in(conn, &cap, None)?;
    if policy.requires_approval(&cap) && !approved {
        return Err(Error::usage(format!(
            "capability {name} is irreversible and needs approval"
        )));
    }
    let input = tool_input(name, args_json);
    let id = new_id("run");
    aidb_run::insert_tool_run(conn, &id, &input, None, "running", None, parent_id)?;
    aidb_run::append_event(conn, &id, "policy", Some(&policy.to_json()))?;
    match execute_handler(name, args_json) {
        Ok(output) => {
            aidb_run::complete_run(conn, &id, "succeeded", Some(&output), None)?;
            aidb_run::append_event(conn, &id, "succeeded", None)?;
            Ok((id, output))
        }
        Err(err) => {
            let message = err.to_string();
            let _ = aidb_run::complete_run(conn, &id, "failed", None, Some(&message));
            let _ = aidb_run::append_event(conn, &id, "failed", Some(&message));
            Err(err)
        }
    }
}

pub fn park_irreversible(
    conn: &Connection,
    name: &str,
    args_json: &str,
    parent_id: Option<&str>,
) -> Result<String> {
    let cap = require(conn, name)?;
    let policy = authorize_in(conn, &cap, None)?;
    if !policy.requires_approval(&cap) {
        return Err(Error::usage(format!(
            "capability {name} does not require approval"
        )));
    }
    let input = tool_input(name, args_json);
    let id = new_id("run");
    let message = format!("approve irreversible tool {name}");
    let output = aidb_run::parked_output_json("awaiting_approval", &message);
    aidb_run::insert_tool_run(
        conn,
        &id,
        &input,
        Some(&output),
        "awaiting_approval",
        None,
        parent_id,
    )?;
    aidb_run::append_event(conn, &id, "policy", Some(&policy.to_json()))?;
    aidb_run::append_event(conn, &id, "awaiting_approval", Some(&message))?;
    Ok(id)
}

pub fn finish_approved(conn: &Connection, run_id: &str) -> Result<String> {
    let row = aidb_run::get_run(conn, run_id)?
        .ok_or_else(|| Error::usage(format!("unknown run: {run_id}")))?;
    if row.kind != "tool" {
        return Err(Error::usage(format!("run {run_id} is not a tool run")));
    }
    let (name, args) = parse_tool_input(&row.input_json)?;
    let cap = require(conn, &name)?;
    authorize_in(conn, &cap, None)?;
    aidb_run::set_running(conn, run_id)?;
    match execute_handler(&name, &args) {
        Ok(output) => {
            aidb_run::complete_run(conn, run_id, "succeeded", Some(&output), None)?;
            aidb_run::append_event(conn, run_id, "succeeded", None)?;
            Ok(output)
        }
        Err(err) => {
            let message = err.to_string();
            let _ = aidb_run::complete_run(conn, run_id, "failed", None, Some(&message));
            let _ = aidb_run::append_event(conn, run_id, "failed", Some(&message));
            Err(err)
        }
    }
}

pub fn tool_row(run_id: String, status: &str, output: String) -> QueryResult {
    QueryResult {
        columns: vec!["run_id".into(), "status".into(), "output".into()],
        rows: vec![vec![
            Value::Text(run_id),
            Value::Text(status.into()),
            Value::Text(output),
        ]],
    }
}

pub fn tool_input(name: &str, args_json: &str) -> String {
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or_else(|_| serde_json::json!({ "raw": args_json }));
    serde_json::json!({ "name": name, "args": args }).to_string()
}

pub fn parse_tool_input(input_json: &str) -> Result<(String, String)> {
    let value: serde_json::Value = serde_json::from_str(input_json)
        .map_err(|err| Error::usage(format!("tool input_json: {err}")))?;
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::usage("tool input_json.name is required"))?
        .to_string();
    let args = value.get("args").cloned().unwrap_or(serde_json::json!({}));
    Ok((name, args.to_string()))
}

fn row_cap(row: &rusqlite::Row<'_>) -> rusqlite::Result<Capability> {
    Ok(Capability {
        name: row.get(0)?,
        inputs: row.get(1)?,
        outputs: row.get(2)?,
        side_effect: row.get(3)?,
        retry: row.get(4)?,
        source: row.get(5)?,
        enabled: row.get::<_, i64>(6)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mcp_tool_list() {
        let caps = parse_mcp_register(
            r#"{"tools":[{"name":"github.read","inputs":{"path":"string"},"side_effect":"none"}]}"#,
        )
        .expect("parse");
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].name, "github.read");
        assert_eq!(caps[0].source, "mcp");
        assert_eq!(caps[0].side_effect, "none");
    }

    #[test]
    fn parses_mcp_tools_list_schema_and_annotations() {
        let caps = parse_mcp_register(
            r#"{"tools":[{"name":"echo.ping","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}}]}"#,
        )
        .expect("parse");
        assert_eq!(caps[0].name, "echo.ping");
        assert_eq!(caps[0].side_effect, "none");
        assert!(caps[0].inputs.contains("object"), "{}", caps[0].inputs);

        let danger = parse_mcp_register(
            r#"{"tools":[{"name":"boom","annotations":{"destructiveHint":true}}]}"#,
        )
        .expect("danger");
        assert_eq!(danger[0].side_effect, "irreversible");
    }

    #[test]
    fn github_read_does_not_use_the_network() {
        let out = execute_handler("github.read", r#"{"path":"README.md"}"#).expect("handler");
        assert!(out.contains("README.md"), "{out}");
        assert!(out.contains("No network"), "{out}");
    }

    #[test]
    fn send_email_is_queued_not_sent() {
        let out =
            execute_handler("send.email", r#"{"to":"a@b.c","subject":"hi"}"#).expect("handler");
        assert!(out.contains("\"sent\":false"), "{out}");
        assert!(out.contains("\"queued\":true"), "{out}");
    }
}
