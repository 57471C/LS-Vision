//! Saved pass and fail inspections.
//!
//! The row lives in SQLite. The snapshot is a JPEG next to that database.
//! The row stores the file path, not the image bytes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Local};
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::local_config;

const DB_FILE: &str = "results.sqlite";
const IMAGE_DIR: &str = "images";

static DB_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    pub saved: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedResult {
    pub id: i64,
    pub passed: bool,
    pub kit_ic: String,
    pub description: String,
    pub job_card: String,
    pub created_at: String,
    pub has_image: bool,
    pub detail: Value,
}

/// Missing, true, or a non-boolean value means do not save. Only `false` writes.
pub(crate) fn saving_disabled(config: &Value) -> bool {
    match config.get("skipResultSave") {
        Some(Value::Bool(value)) => *value,
        _ => true,
    }
}

pub(crate) fn result_filename(passed: bool, kit_ic: &str, when: DateTime<Local>) -> String {
    let mark = if passed { "P" } else { "F" };
    format!(
        "{mark}-{}-{}.jpg",
        sanitize_kit_ic(kit_ic),
        when.format("%d%m%y-%H-%M-%S")
    )
}

fn sanitize_kit_ic(raw: &str) -> String {
    let mut cleaned = String::new();
    for ch in raw.chars() {
        if ch.is_control() || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            continue;
        }
        cleaned.push(ch);
    }
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        return "NONE".to_string();
    }
    trimmed.chars().take(80).collect()
}

fn clip_text(raw: &str, limit: usize) -> String {
    raw.trim().chars().take(limit).collect()
}

fn db_err(err: impl std::fmt::Display) -> String {
    format!("Could not use the results database: {err}")
}

fn results_root() -> Result<PathBuf, String> {
    let dir = local_config::user_save_dir()?.join("results");
    fs::create_dir_all(&dir)
        .map_err(|err| format!("Could not create the results folder: {err}"))?;
    Ok(dir)
}

fn open_db(root: &Path) -> Result<Connection, String> {
    fs::create_dir_all(root.join(IMAGE_DIR))
        .map_err(|err| format!("Could not create the results image folder: {err}"))?;
    let connection = Connection::open(root.join(DB_FILE)).map_err(db_err)?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS results (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                passed INTEGER NOT NULL,
                kit_ic TEXT NOT NULL,
                description TEXT NOT NULL,
                created_at TEXT NOT NULL,
                image_path TEXT,
                detail_json TEXT NOT NULL,
                job_card TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS results_created_at ON results (created_at);",
        )
        .map_err(db_err)?;
    ensure_job_card_column(&connection)?;
    Ok(connection)
}

fn ensure_job_card_column(connection: &Connection) -> Result<(), String> {
    let found = {
        let mut statement = connection
            .prepare("PRAGMA table_info(results)")
            .map_err(db_err)?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(db_err)?;
        let mut present = false;
        for name in names {
            if name.map_err(db_err)?.eq_ignore_ascii_case("job_card") {
                present = true;
                break;
            }
        }
        present
    };
    if found {
        return Ok(());
    }
    connection
        .execute(
            "ALTER TABLE results ADD COLUMN job_card TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(db_err)?;
    Ok(())
}

fn jpeg_bytes(raw: &str) -> Result<Vec<u8>, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("Could not read the result image.".to_string());
    }
    let payload = text.split_once(',').map(|(_, data)| data).unwrap_or(text);
    let compact: String = payload.chars().filter(|ch| !ch.is_whitespace()).collect();
    let bytes = STANDARD
        .decode(compact)
        .map_err(|_| "Could not read the result image.".to_string())?;
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Ok(bytes);
    }
    let image = image::load_from_memory(&bytes)
        .map_err(|_| "Could not read the result image.".to_string())?;
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut out = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut out, 90);
    encoder
        .write_image(rgb.as_raw(), width, height, ExtendedColorType::Rgb8)
        .map_err(|_| "Could not encode the result image.".to_string())?;
    Ok(out)
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|item| item.to_str())
        .unwrap_or("result");
    for index in 2..1000 {
        let next = dir.join(format!("{stem}-{index}.jpg"));
        if !next.exists() {
            return next;
        }
    }
    dir.join(format!("{stem}-1000.jpg"))
}

fn image_file(root: &Path, stored: &str) -> Option<PathBuf> {
    let name = Path::new(stored).file_name()?;
    let path = root.join(IMAGE_DIR).join(name);
    if path.is_file() { Some(path) } else { None }
}

