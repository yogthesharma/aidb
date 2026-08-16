//! Embedding and LLM adapters. Keys come from the environment first, then an optional store.

mod embed;
mod secrets;

use std::io::{BufRead, BufReader};
use std::sync::Arc;

use aidb_core::{Error, Result};

pub use embed::{
    hashed_embedding, local_model_dimensions, lookup_custom_embedder, normalize_distance,
    normalize_local_model, register_custom_embedder, FakeEmbedder, LocalEmbedder, OpenAiEmbedder,
};
pub use secrets::{
    configured_store, default_key_name, resolve_provider_key, resolve_secret, secret_store_uri,
    validate_key_name, SecretStore,
};

pub trait Embedder: Send + Sync {
    fn provider(&self) -> &str;
    fn model(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

#[derive(Clone, Debug)]
pub struct EmbedderConfig {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub key_name: Option<String>,
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        match std::env::var("AIDB_EMBEDDER").ok().as_deref() {
            Some("openai") => Self {
                provider: "openai".into(),
                model: std::env::var("AIDB_EMBED_MODEL")
                    .unwrap_or_else(|_| "text-embedding-3-small".into()),
                dimensions: 1536,
                key_name: None,
            },
            Some("local") => {
                let model = std::env::var("AIDB_EMBED_MODEL")
                    .unwrap_or_else(|_| "BAAI/bge-small-en-v1.5".into());
                let dimensions = embed::local_model_dimensions(&model).unwrap_or(384);
                Self {
                    provider: "local".into(),
                    model,
                    dimensions,
                    key_name: None,
                }
            }
            _ => Self {
                provider: "fake".into(),
                model: "aidb-fake".into(),
                dimensions: 32,
                key_name: None,
            },
        }
    }
}

/// Default LLM bind from `AIDB_LLM` / `AIDB_LLM_MODEL`. Keys stay in env or the optional store.
pub fn default_llm() -> (String, String) {
    match std::env::var("AIDB_LLM").ok().as_deref() {
        Some("openai") => (
            "openai".into(),
            std::env::var("AIDB_LLM_MODEL").unwrap_or_else(|_| "gpt-4.1-mini".into()),
        ),
        Some("anthropic") => (
            "anthropic".into(),
            std::env::var("AIDB_LLM_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".into()),
        ),
        _ => ("fake".into(), "aidb-fake".into()),
    }
}

pub fn default_provider_model(provider: &str) -> &'static str {
    match provider {
        "openai" => "gpt-4.1-mini",
        "anthropic" => "claude-sonnet-4-20250514",
        _ => "aidb-fake",
    }
}

pub fn default_embed_model(provider: &str) -> &'static str {
    match provider {
        "openai" => "text-embedding-3-small",
        "local" => "BAAI/bge-small-en-v1.5",
        "custom" => "",
        _ => "aidb-fake",
    }
}

pub fn known_embed_provider(provider: &str) -> bool {
    matches!(provider, "fake" | "openai" | "local" | "custom")
}

pub fn known_provider(provider: &str) -> bool {
    matches!(provider, "fake" | "openai" | "anthropic")
}

pub fn embedder(config: &EmbedderConfig) -> Result<Arc<dyn Embedder>> {
    match config.provider.as_str() {
        "fake" => Ok(Arc::new(FakeEmbedder {
            model: if config.model.trim().is_empty() {
                "aidb-fake".into()
            } else {
                config.model.clone()
            },
            dimensions: config.dimensions,
        })),
        "openai" => Ok(Arc::new(OpenAiEmbedder::new(
            config.model.clone(),
            config.dimensions,
            config.key_name.as_deref(),
        )?)),
        "local" => Ok(Arc::new(LocalEmbedder::new(
            &config.model,
            config.dimensions,
        )?)),
        "custom" => lookup_custom_embedder(&config.model, config.dimensions),
        other => Err(Error::usage(format!(
            "unknown embedding provider: {other} (use fake, openai, local, or custom)"
        ))),
    }
}

