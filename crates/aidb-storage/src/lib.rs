//! File format and connections: open, pragmas, migrate, execute, query.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::OpenFlags;

use aidb_core::{Error, QueryResult, Result, Value, SCHEMA_VERSION};

pub use rusqlite::Connection;

thread_local! {
    static CURRENT_CONN: Cell<*const Connection> = const { Cell::new(std::ptr::null()) };
}

pub fn current_connection() -> Result<&'static Connection> {
    CURRENT_CONN.with(|cell| {
        let ptr = cell.get();
        if ptr.is_null() {
            Err(Error::usage(
                "SQL function ran without an active connection",
            ))
        } else {
            Ok(unsafe { &*ptr })
        }
    })
}

fn with_current_conn<T>(conn: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            CURRENT_CONN.with(|cell| cell.set(std::ptr::null()));
        }
    }
    CURRENT_CONN.with(|cell| cell.set(conn as *const Connection));
    let _clear = Clear;
    f()
}

#[allow(clippy::missing_transmute_annotations, unnecessary_transmutes)]
fn register_sqlite_vec() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

const SCHEMA_SQL: &str = include_str!("../../../schema/v001.sql");
const SCHEMA_V002_SQL: &str = include_str!("../../../schema/v002.sql");
const SCHEMA_V003_SQL: &str = include_str!("../../../schema/v003.sql");
const SCHEMA_V004_SQL: &str = include_str!("../../../schema/v004.sql");
const SCHEMA_V005_SQL: &str = include_str!("../../../schema/v005.sql");
const SCHEMA_V006_SQL: &str = include_str!("../../../schema/v006.sql");
const SCHEMA_V007_SQL: &str = include_str!("../../../schema/v007.sql");
const SCHEMA_V008_SQL: &str = include_str!("../../../schema/v008.sql");
const SCHEMA_V009_SQL: &str = include_str!("../../../schema/v009.sql");

pub struct Store {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        register_sqlite_vec();
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                    path: Some(parent.to_path_buf()),
                    source,
                })?;
            }
        }

        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )
        .map_err(|err| Error::sqlite(format!("{}: {err}", path.display())))?;

        apply_pragmas(&conn)?;
        migrate(&conn)?;

        Ok(Self {
            path,
            conn: Mutex::new(conn),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn execute(&self, sql: &str) -> Result<u64> {
        let conn = lock(&self.conn)?;
        with_current_conn(&conn, || {
            conn.execute_batch(sql).map_err(sqlite_err)?;
            Ok(conn.changes())
        })
    }

    pub fn query(&self, sql: &str) -> Result<QueryResult> {
        let conn = lock(&self.conn)?;
        with_current_conn(&conn, || {
            let mut stmt = conn.prepare(sql).map_err(sqlite_err)?;
            let columns = stmt
                .column_names()
                .into_iter()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();

            let mut rows = Vec::new();
            let mut raw = stmt.query([]).map_err(sqlite_err)?;
            while let Some(row) = raw.next().map_err(sqlite_err)? {
                let mut values = Vec::with_capacity(columns.len());
                for i in 0..columns.len() {
                    values.push(row_value(row, i)?);
                }
                rows.push(values);
            }

            Ok(QueryResult { columns, rows })
        })
    }

    pub fn journal_mode(&self) -> Result<String> {
        let conn = lock(&self.conn)?;
        conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(sqlite_err)
    }

    pub fn write<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = lock(&self.conn)?;
        with_current_conn(&conn, || f(&conn))
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        self.write(|conn| meta(conn, key))
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.write(|conn| {
            conn.execute(
                "INSERT INTO aidb_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .map_err(sqlite_err)?;
            Ok(())
        })
    }
}

fn lock(conn: &Mutex<Connection>) -> Result<std::sync::MutexGuard<'_, Connection>> {
    conn.lock()
        .map_err(|_| Error::usage("writer lock poisoned"))
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(sqlite_err)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_err)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_err)?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(sqlite_err)?;
    conn.pragma_update(None, "temp_store", "MEMORY")
        .map_err(sqlite_err)?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_SQL).map_err(sqlite_err)?;

    let mut version = meta(conn, "schema_version")?
        .unwrap_or_else(|| "0".to_string())
        .parse::<u32>()
        .map_err(|_| Error::schema("aidb_meta.schema_version is not an integer"))?;

    if version > SCHEMA_VERSION {
        return Err(Error::schema(format!(
            "database schema {version} is newer than this engine ({SCHEMA_VERSION})"
        )));
    }
    if version < 1 {
        return Err(Error::schema(format!(
            "cannot migrate schema {version} to {SCHEMA_VERSION}"
        )));
    }
    if version < 2 {
        conn.execute_batch(SCHEMA_V002_SQL).map_err(sqlite_err)?;
        set_schema_version(conn, 2)?;
        version = 2;
    }
    if version < 3 {
        conn.execute_batch(SCHEMA_V003_SQL).map_err(sqlite_err)?;
        set_schema_version(conn, 3)?;
        version = 3;
    }
    if version < 4 {
        conn.execute_batch(SCHEMA_V004_SQL).map_err(sqlite_err)?;
        set_schema_version(conn, 4)?;
        version = 4;
    }
    if version < 5 {
        conn.execute_batch(SCHEMA_V005_SQL).map_err(sqlite_err)?;
        set_schema_version(conn, 5)?;
        version = 5;
    }
    if version < 6 {
        conn.execute_batch(SCHEMA_V006_SQL).map_err(sqlite_err)?;
        set_schema_version(conn, 6)?;
        version = 6;
    }
    if version < 7 {
        conn.execute_batch(SCHEMA_V007_SQL).map_err(sqlite_err)?;
        set_schema_version(conn, 7)?;
        version = 7;
    }
    if version < 8 {
        conn.execute_batch(SCHEMA_V008_SQL).map_err(sqlite_err)?;
        set_schema_version(conn, 8)?;
        version = 8;
    }
    if version < 9 {
        conn.execute_batch(SCHEMA_V009_SQL).map_err(sqlite_err)?;
        set_schema_version(conn, 9)?;
        version = 9;
    }
    if version < SCHEMA_VERSION {
        return Err(Error::schema(format!(
            "cannot migrate schema {version} to {SCHEMA_VERSION}"
        )));
    }

    if meta(conn, "created_at_ms")?.is_none() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        conn.execute(
            "INSERT INTO aidb_meta (key, value) VALUES ('created_at_ms', ?1)",
            [now.to_string()],
        )
        .map_err(sqlite_err)?;
    }

    Ok(())
}