pub(crate) fn save_in(
    root: &Path,
    passed: bool,
    kit_ic: &str,
    description: &str,
    job_card: &str,
    image_base64: Option<&str>,
    detail: &Value,
    when: DateTime<Local>,
) -> Result<i64, String> {
    let _guard = DB_LOCK
        .lock()
        .map_err(|_| "Results database is busy.".to_string())?;
    let connection = open_db(root)?;
    let mut image_path: Option<String> = None;
    if let Some(raw) = image_base64.filter(|value| !value.trim().is_empty()) {
        let bytes = jpeg_bytes(raw)?;
        let path = unique_path(
            &root.join(IMAGE_DIR),
            &result_filename(passed, kit_ic, when),
        );
        fs::write(&path, bytes)
            .map_err(|err| format!("Could not write the result image: {err}"))?;
        image_path = Some(path.to_string_lossy().into_owned());
    }
    let detail_text = if detail.is_object() {
        serde_json::to_string(detail).unwrap_or_else(|_| "{}".to_string())
    } else {
        "{}".to_string()
    };
    let inserted = connection.execute(
        "INSERT INTO results (
            passed, kit_ic, description, created_at, image_path, detail_json, job_card
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            passed,
            clip_text(kit_ic, 200),
            clip_text(description, 500),
            when.to_rfc3339(),
            image_path,
            detail_text,
            clip_text(job_card, 80),
        ],
    );
    if let Err(err) = inserted {
        if let Some(path) = image_path {
            let _ = fs::remove_file(path);
        }
        return Err(db_err(err));
    }
    Ok(connection.last_insert_rowid())
}