pub fn estimate_usd(prompt_tokens: i64, completion_tokens: i64) -> f64 {
    (prompt_tokens as f64) * 1.5e-7 + (completion_tokens as f64) * 6.0e-7
}

pub struct Completion {
    pub text: String,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

pub trait Llm: Send + Sync {
    fn provider(&self) -> &str;
    fn model(&self) -> &str;
    fn complete(&self, prompt: &str, content: &str) -> Result<Completion>;
    /// Default: one callback with the full text. Fake splits so tests see a prefix.
    fn complete_streaming(
        &self,
        prompt: &str,
        content: &str,
        on_token: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Completion> {
        let completion = self.complete(prompt, content)?;
        if !completion.text.is_empty() {
            on_token(&completion.text)?;
        }
        Ok(completion)
    }
    fn classify(&self, labels: &str, content: &str) -> Result<Completion> {
        let prompt = format!("Classify as one of: {labels}. Reply with only the label.");
        self.complete(&prompt, content)
    }
}

/// Split `text` into small chunks whose concatenation is exactly `text`.
pub fn emit_text_chunks(text: &str, on_token: &mut dyn FnMut(&str) -> Result<()>) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if buf.len() >= 8 && (ch.is_whitespace() || buf.len() >= 16) {
            on_token(&buf)?;
            buf.clear();
        }
    }
    if !buf.is_empty() {
        on_token(&buf)?;
    }
    Ok(())
}

pub fn llm(provider: &str, model: &str) -> Result<Box<dyn Llm>> {
    llm_with_key(provider, model, None)
}

pub fn llm_with_key(provider: &str, model: &str, key_name: Option<&str>) -> Result<Box<dyn Llm>> {
    match provider {
        "fake" => Ok(Box::new(FakeLlm {
            model: model.to_string(),
        })),
        "openai" => Ok(Box::new(OpenAiLlm::new(model.to_string(), key_name)?)),
        "anthropic" => Ok(Box::new(AnthropicLlm::new(model.to_string(), key_name)?)),
        other => Err(Error::usage(format!("unknown llm provider: {other}"))),
    }
}

pub fn parse_labels(labels: &str) -> Vec<String> {
    labels
        .split([',', '/', '|'])
        .flat_map(|part| part.split(" or "))
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn pick_label(labels: &str, content: &str) -> String {
    let options = parse_labels(labels);
    let lower = content.to_ascii_lowercase();
    options
        .iter()
        .find(|label| lower.contains(label.as_str()))
        .cloned()
        .or_else(|| options.into_iter().next())
        .unwrap_or_else(|| labels.trim().to_string())
}

pub const JSON_SCHEMA_MARK: &str = "\nAIDB_JSON_SCHEMA:\n";

pub fn schema_from_prompt(prompt: &str) -> Option<serde_json::Value> {
    let (_, rest) = prompt.split_once(JSON_SCHEMA_MARK)?;
    serde_json::from_str(rest.trim()).ok()
}

/// Fill a JSON value from `content` using the schema as a shape, not as a claim.
/// Required fields the content cannot support are omitted so validation can fail.
pub fn fill_schema(schema: &serde_json::Value, content: &str) -> serde_json::Value {
    if is_decide_schema(schema) {
        return fill_decide(schema, content);
    }
    if let Some(options) = schema.get("enum").and_then(|v| v.as_array()) {
        let labels = options
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" or ");
        let picked = pick_label(&labels, content);
        if options.iter().any(|v| v.as_str() == Some(picked.as_str()))
            && content.to_ascii_lowercase().contains(&picked)
        {
            return serde_json::Value::String(picked);
        }
        return serde_json::Value::String(content.chars().take(40).collect());
    }
    match schema
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("object")
    {
        "object" => {
            let mut out = serde_json::Map::new();
            if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
                for (name, sub) in properties {
                    if schema_const_misses(sub, content) {
                        continue;
                    }
                    out.insert(name.clone(), fill_schema(sub, content));
                }
            }
            serde_json::Value::Object(out)
        }
        "array" => serde_json::Value::Array(Vec::new()),
        "string" => serde_json::Value::String(content.chars().take(160).collect()),
        "number" | "integer" => {
            let digits: String = content
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            digits
                .parse::<i64>()
                .map(serde_json::Value::from)
                .unwrap_or_else(|_| serde_json::Value::String(content.chars().take(40).collect()))
        }
        "boolean" => {
            let lower = content.to_ascii_lowercase();
            if lower.contains("true") || lower.contains("yes") {
                serde_json::Value::Bool(true)
            } else if lower.contains("false") || lower.contains("no") {
                serde_json::Value::Bool(false)
            } else {
                serde_json::Value::String(content.chars().take(40).collect())
            }
        }
        "null" => serde_json::Value::Null,
        _ => serde_json::Value::String(content.chars().take(160).collect()),
    }
}

