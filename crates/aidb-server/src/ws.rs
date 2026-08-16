//! Local WebSocket face for Studio. Same process as POST /sql. Not a second engine.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha1::{Digest, Sha1};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_FRAME: usize = 64 * 1024;

#[derive(Clone)]
pub struct Hub {
    clients: Arc<Mutex<Vec<Sender<String>>>>,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn subscribe(&self) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        self.clients.lock().expect("hub").push(tx);
        rx
    }

    pub fn publish(&self, payload: &str) {
        let mut clients = self.clients.lock().expect("hub");
        clients.retain(|tx| tx.send(payload.to_string()).is_ok());
    }
}

pub fn is_websocket_upgrade(method: &str, path: &str, headers: &[(String, String)]) -> bool {
    if method != "GET" || path != "/ws" {
        return false;
    }
    let upgrade = header(headers, "upgrade").unwrap_or("");
    let connection = header(headers, "connection").unwrap_or("");
    upgrade.eq_ignore_ascii_case("websocket")
        && connection.to_ascii_lowercase().contains("upgrade")
        && header(headers, "sec-websocket-key").is_some()
}

pub fn accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    STANDARD.encode(hasher.finalize())
}

pub fn write_handshake(stream: &mut TcpStream, key: &str) -> std::io::Result<()> {
    let accept = accept_key(key);
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    )?;
    stream.flush()
}

pub fn serve_socket(mut stream: TcpStream, hub: &Hub) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let rx = hub.subscribe();
    let hello = r#"{"type":"hello","ok":true}"#;
    if write_text(&mut stream, hello).is_err() {
        return;
    }
    loop {
        while let Ok(msg) = rx.try_recv() {
            if write_text(&mut stream, &msg).is_err() {
                return;
            }
        }
        match read_frame(&mut stream) {
            Ok(Frame::Text | Frame::Binary) => {}
            Ok(Frame::Ping(payload)) => {
                if write_frame(&mut stream, 0xA, &payload).is_err() {
                    return;
                }
            }
            Ok(Frame::Pong) => {}
            Ok(Frame::Close) => return,
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return,
        }
    }
}

enum Frame {
    Text,
    Binary,
    Ping(Vec<u8>),
    Pong,
    Close,
}

fn read_frame(stream: &mut TcpStream) -> std::io::Result<Frame> {
    let mut header = [0u8; 2];
    read_exact_timeout(stream, &mut header)?;
    let opcode = header[0] & 0x0f;
    let masked = header[1] & 0x80 != 0;
    let mut len = (header[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        read_exact_timeout(stream, &mut ext)?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        read_exact_timeout(stream, &mut ext)?;
        len = u64::from_be_bytes(ext) as usize;
    }
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut mask = [0u8; 4];
    if masked {
        read_exact_timeout(stream, &mut mask)?;
    }
    let mut payload = vec![0u8; len];
    read_exact_timeout(stream, &mut payload)?;
    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }
    match opcode {
        0x1 => Ok(Frame::Text),
        0x2 => Ok(Frame::Binary),
        0x8 => Ok(Frame::Close),
        0x9 => Ok(Frame::Ping(payload)),
        0xA => Ok(Frame::Pong),
        _ => Ok(Frame::Close),
    }
}

fn read_exact_timeout(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match stream.read(&mut buf[filled..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof",
                ));
            }
            Ok(n) => filled += n,
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                if filled == 0 {
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn write_text(stream: &mut TcpStream, text: &str) -> std::io::Result<()> {
    write_frame(stream, 0x1, text.as_bytes())
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        header.push(len as u8);
    } else if len <= u16::MAX as usize {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6455_accept_key() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }
}