pub(crate) fn list_in(root: &Path) -> Result<Vec<ListedResult>, String> {
    let _guard = DB_LOCK
        .lock()
        .map_err(|_| "Results database is busy.".to_string())?;
    if !root.join(DB_FILE).is_file() {
        return Ok(Vec::new());
    }
    let connection = open_db(root)?;
    let mut statement = connection
        .prepare(
            "SELECT id, passed, kit_ic, description, created_at, image_path, detail_json, job_card
             FROM results
             ORDER BY created_at DESC, id DESC",
        )
        .map_err(db_err)?;
    let rows = statement
        .query_map([], |row| {
            let image_path: Option<String> = row.get(5)?;
            let detail_text: String = row.get(6)?;
            let job_card: String = row.get(7)?;
            let detail = serde_json::from_str(&detail_text).unwrap_or_else(|_| json!({}));
            let has_image = image_path
                .as_deref()
                .and_then(|stored| image_file(root, stored))
                .is_some();
            Ok(ListedResult {
                id: row.get(0)?,
                passed: row.get::<_, i64>(1)? != 0,
                kit_ic: row.get(2)?,
                description: row.get(3)?,
                job_card,
                created_at: row.get(4)?,
                has_image,
                detail,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn remove_stored_image(root: &Path, stored: &str) -> Result<(), String> {
    let Some(path) = image_file(root, stored) else {
        return Ok(());
    };
    fs::remove_file(path).map_err(|err| format!("Could not delete the result image: {err}"))
}

pub(crate) fn delete_in(root: &Path, id: i64) -> Result<(), String> {
    let _guard = DB_LOCK
        .lock()
        .map_err(|_| "Results database is busy.".to_string())?;
    let connection = open_db(root)?;
    let stored: Option<String> = connection
        .query_row(
            "SELECT image_path FROM results WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| "Result was not found.".to_string())?;
    if let Some(path) = stored.as_deref() {
        remove_stored_image(root, path)?;
    }
    let changed = connection
        .execute("DELETE FROM results WHERE id = ?1", [id])
        .map_err(db_err)?;
    if changed == 0 {
        return Err("Result was not found.".to_string());
    }
    Ok(())
}

pub(crate) fn image_in(root: &Path, id: i64) -> Result<String, String> {
    let _guard = DB_LOCK
        .lock()
        .map_err(|_| "Results database is busy.".to_string())?;
    if !root.join(DB_FILE).is_file() {
        return Ok(String::new());
    }
    let connection = open_db(root)?;
    let stored: Option<String> = connection
        .query_row(
            "SELECT image_path FROM results WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
        .flatten();
    let Some(stored) = stored else {
        return Ok(String::new());
    };
    let Some(path) = image_file(root, &stored) else {
        return Ok(String::new());
    };
    let bytes = fs::read(path).map_err(|err| format!("Could not read the result image: {err}"))?;
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)))
}

#[tauri::command]
pub fn save_result(
    passed: bool,
    kit_ic: String,
    description: String,
    job_card: Option<String>,
    image_base64: Option<String>,
    detail: Value,
) -> Result<SaveOutcome, String> {
    let config = local_config::load_document(&local_config::project_root()?)?;
    if saving_disabled(&config) {
        return Ok(SaveOutcome { saved: false });
    }
    save_in(
        &results_root()?,
        passed,
        &kit_ic,
        &description,
        job_card.as_deref().unwrap_or(""),
        image_base64.as_deref(),
        &detail,
        Local::now(),
    )?;
    Ok(SaveOutcome { saved: true })
}

#[tauri::command]
pub fn list_results() -> Result<Vec<ListedResult>, String> {
    list_in(&results_root()?)
}

#[tauri::command]
pub fn delete_result(id: i64) -> Result<(), String> {
    delete_in(&results_root()?, id)
}

#[tauri::command]
pub fn result_image(id: i64) -> Result<String, String> {
    image_in(&results_root()?, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use image::{ImageBuffer, ImageEncoder, Rgb};

    fn when() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 10, 19, 14, 5, 3)
            .single()
            .unwrap()
    }

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ls-results-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tiny_jpeg() -> String {
        let image: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(1, 1, Rgb([12, 34, 56]));
        let mut bytes = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut bytes, 80);
        encoder
            .write_image(image.as_raw(), 1, 1, ExtendedColorType::Rgb8)
            .unwrap();
        STANDARD.encode(bytes)
    }

    #[test]
    fn filename_uses_pass_fail_kit_and_local_stamp() {
        let stamp = when();
        assert_eq!(
            result_filename(true, "6PBBK001", stamp),
            "P-6PBBK001-191026-14-05-03.jpg"
        );
        assert_eq!(
            result_filename(false, "6PBBK001", stamp),
            "F-6PBBK001-191026-14-05-03.jpg"
        );
        assert_eq!(
            result_filename(false, "a/b:c*", stamp),
            "F-abc-191026-14-05-03.jpg"
        );
        assert_eq!(
            result_filename(false, "  ", stamp),
            "F-NONE-191026-14-05-03.jpg"
        );
    }

    #[test]
    fn saving_stays_on_unless_the_setting_is_false() {
        assert!(saving_disabled(&json!({})));
        assert!(saving_disabled(&json!({ "skipResultSave": true })));
        assert!(saving_disabled(&json!({ "skipResultSave": "no" })));
        assert!(!saving_disabled(&json!({ "skipResultSave": false })));
    }

    #[test]
    fn save_lists_the_row_and_jpeg_then_delete_removes_both() {
        let root = temp_root("roundtrip");
        let detail = json!({
            "parts": [{ "ic": "2BBMNM08", "des": "Nut", "qty": 2, "seen": 2 }]
        });
        let id = save_in(
            &root,
            true,
            "6PBBK001",
            "Accessory Kit",
            "17834402",
            Some(&tiny_jpeg()),
            &detail,
            when(),
        )
        .unwrap();
        let listed = list_in(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert!(listed[0].passed);
        assert_eq!(listed[0].kit_ic, "6PBBK001");
        assert_eq!(listed[0].description, "Accessory Kit");
        assert_eq!(listed[0].job_card, "17834402");
        assert!(listed[0].has_image);
        assert_eq!(listed[0].detail["parts"][0]["ic"], "2BBMNM08");
        let image = image_in(&root, id).unwrap();
        assert!(image.starts_with("data:image/jpeg;base64,"));
        let stored = root.join(IMAGE_DIR).join("P-6PBBK001-191026-14-05-03.jpg");
        assert!(stored.is_file());
        delete_in(&root, id).unwrap();
        assert!(!stored.exists());
        assert!(list_in(&root).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delete_does_not_remove_a_file_outside_the_image_folder() {
        let root = temp_root("outside");
        let outside = root.join("keep.txt");
        fs::write(&outside, b"keep").unwrap();
        let connection = open_db(&root).unwrap();
        connection
            .execute(
                "INSERT INTO results (passed, kit_ic, description, created_at, image_path, detail_json)
                 VALUES (0, 'X', '', '2026-10-19T14:05:03+00:00', ?1, '{}')",
                [outside.to_string_lossy().as_ref()],
            )
            .unwrap();
        let id = connection.last_insert_rowid();
        drop(connection);
        delete_in(&root, id).unwrap();
        assert!(outside.is_file());
        assert!(list_in(&root).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn old_database_gains_a_job_card_column() {
        let root = temp_root("migrate");
        let connection = Connection::open(root.join(DB_FILE)).unwrap();
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
        let listed = list_in(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].job_card, "");
        let id = save_in(
            &root,
            false,
            "6PBBGR01",
            "Rail",
            "17834402",
            None,
            &json!({}),
            when(),
        )
        .unwrap();
        let saved = list_in(&root)
            .unwrap()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(saved.job_card, "17834402");
        let _ = fs::remove_dir_all(root);
    }
}
