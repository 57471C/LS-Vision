//! Shared SQLite database for inspection results, the DINOv2 catalogue, and
//! backend settings.
//!
//! The file is `results.sqlite` under the user save directory. Results rows
//! stay in the `results` table. Catalogue embeddings and settings use
//! `dino_catalogue` and `app_config` in that same file.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};

const DB_FILE: &str = "results.sqlite";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS dino_catalogue (
    id INTEGER PRIMARY KEY,
    ic TEXT NOT NULL,
    model TEXT NOT NULL,
    embedding BLOB NOT NULL,
    source TEXT,
    created_at TEXT,
    UNIQUE(ic, model, source)
);
CREATE INDEX IF NOT EXISTS dino_catalogue_model ON dino_catalogue (model);
CREATE TABLE IF NOT EXISTS app_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

fn db_err(err: impl std::fmt::Display) -> String {
    format!("Could not use the results database: {err}")
}

/// Production opens the results database. Tests open `scope/results.sqlite`
/// so a temp directory does not touch the operator file.
pub(crate) fn database_path(scope: &Path) -> PathBuf {
    if cfg!(test) {
        return scope.join(DB_FILE);
    }
    shared_database_path().unwrap_or_else(|_| scope.join(DB_FILE))
}

pub(crate) fn shared_database_path() -> Result<PathBuf, String> {
    let dir = crate::local_config::user_save_dir()?.join("results");
    fs::create_dir_all(&dir)
        .map_err(|err| format!("Could not create the results folder: {err}"))?;
    Ok(dir.join(DB_FILE))
}

pub(crate) fn ensure_schema(connection: &Connection) -> Result<(), String> {
    connection.execute_batch(SCHEMA).map_err(db_err)
}

fn connect(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open(path).map_err(db_err)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(db_err)?;
    ensure_schema(&connection)?;
    Ok(connection)
}

fn open_existing(scope: &Path) -> Result<Option<Connection>, String> {
    let path = database_path(scope);
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(connect(&path)?))
}

fn open_write(scope: &Path) -> Result<Connection, String> {
    let path = database_path(scope);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Could not create the results folder: {err}"))?;
    }
    connect(&path)
}

/// Little-endian `f32` bytes. The matcher reads this blob back as floats.
pub(crate) fn embedding_blob(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(crate) fn embedding_from_blob(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if !bytes.len().is_multiple_of(4) {
        return Err("Could not read the catalogue.".to_string());
    }
    let mut values = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let array = [chunk[0], chunk[1], chunk[2], chunk[3]];
        let value = f32::from_le_bytes(array);
        if !value.is_finite() {
            return Err("Could not read the catalogue.".to_string());
        }
        values.push(value);
    }
    Ok(values)
}

pub(crate) fn load_config(scope: &Path) -> Result<Option<Value>, String> {
    let Some(connection) = open_existing(scope)? else {
        return Ok(None);
    };
    let mut statement = connection
        .prepare(
            "SELECT key, value FROM app_config
             WHERE key NOT LIKE 'catalogue.%'
             ORDER BY rowid",
        )
        .map_err(db_err)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_err)?;
    let mut map = Map::new();
    for row in rows {
        let (key, text) = row.map_err(db_err)?;
        let value = serde_json::from_str(&text).unwrap_or(Value::Null);
        map.insert(key, value);
    }
    if map.is_empty() {
        return Ok(None);
    }
    Ok(Some(Value::Object(map)))
}

pub(crate) fn has_config(scope: &Path) -> Result<bool, String> {
    Ok(load_config(scope)?.is_some())
}

/// Replace every settings key. Keys that start with `catalogue.` stay.
pub(crate) fn save_config(scope: &Path, document: &Value) -> Result<(), String> {
    let Some(object) = document.as_object() else {
        return Err("Config must be a JSON object.".to_string());
    };
    let connection = open_write(scope)?;
    let transaction = connection.unchecked_transaction().map_err(db_err)?;
    transaction
        .execute(
            "DELETE FROM app_config WHERE key NOT LIKE 'catalogue.%'",
            [],
        )
        .map_err(db_err)?;
    for (key, value) in object {
        if key.starts_with("catalogue.") {
            continue;
        }
        let text = serde_json::to_string(value)
            .map_err(|_| "Could not write vision/config.local.json.".to_string())?;
        transaction
            .execute(
                "INSERT INTO app_config (key, value) VALUES (?1, ?2)",
                params![key, text],
            )
            .map_err(db_err)?;
    }
    transaction.commit().map_err(db_err)?;
    Ok(())
}

