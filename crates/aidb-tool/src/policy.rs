//! Declarative policy stored in the file. Not a sidecar and not a second DB.

use aidb_core::{Error, QueryResult, Result, Value};
use aidb_storage::{sqlite_err, Connection};

use crate::Capability;

pub const META_KEY: &str = "policy";

#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub name: Option<String>,
    pub allow: Option<Vec<String>>,
    pub deny: Vec<String>,
    pub max_usd: Option<f64>,
    pub max_ms: Option<u64>,
    pub max_llm_calls: Option<u32>,
    pub read_only: bool,
    pub require_approval: Vec<String>,
}

impl Policy {
    pub fn empty() -> Self {
        Self {
            name: None,
            allow: None,
            deny: Vec::new(),
            max_usd: None,
            max_ms: None,
            max_llm_calls: None,
            read_only: false,
            require_approval: Vec::new(),
        }
    }

    pub fn from_env() -> Self {
        let mut policy = Self::empty();
        policy.deny = std::env::var("AIDB_DENY_TOOLS")
            .ok()
            .map(|raw| split_names(&raw))
            .unwrap_or_default();
        policy.max_usd = std::env::var("AIDB_MAX_USD")
            .ok()
            .and_then(|v| v.parse().ok());
        policy.max_ms = std::env::var("AIDB_MAX_MS")
            .ok()
            .and_then(|v| v.parse().ok());
        policy.max_llm_calls = std::env::var("AIDB_MAX_LLM_CALLS")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(Some(64));
        policy
    }

    pub fn parse(json: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|err| Error::usage(format!("policy JSON: {err}")))?;
        let obj = value
            .as_object()
            .ok_or_else(|| Error::usage("policy must be a JSON object"))?;
        reject_secrets(obj)?;
        let mut policy = Self::empty();
        if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
            if !name.is_empty() {
                policy.name = Some(name.to_string());
            }
        }
        if let Some(allow) = obj.get("allow") {
            policy.allow = Some(json_names(allow, "allow")?);
        }
        if let Some(deny) = obj.get("deny") {
            policy.deny = json_names(deny, "deny")?;
        }
        policy.max_usd = json_f64(obj.get("max_usd"), "max_usd")?;
        policy.max_ms = json_u64(obj.get("max_ms"), "max_ms")?;
        policy.max_llm_calls = json_u32(obj.get("max_llm_calls"), "max_llm_calls")?;
        if let Some(read_only) = obj.get("read_only") {
            policy.read_only = read_only
                .as_bool()
                .ok_or_else(|| Error::usage("policy.read_only must be a boolean"))?;
        }
        if let Some(require) = obj.get("require_approval") {
            policy.require_approval = json_names(require, "require_approval")?;
        }
        Ok(policy)
    }

    pub fn to_json(&self) -> String {
        let mut value = serde_json::json!({
            "deny": self.deny,
            "read_only": self.read_only,
            "require_approval": self.require_approval,
        });
        if let Some(name) = &self.name {
            value["name"] = serde_json::Value::String(name.clone());
        }
        if let Some(allow) = &self.allow {
            value["allow"] = serde_json::json!(allow);
        }
        if let Some(usd) = self.max_usd {
            value["max_usd"] = serde_json::json!(usd);
        }
        if let Some(ms) = self.max_ms {
            value["max_ms"] = serde_json::json!(ms);
        }
        if let Some(calls) = self.max_llm_calls {
            value["max_llm_calls"] = serde_json::json!(calls);
        }
        value.to_string()
    }

    pub fn overlay(&self, over: &Self) -> Self {
        Self {
            name: over.name.clone().or_else(|| self.name.clone()),
            allow: match (&self.allow, &over.allow) {
                (Some(a), Some(b)) => Some(intersect(a, b)),
                (Some(a), None) => Some(a.clone()),
                (None, Some(b)) => Some(b.clone()),
                (None, None) => None,
            },
            deny: union(&self.deny, &over.deny),
            max_usd: min_opt(self.max_usd, over.max_usd),
            max_ms: min_opt(self.max_ms, over.max_ms),
            max_llm_calls: min_opt(self.max_llm_calls, over.max_llm_calls),
            read_only: self.read_only || over.read_only,
            require_approval: union(&self.require_approval, &over.require_approval),
        }
    }

    pub fn summary(&self) -> String {
        let deny = if self.deny.is_empty() {
            "none".into()
        } else {
            self.deny.join(",")
        };
        let usd = self
            .max_usd
            .map(|n| n.to_string())
            .unwrap_or_else(|| "none".into());
        format!("read_only={} deny={} max_usd={}", self.read_only, deny, usd)
    }

    pub fn requires_approval(&self, cap: &Capability) -> bool {
        cap.needs_approval() || self.require_approval.iter().any(|name| name == &cap.name)
    }
}

pub fn load(conn: &Connection) -> Result<Policy> {
    match meta_get(conn, META_KEY)? {
        Some(json) if !json.is_empty() && json != "{}" => Policy::parse(&json),
        _ => Ok(Policy::empty()),
    }
}