fn is_decide_schema(schema: &serde_json::Value) -> bool {
    schema
        .get("properties")
        .and_then(|p| p.get("op"))
        .and_then(|op| op.get("enum"))
        .and_then(|v| v.as_array())
        .is_some_and(|options| options.iter().any(|v| v.as_str() == Some("stop")))
}

/// Offline decide: first unused allowed op, then stop. Args come from the goal
/// and last output so tests can demand a recipient or a ticker filter.
fn fill_decide(schema: &serde_json::Value, content: &str) -> serde_json::Value {
    let allowed = schema
        .pointer("/properties/op/enum")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let taken = taken_ops(content);
    let op = allowed
        .iter()
        .find(|name| name.as_str() != "stop" && !taken.iter().any(|t| t == *name))
        .cloned()
        .unwrap_or_else(|| "stop".into());
    serde_json::json!({
        "op": op,
        "args": decide_args(&op, content),
    })
}

fn taken_ops(content: &str) -> Vec<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Taken:") {
            return rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Vec::new()
}

fn decide_args(op: &str, content: &str) -> serde_json::Value {
    match op {
        "search" => {
            let query = section_line(content, "Goal").unwrap_or(content);
            let mut args = serde_json::json!({ "query": query.trim() });
            if let Some(ticker) = ticker_in(query) {
                args["filter"] = serde_json::json!({ "ticker": ticker });
            }
            args
        }
        "send.email" => serde_json::json!({
            "to": email_in(content).unwrap_or("desk@local"),
            "subject": section_line(content, "Goal").unwrap_or("digest"),
            "body": section_block(content, "Last"),
        }),
        "github.read" => serde_json::json!({
            "path": section_line(content, "Goal").unwrap_or("README.md"),
        }),
        "http.get" => serde_json::json!({ "url": "aidb://docs" }),
        _ => serde_json::json!({}),
    }
}

fn section_line<'a>(content: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}:");
    content.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    })
}

fn section_block(content: &str, name: &str) -> String {
    let header = format!("{name}:");
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        if line.trim() == header || line.trim().starts_with(&header) {
            let rest: Vec<&str> = lines
                .take_while(|l| {
                    let t = l.trim();
                    !(t.ends_with(':')
                        && t.len() < 24
                        && t.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
                })
                .collect();
            let text = rest.join("\n").trim().to_string();
            if !text.is_empty() {
                return text;
            }
            break;
        }
    }
    String::new()
}

fn ticker_in(text: &str) -> Option<&str> {
    const SKIP: &[&str] = &[
        "A", "I", "FOR", "THE", "AND", "DONE", "JSON", "SQL", "AIDB", "LLM",
    ];
    text.split(|c: char| !c.is_ascii_alphabetic()).find(|word| {
        let len = word.len();
        (2..=5).contains(&len)
            && word.chars().all(|c| c.is_ascii_uppercase())
            && !SKIP.contains(word)
    })
}

