//! HTTP in front of the same `Aidb` and the same file.
//! Optional. Not a control plane. Not a second run store.

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use aidb::{query_to_json, subscribe_tokens, Aidb, Error, Result, SqlOutput, TokenEvent};

mod ws;

use ws::{is_websocket_upgrade, serve_socket, write_handshake, Hub};

pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Optional bearer from the environment. No users table in the file.
pub fn bearer_from_env() -> Option<String> {
    for key in ["AIDB_BEARER", "AIDB_TOKEN"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn bind_from_env() -> String {
    if let Ok(bind) = std::env::var("AIDB_SERVE_BIND") {
        let trimmed = bind.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(port) = std::env::var("AIDB_SERVE_PORT") {
        let trimmed = port.trim();
        if !trimmed.is_empty() {
            return format!("127.0.0.1:{trimmed}");
        }
    }
    DEFAULT_BIND.to_string()
}

/// Open `path` and accept HTTP on `bind`. Blocks. One writer still (the same mutex).
pub fn serve(path: impl AsRef<Path>, bind: &str) -> Result<()> {
    serve_with_bearer(path, bind, bearer_from_env())
}

pub fn serve_with_bearer(path: impl AsRef<Path>, bind: &str, bearer: Option<String>) -> Result<()> {
    let path = path.as_ref();
    let db = aidb::open(path)?;
    let listener = TcpListener::bind(bind).map_err(|source| Error::Io { path: None, source })?;
    let addr = listener
        .local_addr()
        .map_err(|source| Error::Io { path: None, source })?;
    eprintln!("aidb: serving {} on http://{}", path.display(), addr);
    serve_listener(listener, &db, bearer.as_deref())
}

pub fn serve_listener(listener: TcpListener, db: &Aidb, bearer: Option<&str>) -> Result<()> {
    let hub = Hub::new();
    let stop = AtomicBool::new(false);
    {
        let hub = hub.clone();
        subscribe_tokens(Arc::new(move |event: &TokenEvent| {
            let payload = serde_json::json!({
                "type": "token",
                "run_id": event.run_id,
                "seq": event.seq,
                "text": event.text,
            });
            hub.publish(&payload.to_string());
        }));
    }
    thread::scope(|scope| {
        scope.spawn(|| watch_catalog(db, &hub, &stop));
        for incoming in listener.incoming() {
            match incoming {
                Ok(stream) => {
                    let hub = hub.clone();
                    scope.spawn(move || handle_connection(db, stream, bearer, &hub));
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(source) => {
                    stop.store(true, Ordering::Relaxed);
                    return Err(Error::Io { path: None, source });
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        Ok(())
    })
}

fn handle_connection(db: &Aidb, mut stream: TcpStream, bearer: Option<&str>, hub: &Hub) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(&mut stream);
    let req = match read_request(&mut reader) {
        Ok(req) => req,
        Err(resp) => {
            drop(reader);
            let _ = write_response(&mut stream, &resp);
            return;
        }
    };
    drop(reader);

    if is_websocket_upgrade(&req.method, path_only(&req.target), &req.headers) {
        if let Some(expected) = bearer {
            match request_bearer(&req.headers).or_else(|| query_token(&req.target)) {
                Some(got) if got == expected => {}
                _ => {
                    let resp = Response::error(401, "Unauthorized", "bearer required");
                    let _ = write_response(&mut stream, &resp);
                    return;
                }
            }
        }
        let Some(key) = header(&req.headers, "sec-websocket-key") else {
            let resp = Response::error(400, "Bad Request", "missing Sec-WebSocket-Key");
            let _ = write_response(&mut stream, &resp);
            return;
        };
        if write_handshake(&mut stream, key).is_err() {
            return;
        }
        serve_socket(stream, hub);
        return;
    }

    let resp = dispatch(db, &req, bearer, hub);
    let _ = write_response(&mut stream, &resp);
}

struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    reason: &'static str,
    body: String,
}

impl Response {
    fn json(status: u16, reason: &'static str, value: serde_json::Value) -> Self {
        Self {
            status,
            reason,
            body: value.to_string(),
        }
    }

    fn ok(value: serde_json::Value) -> Self {
        Self::json(200, "OK", value)
    }

    fn error(status: u16, reason: &'static str, message: impl std::fmt::Display) -> Self {
        Self::json(
            status,
            reason,
            serde_json::json!({ "ok": false, "error": message.to_string() }),
        )
    }
}

fn read_request(reader: &mut BufReader<&mut TcpStream>) -> std::result::Result<Request, Response> {
    let mut line = String::new();
    read_line_limited(reader, &mut line, MAX_HEADER_BYTES)?;
    let request_line = line.trim_end_matches(['\r', '\n']);
    let mut parts = request_line.splitn(3, ' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let version = parts.next().unwrap_or("");
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
        return Err(Response::error(
            400,
            "Bad Request",
            "malformed request line",
        ));
    }

    let mut headers = Vec::new();
    let mut header_bytes = request_line.len();
    loop {
        line.clear();
        read_line_limited(reader, &mut line, MAX_HEADER_BYTES - header_bytes)?;
        header_bytes += line.len();
        if header_bytes > MAX_HEADER_BYTES {
            return Err(Response::error(
                431,
                "Request Header Fields Too Large",
                "headers too large",
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(Response::error(400, "Bad Request", "malformed header"));
        };
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }

    let content_length = header_u64(&headers, "content-length").unwrap_or(0);
    if content_length > MAX_BODY_BYTES as u64 {
        return Err(Response::error(413, "Payload Too Large", "body too large"));
    }
    let mut body = vec![0u8; content_length as usize];
    reader
        .read_exact(&mut body)
        .map_err(|_| Response::error(400, "Bad Request", "truncated body"))?;

    Ok(Request {
        method,
        target,
        headers,
        body,
    })
}

fn read_line_limited(
    reader: &mut BufReader<&mut TcpStream>,
    buf: &mut String,
    remaining: usize,
) -> std::result::Result<usize, Response> {
    if remaining == 0 {
        return Err(Response::error(
            431,
            "Request Header Fields Too Large",
            "headers too large",
        ));
    }
    let mut bytes = Vec::new();
    let mut one = [0u8; 1];
    loop {
        if bytes.len() >= remaining {
            return Err(Response::error(
                431,
                "Request Header Fields Too Large",
                "headers too large",
            ));
        }
        let n = reader
            .read(&mut one)
            .map_err(|_| Response::error(400, "Bad Request", "failed to read request"))?;
        if n == 0 {
            if bytes.is_empty() {
                return Err(Response::error(400, "Bad Request", "empty request"));
            }
            break;
        }
        bytes.push(one[0]);
        if one[0] == b'\n' {
            break;
        }
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Response::error(400, "Bad Request", "headers must be utf-8"))?;
    buf.push_str(text);
    Ok(bytes.len())
}

fn header_u64(headers: &[(String, String)], name: &str) -> Option<u64> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.parse().ok())
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn path_only(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

fn dispatch(db: &Aidb, req: &Request, required_bearer: Option<&str>, hub: &Hub) -> Response {
    if let Some(expected) = required_bearer {
        match request_bearer(&req.headers) {
            Some(got) if got == expected => {}
            _ => {
                return Response::error(401, "Unauthorized", "bearer required");
            }
        }
    }

    let path = path_only(&req.target);
    if let Some(id) = run_events_id(path) {
        return match req.method.as_str() {
            "GET" => handle_run_events(db, id),
            _ => Response::error(405, "Method Not Allowed", "GET /runs/{id}/events"),
        };
    }
    match (req.method.as_str(), path) {
        ("GET" | "HEAD", "/health") => Response::ok(serde_json::json!({ "ok": true })),
        ("GET", "/ws") => Response::error(426, "Upgrade Required", "WebSocket upgrade required"),
        ("POST", "/sql") => handle_sql(db, &req.body, hub),
        (_, "/health") => Response::error(405, "Method Not Allowed", "GET /health"),
        (_, "/sql") => Response::error(405, "Method Not Allowed", "POST /sql"),
        (_, "/ws") => Response::error(405, "Method Not Allowed", "GET /ws"),
        _ => Response::error(404, "Not Found", "not found"),
    }
}

fn is_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn run_events_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/runs/")?;
    let (id, tail) = rest.split_once('/')?;
    (tail == "events" && is_run_id(id)).then_some(id)
}

fn sql_escape(text: &str) -> String {
    text.replace('\'', "''")
}

fn handle_run_events(db: &Aidb, run_id: &str) -> Response {
    let sql = format!(
        "SELECT seq, kind, payload_json, created_at_ms FROM run_events WHERE run_id = '{}' ORDER BY seq",
        sql_escape(run_id)
    );
    match db.query(&sql) {
        Ok(result) => {
            let mut value = query_to_json(&result);
            if let serde_json::Value::Object(map) = &mut value {
                map.insert("ok".into(), serde_json::json!(true));
                map.insert("run_id".into(), serde_json::json!(run_id));
            }
            Response::ok(value)
        }
        Err(err) => Response::error(400, "Bad Request", err),
    }
}

fn request_bearer(headers: &[(String, String)]) -> Option<&str> {
    let value = header(headers, "authorization")?;
    let value = value.trim();
    const PREFIX: &str = "Bearer ";
    if value.len() >= PREFIX.len() && value[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        Some(value[PREFIX.len()..].trim())
    } else {
        None
    }
}

fn handle_sql(db: &Aidb, body: &[u8], hub: &Hub) -> Response {
    let sql = match sql_from_body(body) {
        Ok(sql) => sql,
        Err(message) => return Response::error(400, "Bad Request", message),
    };
    if sql.trim().is_empty() {
        return Response::error(400, "Bad Request", "empty sql");
    }
    let notify = sql_touches_file(&sql);
    match db.sql(&sql) {
        Ok(SqlOutput::Query(result)) => {
            if notify {
                hub.publish(r#"{"type":"change","source":"sql"}"#);
            }
            let mut value = query_to_json(&result);
            if let serde_json::Value::Object(map) = &mut value {
                map.insert("ok".into(), serde_json::json!(true));
            }
            Response::ok(value)
        }
        Ok(SqlOutput::Execute(changed)) => {
            hub.publish(r#"{"type":"change","source":"sql"}"#);
            Response::ok(serde_json::json!({ "ok": true, "changed": changed }))
        }
        Err(err) => Response::error(400, "Bad Request", err),
    }
}

fn sql_touches_file(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    if aidb_function_call(&lower) {
        return true;
    }
    let first = lower.trim_start();
    first.starts_with("insert")
        || first.starts_with("update")
        || first.starts_with("delete")
        || first.starts_with("create")
        || first.starts_with("drop")
        || first.starts_with("alter")
        || first.starts_with("replace")
}

/// True for `aidb_generate(` / `aidb_search(` — not for reading table `aidb_meta`.
fn aidb_function_call(lower: &str) -> bool {
    let mut rest = lower;
    while let Some(idx) = rest.find("aidb_") {
        rest = &rest[idx + 5..];
        let ident_end = rest
            .find(|c: char| !c.is_ascii_lowercase() && c != '_')
            .unwrap_or(rest.len());
        if rest[ident_end..].trim_start().starts_with('(') {
            return true;
        }
        rest = &rest[ident_end..];
    }
    false
}

fn query_token(target: &str) -> Option<&str> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token" || key == "bearer").then_some(value)
    })
}

fn catalog_stamp(db: &Aidb) -> Option<String> {
    let result = db
        .query(
            "SELECT (SELECT COUNT(*) FROM documents), \
             (SELECT COALESCE(MAX(updated_at_ms),0) FROM documents), \
             (SELECT COUNT(*) FROM runs), \
             (SELECT COALESCE(MAX(created_at_ms),0) FROM runs), \
             (SELECT COUNT(*) FROM models), \
             (SELECT COUNT(*) FROM runs WHERE status = 'awaiting_approval')",
        )
        .ok()?;
    let row = result.rows.first()?;
    Some(format!("{row:?}"))
}

fn watch_catalog(db: &Aidb, hub: &Hub, stop: &AtomicBool) {
    let mut last = catalog_stamp(db);
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(750));
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let now = catalog_stamp(db);
        if now != last {
            last = now;
            hub.publish(r#"{"type":"change","source":"file"}"#);
        }
    }
}

fn sql_from_body(body: &[u8]) -> std::result::Result<String, String> {
    let text = std::str::from_utf8(body).map_err(|_| "sql must be utf-8".to_string())?;
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        let value: serde_json::Value =
            serde_json::from_str(trimmed).map_err(|err| format!("invalid json: {err}"))?;
        value
            .get("sql")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "json body must include \"sql\"".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn write_response(stream: &mut TcpStream, resp: &Response) -> std::io::Result<()> {
    let bytes = resp.body.as_bytes();
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.status,
        resp.reason,
        bytes.len()
    )?;
    stream.write_all(bytes)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_db() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("aidb-phase23-{nanos}-{seq}.db"))
    }

    fn spawn_file() -> std::path::PathBuf {
        let path = temp_db();
        drop(aidb::open(&path).expect("create"));
        path
    }

    fn spawn_server(path: &Path, bearer: Option<&str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let path = path.to_path_buf();
        let bearer = bearer.map(str::to_string);
        thread::spawn(move || {
            let db = aidb::open(&path).expect("open");
            let _ = serve_listener(listener, &db, bearer.as_deref());
        });
        format!("{addr}")
    }

    #[test]
    fn health_and_schema_version_over_http() {
        let path = spawn_file();
        let addr = spawn_server(&path, None);

        let health: serde_json::Value = ureq::get(&format!("http://{addr}/health"))
            .call()
            .expect("health")
            .into_json()
            .expect("json");
        assert_eq!(health["ok"], true);

        let body: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
            .send_string("SELECT value FROM aidb_meta WHERE key = 'schema_version'")
            .expect("sql")
            .into_json()
            .expect("json");
        assert_eq!(body["ok"], true);
        assert_eq!(body["rows"][0][0], aidb::SCHEMA_VERSION.to_string());
    }

    #[test]
    fn http_writes_the_same_file() {
        let path = spawn_file();
        let addr = spawn_server(&path, None);

        let insert: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
            .send_string("SELECT aidb_insert_document('from-http', 'hello from serve', '{}');")
            .expect("insert")
            .into_json()
            .expect("json");
        assert_eq!(insert["ok"], true);
        let id = insert["rows"][0][0].as_str().expect("id");

        let db = aidb::open(&path).expect("reopen");
        let rows = db
            .query(&format!("SELECT title FROM documents WHERE id = '{id}'"))
            .expect("select");
        assert_eq!(rows.rows[0][0].to_string(), "from-http");
    }

    #[test]
    fn json_sql_body_and_execute() {
        let path = spawn_file();
        let addr = spawn_server(&path, None);

        let body: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
            .set("Content-Type", "application/json")
            .send_string(r#"{"sql":"SELECT 1 AS n"}"#)
            .expect("sql")
            .into_json()
            .expect("json");
        assert_eq!(body["ok"], true);
        assert_eq!(body["columns"][0], "n");
        assert_eq!(body["rows"][0][0], 1);
    }

    #[test]
    fn bearer_from_env_style_gate() {
        let path = spawn_file();
        let addr = spawn_server(&path, Some("s3cret"));

        let denied = ureq::post(&format!("http://{addr}/sql"))
            .send_string("SELECT 1")
            .unwrap_err();
        let denied = denied.into_response().expect("401 body");
        assert_eq!(denied.status(), 401);

        let ok: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
            .set("Authorization", "Bearer s3cret")
            .send_string("SELECT 1")
            .expect("authed")
            .into_json()
            .expect("json");
        assert_eq!(ok["ok"], true);
    }

    #[test]
    fn same_runs_table() {
        let path = spawn_file();
        let addr = spawn_server(&path, None);

        let gen: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
            .send_string("SELECT aidb_generate('one word: hi', 'say hi');")
            .expect("generate")
            .into_json()
            .expect("json");
        assert_eq!(gen["ok"], true);

        let runs: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
            .send_string("SELECT count(*) FROM runs WHERE kind = 'generate'")
            .expect("runs")
            .into_json()
            .expect("json");
        assert_eq!(runs["rows"][0][0], 1);
    }

    #[test]
    fn reading_aidb_meta_is_not_a_file_write() {
        assert!(!sql_touches_file(
            "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
        ));
        assert!(!sql_touches_file(
            "SELECT COUNT(*) FROM documents WHERE index_status = 'ready'"
        ));
        assert!(!sql_touches_file(
            "SELECT id, kind, status FROM runs ORDER BY created_at_ms DESC LIMIT 10"
        ));
        assert!(sql_touches_file(
            "SELECT aidb_generate('Summarize this', content) FROM aidb_search('refunds', 4)"
        ));
        assert!(sql_touches_file(
            "SELECT aidb_insert_document('Refunds', '14 days', '{}')"
        ));
        assert!(sql_touches_file(
            "INSERT INTO tickets (subject, body, created_at_ms) VALUES ('x', 'y', 1)"
        ));
    }
}
