//! Phase 23 contracts. The server is a face: it speaks HTTP, refuses what it does
//! not implement, and never becomes a second engine or a second run store.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

static SEQ: AtomicU64 = AtomicU64::new(0);

struct TempFile {
    dir: PathBuf,
}

impl TempFile {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aidb-http-{tag}-{nanos}-{seq}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("app.db")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn serve(path: &Path, bearer: Option<&str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let path = path.to_path_buf();
    let bearer = bearer.map(str::to_string);
    thread::spawn(move || {
        let db = aidb::open(&path).expect("open");
        let _ = aidb_server::serve_listener(listener, &db, bearer.as_deref());
    });
    addr
}

fn started(tag: &str) -> (TempFile, String) {
    let tmp = TempFile::new(tag);
    drop(aidb::open(tmp.path()).expect("create"));
    let addr = serve(&tmp.path(), None);
    (tmp, addr)
}

fn post(addr: &str, body: &str) -> (u16, serde_json::Value) {
    match ureq::post(&format!("http://{addr}/sql")).send_string(body) {
        Ok(resp) => {
            let status = resp.status();
            (status, resp.into_json().expect("json body"))
        }
        Err(ureq::Error::Status(status, resp)) => (status, resp.into_json().expect("json body")),
        Err(err) => panic!("request failed: {err}"),
    }
}

fn get(addr: &str, target: &str) -> (u16, serde_json::Value) {
    match ureq::get(&format!("http://{addr}{target}")).call() {
        Ok(resp) => {
            let status = resp.status();
            (status, resp.into_json().expect("json body"))
        }
        Err(ureq::Error::Status(status, resp)) => (status, resp.into_json().expect("json body")),
        Err(err) => panic!("request failed: {err}"),
    }
}

#[test]
fn health_is_a_plain_get_and_rejects_other_methods() {
    let (_tmp, addr) = started("health");
    let (status, body) = get(&addr, "/health");
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);

    let (status, body): (u16, serde_json::Value) =
        match ureq::request("DELETE", &format!("http://{addr}/health")).call() {
            Ok(resp) => (resp.status(), resp.into_json().expect("json")),
            Err(ureq::Error::Status(status, resp)) => (status, resp.into_json().expect("json")),
            Err(err) => panic!("{err}"),
        };
    assert_eq!(status, 405);
    assert_eq!(body["ok"], false);
    assert!(body["error"]
        .as_str()
        .is_some_and(|e| e.contains("GET /health")));
}

#[test]
fn an_unknown_route_is_a_json_404() {
    let (_tmp, addr) = started("404");
    let (status, body) = get(&addr, "/admin");
    assert_eq!(status, 404);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "not found");
}

#[test]
fn sql_is_a_post_and_a_get_is_rejected() {
    let (_tmp, addr) = started("sql-method");
    let (status, body) = get(&addr, "/sql");
    assert_eq!(status, 405);
    assert!(body["error"]
        .as_str()
        .is_some_and(|e| e.contains("POST /sql")));
}

#[test]
fn invalid_sql_is_a_400_with_the_engine_error_and_no_rows() {
    let (_tmp, addr) = started("bad-sql");
    let (status, body) = post(&addr, "SELECT * FROM nope");
    assert_eq!(status, 400);
    assert_eq!(body["ok"], false);
    assert!(
        body["error"].as_str().is_some_and(|e| e.contains("nope")),
        "{body}"
    );
    assert!(
        body.get("rows").is_none(),
        "a failure returns no rows: {body}"
    );
}

#[test]
fn an_empty_or_malformed_body_is_rejected_before_the_engine() {
    let (_tmp, addr) = started("bad-body");
    for (body, needle) in [
        ("", "empty sql"),
        ("   ", "empty sql"),
        ("{\"not_sql\":1}", "must include"),
        ("{oops}", "invalid json"),
    ] {
        let (status, value) = post(&addr, body);
        assert_eq!(status, 400, "{body:?} -> {value}");
        assert!(
            value["error"]
                .as_str()
                .is_some_and(|e| e.to_ascii_lowercase().contains(needle)),
            "{body:?} -> {value}"
        );
    }
}

#[test]
fn a_raw_body_and_a_json_body_mean_the_same_thing() {
    let (_tmp, addr) = started("body-forms");
    let (_, raw) = post(&addr, "SELECT 1 AS n");
    let (_, json) = post(&addr, "{\"sql\":\"SELECT 1 AS n\"}");
    assert_eq!(raw["columns"], json["columns"]);
    assert_eq!(raw["rows"], json["rows"]);
    assert_eq!(raw["ok"], true);
}

#[test]
fn a_write_reports_how_many_rows_changed() {
    let (_tmp, addr) = started("execute");
    let (status, created) = post(
        &addr,
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)",
    );
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["ok"], true);
    let (_, inserted) = post(&addr, "INSERT INTO notes (body) VALUES ('hello')");
    assert_eq!(inserted["changed"], 1);
    let (_, rows) = post(&addr, "SELECT body FROM notes");
    assert_eq!(rows["rows"][0][0], "hello");
}