fn email_in(text: &str) -> Option<&str> {
    text.split_whitespace()
        .find(|word| word.contains('@') && word.contains('.'))
        .map(|word| {
            word.trim_matches(|c: char| {
                !c.is_ascii_alphanumeric() && c != '@' && c != '.' && c != '_' && c != '-'
            })
        })
        .filter(|word| word.contains('@'))
}

fn schema_const_misses(schema: &serde_json::Value, content: &str) -> bool {
    let Some(expected) = schema.get("const").and_then(|v| v.as_str()) else {
        return false;
    };
    !content.contains(expected)
}

pub struct FakeLlm {
    model: String,
}

impl Llm for FakeLlm {
    fn provider(&self) -> &str {
        "fake"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn complete(&self, prompt: &str, content: &str) -> Result<Completion> {
        let text = if let Some(schema) = schema_from_prompt(prompt) {
            fill_schema(&schema, content).to_string()
        } else {
            let excerpt: String = content.chars().take(160).collect();
            format!("{prompt}: {excerpt}")
        };
        Ok(Completion {
            text: text.clone(),
            prompt_tokens: Some(((prompt.len() + content.len()) / 4) as i64),
            completion_tokens: Some((text.len() / 4) as i64),
            cost_usd: Some(estimate_usd(
                ((prompt.len() + content.len()) / 4) as i64,
                (text.len() / 4) as i64,
            )),
        })
    }

    fn complete_streaming(
        &self,
        prompt: &str,
        content: &str,
        on_token: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Completion> {
        let completion = self.complete(prompt, content)?;
        emit_text_chunks(&completion.text, on_token)?;
        Ok(completion)
    }

    fn classify(&self, labels: &str, content: &str) -> Result<Completion> {
        let text = pick_label(labels, content);
        Ok(Completion {
            text: text.clone(),
            prompt_tokens: Some(((labels.len() + content.len()) / 4) as i64),
            completion_tokens: Some((text.len() / 4) as i64),
            cost_usd: Some(estimate_usd(
                ((labels.len() + content.len()) / 4) as i64,
                (text.len() / 4) as i64,
            )),
        })
    }
}

pub struct OpenAiLlm {
    model: String,
    api_key: String,
}

impl OpenAiLlm {
    pub fn new(model: String, key_name: Option<&str>) -> Result<Self> {
        let api_key = resolve_provider_key("openai", key_name)?;
        Ok(Self { model, api_key })
    }
}

impl Llm for OpenAiLlm {
    fn provider(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn complete(&self, prompt: &str, content: &str) -> Result<Completion> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": format!("{prompt}\n\n{content}")
            }],
        });
        let resp = ureq::post("https://api.openai.com/v1/chat/completions")
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(body)
            .map_err(|err| Error::ai(err.to_string()))?;
        let value: serde_json::Value =
            resp.into_json().map_err(|err| Error::ai(err.to_string()))?;
        let text = value
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::ai("openai chat response missing content"))?
            .to_string();
        let usage = value.get("usage");
        let prompt_tokens = usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|v| v.as_i64());
        let completion_tokens = usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|v| v.as_i64());
        Ok(Completion {
            text,
            prompt_tokens,
            completion_tokens,
            cost_usd: Some(estimate_usd(
                prompt_tokens.unwrap_or(0),
                completion_tokens.unwrap_or(0),
            )),
        })
    }

    fn complete_streaming(
        &self,
        prompt: &str,
        content: &str,
        on_token: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Completion> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": format!("{prompt}\n\n{content}")
            }],
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        let resp = ureq::post("https://api.openai.com/v1/chat/completions")
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(body)
            .map_err(|err| Error::ai(err.to_string()))?;
        consume_openai_stream(BufReader::new(resp.into_reader()), on_token)
    }
}

