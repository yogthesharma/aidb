//! Run rows for indexing and later execution kinds.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use aidb_core::{now_ms, Error, Result};
use aidb_storage::Connection;

thread_local! {
    static SESSION: RefCell<Option<String>> = const { RefCell::new(None) };
    static LAST_RUN_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn stamp_last_run_id(id: &str) {
    LAST_RUN_ID.with(|slot| *slot.borrow_mut() = Some(id.to_string()));
}

/// Last run id this thread inserted, if any. Empty until the first insert.
pub fn last_run_id() -> Option<String> {
    LAST_RUN_ID.with(|slot| slot.borrow().clone())
}

/// Drop the thread-local last run id. There is no SQL bind/clear; tests reset
/// this the same way they reset the session bind.
pub fn clear_last_run_id() {
    LAST_RUN_ID.with(|slot| *slot.borrow_mut() = None);
}

/// A session name is a user string like a memory scope (`desk:nvda`), not a minted id.
pub fn validate_session_id(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(Error::usage("session name must not be empty"));
    }
    if name.len() > 128 {
        return Err(Error::usage("session name is too long (max 128)"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(Error::usage(
            "session name must not contain control characters",
        ));
    }
    Ok(name.to_string())
}

pub fn bind_session(name: &str) -> Result<String> {
    let name = validate_session_id(name)?;
    SESSION.with(|slot| *slot.borrow_mut() = Some(name.clone()));
    Ok(name)
}

pub fn clear_session() {
    SESSION.with(|slot| *slot.borrow_mut() = None);
}

pub fn active_session() -> Option<String> {
    SESSION.with(|slot| slot.borrow().clone())
}

/// Bind `session` for the duration of `f`, then restore the previous bind.
/// `None` leaves the current bind alone.
pub fn with_session<T>(session: Option<&str>, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match session {
        Some(name) => {
            let previous = active_session();
            bind_session(name)?;
            let result = f();
            match previous {
                Some(prev) => {
                    let _ = bind_session(&prev);
                }
                None => clear_session(),
            }
            result
        }
        None => f(),
    }
}

fn resolved_session(conn: &Connection, parent_id: Option<&str>) -> Result<Option<String>> {
    if let Some(pid) = parent_id {
        match conn.query_row("SELECT session_id FROM runs WHERE id = ?1", [pid], |row| {
            row.get::<_, Option<String>>(0)
        }) {
            Ok(Some(name)) if !name.is_empty() => return Ok(Some(name)),
            Ok(_) => {}
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(err) => return Err(aidb_storage::sqlite_err(err)),
        }
    }
    Ok(active_session())
}

pub fn insert_run(
    conn: &Connection,
    id: &str,
    kind: &str,
    status: &str,
    document_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO runs (id, kind, status, document_id, created_at_ms, started_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        rusqlite::params![id, kind, status, document_id, now_ms()],
    )
    .map_err(aidb_storage::sqlite_err)?;
    stamp_last_run_id(id);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn complete_generate_run(
    conn: &Connection,
    id: &str,
    status: &str,
    output_json: Option<&str>,
    error: Option<&str>,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    cost_usd: Option<f64>,
) -> Result<()> {
    complete_with_accounting(
        conn,
        id,
        status,
        output_json,
        error,
        prompt_tokens,
        completion_tokens,
        cost_usd,
    )
}

/// Finish a run that stands for other runs — an experiment plan, say. Its spend is
/// its children's spend rolled up, so pricing it is one query against the file.
#[allow(clippy::too_many_arguments)]
pub fn complete_rollup_run(
    conn: &Connection,
    id: &str,
    status: &str,
    output_json: Option<&str>,
    error: Option<&str>,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    cost_usd: Option<f64>,
) -> Result<()> {
    complete_with_accounting(
        conn,
        id,
        status,
        output_json,
        error,
        prompt_tokens,
        completion_tokens,
        cost_usd,
    )
}

/// Roll a run's children into it: cost and tokens summed over everything parented to
/// it. Returns (prompt_tokens, completion_tokens, cost_usd).
pub fn rollup_of(conn: &Connection, parent_id: &str) -> Result<(i64, i64, f64)> {
    conn.query_row(
        "SELECT COALESCE(SUM(prompt_tokens), 0),
                COALESCE(SUM(completion_tokens), 0),
                COALESCE(SUM(cost_usd), 0.0)
           FROM runs WHERE parent_id = ?1",
        [parent_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .map_err(aidb_storage::sqlite_err)
}

#[allow(clippy::too_many_arguments)]
fn complete_with_accounting(
    conn: &Connection,
    id: &str,
    status: &str,
    output_json: Option<&str>,
    error: Option<&str>,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    cost_usd: Option<f64>,
) -> Result<()> {
    conn.execute(
        "UPDATE runs SET
            status = ?1,
            output_json = ?2,
            error = ?3,
            prompt_tokens = ?4,
            completion_tokens = ?5,
            cost_usd = ?6,
            finished_at_ms = ?7
         WHERE id = ?8",
        rusqlite::params![
            status,
            output_json,
            error,
            prompt_tokens,
            completion_tokens,
            cost_usd,
            now_ms(),
            id
        ],
    )
    .map_err(aidb_storage::sqlite_err)?;
    Ok(())
}

pub fn finish_run(conn: &Connection, id: &str, status: &str, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE runs SET status = ?1, error = ?2, finished_at_ms = ?3 WHERE id = ?4",
        rusqlite::params![status, error, now_ms(), id],
    )
    .map_err(aidb_storage::sqlite_err)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_generate_run(
    conn: &Connection,
    id: &str,
    model: Option<&str>,
    input_json: &str,
    output_json: Option<&str>,
    status: &str,
    error: Option<&str>,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    cost_usd: Option<f64>,
    parent_id: Option<&str>,
) -> Result<()> {
    let now = now_ms();
    let finished = if status == "running" { None } else { Some(now) };
    let session_id = resolved_session(conn, parent_id)?;
    conn.execute(
        "INSERT INTO runs (
            id, kind, status, parent_id, model, input_json, output_json, error,
            prompt_tokens, completion_tokens, cost_usd,
            created_at_ms, started_at_ms, finished_at_ms, session_id
         ) VALUES (?1, 'generate', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13)",
        rusqlite::params![
            id,
            status,
            parent_id,
            model,
            input_json,
            output_json,
            error,
            prompt_tokens,
            completion_tokens,
            cost_usd,
            now,
            finished,
            session_id
        ],
    )
    .map_err(aidb_storage::sqlite_err)?;
    stamp_last_run_id(id);
    Ok(())
}

pub fn put_checkpoint(
    conn: &Connection,
    run_id: &str,
    node_id: &str,
    artifact_json: Option<&str>,
) -> Result<()> {
    let seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM checkpoints WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(aidb_storage::sqlite_err)?;
    conn.execute(
        "INSERT INTO checkpoints (run_id, node_id, seq, artifact_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(run_id, node_id) DO UPDATE SET
            seq = excluded.seq,
            artifact_json = excluded.artifact_json,
            created_at_ms = excluded.created_at_ms",
        rusqlite::params![run_id, node_id, seq, artifact_json, now_ms()],
    )
    .map_err(aidb_storage::sqlite_err)?;
    Ok(())
}

pub fn has_checkpoint(conn: &Connection, run_id: &str, node_id: &str) -> Result<bool> {
    let found: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM checkpoints WHERE run_id = ?1 AND node_id = ?2",
            rusqlite::params![run_id, node_id],
            |row| row.get(0),
        )
        .map_err(aidb_storage::sqlite_err)?;
    Ok(found > 0)
}

pub fn recover_interrupted(conn: &Connection) -> Result<usize> {
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM runs
                 WHERE status = 'running'
                   AND kind IN ('generate', 'search', 'embed_query', 'tool', 'experiment')",
            )
            .map_err(aidb_storage::sqlite_err)?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(aidb_storage::sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(aidb_storage::sqlite_err)?
    };
    for id in &ids {
        finish_run(conn, id, "failed", Some("interrupted"))?;
        append_event(conn, id, "interrupted", None)?;
    }
    Ok(ids.len())
}

pub fn insert_search_run(
    conn: &Connection,
    id: &str,
    input_json: &str,
    output_json: Option<&str>,
    status: &str,
    error: Option<&str>,
    parent_id: Option<&str>,
) -> Result<()> {
    let now = now_ms();
    let finished = if status == "running" { None } else { Some(now) };
    let session_id = resolved_session(conn, parent_id)?;
    conn.execute(
        "INSERT INTO runs (
            id, kind, status, parent_id, input_json, output_json, error,
            created_at_ms, started_at_ms, finished_at_ms, session_id
         ) VALUES (?1, 'search', ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9)",
        rusqlite::params![
            id,
            status,
            parent_id,
            input_json,
            output_json,
            error,
            now,
            finished,
            session_id
        ],
    )
    .map_err(aidb_storage::sqlite_err)?;
    stamp_last_run_id(id);
    Ok(())
}

pub fn insert_workflow_run(
    conn: &Connection,
    id: &str,
    input_json: &str,
    status: &str,
) -> Result<()> {
    insert_kind_run(conn, id, "workflow", input_json, status)
}

pub fn insert_agent_run(conn: &Connection, id: &str, input_json: &str, status: &str) -> Result<()> {
    insert_agent_run_parent(conn, id, input_json, status, None)
}

pub fn insert_agent_run_parent(
    conn: &Connection,
    id: &str,
    input_json: &str,
    status: &str,
    parent_id: Option<&str>,
) -> Result<()> {
    insert_kind_run_parent(conn, id, "agent", input_json, status, parent_id)
}

pub fn insert_tool_run(
    conn: &Connection,
    id: &str,
    input_json: &str,
    output_json: Option<&str>,
    status: &str,
    error: Option<&str>,
    parent_id: Option<&str>,
) -> Result<()> {
    let now = now_ms();
    let finished = if matches!(status, "running" | "awaiting_approval" | "suspended") {
        None
    } else {
        Some(now)
    };
    let session_id = resolved_session(conn, parent_id)?;
    conn.execute(
        "INSERT INTO runs (
            id, kind, status, parent_id, input_json, output_json, error,
            created_at_ms, started_at_ms, finished_at_ms, session_id
         ) VALUES (?1, 'tool', ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9)",
        rusqlite::params![
            id,
            status,
            parent_id,
            input_json,
            output_json,
            error,
            now,
            finished,
            session_id
        ],
    )
    .map_err(aidb_storage::sqlite_err)?;
    stamp_last_run_id(id);
    Ok(())
}