fn set_schema_version(conn: &Connection, version: u32) -> Result<()> {
    conn.execute(
        "INSERT INTO aidb_meta (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [version.to_string()],
    )
    .map_err(sqlite_err)?;
    Ok(())
}

fn meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn
        .prepare("SELECT value FROM aidb_meta WHERE key = ?1")
        .map_err(sqlite_err)?;
    let mut rows = stmt.query([key]).map_err(sqlite_err)?;
    match rows.next().map_err(sqlite_err)? {
        Some(row) => Ok(Some(row.get(0).map_err(sqlite_err)?)),
        None => Ok(None),
    }
}

fn row_value(row: &rusqlite::Row<'_>, idx: usize) -> Result<Value> {
    let value = row.get_ref(idx).map_err(sqlite_err)?;
    Ok(match value {
        rusqlite::types::ValueRef::Null => Value::Null,
        rusqlite::types::ValueRef::Integer(v) => Value::Integer(v),
        rusqlite::types::ValueRef::Real(v) => Value::Real(v),
        rusqlite::types::ValueRef::Text(v) => Value::Text(String::from_utf8_lossy(v).into_owned()),
        rusqlite::types::ValueRef::Blob(v) => Value::Blob(v.to_vec()),
    })
}

pub fn sqlite_err(err: rusqlite::Error) -> Error {
    Error::sqlite(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("aidb-storage-{nanos}.db"))
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn upgrades_v001_runs_check_to_hitl_statuses() {
        let path = temp_db();
        {
            let conn = rusqlite::Connection::open(&path).expect("create");
            conn.execute_batch(SCHEMA_SQL).expect("v001");
            conn.execute(
                "INSERT INTO runs (id, kind, status, created_at_ms)
                 VALUES ('r1', 'generate', 'succeeded', 1)",
                [],
            )
            .expect("seed");
        }

        let store = Store::open(&path).expect("migrate");
        assert_eq!(
            store
                .meta_get("schema_version")
                .expect("version")
                .as_deref(),
            Some(SCHEMA_VERSION.to_string().as_str())
        );
        store
            .execute(
                "INSERT INTO runs (id, kind, status, created_at_ms)
                 VALUES ('r2', 'workflow', 'awaiting_approval', 2),
                        ('r3', 'workflow', 'suspended', 3),
                        ('r4', 'tool', 'succeeded', 4)",
            )
            .expect("new statuses");
        let rows = store
            .query("SELECT id, status FROM runs ORDER BY id")
            .expect("rows");
        assert_eq!(rows.rows.len(), 4);
        assert_eq!(rows.rows[0][1].to_string(), "succeeded");
        assert_eq!(rows.rows[1][1].to_string(), "awaiting_approval");
        assert_eq!(rows.rows[2][1].to_string(), "suspended");
        let caps = store
            .query("SELECT name FROM capabilities ORDER BY name")
            .expect("capabilities");
        assert_eq!(caps.rows[0][0].to_string(), "generate");
        assert_eq!(caps.rows[1][0].to_string(), "search");
        let view = store
            .query("SELECT name FROM sqlite_master WHERE type = 'view' AND name = 'memory'")
            .expect("memory view");
        assert_eq!(view.rows[0][0].to_string(), "memory");
        store
            .execute(
                "INSERT INTO models (name, kind, provider, provider_model, created_at_ms, key_name)
                 VALUES ('gpt', 'llm', 'openai', 'gpt-4.1-mini', 1, 'OPENAI_API_KEY')",
            )
            .expect("key name");
        let denied = store.execute(
            "INSERT INTO models (name, kind, provider, provider_model, created_at_ms, key_name)
             VALUES ('bad', 'llm', 'openai', 'x', 1, 'sk-secret')",
        );
        assert!(denied.is_err(), "{denied:?}");
        drop(store);
        cleanup(&path);
    }
}
