//! C ABI over `aidb`. Bindings are faces; they do not own storage or runs.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint};
use std::ptr;
use std::time::Duration;

use aidb::{open, open_with, Aidb, EmbedderConfig, QueryResult, Value};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_error(message: impl std::fmt::Display) {
    let text = CString::new(message.to_string().replace('\0', "")).ok();
    LAST_ERROR.with(|slot| *slot.borrow_mut() = text);
}

fn clear_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

fn c_str<'a>(ptr: *const c_char) -> Result<&'a str, ()> {
    if ptr.is_null() {
        set_error("null pointer");
        return Err(());
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().map_err(set_error)
}

fn to_cstring(text: String) -> *mut c_char {
    CString::new(text.replace('\0', ""))
        .map(CString::into_raw)
        .unwrap_or(ptr::null_mut())
}

fn result_json(result: &QueryResult) -> String {
    let rows: Vec<Vec<serde_json::Value>> = result
        .rows
        .iter()
        .map(|row| row.iter().map(json_value).collect())
        .collect();
    serde_json::json!({
        "ok": true,
        "columns": result.columns,
        "rows": rows,
    })
    .to_string()
}

fn json_value(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Integer(v) => serde_json::json!(v),
        Value::Real(v) => serde_json::json!(v),
        Value::Text(v) => serde_json::json!(v),
        Value::Blob(v) => serde_json::json!(format!("X'{}'", hex(v))),
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

fn maybe_drain(db: &Aidb, sql: &str) {
    let lower = sql.to_ascii_lowercase();
    if lower.contains("insert") || lower.contains("aidb_insert_document") {
        let _ = db.drain_index(Duration::from_secs(60));
    }
}

/// Open `path`. Returns an opaque handle, or null on error.
///
/// # Safety
/// `path` may be null (returns null). Non-null must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn aidb_open(path: *const c_char) -> *mut Aidb {
    clear_error();
    let Ok(path) = c_str(path) else {
        return ptr::null_mut();
    };
    match open(path) {
        Ok(db) => Box::into_raw(Box::new(db)),
        Err(err) => {
            set_error(err);
            ptr::null_mut()
        }
    }
}

/// Open with an explicit embedding space. `provider`/`model` may be null for defaults.
///
/// # Safety
/// `path` may be null (returns null). Non-null `path`/`provider`/`model` must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn aidb_open_with(
    path: *const c_char,
    provider: *const c_char,
    model: *const c_char,
    dimensions: c_uint,
) -> *mut Aidb {
    clear_error();
    let Ok(path) = c_str(path) else {
        return ptr::null_mut();
    };
    let mut config = EmbedderConfig::default();
    if let Ok(provider) = c_str(provider) {
        if !provider.is_empty() {
            config.provider = provider.to_string();
        }
    }
    if let Ok(model) = c_str(model) {
        if !model.is_empty() {
            config.model = model.to_string();
        }
    }
    if dimensions > 0 {
        config.dimensions = dimensions as usize;
    }
    match open_with(path, config) {
        Ok(db) => Box::into_raw(Box::new(db)),
        Err(err) => {
            set_error(err);
            ptr::null_mut()
        }
    }
}

/// Run SQL. SELECT-like statements return JSON `{ok,columns,rows}`. Writes return `{ok,changed}`.
///
/// # Safety
/// `db`/`sql` may be null (returns null). Non-null `db` must be a previous `aidb_open` handle;
/// non-null `sql` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn aidb_sql(db: *mut Aidb, sql: *const c_char) -> *mut c_char {
    clear_error();
    if db.is_null() {
        set_error("null database");
        return ptr::null_mut();
    }
    let Ok(sql) = c_str(sql) else {
        return ptr::null_mut();
    };
    let db = unsafe { &*db };
    let trimmed = sql.trim_start();
    let is_query = starts_query(trimmed);
    if is_query {
        match db.query(sql) {
            Ok(result) => {
                maybe_drain(db, sql);
                to_cstring(result_json(&result))
            }
            Err(err) => {
                set_error(err);
                ptr::null_mut()
            }
        }
    } else {
        match db.execute(sql) {
            Ok(changed) => {
                maybe_drain(db, sql);
                to_cstring(serde_json::json!({ "ok": true, "changed": changed }).to_string())
            }
            Err(err) => {
                set_error(err);
                ptr::null_mut()
            }
        }
    }
}

/// Drain the index worker. Returns 0 on success, 1 on error.
///
/// # Safety
/// `db` may be null (returns 1). Non-null must be a previous `aidb_open` handle.
#[no_mangle]
pub unsafe extern "C" fn aidb_drain(db: *mut Aidb, timeout_ms: c_uint) -> c_int {
    clear_error();
    if db.is_null() {
        set_error("null database");
        return 1;
    }
    let db = unsafe { &*db };
    match db.drain_index(Duration::from_millis(timeout_ms as u64)) {
        Ok(()) => 0,
        Err(err) => {
            set_error(err);
            1
        }
    }
}

/// Close a handle from `aidb_open`. Null is a no-op.
///
/// # Safety
/// Non-null `db` must be a previous `aidb_open` handle that has not already been closed.
#[no_mangle]
pub unsafe extern "C" fn aidb_close(db: *mut Aidb) {
    if !db.is_null() {
        unsafe {
            drop(Box::from_raw(db));
        }
    }
}

#[no_mangle]
pub extern "C" fn aidb_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| match slot.borrow().as_ref() {
        Some(text) => text.as_ptr(),
        None => ptr::null(),
    })
}

/// Free a string returned by `aidb_sql`. Null is a no-op.
///
/// # Safety
/// Non-null `ptr` must be a string returned by `aidb_sql` that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn aidb_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            drop(CString::from_raw(ptr));
        }
    }
}

fn starts_query(sql: &str) -> bool {
    ["select", "pragma", "with", "explain"].iter().any(|kw| {
        sql.get(..kw.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(kw))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn ffi_open_query_close() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("aidb-ffi-{nanos}.db"));
        let c_path = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let db = unsafe { aidb_open(c_path.as_ptr()) };
        assert!(!db.is_null(), "{:?}", unsafe {
            CStr::from_ptr(aidb_last_error()).to_string_lossy()
        });
        let sql = CString::new("SELECT value FROM aidb_meta WHERE key = 'schema_version'").unwrap();
        let out = unsafe { aidb_sql(db, sql.as_ptr()) };
        assert!(!out.is_null());
        let json = unsafe { CStr::from_ptr(out) }.to_string_lossy();
        assert!(
            json.contains(&format!("\"{}\"", aidb::SCHEMA_VERSION)),
            "{json}"
        );
        unsafe { aidb_string_free(out) };
        unsafe { aidb_close(db) };
        let _ = std::fs::remove_file(&path);
    }
}
