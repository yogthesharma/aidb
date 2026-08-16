//! SQL dialect convenience. Lowers to the same IR and catalog as the functions.
//! Not a second planner.

use aidb_core::{now_ms, Error, Result};
use aidb_storage::{sqlite_err, Connection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateModel {
    pub name: String,
    pub kind: String,
    pub provider: String,
    pub provider_model: String,
    pub dimensions: Option<i64>,
    pub key_name: Option<String>,
    pub if_not_exists: bool,
}

pub fn parse_search_dialect(sql: &str) -> Option<(String, i64, Option<String>)> {
    let start = find_keyword(sql, "search")?;
    let after = sql[start + 6..].trim_start();
    let (query, rest) = parse_string(after)?;
    let mut rest = rest.trim_start().trim_end_matches(';').trim_end();
    let mut filter = None;
    if let Some(where_rest) = strip_keyword(rest, "where") {
        let (map, after_where) = parse_metadata_where(where_rest)?;
        if !map.is_empty() {
            filter = Some(serde_json::Value::Object(map).to_string());
        }
        rest = after_where.trim_start();
    }
    let k = if let Some(limit) = strip_keyword(rest, "limit") {
        parse_limit(limit).unwrap_or(5)
    } else if rest.is_empty() {
        5
    } else {
        return None;
    };
    Some((query, k.max(1), filter))
}

fn parse_metadata_where(sql: &str) -> Option<(serde_json::Map<String, serde_json::Value>, &str)> {
    let mut map = serde_json::Map::new();
    let mut rest = sql.trim_start();
    loop {
        let (key, after) = take_ident(rest)?;
        let field = key.strip_prefix("metadata.")?;
        if field.is_empty() || !field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        let after = after.trim_start();
        let after = after.strip_prefix('=')?.trim_start();
        let (value, after) = if let Some((text, next)) = parse_string(after) {
            (serde_json::Value::String(text), next)
        } else {
            parse_scalar(after)?
        };
        map.insert(field.to_string(), value);
        rest = after.trim_start();
        if let Some(and_rest) = strip_keyword(rest, "and") {
            rest = and_rest;
            continue;
        }
        break;
    }
    Some((map, rest))
}

fn parse_scalar(sql: &str) -> Option<(serde_json::Value, &str)> {
    let rest = sql.trim_start();
    if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("true") {
        return Some((serde_json::Value::Bool(true), rest[4..].trim_start()));
    }
    if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case("false") {
        return Some((serde_json::Value::Bool(false), rest[5..].trim_start()));
    }
    if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("null") {
        return Some((serde_json::Value::Null, rest[4..].trim_start()));
    }
    let mut end = 0;
    let bytes = rest.as_bytes();
    if bytes.first() == Some(&b'-') {
        end = 1;
    }
    while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
        end += 1;
    }
    if end == 0 || rest[..end] == *"-" || rest[..end] == *"." {
        return None;
    }
    let number: serde_json::Number = rest[..end].parse().ok()?;
    Some((serde_json::Value::Number(number), rest[end..].trim_start()))
}

pub fn parse_create_model(sql: &str) -> Option<CreateModel> {
    let trimmed = sql.trim();
    if trimmed.len() < 12 || !trimmed[..12].eq_ignore_ascii_case("create model") {
        return None;
    }
    let mut rest = trimmed[12..].trim_start();
    let if_not_exists = strip_if_not_exists(&mut rest);
    let (name, rest) = take_ident(rest)?;
    if name.eq_ignore_ascii_case("model") {
        return None;
    }
    let mut spec = if rest.trim_start().starts_with('(') {
        parse_create_model_paren(&name, rest.trim_start())?
    } else {
        parse_create_model_keywords(&name, rest)?
    };
    spec.if_not_exists = if_not_exists;
    Some(spec)
}

fn strip_if_not_exists(rest: &mut &str) -> bool {
    let trimmed = rest.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("if not exists") {
        let after = trimmed["if not exists".len()..].trim_start();
        *rest = after;
        true
    } else {
        false
    }
}