#[test]
fn a_bearer_gate_refuses_a_missing_wrong_or_malformed_token() {
    let tmp = TempFile::new("bearer");
    drop(aidb::open(tmp.path()).expect("create"));
    let addr = serve(&tmp.path(), Some("s3cret"));

    for header in [
        None,
        Some("Bearer wrong"),
        Some("s3cret"),
        Some("Basic s3cret"),
    ] {
        let mut req = ureq::post(&format!("http://{addr}/sql"));
        if let Some(value) = header {
            req = req.set("Authorization", value);
        }
        let status = match req.send_string("SELECT 1") {
            Ok(resp) => resp.status(),
            Err(ureq::Error::Status(status, _)) => status,
            Err(err) => panic!("{err}"),
        };
        assert_eq!(status, 401, "header {header:?} should not be accepted");
    }
    // Health is behind the same gate: the server exposes nothing without the token.
    let status = match ureq::get(&format!("http://{addr}/health")).call() {
        Ok(resp) => resp.status(),
        Err(ureq::Error::Status(status, _)) => status,
        Err(err) => panic!("{err}"),
    };
    assert_eq!(status, 401);

    // With the token, everything works, and the scheme is case-insensitive.
    let ok: serde_json::Value = ureq::post(&format!("http://{addr}/sql"))
        .set("Authorization", "bearer s3cret")
        .send_string("SELECT 1")
        .expect("authed")
        .into_json()
        .expect("json");
    assert_eq!(ok["ok"], true);
}

#[test]
fn the_server_uses_the_same_runs_and_documents_as_every_other_face() {
    let tmp = TempFile::new("same-engine");
    drop(aidb::open(tmp.path()).expect("create"));
    let addr = serve(&tmp.path(), None);

    let (_, inserted) = post(
        &addr,
        "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')",
    );
    let id = inserted["rows"][0][0].as_str().expect("id").to_string();
    let (_, hits) = post(&addr, "SELECT * FROM aidb_search('how do refunds work', 5)");
    assert!(
        hits["rows"].as_array().is_some_and(|r| !r.is_empty()),
        "the server indexed and searched: {hits}"
    );

    // The Rust API on the same file sees the document, its chunks, its vectors and
    // its runs. There is no second store anywhere.
    let db = aidb::open(tmp.path()).expect("reopen");
    let scalar = |sql: &str| db.query(sql).expect(sql).rows[0][0].to_string();
    assert_eq!(
        scalar(&format!("SELECT title FROM documents WHERE id = '{id}'")),
        "Refunds"
    );
    assert_ne!(
        scalar(&format!(
            "SELECT COUNT(*) FROM chunks WHERE document_id = '{id}'"
        )),
        "0"
    );
    assert_ne!(
        scalar(&format!(
            "SELECT COUNT(*) FROM vec_chunks WHERE document_id = '{id}'"
        )),
        "0"
    );
    assert_ne!(
        scalar("SELECT COUNT(*) FROM runs WHERE kind = 'index_document'"),
        "0"
    );
    assert_ne!(
        scalar("SELECT COUNT(*) FROM runs WHERE kind = 'search'"),
        "0"
    );

    // The file is the only durable artifact the server created.
    let extra: Vec<String> = std::fs::read_dir(tmp.path().parent().expect("dir"))
        .expect("read dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with("app.db"))
        .collect();
    assert!(extra.is_empty(), "unexpected files: {extra:?}");
}

#[test]
fn a_workflow_parked_over_http_can_be_approved_over_http() {
    let (_tmp, addr) = started("hitl");
    let spec = "{\"then\":[{\"approve\":{\"message\":\"Send this answer?\"}},\
        {\"generate\":{\"prompt\":\"Draft the reply\"}}]}";
    let (_, parked) = post(&addr, &format!("SELECT aidb_workflow('{spec}')"));
    assert_eq!(parked["rows"][0][1], "awaiting_approval");
    let run_id = parked["rows"][0][0].as_str().expect("run id");

    let (_, waiting) = post(
        &addr,
        "SELECT id FROM runs WHERE status = 'awaiting_approval'",
    );
    assert_eq!(waiting["rows"][0][0], run_id);

    let (status, resumed) = post(
        &addr,
        &format!("SELECT aidb_resume('{run_id}', '{{\"approved\":true}}')"),
    );
    assert_eq!(status, 200, "{resumed}");
    assert_eq!(resumed["rows"][0][1], "succeeded");
}

#[test]
fn many_clients_can_read_and_write_at_the_same_time() {
    let (_tmp, addr) = started("concurrent");
    post(
        &addr,
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)",
    );

    let readers: Vec<_> = (0..8)
        .map(|i| {
            let addr = addr.clone();
            thread::spawn(move || {
                if i % 2 == 0 {
                    let (status, body) = post(&addr, "SELECT COUNT(*) FROM notes");
                    assert_eq!(status, 200, "{body}");
                } else {
                    let (status, body) = post(
                        &addr,
                        &format!("INSERT INTO notes (body) VALUES ('row {i}')"),
                    );
                    assert_eq!(status, 200, "{body}");
                }
            })
        })
        .collect();
    for reader in readers {
        reader.join().expect("client thread");
    }

    let (_, counted) = post(&addr, "SELECT COUNT(*) FROM notes");
    assert_eq!(
        counted["rows"][0][0], 4,
        "every write committed exactly once: {counted}"
    );
}

