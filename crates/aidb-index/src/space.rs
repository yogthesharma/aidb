//! Named embedding spaces. The default space stays `vec_chunks` + `aidb_meta`.

use std::sync::Arc;

use aidb_ai::{
    default_embed_model, embedder, known_embed_provider, normalize_distance, normalize_local_model,
    Embedder, EmbedderConfig,
};
use aidb_core::{now_ms, Error, QueryResult, Result, Value};
use aidb_storage::{sqlite_err, Connection};

use crate::{ensure_vec_table, missing_vec_in, upsert_vec};

pub const DEFAULT_SPACE: &str = "default";
pub const DEFAULT_VEC_TABLE: &str = "vec_chunks";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingSpace {
    pub name: String,
    pub provider: String,
    pub provider_model: String,
    pub dimensions: usize,
    pub distance: String,
    pub vec_table: String,
}

pub fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::usage("space name is required"));
    }
    if name.eq_ignore_ascii_case(DEFAULT_SPACE) {
        return Err(Error::usage(
            "space name 'default' is reserved; omit the space argument",
        ));
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(Error::usage("space name is required"));
    };
    if !first.is_ascii_alphabetic() || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::usage(
            "space name must be an identifier (letters, digits, underscore)",
        ));
    }
    Ok(name)
}

pub fn get(conn: &Connection, name: &str) -> Result<Option<EmbeddingSpace>> {
    if name.is_empty() || name.eq_ignore_ascii_case(DEFAULT_SPACE) {
        return Ok(None);
    }
    match conn.query_row(
        "SELECT name, provider, provider_model, dimensions, distance, vec_table
         FROM embedding_spaces WHERE name = ?1",
        [name],
        row_space,
    ) {
        Ok(space) => Ok(Some(space)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(sqlite_err(err)),
    }
}

pub fn require(conn: &Connection, name: &str) -> Result<EmbeddingSpace> {
    get(conn, name)?.ok_or_else(|| Error::usage(format!("unknown embedding space: {name}")))
}

pub fn list(conn: &Connection) -> Result<Vec<EmbeddingSpace>> {
    if !crate::table_exists(conn, "embedding_spaces") {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT name, provider, provider_model, dimensions, distance, vec_table
             FROM embedding_spaces ORDER BY name",
        )
        .map_err(sqlite_err)?;
    let rows = stmt.query_map([], row_space).map_err(sqlite_err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)
}

pub fn resolve(conn: &Connection, space: Option<&str>) -> Result<Option<EmbeddingSpace>> {
    match space {
        None | Some("") | Some(DEFAULT_SPACE) => Ok(None),
        Some(name) => Ok(Some(require(conn, validate_name(name)?)?)),
    }
}

pub fn matches_embedder(space: &EmbeddingSpace, embedder: &dyn Embedder) -> bool {
    space.provider == embedder.provider()
        && space.provider_model == embedder.model()
        && space.dimensions == embedder.dimensions()
}

pub fn embedder_for_space(space: &EmbeddingSpace) -> Result<Arc<dyn Embedder>> {
    embedder(&EmbedderConfig {
        provider: space.provider.clone(),
        model: space.provider_model.clone(),
        dimensions: space.dimensions,
        key_name: None,
    })
}

pub fn create(
    conn: &Connection,
    name: &str,
    provider: &str,
    dimensions: i64,
    model: Option<&str>,
    distance: Option<&str>,
) -> Result<EmbeddingSpace> {
    let name = validate_name(name)?.to_string();
    let provider = provider.trim().to_ascii_lowercase();
    if !known_embed_provider(&provider) {
        return Err(Error::usage(format!(
            "unknown embedding provider: {provider} (use fake, openai, local, or custom)"
        )));
    }
    if dimensions <= 0 || dimensions > 4096 {
        return Err(Error::usage("space dimensions must be between 1 and 4096"));
    }
    let mut dimensions = dimensions as usize;
    let distance = normalize_distance(distance.unwrap_or("cosine"))?;
    let provider_model = match provider.as_str() {
        "local" => {
            let raw = model
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .ok_or_else(|| {
                    Error::usage("local embedding space requires a model name (BGE, Nomic, or E5)")
                })?;
            let canonical = normalize_local_model(raw)?;
            let expected = aidb_ai::local_model_dimensions(canonical)?;
            if dimensions != expected {
                return Err(Error::usage(format!(
                    "local model {canonical} is {expected} dimensions, got {dimensions}"
                )));
            }
            dimensions = expected;
            canonical.to_string()
        }
        "custom" => model
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| Error::usage("custom embedding space requires a model name"))?
            .to_string(),
        _ => model
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default_embed_model(&provider).to_string()),
    };
    let vec_table = format!("vec_chunks_{name}");
    if get(conn, &name)?.is_some() {
        return Err(Error::usage(format!(
            "embedding space {name} already exists"
        )));
    }
    // A space that cannot embed must never exist: resolving the embedder before
    // any write keeps a missing key, model, or registration from leaving behind a
    // space that would later have to guess which model to use.
    let _ = embedder_for_space(&EmbeddingSpace {
        name: name.clone(),
        provider: provider.clone(),
        provider_model: provider_model.clone(),
        dimensions,
        distance: distance.clone(),
        vec_table: vec_table.clone(),
    })?;
    ensure_vec_table(conn, &vec_table, dimensions, &distance)?;
    conn.execute(
        "INSERT INTO embedding_spaces
            (name, provider, provider_model, dimensions, distance, vec_table, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            name,
            provider,
            provider_model,
            dimensions as i64,
            distance,
            vec_table,
            now_ms()
        ],
    )
    .map_err(sqlite_err)?;
    conn.execute(
        "INSERT OR IGNORE INTO models (name, kind, provider, provider_model, dimensions, created_at_ms)
         VALUES (?1, 'embedding', ?2, ?3, ?4, ?5)",
        rusqlite::params![
            format!("space-{name}"),
            provider,
            provider_model,
            dimensions as i64,
            now_ms()
        ],
    )
    .map_err(sqlite_err)?;
    Ok(EmbeddingSpace {
        vec_table: format!("vec_chunks_{name}"),
        name,
        provider,
        provider_model,
        dimensions,
        distance,
    })
}

