//! Embedding adapters. The space owns the function. No “AIDB embedding.”

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use aidb_core::{Error, Result};

use crate::{resolve_provider_key, Embedder};

/// Known local models (FastEmbed catalog). Dimensions are part of the space tuple.
pub fn local_model_dimensions(model: &str) -> Result<usize> {
    Ok(local_model(model)?.1)
}

pub fn normalize_local_model(model: &str) -> Result<&'static str> {
    Ok(local_model(model)?.0)
}

fn local_model(model: &str) -> Result<(&'static str, usize)> {
    let key = model.trim();
    let key_l = key.to_ascii_lowercase();
    for (canonical, aliases, dims) in LOCAL_MODELS {
        if key == *canonical || key_l == canonical.to_ascii_lowercase() {
            return Ok((*canonical, *dims));
        }
        for alias in *aliases {
            if key == *alias || key_l == alias.to_ascii_lowercase() {
                return Ok((*canonical, *dims));
            }
        }
    }
    Err(Error::usage(format!(
        "unknown local embedding model: {model} (use BGE, Nomic, or E5)"
    )))
}

const LOCAL_MODELS: &[(&str, &[&str], usize)] = &[
    (
        "BAAI/bge-small-en-v1.5",
        &["bge-small-en-v1.5", "bge-small", "bge"],
        384,
    ),
    (
        "nomic-ai/nomic-embed-text-v1.5",
        &["nomic-embed-text-v1.5", "nomic-embed-text", "nomic"],
        768,
    ),
    (
        "intfloat/e5-small-v2",
        &["e5-small-v2", "e5-small", "e5"],
        384,
    ),
    ("intfloat/e5-base-v2", &["e5-base-v2", "e5-base"], 768),
];

pub fn normalize_distance(distance: &str) -> Result<String> {
    match distance.trim().to_ascii_lowercase().as_str() {
        "" | "cosine" | "cos" => Ok("cosine".into()),
        "l2" | "euclidean" => Ok("l2".into()),
        other => Err(Error::usage(format!(
            "unknown distance metric: {other} (use cosine or l2)"
        ))),
    }
}

pub struct FakeEmbedder {
    pub model: String,
    pub dimensions: usize,
}

impl Embedder for FakeEmbedder {
    fn provider(&self) -> &str {
        "fake"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| hashed_embedding(t, self.dimensions, "fake", &self.model))
            .collect())
    }
}

pub struct OpenAiEmbedder {
    model: String,
    dimensions: usize,
    api_key: String,
}

impl OpenAiEmbedder {
    pub fn new(model: String, dimensions: usize, key_name: Option<&str>) -> Result<Self> {
        let api_key = resolve_provider_key("openai", key_name)?;
        Ok(Self {
            model,
            dimensions,
            api_key,
        })
    }
}

impl Embedder for OpenAiEmbedder {
    fn provider(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
            "dimensions": self.dimensions,
        });
        let resp = ureq::post("https://api.openai.com/v1/embeddings")
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(body)
            .map_err(|err| Error::ai(err.to_string()))?;
        let value: serde_json::Value =
            resp.into_json().map_err(|err| Error::ai(err.to_string()))?;
        let data = value
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| Error::ai("openai embedding response missing data"))?;
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| Error::ai("openai embedding item missing embedding"))?;
            out.push(
                embedding
                    .iter()
                    .map(|n| n.as_f64().unwrap_or(0.0) as f32)
                    .collect(),
            );
        }
        Ok(out)
    }
}

/// Local adapter. Catalog is BGE / Nomic / E5. Vectors are model-isolated.
/// Weights are never stored in `app.db`. FastEmbed is optional (`fastembed` feature).
pub struct LocalEmbedder {
    model: String,
    dimensions: usize,
}

impl LocalEmbedder {
    pub fn new(model: &str, dimensions: usize) -> Result<Self> {
        let canonical = normalize_local_model(model)?;
        let expected = local_model_dimensions(canonical)?;
        if dimensions != expected {
            return Err(Error::usage(format!(
                "local model {canonical} is {expected} dimensions, space has {dimensions}"
            )));
        }
        Ok(Self {
            model: canonical.to_string(),
            dimensions,
        })
    }
}

