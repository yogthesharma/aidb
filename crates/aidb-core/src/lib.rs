//! Shared types for AIDB. Keep this crate free of SQLite and network I/O.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub const SCHEMA_VERSION: u32 = 9;

/// Physical retrieval for `aidb_search`. Not a second user API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retrieval {
    Vec,
    Fts,
    Hybrid,
}

impl Retrieval {
    pub fn algorithm(self) -> &'static str {
        match self {
            Self::Vec => "sqlite-vec knn",
            Self::Fts => "fts5 match",
            Self::Hybrid => "hybrid rrf (vec+fts)",
        }
    }

    pub fn choose(query: &str, has_vec: bool, has_fts: bool) -> Self {
        match (has_vec, has_fts) {
            (false, true) => Self::Fts,
            (true, false) | (false, false) => Self::Vec,
            (true, true) => {
                if keyword_only(query) {
                    Self::Fts
                } else {
                    Self::Hybrid
                }
            }
        }
    }
}

fn keyword_only(query: &str) -> bool {
    let tokens: Vec<&str> = query
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 2)
        .collect();
    if tokens.is_empty() {
        return false;
    }
    let identifierish = tokens.iter().any(|t| identifier_like(t));
    let semantic = tokens.iter().any(|t| !identifier_like(t) && t.len() > 3);
    identifierish && !semantic
}

fn identifier_like(token: &str) -> bool {
    let has_digit = token.chars().any(|c| c.is_ascii_digit());
    let has_alpha = token.chars().any(|c| c.is_ascii_alphabetic());
    (has_digit && has_alpha)
        || (token.len() >= 6
            && token
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

static ID_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn new_id(prefix: &str) -> String {
    let ms = now_ms() as u64;
    let n = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{ms:x}_{n:x}")
}

/// Kill the process at a named execution boundary when `AIDB_TEST_CRASH_POINT`
/// matches. Crash/resume is a durability contract, so it has to be tested against
/// a real process death rather than a simulated one.
///
/// This is compiled out unless `debug_assertions` are on, so a release build can
/// never abort because of an environment variable.
#[inline]
pub fn crash_point(name: &str) {
    #[cfg(debug_assertions)]
    {
        if std::env::var("AIDB_TEST_CRASH_POINT").ok().as_deref() == Some(name) {
            // Skip unwinding and destructors: this must look like `kill -9`.
            std::process::abort();
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = name;
}

pub fn content_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

#[derive(Debug)]
pub enum Error {
    Io {
        path: Option<PathBuf>,
        source: std::io::Error,
    },
    Sqlite(String),
    Schema(String),
    Usage(String),
    Ai(String),
}

impl Error {
    pub fn sqlite(err: impl fmt::Display) -> Self {
        Self::Sqlite(err.to_string())
    }

    pub fn schema(msg: impl Into<String>) -> Self {
        Self::Schema(msg.into())
    }

    pub fn usage(msg: impl Into<String>) -> Self {
        Self::Usage(msg.into())
    }

    pub fn ai(msg: impl Into<String>) -> Self {
        Self::Ai(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                path: Some(path),
                source,
            } => {
                write!(f, "{path}: {source}", path = path.display())
            }
            Self::Io { path: None, source } => write!(f, "{source}"),
            Self::Sqlite(msg) | Self::Schema(msg) | Self::Usage(msg) | Self::Ai(msg) => {
                f.write_str(msg)
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str(""),
            Self::Integer(v) => write!(f, "{v}"),
            Self::Real(v) => write!(f, "{v}"),
            Self::Text(v) => f.write_str(v),
            Self::Blob(v) => write!(f, "X'{}'", hex(v)),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl QueryResult {
    pub fn to_tsv(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.columns.join("\t"));
        if !self.columns.is_empty() {
            out.push('\n');
        }
        for (i, row) in self.rows.iter().enumerate() {
            let line: Vec<String> = row.iter().map(ToString::to_string).collect();
            out.push_str(&line.join("\t"));
            if i + 1 < self.rows.len() {
                out.push('\n');
            }
        }
        if !self.rows.is_empty() {
            out.push('\n');
        }
        out
    }
}