fn sse_data_payload(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?.trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

fn fold_provider_stream(
    reader: impl BufRead,
    mut on_event: impl FnMut(serde_json::Value) -> Result<()>,
) -> Result<bool> {
    let mut raw = String::new();
    let mut saw_data = false;
    for line in reader.lines() {
        let line = line.map_err(|err| Error::ai(err.to_string()))?;
        if let Some(payload) = sse_data_payload(&line) {
            saw_data = true;
            if payload == "[DONE]" {
                break;
            }
            let value: serde_json::Value =
                serde_json::from_str(payload).map_err(|err| Error::ai(err.to_string()))?;
            on_event(value)?;
            continue;
        }
        if !saw_data {
            if !raw.is_empty() {
                raw.push('\n');
            }
            raw.push_str(&line);
        }
    }
    if !saw_data {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let value: serde_json::Value =
                serde_json::from_str(trimmed).map_err(|err| Error::ai(err.to_string()))?;
            on_event(value)?;
        }
    }
    Ok(saw_data)
}

fn stream_error(value: &serde_json::Value, fallback: &str) -> Result<()> {
    if let Some(err) = value.get("error") {
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or(fallback);
        return Err(Error::ai(message.to_string()));
    }
    if value.get("type").and_then(|v| v.as_str()) == Some("error") {
        let message = value
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or(fallback);
        return Err(Error::ai(message.to_string()));
    }
    Ok(())
}

