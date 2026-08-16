//! Local MCP stdio fixture. No network. Used by Phase 19 tests and demos.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if msg.get("id").is_none() {
            continue;
        }
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake-mcp", "version": "0.0.0" }
            }),
            "tools/list" => json!({
                "tools": [{
                    "name": "echo.ping",
                    "description": "Echo arguments. Local fixture, no network.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "text": { "type": "string" } }
                    },
                    "annotations": { "readOnlyHint": true }
                }]
            }),
            "tools/call" => call(msg.get("params")),
            other => {
                write_msg(
                    &mut stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("unknown method {other}") }
                    }),
                );
                continue;
            }
        };
        write_msg(
            &mut stdout,
            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        );
    }
}

fn call(params: Option<&Value>) -> Value {
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let args = params
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or(json!({}));
    if name != "echo.ping" {
        return json!({
            "isError": true,
            "content": [{ "type": "text", "text": format!("unknown tool {name}") }]
        });
    }
    let text = args
        .get("text")
        .or_else(|| args.get("goal"))
        .and_then(|v| v.as_str())
        .unwrap_or("pong");
    json!({
        "content": [{
            "type": "text",
            "text": json!({ "pong": true, "text": text, "source": "fake-mcp" }).to_string()
        }],
        "isError": false
    })
}

fn write_msg(out: &mut io::Stdout, msg: &Value) {
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}
