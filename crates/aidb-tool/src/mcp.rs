//! Live MCP stdio client. Lists tools into the capability catalog.
//! The process is not a second tool runtime.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use aidb_core::{Error, Result};
use serde_json::{json, Value};

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    tools: Vec<String>,
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static SESSION: Mutex<Option<McpClient>> = Mutex::new(None);

fn session() -> std::sync::MutexGuard<'static, Option<McpClient>> {
    SESSION.lock().unwrap_or_else(|err| err.into_inner())
}

pub fn has_tool(name: &str) -> bool {
    session()
        .as_ref()
        .is_some_and(|client| client.tools.iter().any(|tool| tool == name))
}

pub fn disconnect() {
    *session() = None;
}

pub fn connect(command: &str) -> Result<Value> {
    let (program, args) = parse_command(command)?;
    let mut child = Command::new(&program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| Error::usage(format!("mcp spawn {program}: {err}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::usage("mcp child stdin is missing"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::usage("mcp child stdout is missing"))?;
    let mut client = McpClient {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 1,
        tools: Vec::new(),
    };
    client.rpc(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "aidb", "version": "0.0.0" }
        }),
    )?;
    client.notify("notifications/initialized", json!({}))?;
    let listed = client.rpc("tools/list", json!({}))?;
    let tools = listed
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if tools.is_empty() {
        return Err(Error::usage("mcp tools/list returned no tools"));
    }
    client.tools = tools
        .iter()
        .filter_map(|tool| {
            tool.get("name")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
        })
        .collect();
    let payload = json!({ "tools": tools });
    *session() = Some(client);
    Ok(payload)
}

pub fn call(name: &str, args_json: &str) -> Result<String> {
    let args: Value =
        serde_json::from_str(args_json).unwrap_or_else(|_| json!({ "raw": args_json }));
    let mut slot = session();
    let client = slot
        .as_mut()
        .ok_or_else(|| Error::usage("mcp server is not connected"))?;
    if !client.tools.iter().any(|tool| tool == name) {
        return Err(Error::usage(format!(
            "connected mcp server does not advertise {name}"
        )));
    }
    let result = client.rpc("tools/call", json!({ "name": name, "arguments": args }))?;
    if result.get("isError").and_then(|v| v.as_bool()) == Some(true) {
        return Err(Error::usage(format!(
            "mcp tool {name} failed: {}",
            result
                .get("content")
                .map(|v| v.to_string())
                .unwrap_or_else(|| result.to_string())
        )));
    }
    Ok(mcp_output(&result, name, &args))
}

fn mcp_output(result: &Value, name: &str, args: &Value) -> String {
    let text = result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|items| {
            items.iter().find_map(|item| {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    item.get("text")
                        .and_then(|t| t.as_str())
                        .map(ToOwned::to_owned)
                } else {
                    None
                }
            })
        });
    if let Some(text) = text {
        if let Ok(json) = serde_json::from_str::<Value>(&text) {
            return json.to_string();
        }
        return json!({ "text": text, "name": name, "source": "mcp" }).to_string();
    }
    json!({ "result": result, "name": name, "args": args, "source": "mcp" }).to_string()
}

impl McpClient {
    fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.write(&req)?;
        loop {
            let msg = self.read()?;
            if msg.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(Error::usage(format!("mcp {method}: {err}")));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
    }

    fn write(&mut self, msg: &Value) -> Result<()> {
        writeln!(self.stdin, "{msg}")
            .and_then(|_| self.stdin.flush())
            .map_err(|err| Error::usage(format!("mcp write: {err}")))
    }

    fn read(&mut self) -> Result<Value> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|err| Error::usage(format!("mcp read: {err}")))?;
        if n == 0 {
            return Err(Error::usage("mcp server closed stdout"));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return self.read();
        }
        if trimmed.to_ascii_lowercase().starts_with("content-length:") {
            return self.read_framed(trimmed);
        }
        serde_json::from_str(trimmed).map_err(|err| Error::usage(format!("mcp json: {err}")))
    }

    fn read_framed(&mut self, first: &str) -> Result<Value> {
        let len: usize = first
            .split(':')
            .nth(1)
            .and_then(|v| v.trim().parse().ok())
            .ok_or_else(|| Error::usage("mcp Content-Length is invalid"))?;
        let mut blank = String::new();
        self.stdout
            .read_line(&mut blank)
            .map_err(|err| Error::usage(format!("mcp frame: {err}")))?;
        let mut buf = vec![0_u8; len];
        use std::io::Read;
        self.stdout
            .read_exact(&mut buf)
            .map_err(|err| Error::usage(format!("mcp frame body: {err}")))?;
        serde_json::from_slice(&buf).map_err(|err| Error::usage(format!("mcp json: {err}")))
    }
}

pub fn parse_command(command: &str) -> Result<(String, Vec<String>)> {
    let command = command.trim();
    if command.is_empty() {
        return Err(Error::usage("mcp command is empty"));
    }
    if command.contains("://") {
        return Err(Error::usage(
            "mcp stdio command must be a local executable (no URL)",
        ));
    }
    if command.contains(['|', ';', '&', '`', '\n', '\r']) {
        return Err(Error::usage("mcp command must not be a shell pipeline"));
    }
    let mut parts = command.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| Error::usage("mcp command is empty"))?
        .to_string();
    Ok((program, parts.map(ToOwned::to_owned).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_urls_and_pipelines() {
        assert!(parse_command("https://example.com/mcp").is_err());
        assert!(parse_command("fake-mcp | cat").is_err());
        let (prog, args) = parse_command("./fake-mcp --quiet").expect("ok");
        assert_eq!(prog, "./fake-mcp");
        assert_eq!(args, ["--quiet"]);
    }
}