fn openai_delta_text(value: &serde_json::Value) -> Option<&str> {
    value
        .pointer("/choices/0/delta/content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn openai_message_text(value: &serde_json::Value) -> Option<&str> {
    value
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn openai_usage_tokens(value: &serde_json::Value) -> Option<(Option<i64>, Option<i64>)> {
    let usage = value.get("usage")?;
    let prompt = usage.get("prompt_tokens").and_then(|v| v.as_i64());
    let completion = usage.get("completion_tokens").and_then(|v| v.as_i64());
    if prompt.is_none() && completion.is_none() {
        None
    } else {
        Some((prompt, completion))
    }
}

fn finish_stream_text(
    mut text: String,
    fallback: Option<String>,
    saw_data: bool,
    missing: &str,
    on_token: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    if text.is_empty() {
        if let Some(full) = fallback {
            if !full.is_empty() {
                on_token(&full)?;
            }
            text = full;
        } else if !saw_data {
            return Err(Error::ai(missing));
        }
    }
    Ok(text)
}

fn completion_from_usage(
    text: String,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
) -> Completion {
    Completion {
        text,
        prompt_tokens,
        completion_tokens,
        cost_usd: Some(estimate_usd(
            prompt_tokens.unwrap_or(0),
            completion_tokens.unwrap_or(0),
        )),
    }
}

fn consume_openai_stream(
    reader: impl BufRead,
    on_token: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Completion> {
    let mut text = String::new();
    let mut fallback = None;
    let mut prompt_tokens = None;
    let mut completion_tokens = None;
    let saw_data = fold_provider_stream(reader, |value| {
        stream_error(&value, "openai stream error")?;
        if let Some((prompt, completion)) = openai_usage_tokens(&value) {
            if prompt.is_some() {
                prompt_tokens = prompt;
            }
            if completion.is_some() {
                completion_tokens = completion;
            }
        }
        if let Some(delta) = openai_delta_text(&value) {
            text.push_str(delta);
            fallback = None;
            on_token(delta)?;
        } else if text.is_empty() {
            if let Some(full) = openai_message_text(&value) {
                fallback = Some(full.to_string());
            }
        }
        Ok(())
    })?;
    let text = finish_stream_text(
        text,
        fallback,
        saw_data,
        "openai chat response missing content",
        on_token,
    )?;
    Ok(completion_from_usage(
        text,
        prompt_tokens,
        completion_tokens,
    ))
}

fn anthropic_delta_text(value: &serde_json::Value) -> Option<&str> {
    if value.get("type").and_then(|v| v.as_str()) != Some("content_block_delta") {
        return None;
    }
    let delta = value.get("delta")?;
    if delta.get("type").and_then(|v| v.as_str()) != Some("text_delta") {
        return None;
    }
    delta
        .get("text")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn anthropic_message_text(value: &serde_json::Value) -> Option<&str> {
    value
        .pointer("/content/0/text")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            value
                .pointer("/message/content/0/text")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            if value.get("type").and_then(|v| v.as_str()) == Some("content_block_start") {
                value
                    .pointer("/content_block/text")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            } else {
                None
            }
        })
}

fn take_anthropic_usage(
    value: &serde_json::Value,
    prompt_tokens: &mut Option<i64>,
    completion_tokens: &mut Option<i64>,
) {
    for usage in [value.get("usage"), value.pointer("/message/usage")]
        .into_iter()
        .flatten()
    {
        if let Some(n) = usage.get("input_tokens").and_then(|v| v.as_i64()) {
            *prompt_tokens = Some(n);
        }
        if let Some(n) = usage.get("output_tokens").and_then(|v| v.as_i64()) {
            *completion_tokens = Some(n);
        }
    }
}

fn consume_anthropic_stream(
    reader: impl BufRead,
    on_token: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Completion> {
    let mut text = String::new();
    let mut fallback = None;
    let mut prompt_tokens = None;
    let mut completion_tokens = None;
    let saw_data = fold_provider_stream(reader, |value| {
        stream_error(&value, "anthropic stream error")?;
        take_anthropic_usage(&value, &mut prompt_tokens, &mut completion_tokens);
        if let Some(delta) = anthropic_delta_text(&value) {
            text.push_str(delta);
            fallback = None;
            on_token(delta)?;
        } else if text.is_empty() {
            if let Some(full) = anthropic_message_text(&value) {
                fallback = Some(full.to_string());
            }
        }
        Ok(())
    })?;
    let text = finish_stream_text(
        text,
        fallback,
        saw_data,
        "anthropic response missing content",
        on_token,
    )?;
    Ok(completion_from_usage(
        text,
        prompt_tokens,
        completion_tokens,
    ))
}

pub struct AnthropicLlm {
    model: String,
    api_key: String,
}

impl AnthropicLlm {
    pub fn new(model: String, key_name: Option<&str>) -> Result<Self> {
        let api_key = resolve_provider_key("anthropic", key_name)?;
        Ok(Self { model, api_key })
    }
}

impl Llm for AnthropicLlm {
    fn provider(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn complete(&self, prompt: &str, content: &str) -> Result<Completion> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 256,
            "messages": [{
                "role": "user",
                "content": format!("{prompt}\n\n{content}")
            }],
        });
        let resp = ureq::post("https://api.anthropic.com/v1/messages")
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", "2023-06-01")
            .send_json(body)
            .map_err(|err| Error::ai(err.to_string()))?;
        let value: serde_json::Value =
            resp.into_json().map_err(|err| Error::ai(err.to_string()))?;
        let text = value
            .pointer("/content/0/text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::ai("anthropic response missing content"))?
            .to_string();
        let usage = value.get("usage");
        let prompt_tokens = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|v| v.as_i64());
        let completion_tokens = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_i64());
        Ok(Completion {
            text,
            prompt_tokens,
            completion_tokens,
            cost_usd: Some(estimate_usd(
                prompt_tokens.unwrap_or(0),
                completion_tokens.unwrap_or(0),
            )),
        })
    }

    fn complete_streaming(
        &self,
        prompt: &str,
        content: &str,
        on_token: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Completion> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 256,
            "stream": true,
            "messages": [{
                "role": "user",
                "content": format!("{prompt}\n\n{content}")
            }],
        });
        let resp = ureq::post("https://api.anthropic.com/v1/messages")
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", "2023-06-01")
            .send_json(body)
            .map_err(|err| Error::ai(err.to_string()))?;
        consume_anthropic_stream(BufReader::new(resp.into_reader()), on_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_classify_picks_a_label() {
        let llm = FakeLlm {
            model: "aidb-fake".into(),
        };
        let out = llm
            .classify(
                "positive or negative",
                "This refund was a negative surprise.",
            )
            .expect("classify");
        assert_eq!(out.text, "negative");
        assert_eq!(
            pick_label("refund / shipping", "How do refunds work?"),
            "refund"
        );
    }

    #[test]
    fn streaming_chunks_concatenate_to_the_original_text() {
        let text = "Refunds are issued within 14 days of purchase.";
        let mut parts = Vec::new();
        emit_text_chunks(text, &mut |chunk| {
            parts.push(chunk.to_string());
            Ok(())
        })
        .expect("chunks");
        assert!(parts.len() > 1, "{parts:?}");
        assert_eq!(parts.concat(), text);
    }

    #[test]
    fn openai_sse_deltas_concatenate_to_the_completion_text() {
        let chunk = |text: &str| {
            serde_json::json!({
                "choices": [{ "delta": { "content": text } }]
            })
            .to_string()
        };
        let usage = serde_json::json!({
            "choices": [],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        })
        .to_string();
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n",
            serde_json::json!({ "choices": [{ "delta": { "role": "assistant" } }] }),
            chunk("Hello"),
            chunk(" world"),
            usage
        );
        let mut parts = Vec::new();
        let out = consume_openai_stream(body.as_bytes(), &mut |delta| {
            parts.push(delta.to_string());
            Ok(())
        })
        .expect("sse");
        assert_eq!(parts, vec!["Hello".to_string(), " world".to_string()]);
        assert_eq!(parts.concat(), out.text);
        assert_eq!(out.text, "Hello world");
        assert_eq!(out.prompt_tokens, Some(4));
        assert_eq!(out.completion_tokens, Some(2));
        assert_eq!(out.cost_usd, Some(estimate_usd(4, 2)));
    }

    #[test]
    fn openai_complete_body_without_deltas_emits_one_token() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "one shot" } }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 2 }
        })
        .to_string();
        let mut parts = Vec::new();
        let out = consume_openai_stream(body.as_bytes(), &mut |delta| {
            parts.push(delta.to_string());
            Ok(())
        })
        .expect("complete body");
        assert_eq!(parts, vec!["one shot".to_string()]);
        assert_eq!(out.text, "one shot");
        assert_eq!(out.prompt_tokens, Some(1));
        assert_eq!(out.completion_tokens, Some(2));
    }

    #[test]
    fn anthropic_sse_text_deltas_concatenate_to_the_completion_text() {
        let delta = |text: &str| {
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": text }
            })
            .to_string()
        };
        let body = format!(
            "event: message_start\ndata: {}\n\n\
             event: content_block_delta\ndata: {}\n\n\
             event: content_block_delta\ndata: {}\n\n\
             event: message_delta\ndata: {}\n\n\
             event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n",
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "content": [],
                    "usage": { "input_tokens": 8, "output_tokens": 1 }
                }
            }),
            delta("Hello"),
            delta(" world"),
            serde_json::json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": { "output_tokens": 2 }
            })
        );
        let mut parts = Vec::new();
        let out = consume_anthropic_stream(body.as_bytes(), &mut |chunk| {
            parts.push(chunk.to_string());
            Ok(())
        })
        .expect("sse");
        assert_eq!(parts.concat(), "Hello world");
        assert_eq!(out.text, "Hello world");
        assert_eq!(out.prompt_tokens, Some(8));
        assert_eq!(out.completion_tokens, Some(2));
        assert_eq!(out.cost_usd, Some(estimate_usd(8, 2)));
    }

    #[test]
    fn fake_complete_fills_a_schema_from_the_prompt_marker() {
        let llm = FakeLlm {
            model: "aidb-fake".into(),
        };
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "summary": { "type": "string" } },
            "required": ["summary"]
        });
        let prompt = format!("Extract{JSON_SCHEMA_MARK}{schema}");
        let out = llm
            .complete(&prompt, "Refunds take 14 days.")
            .expect("complete");
        let value: serde_json::Value = serde_json::from_str(&out.text).expect("json");
        assert!(
            value["summary"].as_str().unwrap().contains("Refunds"),
            "{value}"
        );
    }

    #[test]
    fn fill_schema_omits_a_required_const_the_content_does_not_support() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "nonce": { "const": "UNSAT" } },
            "required": ["nonce"]
        });
        let filled = fill_schema(&schema, "Refunds take 14 days.");
        assert!(filled.get("nonce").is_none(), "{filled}");
    }

    #[test]
    fn fill_decide_picks_the_next_unused_op_and_a_ticker_filter() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "enum": ["search", "generate", "send.email", "stop"] },
                "args": { "type": "object" }
            },
            "required": ["op"]
        });
        let first = fill_schema(&schema, "Goal: Morning digest for NVDA\nLast:\n\nTaken:");
        assert_eq!(first["op"], "search");
        assert_eq!(first["args"]["filter"]["ticker"], "NVDA");
        let after_search = fill_schema(&schema, "Goal: Morning digest for NVDA\nTaken: search");
        assert_eq!(after_search["op"], "generate");
        let stop = fill_schema(
            &schema,
            "Goal: Morning digest for NVDA\nTaken: search, generate, send.email",
        );
        assert_eq!(stop["op"], "stop");
        let email = fill_schema(
            &schema,
            "Goal: Morning digest for NVDA\nLast:\nThe brief.\nTaken: search, generate",
        );
        assert_eq!(email["op"], "send.email");
        assert_eq!(email["args"]["to"], "desk@local");
        assert!(email["args"]["body"].as_str().unwrap().contains("brief"));
    }

    #[test]
    fn local_and_custom_are_known_embed_providers() {
        assert!(known_embed_provider("local"));
        assert!(known_embed_provider("custom"));
        assert!(!known_embed_provider("hosted-mystery"));
        let local = embedder(&EmbedderConfig {
            provider: "local".into(),
            model: "bge-small".into(),
            dimensions: 384,
            key_name: None,
        })
        .expect("local");
        assert_eq!(local.provider(), "local");
        assert_eq!(local.model(), "BAAI/bge-small-en-v1.5");
        assert_eq!(local.dimensions(), 384);
    }

    #[test]
    fn anthropic_is_a_known_provider() {
        assert!(known_provider("anthropic"));
        assert!(!known_provider("hosted-mystery"));
        match llm_with_key(
            "anthropic",
            "claude-sonnet-4-20250514",
            Some("AIDB_MISSING_PHASE25"),
        ) {
            Ok(_) => panic!("custom key name must resolve"),
            Err(err) => assert!(
                err.to_string().contains("AIDB_MISSING_PHASE25 is not set"),
                "{err}"
            ),
        }
    }

    #[test]
    fn file_store_constructs_openai_without_env() {
        let _guard = secrets::test_env_lock();
        let dir = std::env::temp_dir().join(format!(
            "aidb-ai-openai-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.env");
        std::fs::write(&path, "AIDB_FILE_OPENAI=sk-test-not-used\n").unwrap();
        let prev = std::env::var("AIDB_SECRET_STORE").ok();
        unsafe {
            std::env::remove_var("AIDB_FILE_OPENAI");
            std::env::set_var("AIDB_SECRET_STORE", format!("file:{}", path.display()));
        }
        let client = llm_with_key("openai", "gpt-4.1-mini", Some("AIDB_FILE_OPENAI"));
        match prev {
            Some(value) => unsafe { std::env::set_var("AIDB_SECRET_STORE", value) },
            None => unsafe { std::env::remove_var("AIDB_SECRET_STORE") },
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(client.is_ok(), "{}", client.as_ref().err().unwrap());
    }
}
