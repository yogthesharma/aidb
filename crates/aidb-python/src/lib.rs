//! In-process CPython module. The Python face imports this; it does not use ctypes.

use aidb::{open, open_with, query_to_json, Aidb, EmbedderConfig, SqlOutput};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pyclass]
pub struct Database {
    inner: Option<Aidb>,
    path: String,
}

#[pymethods]
impl Database {
    #[getter]
    fn path(&self) -> &str {
        &self.path
    }

    fn query(&self, py: Python<'_>, sql: &str) -> PyResult<Py<PyAny>> {
        let db = self
            .inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("database is closed"))?;
        match db.sql(sql).map_err(py_err)? {
            SqlOutput::Query(result) => loads_json(py, &query_to_json(&result).to_string()),
            SqlOutput::Execute(changed) => {
                let dict = PyDict::new(py);
                dict.set_item("changed", changed)?;
                Ok(dict.into())
            }
        }
    }

    fn execute(&self, sql: &str) -> PyResult<u64> {
        let db = self
            .inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("database is closed"))?;
        match db.sql(sql).map_err(py_err)? {
            SqlOutput::Execute(changed) => Ok(changed),
            SqlOutput::Query(_) => Ok(0),
        }
    }

    fn close(&mut self) {
        self.inner = None;
    }
}

#[pyfunction]
#[pyo3(signature = (path, provider=None, model=None, dimensions=None))]
fn open_db(
    path: &str,
    provider: Option<&str>,
    model: Option<&str>,
    dimensions: Option<u32>,
) -> PyResult<Database> {
    let db = if provider.is_some() || model.is_some() || dimensions.is_some() {
        let mut config = EmbedderConfig::default();
        if let Some(provider) = provider.filter(|s| !s.is_empty()) {
            config.provider = provider.to_string();
        }
        if let Some(model) = model.filter(|s| !s.is_empty()) {
            config.model = model.to_string();
        }
        if let Some(dimensions) = dimensions.filter(|d| *d > 0) {
            config.dimensions = dimensions as usize;
        }
        open_with(path, config)
    } else {
        open(path)
    }
    .map_err(py_err)?;
    Ok(Database {
        inner: Some(db),
        path: path.to_string(),
    })
}

fn py_err(err: aidb::Error) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

fn loads_json(py: Python<'_>, text: &str) -> PyResult<Py<PyAny>> {
    Ok(py.import("json")?.call_method1("loads", (text,))?.unbind())
}

#[pymodule]
fn aidb_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Database>()?;
    m.add_function(wrap_pyfunction!(open_db, m)?)?;
    m.add("RUNTIME", "pyo3")?;
    Ok(())
}
