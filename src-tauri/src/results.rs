//! Saved pass and fail inspections.
//!
//! The row lives in SQLite. The detection snapshot is recompressed to a JPEG
//! next to that database. The row stores a relative path, not the image bytes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

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
const JPEG_QUALITY: u8 = 75;
const MAX_EDGE: u32 = 1280;
const POST_TIMEOUT: Duration = Duration::from_secs(15);

static DB_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    pub saved: bool,
    pub created_at: String,
    pub image_path: String,
}

pub(crate) struct SavedInspection {
    pub id: i64,
    pub created_at: String,
    pub image_path: String,
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
    pub slot: i64,
}

/// Test mode skips the write. Otherwise a missing, true, or non-boolean
/// `skipResultSave` means do not save. Only JSON `false` writes.
pub(crate) fn saving_disabled(config: &Value) -> bool {
    if config.get("testMode") == Some(&Value::Bool(true)) {
        return true;
    }
    match config.get("skipResultSave") {
        Some(Value::Bool(value)) => *value,
        _ => true,
    }
}

fn skipped_outcome() -> SaveOutcome {
    SaveOutcome {
        saved: false,
        created_at: String::new(),
        image_path: String::new(),
    }
}

/// `{job card}_{YYYYMMDD-HHMMSS}.jpg`. An empty job card uses the kit IC, then `result`.
pub(crate) fn result_filename(job_card: &str, kit_ic: &str, when: DateTime<Local>) -> String {
    let job = sanitize_token(job_card);
    let stem = if !job.is_empty() {
        job
    } else {
        let kit = sanitize_token(kit_ic);
        if kit.is_empty() {
            "result".to_string()
        } else {
            kit
        }
    };
    format!("{stem}_{}.jpg", when.format("%Y%m%d-%H%M%S"))
}

fn sanitize_token(raw: &str) -> String {
    let mut cleaned = String::new();
    for ch in raw.chars() {
        if ch.is_control() || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            continue;
        }
        cleaned.push(ch);
    }
    cleaned
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(80)
        .collect()
}

fn clip_text(raw: &str, limit: usize) -> String {
    raw.trim().chars().take(limit).collect()
}

/// Stored and posted slot. Missing or out-of-range values stay 1 so older
/// callers, and a single full-frame inspection, keep a valid row.
fn result_slot(slot: Option<i64>) -> i64 {
    match slot {
        Some(value) if (1..=4).contains(&value) => value,
        _ => 1,
    }
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
                job_card TEXT NOT NULL DEFAULT '',
                slot INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS results_created_at ON results (created_at);",
        )
        .map_err(db_err)?;
    ensure_job_card_column(&connection)?;
    ensure_slot_column(&connection)?;
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