pub fn execute_create_model(conn: &Connection, spec: &CreateModel) -> Result<u64> {
    if !matches!(spec.kind.as_str(), "llm" | "embedding" | "rerank") {
        return Err(Error::usage(format!(
            "CREATE MODEL kind must be llm, embedding, or rerank (got {})",
            spec.kind
        )));
    }
    if !aidb_ai::known_provider(&spec.provider) {
        return Err(Error::usage(format!(
            "unknown model provider: {} (use fake, openai, or anthropic)",
            spec.provider
        )));
    }
    if let Some(name) = spec
        .key_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        aidb_ai::validate_key_name(name)?;
    }
    let key_name = spec
        .key_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let sql = if spec.if_not_exists {
        "INSERT OR IGNORE INTO models (name, kind, provider, provider_model, dimensions, key_name, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
    } else {
        "INSERT INTO models (name, kind, provider, provider_model, dimensions, key_name, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(name) DO UPDATE SET
            kind = excluded.kind,
            provider = excluded.provider,
            provider_model = excluded.provider_model,
            dimensions = excluded.dimensions,
            key_name = excluded.key_name"
    };
    conn.execute(
        sql,
        rusqlite::params![
            spec.name,
            spec.kind,
            spec.provider,
            spec.provider_model,
            spec.dimensions,
            key_name,
            now_ms()
        ],
    )
    .map_err(sqlite_err)?;
    Ok(1)
}

fn parse_create_model_paren(name: &str, rest: &str) -> Option<CreateModel> {
    let inner = rest.strip_prefix('(')?.trim_start();
    let end = inner.find(')')?;
    let mut spec = CreateModel {
        name: name.to_string(),
        kind: "llm".into(),
        provider: String::new(),
        provider_model: String::new(),
        dimensions: None,
        key_name: None,
        if_not_exists: false,
    };
    for part in inner[..end].split(',') {
        let (key, value) = split_eq(part.trim())?;
        apply_model_field(&mut spec, &key, &value)?;
    }
    finalize_create_model(spec)
}

fn parse_create_model_keywords(name: &str, mut rest: &str) -> Option<CreateModel> {
    let mut spec = CreateModel {
        name: name.to_string(),
        kind: "llm".into(),
        provider: String::new(),
        provider_model: String::new(),
        dimensions: None,
        key_name: None,
        if_not_exists: false,
    };
    rest = rest.trim_start().trim_end_matches(';').trim_end();
    while !rest.is_empty() {
        let (key, after) = take_ident(rest)?;
        let after = after.trim_start();
        let (value, next) = if let Some((value, next)) = parse_string(after) {
            (value, next)
        } else {
            let (ident, next) = take_ident(after)?;
            (ident, next)
        };
        apply_model_field(&mut spec, &key, &value)?;
        rest = next.trim_start();
    }
    finalize_create_model(spec)
}

fn finalize_create_model(mut spec: CreateModel) -> Option<CreateModel> {
    if spec.provider.is_empty() {
        return None;
    }
    spec.provider = spec.provider.to_ascii_lowercase();
    if spec.provider_model.is_empty() {
        spec.provider_model = aidb_ai::default_provider_model(&spec.provider).to_string();
    }
    Some(spec)
}

fn apply_model_field(spec: &mut CreateModel, key: &str, value: &str) -> Option<()> {
    match key.to_ascii_lowercase().as_str() {
        "kind" => spec.kind = value.to_ascii_lowercase(),
        "provider" => spec.provider = value.to_ascii_lowercase(),
        "provider_model" | "model" => spec.provider_model = value.to_string(),
        "dimensions" => spec.dimensions = value.parse().ok(),
        "key_name" | "key" => spec.key_name = Some(value.to_string()),
        "api_key" | "secret" | "token" => return None,
        _ => return None,
    }
    Some(())
}

fn split_eq(part: &str) -> Option<(String, String)> {
    let eq = part.find('=')?;
    let key = part[..eq].trim().to_string();
    let raw = part[eq + 1..].trim();
    let value = if let Some((s, rest)) = parse_string(raw) {
        if !rest.trim().is_empty() {
            return None;
        }
        s
    } else {
        raw.trim_end_matches(',').trim().to_string()
    };
    if key.is_empty() || value.is_empty() {
        return None;
    }
    Some((key, value))
}

fn find_keyword(sql: &str, word: &str) -> Option<usize> {
    let lower = sql.to_ascii_lowercase();
    let mut i = 0;
    while let Some(rel) = lower[i..].find(word) {
        let abs = i + rel;
        let before = if abs == 0 {
            ' '
        } else {
            lower.as_bytes()[abs - 1] as char
        };
        let after = lower
            .as_bytes()
            .get(abs + word.len())
            .map(|b| *b as char)
            .unwrap_or(' ');
        if !is_ident_char(before) && !is_ident_char(after) {
            return Some(abs);
        }
        i = abs + word.len();
    }
    None
}