fn fill_with_owned(
    conn: &Connection,
    space: EmbeddingSpace,
    _fallback: &dyn Embedder,
) -> Result<QueryResult> {
    let owned = embedder_for_space(&space)?;
    let filled = backfill(conn, &space, owned.as_ref())?;
    Ok(space_row(space, filled))
}

fn space_row(space: EmbeddingSpace, filled: usize) -> QueryResult {
    QueryResult {
        columns: vec![
            "name".into(),
            "provider".into(),
            "model".into(),
            "dimensions".into(),
            "distance".into(),
            "vec_table".into(),
            "indexed".into(),
        ],
        rows: vec![vec![
            Value::Text(space.name),
            Value::Text(space.provider),
            Value::Text(space.provider_model),
            Value::Integer(space.dimensions as i64),
            Value::Text(space.distance),
            Value::Text(space.vec_table),
            Value::Integer(filled as i64),
        ]],
    }
}

/// Creating a space is one durable step: either the space exists with a complete
/// index, or the file is untouched. A half-filled space would answer searches from
/// vectors it never finished writing.
pub fn create_and_fill(
    conn: &Connection,
    name: &str,
    provider: &str,
    dimensions: i64,
    model: Option<&str>,
    distance: Option<&str>,
    fallback: &dyn Embedder,
) -> Result<QueryResult> {
    conn.execute_batch("SAVEPOINT aidb_create_space")
        .map_err(sqlite_err)?;
    let result = create_and_fill_inner(conn, name, provider, dimensions, model, distance, fallback);
    let unwind = if result.is_ok() {
        "RELEASE aidb_create_space"
    } else {
        "ROLLBACK TO aidb_create_space; RELEASE aidb_create_space"
    };
    conn.execute_batch(unwind).map_err(sqlite_err)?;
    result
}

fn create_and_fill_inner(
    conn: &Connection,
    name: &str,
    provider: &str,
    dimensions: i64,
    model: Option<&str>,
    distance: Option<&str>,
    fallback: &dyn Embedder,
) -> Result<QueryResult> {
    let space = create(conn, name, provider, dimensions, model, distance)?;
    let used: &dyn Embedder = if matches_embedder(&space, fallback) {
        fallback
    } else {
        return fill_with_owned(conn, space, fallback);
    };
    let filled = backfill(conn, &space, used)?;
    Ok(space_row(space, filled))
}

pub fn backfill(
    conn: &Connection,
    space: &EmbeddingSpace,
    embedder: &dyn Embedder,
) -> Result<usize> {
    if embedder.dimensions() != space.dimensions {
        return Err(Error::schema(format!(
            "space {} expects {} dimensions, embedder has {}",
            space.name,
            space.dimensions,
            embedder.dimensions()
        )));
    }
    let docs: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM documents WHERE index_status = 'ready' ORDER BY created_at_ms")
            .map_err(sqlite_err)?;
        let rows = stmt.query_map([], |row| row.get(0)).map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)
    }?;
    let mut n = 0;
    for doc_id in docs {
        n += index_doc_into(conn, &doc_id, space, embedder)?;
    }
    Ok(n)
}

pub fn index_doc_into(
    conn: &Connection,
    doc_id: &str,
    space: &EmbeddingSpace,
    embedder: &dyn Embedder,
) -> Result<usize> {
    let needed = missing_vec_in(conn, doc_id, &space.vec_table)?;
    if needed.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = needed.iter().map(|(_, text)| text.clone()).collect();
    let vectors = embedder.embed(&texts)?;
    if vectors.len() != needed.len() {
        return Err(Error::ai("embedder returned the wrong number of vectors"));
    }
    for ((chunk_id, _), vector) in needed.iter().zip(vectors.iter()) {
        if vector.len() != space.dimensions {
            return Err(Error::schema("embedding dimension mismatch"));
        }
        upsert_vec(conn, &space.vec_table, *chunk_id, vector, doc_id)?;
    }
    Ok(needed.len())
}

fn row_space(row: &rusqlite::Row<'_>) -> rusqlite::Result<EmbeddingSpace> {
    Ok(EmbeddingSpace {
        name: row.get(0)?,
        provider: row.get(1)?,
        provider_model: row.get(2)?,
        dimensions: row.get::<_, i64>(3)? as usize,
        distance: row.get(4)?,
        vec_table: row.get(5)?,
    })
}