pub fn insert_kind_run(
    conn: &Connection,
    id: &str,
    kind: &str,
    input_json: &str,
    status: &str,
) -> Result<()> {
    insert_kind_run_parent(conn, id, kind, input_json, status, None)
}

pub fn insert_kind_run_parent(
    conn: &Connection,
    id: &str,
    kind: &str,
    input_json: &str,
    status: &str,
    parent_id: Option<&str>,
) -> Result<()> {
    let now = now_ms();
    let session_id = resolved_session(conn, parent_id)?;
    conn.execute(
        "INSERT INTO runs (id, kind, status, parent_id, input_json, created_at_ms, started_at_ms, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
        rusqlite::params![id, kind, status, parent_id, input_json, now, session_id],
    )
    .map_err(aidb_storage::sqlite_err)?;
    stamp_last_run_id(id);
    Ok(())
}

pub fn complete_run(
    conn: &Connection,
    id: &str,
    status: &str,
    output_json: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE runs SET status = ?1, output_json = ?2, error = ?3, finished_at_ms = ?4 WHERE id = ?5",
        rusqlite::params![status, output_json, error, now_ms(), id],
    )
    .map_err(aidb_storage::sqlite_err)?;
    Ok(())
}

pub fn get_checkpoint(conn: &Connection, run_id: &str, node_id: &str) -> Result<Option<String>> {
    match conn.query_row(
        "SELECT artifact_json FROM checkpoints WHERE run_id = ?1 AND node_id = ?2",
        rusqlite::params![run_id, node_id],
        |row| row.get::<_, Option<String>>(0),
    ) {
        Ok(value) => Ok(value),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(aidb_storage::sqlite_err(err)),
    }
}