fn strip_keyword<'a>(sql: &'a str, word: &str) -> Option<&'a str> {
    let trimmed = sql.trim_start();
    if trimmed.len() >= word.len() && trimmed[..word.len()].eq_ignore_ascii_case(word) {
        let after = &trimmed[word.len()..];
        if after.starts_with(|c: char| c.is_ascii_whitespace() || c.is_ascii_digit()) {
            return Some(after.trim_start());
        }
    }
    None
}

fn parse_limit(sql: &str) -> Option<i64> {
    let digits: String = sql
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn take_ident(sql: &str) -> Option<(String, &str)> {
    let rest = sql.trim_start();
    let ident: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect();
    if ident.is_empty() || ident.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    Some((ident.clone(), rest[ident.len()..].trim_start()))
}

fn parse_string(sql: &str) -> Option<(String, &str)> {
    let rest = sql.trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = String::new();
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == quote as u8 {
            if i + 1 < bytes.len() && bytes[i + 1] == quote as u8 {
                out.push(quote);
                i += 2;
                continue;
            }
            return Some((out, rest[i + 1..].trim_start()));
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    None
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_search_and_from_documents() {
        let (q, k, filter) = parse_search_dialect("SEARCH 'How do refunds work?' LIMIT 5").unwrap();
        assert_eq!(q, "How do refunds work?");
        assert_eq!(k, 5);
        assert_eq!(filter, None);
        let (q, k, filter) =
            parse_search_dialect("SELECT * FROM documents SEARCH 'How do refunds work?' LIMIT 5;")
                .unwrap();
        assert_eq!(q, "How do refunds work?");
        assert_eq!(k, 5);
        assert_eq!(filter, None);
        assert!(parse_search_dialect("SELECT aidb_search('q', 5)").is_none());
    }

    #[test]
    fn parses_search_where_metadata() {
        let (q, k, filter) =
            parse_search_dialect("SEARCH 'refund policy' WHERE metadata.dept = 'support' LIMIT 5")
                .unwrap();
        assert_eq!(q, "refund policy");
        assert_eq!(k, 5);
        assert_eq!(filter.as_deref(), Some(r#"{"dept":"support"}"#));
        let (_, _, filter) = parse_search_dialect(
            "SEARCH 'refunds' WHERE metadata.dept = 'support' AND metadata.kind = 'faq' LIMIT 3",
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&filter.unwrap()).unwrap();
        assert_eq!(value["dept"], "support");
        assert_eq!(value["kind"], "faq");
    }

    #[test]
    fn parses_create_model_paren_and_keywords() {
        let spec = parse_create_model(
            "CREATE MODEL gpt (kind = llm, provider = openai, provider_model = 'gpt-4.1-mini');",
        )
        .unwrap();
        assert_eq!(spec.name, "gpt");
        assert_eq!(spec.kind, "llm");
        assert_eq!(spec.provider, "openai");
        assert_eq!(spec.provider_model, "gpt-4.1-mini");

        let spec =
            parse_create_model("CREATE MODEL gpt PROVIDER openai MODEL 'gpt-4.1-mini'").unwrap();
        assert_eq!(spec.provider_model, "gpt-4.1-mini");
        assert!(parse_create_model(
            "CREATE MODEL gpt (kind = llm, provider = openai, provider_model = 'x', api_key = 'sk')"
        )
        .is_none());

        let spec = parse_create_model("CREATE MODEL IF NOT EXISTS cls PROVIDER 'fake' KIND 'llm'")
            .unwrap();
        assert_eq!(spec.name, "cls");
        assert_eq!(spec.provider, "fake");
        assert_eq!(spec.kind, "llm");
        assert_eq!(spec.provider_model, "aidb-fake");
        assert!(spec.if_not_exists);

        let spec = parse_create_model("CREATE MODEL claude PROVIDER anthropic KIND llm").unwrap();
        assert_eq!(spec.provider, "anthropic");
        assert_eq!(spec.provider_model, "claude-sonnet-4-20250514");
        assert!(!spec.if_not_exists);

        let spec = parse_create_model("CREATE MODEL gpt PROVIDER openai KEY_NAME 'OPENAI_API_KEY'")
            .unwrap();
        assert_eq!(spec.key_name.as_deref(), Some("OPENAI_API_KEY"));
        let spec = parse_create_model(
            "CREATE MODEL gpt (kind = llm, provider = openai, key_name = 'prod-openai')",
        )
        .unwrap();
        assert_eq!(spec.key_name.as_deref(), Some("prod-openai"));
    }
}