pub fn save(conn: &Connection, policy: &Policy) -> Result<()> {
    let json = policy.to_json();
    meta_set(conn, META_KEY, &json)?;
    if let Some(name) = &policy.name {
        meta_set(conn, &format!("policy:{name}"), &json)?;
    }
    Ok(())
}

pub fn set_json(conn: &Connection, json: &str, name: Option<&str>) -> Result<Policy> {
    let mut policy = Policy::parse(json)?;
    if let Some(name) = name {
        if !name.is_empty() {
            policy.name = Some(name.to_string());
        }
    }
    save(conn, &policy)?;
    Ok(policy)
}

pub fn get_json(conn: &Connection) -> Result<String> {
    Ok(load(conn)?.to_json())
}

pub fn set_sql(conn: &Connection, json: &str, name: Option<&str>) -> Result<QueryResult> {
    let policy = set_json(conn, json, name)?;
    let stored = policy.to_json();
    Ok(QueryResult {
        columns: vec!["policy".into()],
        rows: vec![vec![Value::Text(stored)]],
    })
}

pub fn get_sql(conn: &Connection) -> Result<QueryResult> {
    Ok(QueryResult {
        columns: vec!["policy".into()],
        rows: vec![vec![Value::Text(get_json(conn)?)]],
    })
}

pub fn effective(conn: &Connection) -> Result<Policy> {
    let file = load(conn)?;
    let mut policy = file.overlay(&Policy::from_env());
    if let Some(deny) = crate::deny_override() {
        policy.deny = union(&policy.deny, &deny);
    }
    Ok(policy)
}

fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn
        .prepare("SELECT value FROM aidb_meta WHERE key = ?1")
        .map_err(sqlite_err)?;
    let mut rows = stmt.query([key]).map_err(sqlite_err)?;
    match rows.next().map_err(sqlite_err)? {
        Some(row) => Ok(Some(row.get(0).map_err(sqlite_err)?)),
        None => Ok(None),
    }
}

fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO aidb_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )
    .map_err(sqlite_err)?;
    Ok(())
}

fn reject_secrets(obj: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
    for key in obj.keys() {
        match key.to_ascii_lowercase().as_str() {
            "api_key" | "key" | "secret" | "token" | "password" => {
                return Err(Error::usage(
                    "policy cannot store secrets; keys stay in the environment",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn json_names(value: &serde_json::Value, field: &str) -> Result<Vec<String>> {
    if let Some(s) = value.as_str() {
        return Ok(split_names(s));
    }
    let arr = value
        .as_array()
        .ok_or_else(|| Error::usage(format!("policy.{field} must be an array of names")))?;
    let mut out = Vec::new();
    for item in arr {
        let name = item
            .as_str()
            .ok_or_else(|| Error::usage(format!("policy.{field} entries must be strings")))?;
        if !name.is_empty() && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    Ok(out)
}

fn json_f64(value: Option<&serde_json::Value>, field: &str) -> Result<Option<f64>> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .ok_or_else(|| Error::usage(format!("policy.{field} must be a number")))
            .map(Some),
    }
}

fn json_u64(value: Option<&serde_json::Value>, field: &str) -> Result<Option<u64>> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .ok_or_else(|| Error::usage(format!("policy.{field} must be an integer")))
            .map(Some),
    }
}

fn json_u32(value: Option<&serde_json::Value>, field: &str) -> Result<Option<u32>> {
    match json_u64(value, field)? {
        None => Ok(None),
        Some(n) if n <= u32::MAX as u64 => Ok(Some(n as u32)),
        Some(_) => Err(Error::usage(format!("policy.{field} is too large"))),
    }
}

fn split_names(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn union(a: &[String], b: &[String]) -> Vec<String> {
    let mut out = a.to_vec();
    for name in b {
        if !out.iter().any(|n| n == name) {
            out.push(name.clone());
        }
    }
    out
}

fn intersect(a: &[String], b: &[String]) -> Vec<String> {
    a.iter()
        .filter(|name| b.iter().any(|other| other == *name))
        .cloned()
        .collect()
}

fn min_opt<T: Copy + PartialOrd>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if x < y { x } else { y }),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_phase_example_and_rejects_secrets() {
        let policy = Policy::parse(r#"{"read_only":true,"deny":["send.email"],"max_usd":0.10}"#)
            .expect("parse");
        assert!(policy.read_only);
        assert_eq!(policy.deny, ["send.email"]);
        assert_eq!(policy.max_usd, Some(0.10));
        let err = Policy::parse(r#"{"deny":[],"api_key":"sk"}"#).expect_err("secret");
        assert!(err.to_string().contains("secrets"), "{err}");
    }

    #[test]
    fn overlay_tightens_budget_and_unions_deny() {
        let file = Policy::parse(r#"{"deny":["send.email"],"max_usd":0.10}"#).unwrap();
        let over = Policy {
            deny: vec!["http.get".into()],
            max_usd: Some(1.0),
            read_only: true,
            require_approval: vec!["github.read".into()],
            ..Policy::empty()
        };
        let out = file.overlay(&over);
        assert!(out.read_only);
        assert_eq!(out.deny, ["send.email", "http.get"]);
        assert_eq!(out.max_usd, Some(0.10));
        assert_eq!(out.require_approval, ["github.read"]);
    }
}