fn column_present(connection: &Connection, column: &str) -> Result<bool, String> {
    let mut statement = connection
        .prepare("PRAGMA table_info(results)")
        .map_err(db_err)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_err)?;
    for name in names {
        if name.map_err(db_err)?.eq_ignore_ascii_case(column) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_slot_column(connection: &Connection) -> Result<(), String> {
    if column_present(connection, "slot")? {
        return Ok(());
    }
    connection
        .execute(
            "ALTER TABLE results ADD COLUMN slot INTEGER NOT NULL DEFAULT 1",
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
    let image = image::load_from_memory(&bytes)
        .map_err(|_| "Could not read the result image.".to_string())?;
    let scaled = if image.width().max(image.height()) > MAX_EDGE {
        image.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgb = scaled.to_rgb8();
    let (width, height) = rgb.dimensions();
    let mut out = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
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

fn stored_image_path(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|item| item.to_str())
        .unwrap_or("result.jpg");
    format!("{IMAGE_DIR}/{name}")
}

pub(crate) fn save_in(
    root: &Path,
    passed: bool,
    kit_ic: &str,
    description: &str,
    job_card: &str,
    image_base64: Option<&str>,
    detail: &Value,
    slot: i64,
    when: DateTime<Local>,
) -> Result<SavedInspection, String> {
    let _guard = DB_LOCK
        .lock()
        .map_err(|_| "Results database is busy.".to_string())?;
    let connection = open_db(root)?;
    let created_at = when.to_rfc3339();
    let mut written: Option<PathBuf> = None;
    let mut image_path: Option<String> = None;
    if let Some(raw) = image_base64.filter(|value| !value.trim().is_empty()) {
        let bytes = jpeg_bytes(raw)?;
        let path = unique_path(
            &root.join(IMAGE_DIR),
            &result_filename(job_card, kit_ic, when),
        );
        fs::write(&path, bytes)
            .map_err(|err| format!("Could not write the result image: {err}"))?;
        image_path = Some(stored_image_path(&path));
        written = Some(path);
    }
    let detail_text = if detail.is_object() {
        serde_json::to_string(detail).unwrap_or_else(|_| "{}".to_string())
    } else {
        "{}".to_string()
    };
    let inserted = connection.execute(
        "INSERT INTO results (
            passed, kit_ic, description, created_at, image_path, detail_json, job_card, slot
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            passed,
            clip_text(kit_ic, 200),
            clip_text(description, 500),
            created_at,
            image_path,
            detail_text,
            clip_text(job_card, 80),
            result_slot(Some(slot)),
        ],
    );
    if let Err(err) = inserted {
        if let Some(path) = written {
            let _ = fs::remove_file(path);
        }
        return Err(db_err(err));
    }
    Ok(SavedInspection {
        id: connection.last_insert_rowid(),
        created_at,
        image_path: image_path.unwrap_or_default(),
    })
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
            "SELECT id, passed, kit_ic, description, created_at, image_path, detail_json, job_card, slot
             FROM results
             ORDER BY created_at DESC, id DESC",
        )
        .map_err(db_err)?;
    let rows = statement
        .query_map([], |row| {
            let image_path: Option<String> = row.get(5)?;
            let detail_text: String = row.get(6)?;
            let job_card: String = row.get(7)?;
            let slot: i64 = row.get(8)?;
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
                slot,
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
    slot: Option<i64>,
) -> Result<SaveOutcome, String> {
    let config = local_config::load_document(&local_config::project_root()?)?;
    if saving_disabled(&config) {
        return Ok(skipped_outcome());
    }
    save_enabled(
        &config,
        &results_root()?,
        passed,
        &kit_ic,
        &description,
        job_card.as_deref().unwrap_or(""),
        image_base64.as_deref(),
        &detail,
        result_slot(slot),
        Local::now(),
    )
}

fn outcome_of(saved: SavedInspection) -> SaveOutcome {
    let _row = saved.id;
    SaveOutcome {
        saved: true,
        created_at: saved.created_at,
        image_path: saved.image_path,
    }
}

/// Writes when saving is enabled. A disabled config leaves the folder untouched.
pub(crate) fn save_enabled(
    config: &Value,
    root: &Path,
    passed: bool,
    kit_ic: &str,
    description: &str,
    job_card: &str,
    image_base64: Option<&str>,
    detail: &Value,
    slot: i64,
    when: DateTime<Local>,
) -> Result<SaveOutcome, String> {
    if saving_disabled(config) {
        return Ok(skipped_outcome());
    }
    let saved = save_in(
        root,
        passed,
        kit_ic,
        description,
        job_card,
        image_base64,
        detail,
        slot,
        when,
    )?;
    Ok(outcome_of(saved))
}

fn image_base64_in(root: &Path, stored: &str) -> Result<String, String> {
    let stored = stored.trim();
    if stored.is_empty() {
        return Ok(String::new());
    }
    let Some(path) = image_file(root, stored) else {
        return Ok(String::new());
    };
    let bytes = fs::read(path).map_err(|err| format!("Could not read the result image: {err}"))?;
    Ok(STANDARD.encode(bytes))
}

pub(crate) fn inspection_body(
    job_card: &str,
    timestamp: &str,
    passed: bool,
    kit_ic: &str,
    description: &str,
    image_path: &str,
    image_base64: &str,
    detail: &Value,
    slot: i64,
) -> Value {
    let mut body = json!({
        "jobCard": clip_text(job_card, 80),
        "timestamp": timestamp.trim(),
        "passed": passed,
        "slot": result_slot(Some(slot)),
        "kitIc": clip_text(kit_ic, 200),
        "description": clip_text(description, 500),
        "imagePath": image_path.trim(),
        "detail": if detail.is_object() { detail.clone() } else { json!({}) },
    });
    if !image_base64.is_empty() {
        body["imageBase64"] = json!(image_base64);
    }
    body
}

fn results_path(config: &Value) -> String {
    config
        .get("api")
        .and_then(|value| value.get("resultsPath"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Posts one new result. Test mode, a disabled save, and a blank path do nothing.
pub(crate) fn post_if_enabled(
    config: &Value,
    root: &Path,
    job_card: &str,
    timestamp: &str,
    passed: bool,
    kit_ic: &str,
    description: &str,
    image_path: &str,
    detail: &Value,
    slot: i64,
) -> Result<(), String> {
    if saving_disabled(config) || results_path(config).is_empty() {
        return Ok(());
    }
    let image_base64 = image_base64_in(root, image_path)?;
    let body = inspection_body(
        job_card,
        timestamp,
        passed,
        kit_ic,
        description,
        image_path,
        &image_base64,
        detail,
        slot,
    );
    let bytes = serde_json::to_vec(&body).map_err(|_| "Could not build the result.".to_string())?;
    crate::bom::post_api_path(config, "resultsPath", &bytes, POST_TIMEOUT)
}

#[tauri::command]
pub fn post_inspection_result(
    job_card: String,
    timestamp: String,
    passed: bool,
    kit_ic: String,
    description: String,
    image_path: Option<String>,
    detail: Value,
    slot: Option<i64>,
) -> Result<(), String> {
    let config = local_config::load_document(&local_config::project_root()?)?;
    if saving_disabled(&config) || results_path(&config).is_empty() {
        return Ok(());
    }
    post_if_enabled(
        &config,
        &results_root()?,
        &job_card,
        &timestamp,
        passed,
        &kit_ic,
        &description,
        image_path.as_deref().unwrap_or(""),
        &detail,
        result_slot(slot),
    )
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
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

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
    fn filename_uses_the_job_card_and_local_stamp() {
        let stamp = when();
        assert_eq!(
            result_filename("17834402", "6PBBK001", stamp),
            "17834402_20261019-140503.jpg"
        );
        assert_eq!(
            result_filename("a/b:c*", "6PBBK001", stamp),
            "abc_20261019-140503.jpg"
        );
        assert_eq!(
            result_filename("  ", "6PBBK001", stamp),
            "6PBBK001_20261019-140503.jpg"
        );
        assert_eq!(
            result_filename("  ", "  ", stamp),
            "result_20261019-140503.jpg"
        );
    }

    #[test]
    fn saving_stays_on_unless_the_setting_is_false() {
        assert!(saving_disabled(&json!({})));
        assert!(saving_disabled(&json!({ "skipResultSave": true })));
        assert!(saving_disabled(&json!({ "skipResultSave": "no" })));
        assert!(!saving_disabled(&json!({ "skipResultSave": false })));
        assert!(saving_disabled(
            &json!({ "skipResultSave": false, "testMode": true })
        ));
        assert!(!saving_disabled(
            &json!({ "skipResultSave": false, "testMode": false })
        ));
        assert!(!saving_disabled(
            &json!({ "skipResultSave": false, "testMode": "yes" })
        ));
    }

    #[test]
    fn save_lists_the_row_and_jpeg_then_delete_removes_both() {
        let root = temp_root("roundtrip");
        let detail = json!({
            "parts": [{ "ic": "2BBMNM08", "des": "Nut", "qty": 2, "seen": 2 }]
        });
        let saved = save_in(
            &root,
            true,
            "6PBBK001",
            "Accessory Kit",
            "17834402",
            Some(&tiny_jpeg()),
            &detail,
            1,
            when(),
        )
        .unwrap();
        let id = saved.id;
        assert_eq!(saved.image_path, "images/17834402_20261019-140503.jpg");
        assert_eq!(saved.created_at, when().to_rfc3339());
        let connection = Connection::open(root.join(DB_FILE)).unwrap();
        let stored_path: String = connection
            .query_row(
                "SELECT image_path FROM results WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);
        assert_eq!(stored_path, "images/17834402_20261019-140503.jpg");
        let listed = list_in(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert!(listed[0].passed);
        assert_eq!(listed[0].kit_ic, "6PBBK001");
        assert_eq!(listed[0].description, "Accessory Kit");
        assert_eq!(listed[0].job_card, "17834402");
        assert_eq!(listed[0].slot, 1);
        assert!(listed[0].has_image);
        assert_eq!(listed[0].detail["parts"][0]["ic"], "2BBMNM08");
        let image = image_in(&root, id).unwrap();
        assert!(image.starts_with("data:image/jpeg;base64,"));
        let stored = root.join(IMAGE_DIR).join("17834402_20261019-140503.jpg");
        assert!(stored.is_file());
        let bytes = fs::read(&stored).unwrap();
        assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]));
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
        assert_eq!(listed[0].slot, 1);
        let inserted = save_in(
            &root,
            false,
            "6PBBGR01",
            "Rail",
            "17834402",
            None,
            &json!({}),
            1,
            when(),
        )
        .unwrap();
        assert!(inserted.image_path.is_empty());
        let saved = list_in(&root)
            .unwrap()
            .into_iter()
            .find(|row| row.id == inserted.id)
            .unwrap();
        assert_eq!(saved.job_card, "17834402");
        let _ = fs::remove_dir_all(root);
    }

    fn wide_png() -> String {
        let image: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(2000, 800, |x, y| {
            Rgb([(x % 255) as u8, (y % 255) as u8, 40])
        });
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(image.as_raw(), 2000, 800, ExtendedColorType::Rgb8)
            .unwrap();
        STANDARD.encode(bytes)
    }

    fn wide_jpeg() -> String {
        let image: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(1800, 900, |x, y| {
            Rgb([20, (x % 255) as u8, (y % 255) as u8])
        });
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 90)
            .write_image(image.as_raw(), 1800, 900, ExtendedColorType::Rgb8)
            .unwrap();
        STANDARD.encode(bytes)
    }

    fn assert_capped_jpeg(bytes: &[u8]) {
        assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]));
        let decoded = image::load_from_memory(bytes).unwrap();
        assert!(decoded.width() <= MAX_EDGE);
        assert!(decoded.height() <= MAX_EDGE);
        assert_eq!(decoded.width().max(decoded.height()), MAX_EDGE);
    }

    #[test]
    fn wide_images_are_recompressed_under_the_long_edge_cap() {
        assert_capped_jpeg(&jpeg_bytes(&wide_png()).unwrap());
        assert_capped_jpeg(&jpeg_bytes(&wide_jpeg()).unwrap());
    }

    #[test]
    fn test_mode_and_skip_write_nothing() {
        let root = temp_root("skip");
        let detail = json!({});
        for config in [
            json!({ "skipResultSave": false, "testMode": true }),
            json!({ "skipResultSave": true, "testMode": false }),
            json!({ "testMode": false }),
        ] {
            let outcome = save_enabled(
                &config,
                &root,
                false,
                "6PBBK001",
                "Kit",
                "17834402",
                Some(&tiny_jpeg()),
                &detail,
                1,
                when(),
            )
            .unwrap();
            assert!(!outcome.saved);
            assert!(outcome.image_path.is_empty());
            assert!(outcome.created_at.is_empty());
            assert!(!root.join(DB_FILE).exists());
            assert!(!root.join(IMAGE_DIR).exists());
        }
        let _ = fs::remove_dir_all(root);
    }

    fn request_ready(buf: &[u8]) -> bool {
        let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let Ok(head) = std::str::from_utf8(&buf[..split]) else {
            return true;
        };
        let length = head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        });
        match length {
            Some(length) => buf.len() >= split + 4 + length,
            None => true,
        }
    }

    fn capture_post() -> (u16, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
            let mut received = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        received.extend_from_slice(&buf[..n]);
                        if request_ready(&received) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let response =
                b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = sock.write_all(response);
            let _ = tx.send(received);
        });
        (port, rx)
    }

    #[test]
    fn post_sends_the_saved_result_and_the_compressed_image() {
        let root = temp_root("post");
        let detail = json!({
            "parts": [{ "ic": "2BBMNM08", "qty": 1, "seen": 1 }],
            "extras": []
        });
        let saved = save_in(
            &root,
            true,
            "6PBBK001",
            "Accessory Kit",
            "17834402",
            Some(&wide_png()),
            &detail,
            1,
            when(),
        )
        .unwrap();
        let (port, received) = capture_post();
        let config = json!({
            "skipResultSave": false,
            "testMode": false,
            "api": {
                "baseUrl": format!("http://127.0.0.1:{port}"),
                "path": "/GetBoltBagParts",
                "key": "test-key",
                "resultsPath": "/VisionAppApi/SubmitResult"
            }
        });
        post_if_enabled(
            &config,
            &root,
            "17834402",
            &saved.created_at,
            true,
            "6PBBK001",
            "Accessory Kit",
            &saved.image_path,
            &detail,
            1,
        )
        .unwrap();
        let bytes = received.recv_timeout(Duration::from_secs(2)).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("POST /VisionAppApi/SubmitResult HTTP/1.1\r\n"));
        assert!(text.contains("X-Api-Key: test-key\r\n"));
        assert!(!text.contains("GetBoltBagParts"));
        assert!(!text.contains("jobCardNumber"));
        let body = text.split("\r\n\r\n").nth(1).unwrap();
        let payload: Value = serde_json::from_str(body).unwrap();
        assert_eq!(payload["jobCard"], "17834402");
        assert_eq!(payload["timestamp"], saved.created_at);
        assert_eq!(payload["passed"], true);
        assert_eq!(payload["slot"], 1);
        assert_eq!(payload["kitIc"], "6PBBK001");
        assert_eq!(payload["description"], "Accessory Kit");
        assert_eq!(payload["imagePath"], saved.image_path);
        assert_eq!(payload["detail"]["parts"][0]["seen"], 1);
        assert!(payload.get("id").is_none());
        let image = STANDARD
            .decode(payload["imageBase64"].as_str().unwrap())
            .unwrap();
        assert_capped_jpeg(&image);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_chosen_slot_is_stored_and_posted() {
        let body = inspection_body(
            "17834402",
            "2026-10-19T14:05:03+11:00",
            false,
            "6PBBK001",
            "Kit",
            "images/result.jpg",
            "",
            &json!({ "parts": [] }),
            3,
        );
        assert_eq!(body["slot"], 3);
        assert_eq!(
            inspection_body(
                "17834402",
                "2026-10-19T14:05:03+11:00",
                true,
                "6PBBK001",
                "Kit",
                "",
                "",
                &json!({}),
                9,
            )["slot"],
            1
        );
        let root = temp_root("slot");
        let saved = save_in(
            &root,
            false,
            "6PBBK001",
            "Kit",
            "17834402",
            None,
            &json!({}),
            3,
            when(),
        )
        .unwrap();
        let listed = list_in(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, saved.id);
        assert_eq!(listed[0].slot, 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_blank_path_and_test_mode_do_not_post() {
        let root = temp_root("nopost");
        let detail = json!({});
        let closed = json!({
            "skipResultSave": false,
            "testMode": false,
            "api": {
                "baseUrl": "http://127.0.0.1:9",
                "path": "/GetBoltBagParts",
                "key": "secret-key",
                "resultsPath": ""
            }
        });
        post_if_enabled(
            &closed,
            &root,
            "17834402",
            "2026-10-19T14:05:03+11:00",
            false,
            "6PBBK001",
            "Kit",
            "",
            &detail,
            1,
        )
        .unwrap();
        let testing = json!({
            "skipResultSave": false,
            "testMode": true,
            "api": {
                "baseUrl": "http://127.0.0.1:9",
                "key": "secret-key",
                "resultsPath": "/VisionAppApi/SubmitResult"
            }
        });
        post_if_enabled(
            &testing,
            &root,
            "17834402",
            "2026-10-19T14:05:03+11:00",
            false,
            "6PBBK001",
            "Kit",
            "images/17834402_20261019-140503.jpg",
            &detail,
            1,
        )
        .unwrap();
        let _ = fs::remove_dir_all(root);
    }
}