#[test]
fn the_served_file_survives_the_server() {
    let tmp = TempFile::new("durable");
    drop(aidb::open(tmp.path()).expect("create"));
    {
        let addr = serve(&tmp.path(), None);
        post(
            &addr,
            "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days.', '{}')",
        );
    }
    // A new server over the same path sees the earlier state.
    let addr = serve(&tmp.path(), None);
    let (_, rows) = post(&addr, "SELECT title FROM documents");
    assert_eq!(rows["rows"][0][0], "Refunds");
}

#[test]
fn websocket_upgrade_is_required_on_ws() {
    let (_tmp, addr) = started("ws-method");
    let (status, body) = get(&addr, "/ws");
    assert_eq!(status, 426);
    assert_eq!(body["ok"], false);
}

#[test]
fn websocket_handshake_and_hello() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let (_tmp, addr) = started("ws-hello");
    let mut stream = TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    stream
        .write_all(
            b"GET /ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .expect("handshake write");
    let mut buf = Vec::new();
    let mut one = [0u8; 1];
    loop {
        stream.read_exact(&mut one).expect("handshake byte");
        buf.push(one[0]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 2048 {
            panic!("handshake too large: {}", String::from_utf8_lossy(&buf));
        }
    }
    let text = String::from_utf8_lossy(&buf);
    assert!(text.starts_with("HTTP/1.1 101"), "{text}");
    assert!(
        text.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "{text}"
    );
}

#[test]
fn a_protected_websocket_accepts_the_token_on_the_query_string() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    fn handshake(addr: &str, target: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let req = format!(
            "GET {target} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).expect("write");
        let mut buf = Vec::new();
        let mut one = [0u8; 1];
        loop {
            stream.read_exact(&mut one).expect("byte");
            buf.push(one[0]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            if buf.len() > 2048 {
                panic!("handshake too large");
            }
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let status: u16 = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .expect("status");
        (status, text)
    }

    let tmp = TempFile::new("ws-bearer");
    drop(aidb::open(tmp.path()).expect("create"));
    let addr = serve(&tmp.path(), Some("s3cret"));

    let (status, _) = handshake(&addr, "/ws");
    assert_eq!(status, 401, "a missing token must not upgrade");

    let (status, body) = handshake(&addr, "/ws?token=s3cret");
    assert_eq!(status, 101, "{body}");
}

#[test]
fn generate_tokens_are_readable_over_http_after_the_run() {
    let (_tmp, addr) = started("stream-events");
    let (status, body) = post(
        &addr,
        "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.')",
    );
    assert_eq!(status, 200, "{body}");
    let text = body["rows"][0][0].as_str().expect("text").to_string();
    let (_, runs) = post(
        &addr,
        "SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1",
    );
    let run_id = runs["rows"][0][0].as_str().expect("id");
    let (status, events) = get(&addr, &format!("/runs/{run_id}/events"));
    assert_eq!(status, 200, "{events}");
    assert_eq!(events["ok"], true);
    assert_eq!(events["run_id"], run_id);
    let rows = events["rows"].as_array().expect("rows");
    let tokens: Vec<String> = rows
        .iter()
        .filter(|row| row[1] == "token")
        .filter_map(|row| {
            let payload: serde_json::Value = serde_json::from_str(row[2].as_str()?).ok()?;
            payload.get("text")?.as_str().map(ToOwned::to_owned)
        })
        .collect();
    assert!(tokens.len() > 1, "{events}");
    assert_eq!(tokens.concat(), text);
}

#[test]
fn websocket_receives_token_events_for_a_generate() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let (_tmp, addr) = started("stream-ws");
    let mut stream = TcpStream::connect(&addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    stream
        .write_all(
            b"GET /ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .expect("handshake write");
    let mut buf = Vec::new();
    let mut one = [0u8; 1];
    loop {
        stream.read_exact(&mut one).expect("handshake byte");
        buf.push(one[0]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 2048 {
            panic!("handshake too large");
        }
    }
    let hello = read_ws_text(&mut stream);
    assert!(hello.contains("hello"), "{hello}");

    let (status, body) = post(
        &addr,
        "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.')",
    );
    assert_eq!(status, 200, "{body}");

    let mut saw_token = false;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("timeout");
    for _ in 0..32 {
        match read_ws_text_opt(&mut stream) {
            Some(msg) if msg.contains("\"token\"") => {
                saw_token = true;
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert!(saw_token, "live listeners must see token events");
}

fn read_ws_text(stream: &mut std::net::TcpStream) -> String {
    read_ws_text_opt(stream).expect("ws text frame")
}

fn read_ws_text_opt(stream: &mut std::net::TcpStream) -> Option<String> {
    use std::io::Read;
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).ok()?;
    let mut len = (header[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext).ok()?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext).ok()?;
        len = u64::from_be_bytes(ext) as usize;
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).ok()?;
    String::from_utf8(payload).ok()
}