pub fn running_workflows(conn: &Connection) -> Result<Vec<(String, String)>> {
    Ok(running_durable(conn)?
        .into_iter()
        .filter(|(_, kind, _)| kind == "workflow")
        .map(|(id, _, input)| (id, input))
        .collect())
}

pub fn running_durable(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, COALESCE(input_json, '{}') FROM runs
             WHERE kind IN ('workflow', 'agent') AND status = 'running'
             ORDER BY created_at_ms",
        )
        .map_err(aidb_storage::sqlite_err)?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(aidb_storage::sqlite_err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(aidb_storage::sqlite_err)
}

pub struct RunRow {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub input_json: String,
}

pub fn get_run(conn: &Connection, id: &str) -> Result<Option<RunRow>> {
    match conn.query_row(
        "SELECT id, kind, status, COALESCE(input_json, '{}') FROM runs WHERE id = ?1",
        [id],
        |row| {
            Ok(RunRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                status: row.get(2)?,
                input_json: row.get(3)?,
            })
        },
    ) {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(aidb_storage::sqlite_err(err)),
    }
}

pub fn is_waiting_status(status: &str) -> bool {
    status == "awaiting_approval" || status == "suspended"
}

/// JSON stored on a parked run. Plain text is wrapped; an object that already
/// has `message` or `paused` is left alone so callers do not double-wrap.
pub fn parked_output_json(status: &str, raw: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(obj) = value.as_object() {
            if obj.contains_key("message") || obj.contains_key("paused") {
                return raw.to_string();
            }
        }
    }
    serde_json::json!({
        "paused": true,
        "status": status,
        "message": raw,
    })
    .to_string()
}