pub(crate) fn get_text(scope: &Path, key: &str) -> Result<Option<String>, String> {
    let Some(connection) = open_existing(scope)? else {
        return Ok(None);
    };
    connection
        .query_row(
            "SELECT value FROM app_config WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)
}

pub(crate) fn put_text(scope: &Path, key: &str, value: &str) -> Result<(), String> {
    let connection = open_write(scope)?;
    connection
        .execute(
            "INSERT OR REPLACE INTO app_config (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(db_err)?;
    Ok(())
}

pub(crate) struct CatalogueMeta {
    pub embedding_model: Option<String>,
    pub embeddings_stale: bool,
    pub parts_json: String,
}

pub(crate) struct EmbeddingRow {
    pub ic: String,
    pub source: String,
    pub values: Vec<f32>,
}

pub(crate) fn read_catalogue_meta(scope: &Path) -> Result<Option<CatalogueMeta>, String> {
    let Some(parts_json) = get_text(scope, "catalogue.parts")? else {
        return Ok(None);
    };
    let embedding_model =
        get_text(scope, "catalogue.embeddingModel")?.filter(|value| !value.is_empty());
    let embeddings_stale =
        get_text(scope, "catalogue.embeddingsStale")?.is_some_and(|value| value == "true");
    Ok(Some(CatalogueMeta {
        embedding_model,
        embeddings_stale,
        parts_json,
    }))
}

pub(crate) fn embeddings_for_model(scope: &Path, model: &str) -> Result<Vec<EmbeddingRow>, String> {
    let Some(connection) = open_existing(scope)? else {
        return Ok(Vec::new());
    };
    let mut statement = connection
        .prepare(
            "SELECT ic, source, embedding FROM dino_catalogue
             WHERE model = ?1
             ORDER BY id",
        )
        .map_err(db_err)?;
    let rows = statement
        .query_map(params![model], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(db_err)?;
    let mut embeddings = Vec::new();
    for row in rows {
        let (ic, source, blob) = row.map_err(db_err)?;
        embeddings.push(EmbeddingRow {
            ic,
            source: source.unwrap_or_default(),
            values: embedding_from_blob(&blob)?,
        });
    }
    Ok(embeddings)
}

/// Replace one model's embeddings and drop rows whose crop is gone.
/// Rows for another model stay when that crop is still in `live_sources`.
pub(crate) fn write_catalogue(
    scope: &Path,
    meta: &CatalogueMeta,
    model: &str,
    rows: &[EmbeddingRow],
    live_sources: &[(String, String)],
) -> Result<(), String> {
    let connection = open_write(scope)?;
    let transaction = connection.unchecked_transaction().map_err(db_err)?;
    put_meta(&transaction, meta)?;
    transaction
        .execute(
            "DELETE FROM dino_catalogue WHERE model = ?1",
            params![model],
        )
        .map_err(db_err)?;
    transaction
        .execute(
            "CREATE TEMP TABLE live_source (ic TEXT NOT NULL, source TEXT NOT NULL)",
            [],
        )
        .map_err(db_err)?;
    {
        let mut insert_live = transaction
            .prepare("INSERT INTO live_source (ic, source) VALUES (?1, ?2)")
            .map_err(db_err)?;
        for (ic, source) in live_sources {
            insert_live.execute(params![ic, source]).map_err(db_err)?;
        }
    }
    transaction
        .execute(
            "DELETE FROM dino_catalogue
             WHERE NOT EXISTS (
                 SELECT 1 FROM live_source
                 WHERE live_source.ic = dino_catalogue.ic
                   AND live_source.source = IFNULL(dino_catalogue.source, '')
             )",
            [],
        )
        .map_err(db_err)?;
    let created_at = chrono::Local::now().to_rfc3339();
    {
        let mut insert = transaction
            .prepare(
                "INSERT OR REPLACE INTO dino_catalogue
                    (ic, model, embedding, source, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(db_err)?;
        for row in rows {
            let blob = embedding_blob(&row.values);
            insert
                .execute(params![row.ic, model, blob, row.source, created_at])
                .map_err(db_err)?;
        }
    }
    transaction.commit().map_err(db_err)?;
    Ok(())
}

fn put_meta(connection: &Connection, meta: &CatalogueMeta) -> Result<(), String> {
    let model = meta.embedding_model.clone().unwrap_or_default();
    let stale = if meta.embeddings_stale {
        "true"
    } else {
        "false"
    };
    for (key, value) in [
        ("catalogue.version", "1"),
        ("catalogue.embeddingModel", model.as_str()),
        ("catalogue.embeddingsStale", stale),
        ("catalogue.parts", meta.parts_json.as_str()),
    ] {
        connection
            .execute(
                "INSERT OR REPLACE INTO app_config (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(db_err)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_scope(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ls-vision-app-db-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn embeddings_round_trip_as_little_endian_floats() {
        let blob = embedding_blob(&[1.0, -2.0]);
        assert_eq!(blob, [0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0xc0]);
        assert_eq!(embedding_from_blob(&blob).unwrap(), vec![1.0, -2.0]);
    }

    #[test]
    fn replace_keeps_one_row_per_ic_model_and_source() {
        let scope = temp_scope("replace");
        let meta = CatalogueMeta {
            embedding_model: Some("b_reg".to_string()),
            embeddings_stale: false,
            parts_json: "[]".to_string(),
        };
        let first = EmbeddingRow {
            ic: "FAST-1".to_string(),
            source: "catalogue-crops/FAST-1/a.jpg".to_string(),
            values: vec![1.0, 0.0],
        };
        write_catalogue(
            &scope,
            &meta,
            "b_reg",
            &[first],
            &[(
                "FAST-1".to_string(),
                "catalogue-crops/FAST-1/a.jpg".to_string(),
            )],
        )
        .unwrap();
        let second = EmbeddingRow {
            ic: "FAST-1".to_string(),
            source: "catalogue-crops/FAST-1/a.jpg".to_string(),
            values: vec![0.0, 9.0],
        };
        let other = EmbeddingRow {
            ic: "FAST-1".to_string(),
            source: "catalogue-crops/FAST-1/a.jpg".to_string(),
            values: vec![4.0, 0.0],
        };
        write_catalogue(
            &scope,
            &meta,
            "s_reg",
            &[other],
            &[(
                "FAST-1".to_string(),
                "catalogue-crops/FAST-1/a.jpg".to_string(),
            )],
        )
        .unwrap();
        write_catalogue(
            &scope,
            &meta,
            "b_reg",
            &[second],
            &[(
                "FAST-1".to_string(),
                "catalogue-crops/FAST-1/a.jpg".to_string(),
            )],
        )
        .unwrap();
        let b_reg = embeddings_for_model(&scope, "b_reg").unwrap();
        let s_reg = embeddings_for_model(&scope, "s_reg").unwrap();
        assert_eq!(b_reg.len(), 1);
        assert_eq!(b_reg[0].values, vec![0.0, 9.0]);
        assert_eq!(s_reg.len(), 1);
        assert_eq!(s_reg[0].values, vec![4.0, 0.0]);
        let count: i64 = connect(&database_path(&scope))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM dino_catalogue", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let _ = fs::remove_dir_all(scope);
    }

    #[test]
    fn new_tables_leave_the_results_table_unchanged() {
        let scope = temp_scope("results");
        let path = scope.join(DB_FILE);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE results (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    passed INTEGER NOT NULL,
                    kit_ic TEXT NOT NULL,
                    description TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    image_path TEXT,
                    detail_json TEXT NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO results (passed, kit_ic, description, created_at, image_path, detail_json)
                 VALUES (1, '6PBBK001', 'Kit', '2026-10-19T00:00:00+00:00', NULL, '{}')",
                [],
            )
            .unwrap();
        drop(connection);
        ensure_schema(&connect(&path).unwrap()).unwrap();
        let connection = connect(&path).unwrap();
        let columns = {
            let mut statement = connection.prepare("PRAGMA table_info(results)").unwrap();
            let names = statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap();
            names.map(|name| name.unwrap()).collect::<Vec<_>>()
        };
        assert_eq!(
            columns,
            vec![
                "id",
                "passed",
                "kit_ic",
                "description",
                "created_at",
                "image_path",
                "detail_json",
            ]
        );
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM results", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let tables: Vec<String> = {
            let mut statement = connection
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(|name| name.unwrap())
                .collect()
        };
        assert!(tables.iter().any(|name| name == "dino_catalogue"));
        assert!(tables.iter().any(|name| name == "app_config"));
        assert!(tables.iter().any(|name| name == "results"));
        let _ = fs::remove_dir_all(scope);
    }

    #[test]
    fn config_keys_round_trip_and_leave_catalogue_keys() {
        let scope = temp_scope("config");
        put_text(&scope, "catalogue.parts", "[]").unwrap();
        let mut map = Map::new();
        map.insert("zebra".to_string(), Value::from(1));
        map.insert("dinoModel".to_string(), Value::from("b_reg"));
        map.insert(
            "erp".to_string(),
            serde_json::json!({ "name": "erp", "url": "https://example.test" }),
        );
        save_config(&scope, &Value::Object(map)).unwrap();
        let loaded = load_config(&scope).unwrap().unwrap();
        let keys: Vec<_> = loaded.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            vec![
                "zebra".to_string(),
                "dinoModel".to_string(),
                "erp".to_string()
            ]
        );
        assert_eq!(loaded["dinoModel"], "b_reg");
        assert_eq!(loaded["erp"]["url"], "https://example.test");
        assert_eq!(
            get_text(&scope, "catalogue.parts").unwrap().as_deref(),
            Some("[]")
        );
        let _ = fs::remove_dir_all(scope);
    }
}
