//! In-process Node addon. The TypeScript face loads this; it does not spawn `aidb sql`.

use std::sync::Mutex;

use aidb::{open, open_with, query_to_json, Aidb, EmbedderConfig, SqlOutput};
use napi::bindgen_prelude::*;
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
    inner: Mutex<Option<Aidb>>,
    path: String,
}

#[napi]
impl Database {
    #[napi(getter)]
    pub fn path(&self) -> String {
        self.path.clone()
    }

    #[napi]
    pub fn query(&self, sql: String) -> Result<serde_json::Value> {
        match self.with_db(|db| db.sql(&sql))? {
            SqlOutput::Query(result) => Ok(query_to_json(&result)),
            SqlOutput::Execute(_) => Err(Error::from_reason("expected a query")),
        }
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
        inner: Mutex::new(Some(db)),
        path,
    })
}