pub fn park_run(
    conn: &Connection,
    id: &str,
    status: &str,
    output_json: Option<&str>,
) -> Result<()> {
    let wrapped = output_json.map(|raw| parked_output_json(status, raw));
    conn.execute(
        "UPDATE runs SET status = ?1, output_json = COALESCE(?2, output_json), error = NULL, finished_at_ms = NULL
         WHERE id = ?3",
        rusqlite::params![status, wrapped, id],
    )
    .map_err(aidb_storage::sqlite_err)?;
    Ok(())
}

pub fn set_running(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE runs SET status = 'running', error = NULL, finished_at_ms = NULL,
            started_at_ms = COALESCE(started_at_ms, ?1)
         WHERE id = ?2",
        rusqlite::params![now_ms(), id],
    )
    .map_err(aidb_storage::sqlite_err)?;
    Ok(())
}

fn pause_flag(artifact: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(artifact)
        .ok()
        .and_then(|v| v.get("paused").and_then(|p| p.as_bool()))
        == Some(true)
}

pub fn has_unresolved_pause(conn: &Connection, run_id: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare("SELECT artifact_json FROM checkpoints WHERE run_id = ?1")
        .map_err(aidb_storage::sqlite_err)?;
    let rows = stmt
        .query_map([run_id], |row| row.get::<_, Option<String>>(0))
        .map_err(aidb_storage::sqlite_err)?;
    for artifact in rows {
        let artifact = artifact.map_err(aidb_storage::sqlite_err)?;
        if artifact.as_deref().is_some_and(pause_flag) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn pause_status(conn: &Connection, run_id: &str) -> Result<Option<String>> {
    let mut stmt = conn
        .prepare("SELECT artifact_json FROM checkpoints WHERE run_id = ?1")
        .map_err(aidb_storage::sqlite_err)?;
    let rows = stmt
        .query_map([run_id], |row| row.get::<_, Option<String>>(0))
        .map_err(aidb_storage::sqlite_err)?;
    for artifact in rows {
        let Some(artifact) = artifact.map_err(aidb_storage::sqlite_err)? else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&artifact) {
            if value.get("paused").and_then(|p| p.as_bool()) == Some(true) {
                return Ok(value
                    .get("status")
                    .and_then(|s| s.as_str())
                    .map(ToOwned::to_owned));
            }
        }
    }
    Ok(None)
}

pub fn resolve_pauses(conn: &Connection, run_id: &str, decision: &str) -> Result<()> {
    let parsed: serde_json::Value =
        serde_json::from_str(decision).unwrap_or_else(|_| serde_json::json!({}));
    let mut stmt = conn
        .prepare("SELECT node_id, artifact_json FROM checkpoints WHERE run_id = ?1")
        .map_err(aidb_storage::sqlite_err)?;
    let rows = stmt
        .query_map([run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(aidb_storage::sqlite_err)?;
    let mut updates = Vec::new();
    for row in rows {
        let (node_id, artifact) = row.map_err(aidb_storage::sqlite_err)?;
        let Some(artifact) = artifact else {
            continue;
        };
        if !pause_flag(&artifact) {
            continue;
        }
        let mut value = serde_json::from_str::<serde_json::Value>(&artifact)
            .unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = value.as_object_mut() {
            obj.insert("paused".into(), serde_json::json!(false));
            if let Some(approved) = parsed.get("approved") {
                obj.insert("approved".into(), approved.clone());
            }
            obj.insert("resumed".into(), serde_json::json!(true));
        }
        updates.push((node_id, value.to_string()));
    }
    drop(stmt);
    for (node_id, artifact) in updates {
        put_checkpoint(conn, run_id, &node_id, Some(&artifact))?;
    }
    Ok(())
}

pub fn append_event(
    conn: &Connection,
    run_id: &str,
    kind: &str,
    payload: Option<&str>,
) -> Result<()> {
    let seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM run_events WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(aidb_storage::sqlite_err)?;
    conn.execute(
        "INSERT INTO run_events (run_id, seq, kind, payload_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![run_id, seq, kind, payload, now_ms()],
    )
    .map_err(aidb_storage::sqlite_err)?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct TokenEvent {
    pub run_id: String,
    pub seq: i64,
    pub text: String,
}

type TokenListener = Arc<dyn Fn(&TokenEvent) + Send + Sync>;

static TOKEN_LISTENERS: Mutex<Vec<TokenListener>> = Mutex::new(Vec::new());

pub fn subscribe_tokens(listener: TokenListener) {
    if let Ok(mut slot) = TOKEN_LISTENERS.lock() {
        slot.push(listener);
    }
}

fn notify_token(event: &TokenEvent) {
    let listeners = TOKEN_LISTENERS
        .lock()
        .ok()
        .map(|slot| slot.clone())
        .unwrap_or_default();
    for listener in listeners {
        listener(event);
    }
}

/// Append a token event and notify live listeners. Concatenating `$.text` is the prefix.
pub fn append_token(conn: &Connection, run_id: &str, text: &str) -> Result<i64> {
    let payload = serde_json::json!({ "text": text }).to_string();
    let seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM run_events WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .map_err(aidb_storage::sqlite_err)?;
    conn.execute(
        "INSERT INTO run_events (run_id, seq, kind, payload_json, created_at_ms)
         VALUES (?1, ?2, 'token', ?3, ?4)",
        rusqlite::params![run_id, seq, payload, now_ms()],
    )
    .map_err(aidb_storage::sqlite_err)?;
    notify_token(&TokenEvent {
        run_id: run_id.to_string(),
        seq,
        text: text.to_string(),
    });
    Ok(seq)
}