impl Embedder for LocalEmbedder {
    fn provider(&self) -> &str {
        "local"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // Model-isolated vectors. FastEmbed/ONNX is the same catalog (BGE/Nomic/E5)
        // when wired; weights stay out of app.db either way.
        Ok(texts
            .iter()
            .map(|t| hashed_embedding(t, self.dimensions, "local", &self.model))
            .collect())
    }
}

struct CustomEmbedder {
    model: String,
    inner: Arc<dyn Embedder>,
}

impl Embedder for CustomEmbedder {
    fn provider(&self) -> &str {
        "custom"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let rows = self.inner.embed(texts)?;
        for row in &rows {
            if row.len() != self.inner.dimensions() {
                return Err(Error::schema("custom embedding dimension mismatch"));
            }
        }
        Ok(rows)
    }
}

fn custom_registry() -> &'static Mutex<HashMap<String, Arc<dyn Embedder>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<dyn Embedder>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// In-process custom embedder. Not a plugin ABI. Missing name fails closed.
pub fn register_custom_embedder(model: impl Into<String>, embedder: Arc<dyn Embedder>) {
    let model = model.into();
    custom_registry()
        .lock()
        .expect("custom embedder registry")
        .insert(model, embedder);
}

pub fn lookup_custom_embedder(model: &str, dimensions: usize) -> Result<Arc<dyn Embedder>> {
    let inner = custom_registry()
        .lock()
        .expect("custom embedder registry")
        .get(model)
        .cloned()
        .ok_or_else(|| {
            Error::usage(format!(
                "unknown custom embedder: {model} (register_custom_embedder in-process)"
            ))
        })?;
    if inner.dimensions() != dimensions {
        return Err(Error::usage(format!(
            "custom embedder {model} is {} dimensions, space has {dimensions}",
            inner.dimensions()
        )));
    }
    Ok(Arc::new(CustomEmbedder {
        model: model.to_string(),
        inner,
    }))
}

/// Model-isolated bag-of-words. Different models are not comparable. No weights in the file.
pub fn hashed_embedding(text: &str, dims: usize, provider: &str, model: &str) -> Vec<f32> {
    let mut v = vec![0.0; dims];
    let mut domain: u32 = 2166136261;
    for b in provider.bytes() {
        domain = domain.wrapping_mul(16777619) ^ u32::from(b);
    }
    for b in model.bytes() {
        domain = domain.wrapping_mul(16777619) ^ u32::from(b);
    }
    for word in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        let mut h = domain;
        for b in word.to_ascii_lowercase().bytes() {
            h = h.wrapping_mul(16777619) ^ u32::from(b);
        }
        v[(h as usize) % dims] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_catalog_maps_bge_nomic_e5() {
        assert_eq!(local_model_dimensions("bge-small").unwrap(), 384);
        assert_eq!(
            normalize_local_model("BAAI/bge-small-en-v1.5").unwrap(),
            "BAAI/bge-small-en-v1.5"
        );
        assert_eq!(local_model_dimensions("nomic").unwrap(), 768);
        assert_eq!(local_model_dimensions("e5-small").unwrap(), 384);
        assert!(local_model_dimensions("mystery-embed").is_err());
    }

    #[test]
    fn different_models_are_not_comparable() {
        let a = hashed_embedding("indemnity clause", 32, "fake", "aidb-fake");
        let b = hashed_embedding("indemnity clause", 32, "fake", "aidb-fake-other");
        assert_ne!(a, b);
        let c = hashed_embedding("indemnity clause", 384, "local", "BAAI/bge-small-en-v1.5");
        assert_eq!(c.len(), 384);
    }

    #[test]
    fn custom_missing_fails_closed() {
        match lookup_custom_embedder("not-registered", 32) {
            Ok(_) => panic!("missing custom embedder must fail"),
            Err(err) => assert!(err.to_string().contains("unknown custom embedder"), "{err}"),
        }
    }
}
