//! In-process Node addon. The TypeScript face loads this; it does not spawn `aidb sql`.

use std::sync::{Arc, Mutex};

use aidb::{
    open, open_with, query_to_json, subscribe_tokens as engine_subscribe_tokens, Aidb,
    EmbedderConfig, SqlOutput,
};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::JsFunction;
use napi_derive::napi;

#[napi]
pub const RUNTIME: &str = "napi";

#[napi(object)]
pub struct EmbeddingOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub dimensions: Option<u32>,
}

#[napi]
pub struct Database {
    inner: Arc<Mutex<Option<Aidb>>>,
    path: String,
}

#[napi]
impl Database {
    #[napi(getter)]
    pub fn path(&self) -> String {
        self.path.clone()
    }

    #[napi]
    pub async fn query(&self, sql: String) -> Result<serde_json::Value> {
        let inner = Arc::clone(&self.inner);
        napi::tokio::task::spawn_blocking(move || {
            let guard = inner
                .lock()
                .map_err(|_| Error::from_reason("database lock poisoned"))?;
            let db = guard
                .as_ref()
                .ok_or_else(|| Error::from_reason("database is closed"))?;
            match db
                .sql(&sql)
                .map_err(|err| Error::from_reason(err.to_string()))?
            {
                SqlOutput::Query(result) => Ok(query_to_json(&result)),
                SqlOutput::Execute(_) => Err(Error::from_reason("expected a query")),
            }
        })
        .await
        .map_err(|err| Error::from_reason(err.to_string()))?
    }

    #[napi]
    pub fn execute(&self, sql: String) -> Result<i64> {
        match self.with_db(|db| db.sql(&sql))? {
            SqlOutput::Execute(changed) => Ok(changed as i64),
            SqlOutput::Query(_) => Ok(0),
        }
    }

    #[napi]
    pub fn close(&self) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("database lock poisoned"))?;
        *guard = None;
        Ok(())
    }

    fn with_db<T>(&self, f: impl FnOnce(&Aidb) -> aidb::Result<T>) -> Result<T> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("database lock poisoned"))?;
        let db = guard
            .as_ref()
            .ok_or_else(|| Error::from_reason("database is closed"))?;
        f(db).map_err(|err| Error::from_reason(err.to_string()))
    }
}

#[napi]
pub fn open_db(path: String, embedding: Option<EmbeddingOptions>) -> Result<Database> {
    let db = match embedding {
        Some(opts) => {
            let mut config = EmbedderConfig::default();
            if let Some(provider) = opts.provider.filter(|s| !s.is_empty()) {
                config.provider = provider;
            }
            if let Some(model) = opts.model.filter(|s| !s.is_empty()) {
                config.model = model;
            }
            if let Some(dimensions) = opts.dimensions.filter(|d| *d > 0) {
                config.dimensions = dimensions as usize;
            }
            open_with(&path, config)
        }
        None => open(&path),
    }
    .map_err(|err| Error::from_reason(err.to_string()))?;
    Ok(Database {
        inner: Arc::new(Mutex::new(Some(db))),
        path,
    })
}

/// Live `run_events` tokens. Same events `aidb serve` publishes on `/ws`.
#[napi]
pub fn subscribe_tokens(callback: JsFunction) -> Result<()> {
    let tsfn: ThreadsafeFunction<serde_json::Value, ErrorStrategy::Fatal> =
        callback.create_threadsafe_function(0, |ctx| Ok(vec![ctx.value]))?;
    engine_subscribe_tokens(Arc::new(move |event| {
        let payload = serde_json::json!({
            "run_id": event.run_id,
            "seq": event.seq,
            "text": event.text,
        });
        let _ = tsfn.call(payload, ThreadsafeFunctionCallMode::NonBlocking);
    }));
    Ok(())
}
