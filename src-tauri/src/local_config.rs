//! Read and write `vision/config.local.json`, and list weight files in the app data models folder.
//! These commands do not load a model or open a camera. Detection loads the saved `.onnx` file in Rust.
//! The active filename stays in `vision/config.local.json`. The weight file lives under the Tauri app data directory.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

const CONFIG_FILE: &str = "config.local.json";
const LEGACY_FILE: &str = "bom-source.json";
const DEFAULT_MODEL_NAME: &str = "best.pt";
const DEFAULT_ONNX_NAME: &str = "best.onnx";
const DEFAULT_SQL_DRIVER: &str = "ODBC Driver 18 for SQL Server";
const MAX_MODEL_NAME: usize = 180;
const APP_IDENTIFIER: &str = "studio.lean.ls-vision";

static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Serialize)]
pub struct ActiveModel {
    pub model: String,
    pub active: String,
    pub files: Vec<String>,
    pub message: String,
}

pub(crate) fn project_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Could not find the project root.".to_string())
}

/// Folder for photos, snapshots, and logs.
/// LocalAppData is used when it is set. Otherwise the folder is under Documents.
/// The result is never inside Program Files.
pub(crate) fn user_save_dir() -> Result<PathBuf, String> {
    let dir = user_save_dir_path()?;
    fs::create_dir_all(&dir)
        .map_err(|err| format!("Could not create the save folder {}: {err}", dir.display()))?;
    Ok(dir)
}

fn user_save_dir_path() -> Result<PathBuf, String> {
    let dir = if let Some(local) = env_path("LOCALAPPDATA") {
        local.join("studio.lean.ls-vision")
    } else if let Some(home) = env_path("USERPROFILE").or_else(|| env_path("HOME")) {
        home.join("Documents").join("ls-vision")
    } else {
        return Err("Could not find LocalAppData or Documents.".to_string());
    };
    if is_program_files(&dir) {
        return Err(
            "Saved photos, snapshots, and logs cannot be written under Program Files.".to_string(),
        );
    }
    Ok(dir)
}

fn env_path(key: &str) -> Option<PathBuf> {
    let value = std::env::var_os(key)?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// Training snapshots use Documents even when LocalAppData exists.
pub(crate) fn training_dir_for_home(home: &Path) -> PathBuf {
    home.join("Documents").join("ls-vision").join("training")
}

pub(crate) fn default_training_dir() -> Result<PathBuf, String> {
    let Some(home) = env_path("USERPROFILE").or_else(|| env_path("HOME")) else {
        return Err("Could not find Documents.".to_string());
    };
    let dir = training_dir_for_home(&home);
    if is_program_files(&dir) {
        return Err("Training images cannot be written under Program Files.".to_string());
    }
    Ok(dir)
}

pub(crate) fn path_is_program_files(path: &Path) -> bool {
    is_program_files(path)
}

fn is_program_files(path: &Path) -> bool {
    let text = normalize_path(path);
    if text.ends_with("\\program files") || text.ends_with("\\program files (x86)") {
        return true;
    }
    if text.contains("\\program files\\") || text.contains("\\program files (x86)\\") {
        return true;
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        let Some(root) = env_path(key) else {
            continue;
        };
        let root = normalize_path(&root);
        if text == root || text.starts_with(&(root + "\\")) {
            return true;
        }
    }
    false
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// Record `app.path().app_data_dir()` so dev and the installed app share one folder.
pub(crate) fn set_app_data_dir(dir: PathBuf) {
    let _ = APP_DATA_DIR.set(dir);
}

pub(crate) fn app_data_dir() -> Result<PathBuf, String> {
    if let Some(dir) = APP_DATA_DIR.get() {
        return Ok(dir.clone());
    }
    fallback_app_data_dir()
}

fn fallback_app_data_dir() -> Result<PathBuf, String> {
    if let Some(roaming) = env_path("APPDATA") {
        return Ok(roaming.join(APP_IDENTIFIER));
    }
    if let Some(xdg) = env_path("XDG_DATA_HOME") {
        return Ok(xdg.join(APP_IDENTIFIER));
    }
    if let Some(home) = env_path("HOME").or_else(|| env_path("USERPROFILE")) {
        return Ok(home.join(".local").join("share").join(APP_IDENTIFIER));
    }
    Err("Could not find the app data folder.".to_string())
}

/// `models` under the Tauri app data directory. The folder is created when it is missing.
pub(crate) fn app_models_dir() -> Result<PathBuf, String> {
    let dir = app_data_dir()?.join("models");
    if is_program_files(&dir) {
        return Err("Models cannot be stored under Program Files.".to_string());
    }
    fs::create_dir_all(&dir).map_err(|err| {
        format!(
            "Could not create the models folder {}: {err}",
            dir.display()
        )
    })?;
    Ok(dir)
}

fn vision_dir(root: &Path) -> PathBuf {
    root.join("vision")
}

fn config_file(root: &Path) -> PathBuf {
    vision_dir(root).join(CONFIG_FILE)
}

fn legacy_file(root: &Path) -> PathBuf {
    vision_dir(root).join(LEGACY_FILE)
}

#[cfg(test)]
fn models_dir(root: &Path) -> PathBuf {
    vision_dir(root).join("models")
}

fn default_config() -> Value {
    serde_json::json!({
        "source": "json",
        "mssql": {
            "driver": DEFAULT_SQL_DRIVER,
            "server": "",
            "database": "",
            "auth": "windows",
            "user": "",
            "password": ""
        },
        "api": {
            "baseUrl": "",
            "path": "",
            "key": "",
            "descriptionPath": "/VisionAppApi/ListInventoryDescription",
            "resultsPath": ""
        },
        "erp": {
            "name": "",
            "url": ""
        },
        "scanSeconds": 8,
        "testMode": false,
        "skipResultSave": true,
        "detectRoi": null,
        "detectRois": [null, null, null, null],
        "barcodeRoi": null,
        "model": "",
        "confidence": 0.45,
        "matchThreshold": 0.7,
        "partRules": [],
        "inferenceEp": "directml",
        "trainingDir": "",
        "trainingImageName": "",
        "camera": {
            "index": 0,
            "resolution": "",
            "focus": null,
            "brightness": null,
            "contrast": null,
            "displayFps": 15
        }
    })
}

fn read_json_object(path: &Path, label: &str) -> Result<Option<Value>, String> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let value: Value =
                serde_json::from_str(&text).map_err(|_| format!("{label} is not valid JSON."))?;
            if !value.is_object() {
                return Err(format!("{label} must be a JSON object."));
            }
            Ok(Some(value))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("Could not read {label}: {err}")),
    }
}

pub(crate) fn load_document(root: &Path) -> Result<Value, String> {
    let config_path = config_file(root);
    if let Some(value) = read_json_object(&config_path, "vision/config.local.json")? {
        return Ok(value);
    }
    // bom-source.json is used only when config.local.json is absent.
    if let Some(value) = read_json_object(&legacy_file(root), "vision/bom-source.json")? {
        return Ok(value);
    }
    Ok(default_config())
}

fn write_json_file(path: &Path, value: &Value) -> Result<(), String> {
    if !value.is_object() {
        return Err("Config must be a JSON object.".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Could not write vision/config.local.json.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("Could not write vision/config.local.json: {err}"))?;
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|_| "Could not write vision/config.local.json.".to_string())?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text.as_bytes())
        .map_err(|err| format!("Could not write vision/config.local.json: {err}"))?;
    replace_file(&tmp, path)
}

fn replace_file(tmp: &Path, path: &Path) -> Result<(), String> {
    if fs::rename(tmp, path).is_ok() {
        return Ok(());
    }
    // Windows does not replace an existing file on rename.
    if path.exists() {
        fs::remove_file(path).map_err(|err| {
            let _ = fs::remove_file(tmp);
            format!("Could not replace vision/config.local.json: {err}")
        })?;
    }
    fs::rename(tmp, path).map_err(|err| {
        format!(
            "Could not replace vision/config.local.json: {err}. The new copy is still vision/config.local.json.tmp."
        )
    })
}

pub(crate) fn save_document(root: &Path, config: &Value) -> Result<(), String> {
    write_json_file(&config_file(root), config)
}

fn clean_model_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.len() > MAX_MODEL_NAME {
        return None;
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') || name.contains("..") {
        return None;
    }
    let path = Path::new(name);
    if path.file_name().and_then(|item| item.to_str()) != Some(name) {
        return None;
    }
    let suffix = path.extension()?.to_str()?.to_ascii_lowercase();
    if suffix == "pt" || suffix == "onnx" {
        Some(name.to_string())
    } else {
        None
    }
}

fn match_model(name: &str, files: &[String]) -> Option<String> {
    let cleaned = clean_model_name(name)?;
    files
        .iter()
        .find(|item| item.eq_ignore_ascii_case(&cleaned))
        .cloned()
}

fn list_model_files(dir: &Path) -> Result<Vec<String>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let root = dir
        .canonicalize()
        .map_err(|err| format!("Could not read the models folder: {err}"))?;
    let entries =
        fs::read_dir(&root).map_err(|err| format!("Could not read the models folder: {err}"))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("Could not read the models folder: {err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|item| item.to_str()) else {
            continue;
        };
        if clean_model_name(name).is_none() {
            continue;
        }
        let Ok(resolved) = path.canonicalize() else {
            continue;
        };
        if !resolved.starts_with(&root) {
            continue;
        }
        names.push(name.to_string());
    }
    names.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    Ok(names)
}

fn select_model_name(raw: &str, files: &[String]) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    let cleaned = clean_model_name(raw)
        .ok_or_else(|| "Choose a .pt or .onnx file in the models folder.".to_string())?;
    match_model(&cleaned, files)
        .ok_or_else(|| format!("{cleaned} was not found in the models folder."))
}

fn assign_model(document: &mut Value, model: &str) -> Result<(), String> {
    let object = document
        .as_object_mut()
        .ok_or_else(|| "Config must be a JSON object.".to_string())?;
    object.insert("model".to_string(), Value::String(model.to_string()));
    Ok(())
}

fn preferred_model(files: &[String]) -> String {
    // An empty selection uses best.onnx when that file is present, then best.pt.
    if let Some(found) = match_model("best.onnx", files) {
        return found;
    }
    if let Some(found) = match_model(DEFAULT_MODEL_NAME, files) {
        return found;
    }
    if let Some(found) = files
        .iter()
        .find(|name| name.to_ascii_lowercase().ends_with(".onnx"))
    {
        return found.clone();
    }
    files
        .first()
        .cloned()
        .unwrap_or_else(|| "best.onnx".to_string())
}

fn active_name(stored: &str, files: &[String]) -> String {
    if let Some(found) = match_model(stored, files) {
        return found;
    }
    preferred_model(files)
}

pub(crate) fn configured_model_name(root: &Path) -> Result<String, String> {
    let document = load_document(root)?;
    let stored = document
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let files = list_model_files(&app_models_dir()?)?;
    Ok(active_name(stored, &files))
}

fn store_active_model(root: &Path, models: &Path, filename: &str) -> Result<ActiveModel, String> {
    let files = list_model_files(models)?;
    let model = select_model_name(filename, &files)?;
    let mut document = load_document(root)?;
    assign_model(&mut document, &model)?;
    save_document(root, &document)?;
    let active = active_name(&model, &files);
    let message = format!("Saved. Active model is {active}.");
    Ok(ActiveModel {
        model,
        active,
        files,
        message,
    })
}

fn model_state(root: &Path, models: &Path, message: String) -> Result<ActiveModel, String> {
    let files = list_model_files(models)?;
    let document = load_document(root)?;
    let stored = document
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let active = active_name(&stored, &files);
    let model = if stored.trim().is_empty() {
        String::new()
    } else {
        match_model(&stored, &files).unwrap_or(stored)
    };
    Ok(ActiveModel {
        model,
        active,
        files,
        message,
    })
}

fn onnx_destination_name(preferred: &str) -> String {
    match clean_model_name(preferred) {
        Some(name) if name.to_ascii_lowercase().ends_with(".onnx") => name,
        _ => DEFAULT_ONNX_NAME.to_string(),
    }
}

fn is_onnx_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("onnx"))
}

/// ModelProto starts with `ir_version` (field 1, one-byte varint), then another
/// model field. Text and image files renamed to `.onnx` do not.
fn onnx_model_header(header: &[u8]) -> bool {
    header.len() >= 3
        && header[0] == 0x08
        && (1..0x80).contains(&header[1])
        && matches!(header[2], 0x12 | 0x1A | 0x22 | 0x28 | 0x32 | 0x3A)
}

fn looks_like_onnx_model(path: &Path) -> Result<(), String> {
    let mut file =
        fs::File::open(path).map_err(|_| "Could not read the selected file.".to_string())?;
    let mut header = [0u8; 8];
    let read = file
        .read(&mut header)
        .map_err(|_| "Could not read the selected file.".to_string())?;
    if onnx_model_header(&header[..read]) {
        Ok(())
    } else {
        Err("Selected file is not a valid ONNX model".to_string())
    }
}

fn copy_model_file(dir: &Path, source: &Path, dest_name: &str) -> Result<(), String> {
    if !source.is_file() || !is_onnx_file(source) {
        return Err("Choose an .onnx file.".to_string());
    }
    looks_like_onnx_model(source)?;
    let dest_name = clean_model_name(dest_name)
        .filter(|name| name.to_ascii_lowercase().ends_with(".onnx"))
        .ok_or_else(|| "Choose an .onnx file.".to_string())?;
    fs::create_dir_all(dir).map_err(|err| format!("Could not create the models folder: {err}"))?;
    let root = dir
        .canonicalize()
        .map_err(|err| format!("Could not read the models folder: {err}"))?;
    let dest = root.join(&dest_name);
    if dest.parent() != Some(root.as_path()) {
        return Err("Choose an .onnx file.".to_string());
    }
    if let Ok(resolved) = dest.canonicalize()
        && !resolved.starts_with(&root)
    {
        return Err("Could not replace the model.".to_string());
    }
    let partial = root.join(format!(".{dest_name}.partial"));
    if let Err(err) = fs::copy(source, &partial) {
        let _ = fs::remove_file(&partial);
        return Err(format!("Could not copy the model: {err}"));
    }
    if let Err(err) = replace_model_file(&partial, &dest) {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    if let Ok(file) = fs::File::options().write(true).open(&dest) {
        let _ = file.set_modified(SystemTime::now());
    }
    Ok(())
}

fn replace_model_file(partial: &Path, dest: &Path) -> Result<(), String> {
    let backup_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(DEFAULT_ONNX_NAME);
    let backup = dest.with_file_name(format!(".{backup_name}.bak"));
    let _ = fs::remove_file(&backup);
    let had_dest = dest.is_file();
    if had_dest && fs::rename(dest, &backup).is_err() {
        fs::remove_file(dest).map_err(|err| format!("Could not replace the model: {err}"))?;
    }
    if let Err(err) = fs::rename(partial, dest) {
        if had_dest {
            let _ = fs::rename(&backup, dest);
        }
        return Err(format!("Could not replace the model: {err}"));
    }
    let _ = fs::remove_file(&backup);
    Ok(())
}

fn install_model_at(
    root: &Path,
    models: &Path,
    source: &Path,
    preferred: &str,
) -> Result<ActiveModel, String> {
    let dest_name = onnx_destination_name(preferred);
    copy_model_file(models, source, &dest_name)?;
    let dest = models.join(&dest_name);
    let source_len = fs::metadata(source).map(|meta| meta.len()).unwrap_or(0);
    let dest_len = fs::metadata(&dest).map(|meta| meta.len()).unwrap_or(0);
    if !dest.is_file() || source_len == 0 || dest_len != source_len {
        return Err(format!("The model was not copied to {}.", dest.display()));
    }
    let mut saved = store_active_model(root, models, &dest_name)?;
    let document = load_document(root)?;
    let stored = document
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if !stored.eq_ignore_ascii_case(&dest_name) || !saved.active.eq_ignore_ascii_case(&dest_name) {
        return Err(format!(
            "Copied {} but it is not the active model.",
            dest.display()
        ));
    }
    saved.message = format!(
        "Updated {}. Active model is {}.",
        dest.display(),
        saved.active
    );
    Ok(saved)
}

/// A failed copy stays on the success payload. Settings reads `message`.
fn report_model_update(
    root: &Path,
    models: &Path,
    outcome: Result<ActiveModel, String>,
) -> Result<ActiveModel, String> {
    match outcome {
        Ok(saved) => Ok(saved),
        Err(err) => model_state(root, models, err),
    }
}

#[tauri::command]
pub fn load_local_config() -> Result<Value, String> {
    load_document(&project_root()?)
}

#[tauri::command]
pub fn save_local_config(config: Value) -> Result<Value, String> {
    let root = project_root()?;
    save_document(&root, &config)?;
    Ok(config)
}

#[tauri::command]
pub fn list_vision_models() -> Result<Vec<String>, String> {
    list_model_files(&app_models_dir()?)
}

#[tauri::command]
pub fn set_active_model(filename: String) -> Result<ActiveModel, String> {
    store_active_model(&project_root()?, &app_models_dir()?, &filename)
}

/// Copy a chosen `.onnx` file into the app data models folder and make it active.
/// The next detection loads that file. Cancel returns a short notice.
#[tauri::command]
pub fn update_model(app: AppHandle, filename: String) -> Result<ActiveModel, String> {
    let root = project_root()?;
    let models = app_models_dir()?;
    let picked = app
        .dialog()
        .file()
        .add_filter("ONNX model", &["onnx"])
        .set_title("Update model")
        .blocking_pick_file();
    let Some(picked) = picked else {
        return model_state(&root, &models, "No file selected.".to_string());
    };
    let source = match picked.into_path() {
        Ok(path) => path,
        Err(_) => {
            return model_state(
                &root,
                &models,
                "Could not read the selected file.".to_string(),
            );
        }
    };
    crate::detect::forget_session();
    let outcome = install_model_at(&root, &models, &source, &filename);
    crate::detect::forget_session();
    report_model_update(&root, &models, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "ls-vision-config-{name}-{id}-{}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("vision").join("models")).unwrap();
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn saved_files_stay_out_of_program_files() {
        let dir = super::user_save_dir_path().unwrap();
        let text = super::normalize_path(&dir);
        assert!(
            text.contains("\\appdata\\local\\") || text.contains("\\documents\\"),
            "{text}"
        );
        assert!(!super::is_program_files(&dir), "{text}");
        assert!(super::is_program_files(Path::new(
            r"C:\Program Files\ls-vision\debug-barcode.jpg"
        )));
        assert!(super::is_program_files(Path::new(
            r"C:\Program Files (x86)\ls-vision\barcode-debug.log"
        )));
        assert!(!super::is_program_files(Path::new(
            r"C:\Users\operator\AppData\Local\studio.lean.ls-vision\debug-barcode.jpg"
        )));
    }

    #[test]
    fn project_root_is_the_repo() {
        let root = project_root().unwrap();
        assert!(root.join("vision").join("config.example.json").is_file());
        assert!(root.join("src-tauri").join("Cargo.toml").is_file());
    }

    #[test]
    fn model_names_are_basenames_with_a_weight_suffix() {
        assert_eq!(clean_model_name(" best.pt ").as_deref(), Some("best.pt"));
        assert_eq!(
            clean_model_name("Model.ONNX").as_deref(),
            Some("Model.ONNX")
        );
        assert_eq!(
            clean_model_name("best.pt.onnx").as_deref(),
            Some("best.pt.onnx")
        );
        assert!(clean_model_name("").is_none());
        assert!(clean_model_name("best.txt").is_none());
        assert!(clean_model_name("best.pt.exe").is_none());
        assert!(clean_model_name(r"vision\models\best.pt").is_none());
        assert!(clean_model_name("vision/models/best.pt").is_none());
        assert!(clean_model_name(r"..\best.pt").is_none());
        assert!(clean_model_name("best..pt").is_none());
        assert!(clean_model_name(&format!("{}.pt", "a".repeat(MAX_MODEL_NAME))).is_none());
    }

    #[test]
    fn list_keeps_weight_files_inside_the_models_directory() {
        let project = TempProject::new("list");
        let models = models_dir(project.root());
        fs::write(models.join("b.onnx"), b"x").unwrap();
        fs::write(models.join("A.pt"), b"x").unwrap();
        fs::write(models.join("notes.txt"), b"x").unwrap();
        fs::write(models.join("skip.PT.bak"), b"x").unwrap();
        fs::create_dir(models.join("nested.pt")).unwrap();
        fs::write(vision_dir(project.root()).join("other.pt"), b"x").unwrap();

        assert_eq!(
            list_model_files(&models_dir(project.root())).unwrap(),
            vec!["A.pt".to_string(), "b.onnx".to_string()]
        );
    }

    #[test]
    fn missing_models_directory_is_an_empty_list() {
        let project = TempProject::new("nomodels");
        fs::remove_dir_all(models_dir(project.root())).unwrap();
        assert!(
            list_model_files(&models_dir(project.root()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn missing_files_load_the_default_document() {
        let project = TempProject::new("default");
        let document = load_document(project.root()).unwrap();
        assert_eq!(document["source"], "json");
        assert_eq!(document["model"], "");
        assert_eq!(document["confidence"], 0.45);
        assert_eq!(document["matchThreshold"], 0.7);
        assert_eq!(document["partRules"], serde_json::json!([]));
        assert_eq!(document["inferenceEp"], "directml");
        assert_eq!(document["trainingDir"], "");
        assert_eq!(document["trainingImageName"], "");
        assert_eq!(document["scanSeconds"], 8);
        assert!(!config_file(project.root()).exists());
    }

    #[test]
    fn local_config_wins_over_the_legacy_file() {
        let project = TempProject::new("fallback");
        fs::write(
            legacy_file(project.root()),
            r#"{"source":"api","model":"old.pt"}"#,
        )
        .unwrap();
        assert_eq!(load_document(project.root()).unwrap()["source"], "api");

        fs::write(
            config_file(project.root()),
            r#"{"source":"mssql","model":"best.pt"}"#,
        )
        .unwrap();
        assert_eq!(load_document(project.root()).unwrap()["source"], "mssql");
    }

    #[test]
    fn invalid_config_is_left_in_place() {
        let project = TempProject::new("invalid");
        let path = config_file(project.root());
        fs::write(&path, b"{").unwrap();
        fs::write(models_dir(project.root()).join("best.pt"), b"x").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(
            load_document(project.root())
                .unwrap_err()
                .contains("not valid JSON")
        );
        assert!(
            store_active_model(project.root(), &models_dir(project.root()), "best.pt").is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn save_rejects_a_non_object_and_keeps_key_order() {
        let project = TempProject::new("order");
        assert!(save_document(project.root(), &Value::Array(Vec::new())).is_err());

        let mut map = Map::new();
        map.insert("zebra".to_string(), Value::from(1));
        map.insert("alpha".to_string(), Value::from("kept"));
        save_document(project.root(), &Value::Object(map)).unwrap();
        let text = fs::read_to_string(config_file(project.root())).unwrap();
        let zebra = text.find("zebra").unwrap();
        let alpha = text.find("alpha").unwrap();
        assert!(zebra < alpha);
        assert!(text.ends_with('\n'));
        assert!(!text.contains('\r'));
        assert_eq!(load_document(project.root()).unwrap()["alpha"], "kept");
    }

    #[test]
    fn set_model_stores_the_on_disk_name_and_keeps_other_fields() {
        let project = TempProject::new("set");
        let path = config_file(project.root());
        fs::write(
            &path,
            "{\n  \"source\": \"api\",\n  \"mssql\": { \"password\": \"secret\" },\n  \"camera\": { \"index\": 2 },\n  \"model\": \"\"\n}\n",
        )
        .unwrap();
        fs::write(models_dir(project.root()).join("Best.pt"), b"x").unwrap();
        fs::write(models_dir(project.root()).join("other.onnx"), b"x").unwrap();

        let saved =
            store_active_model(project.root(), &models_dir(project.root()), "best.pt").unwrap();
        assert_eq!(saved.model, "Best.pt");
        assert_eq!(saved.active, "Best.pt");
        assert_eq!(
            saved.files,
            vec!["Best.pt".to_string(), "other.onnx".to_string()]
        );
        assert_eq!(saved.message, "Saved. Active model is Best.pt.");

        let document = load_document(project.root()).unwrap();
        assert_eq!(document["model"], "Best.pt");
        assert_eq!(document["source"], "api");
        assert_eq!(document["mssql"]["password"], "secret");
        assert_eq!(document["camera"]["index"], 2);
        assert!(!legacy_file(project.root()).exists());
    }

    #[test]
    fn set_model_rejects_a_missing_or_unsafe_name() {
        let project = TempProject::new("reject");
        fs::write(models_dir(project.root()).join("best.pt"), b"x").unwrap();
        fs::write(config_file(project.root()), "{\"source\":\"json\"}\n").unwrap();
        let before = fs::read_to_string(config_file(project.root())).unwrap();

        let missing = store_active_model(project.root(), &models_dir(project.root()), "nope.onnx")
            .unwrap_err();
        assert!(missing.contains("was not found"));
        let path = store_active_model(
            project.root(),
            &models_dir(project.root()),
            r"vision/models/best.pt",
        )
        .unwrap_err();
        assert!(path.contains("Choose a .pt or .onnx"));
        assert_eq!(
            fs::read_to_string(config_file(project.root())).unwrap(),
            before
        );
    }

    #[test]
    fn clearing_the_model_uses_best_pt_when_that_file_exists() {
        let project = TempProject::new("clear");
        fs::write(models_dir(project.root()).join("best.pt"), b"x").unwrap();
        let saved = store_active_model(project.root(), &models_dir(project.root()), "  ").unwrap();
        assert_eq!(saved.model, "");
        assert_eq!(saved.active, "best.pt");
        assert_eq!(saved.message, "Saved. Active model is best.pt.");
        assert_eq!(load_document(project.root()).unwrap()["model"], "");
        assert_eq!(load_document(project.root()).unwrap()["source"], "json");
    }

    #[test]
    fn clearing_the_model_prefers_best_onnx_when_both_exist() {
        let project = TempProject::new("clear-onnx");
        fs::write(models_dir(project.root()).join("best.pt"), b"x").unwrap();
        fs::write(models_dir(project.root()).join("best.onnx"), b"x").unwrap();
        let saved = store_active_model(project.root(), &models_dir(project.root()), "").unwrap();
        assert_eq!(saved.model, "");
        assert_eq!(saved.active, "best.onnx");
        assert_eq!(saved.message, "Saved. Active model is best.onnx.");
    }

    #[test]
    fn legacy_file_is_copied_forward_when_the_model_is_saved() {
        let project = TempProject::new("promote");
        fs::write(
            legacy_file(project.root()),
            "{\"source\":\"api\",\"api\":{\"key\":\"hidden\"}}\n",
        )
        .unwrap();
        fs::write(models_dir(project.root()).join("kit.onnx"), b"x").unwrap();
        let saved =
            store_active_model(project.root(), &models_dir(project.root()), "kit.onnx").unwrap();
        assert_eq!(saved.model, "kit.onnx");
        let document = load_document(project.root()).unwrap();
        assert_eq!(document["source"], "api");
        assert_eq!(document["api"]["key"], "hidden");
        assert_eq!(document["model"], "kit.onnx");
        assert!(config_file(project.root()).is_file());
        assert!(legacy_file(project.root()).is_file());
    }

    #[test]
    fn app_data_models_folder_is_roaming_appdata() {
        let dir = fallback_app_data_dir().unwrap().join("models");
        let text = normalize_path(&dir);
        assert!(
            text.contains("\\appdata\\roaming\\studio.lean.ls-vision\\models")
                || text.contains("\\.local\\share\\studio.lean.ls-vision\\models"),
            "{text}"
        );
        assert!(!text.contains("\\appdata\\local\\"), "{text}");
        assert!(!is_program_files(&dir), "{text}");
    }

    #[test]
    fn installer_config_does_not_bundle_onnx_models() {
        let text = fs::read_to_string(
            project_root()
                .unwrap()
                .join("src-tauri")
                .join("tauri.conf.json"),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        let resources = &value["bundle"]["resources"];
        let empty = resources.is_null()
            || resources.as_array().is_some_and(|items| items.is_empty())
            || resources.as_object().is_some_and(|map| map.is_empty());
        assert!(empty, "{resources}");
        assert!(!text.to_ascii_lowercase().contains(".onnx"));
    }

    fn onnx_bytes(payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x08, 0x08, 0x12, 0x00];
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn onnx_header_accepts_a_model_and_rejects_other_bytes() {
        assert!(onnx_model_header(&[0x08, 0x08, 0x12, 0x07]));
        assert!(!onnx_model_header(b"this is not a model"));
        assert!(!onnx_model_header(&[0x89, 0x50, 0x4E, 0x47]));
        assert!(!onnx_model_header(&[0x08, 0x08]));
    }

    #[test]
    fn update_overwrites_the_selected_onnx_name() {
        let project = TempProject::new("install");
        let models = models_dir(project.root());
        let source = project.root().join("fresh.onnx");
        let body = onnx_bytes(b"new-bytes");
        fs::write(&source, &body).unwrap();
        fs::write(models.join("kit.onnx"), b"old").unwrap();
        fs::write(models.join("best.pt"), b"pt").unwrap();

        let saved = install_model_at(project.root(), &models, &source, "kit.onnx").unwrap();
        assert_eq!(saved.model, "kit.onnx");
        assert_eq!(saved.active, "kit.onnx");
        assert_eq!(
            saved.message,
            format!(
                "Updated {}. Active model is kit.onnx.",
                models.join("kit.onnx").display()
            )
        );
        assert_eq!(fs::read(models.join("kit.onnx")).unwrap(), body);
        assert_eq!(fs::read(models.join("best.pt")).unwrap(), b"pt");
        assert_eq!(load_document(project.root()).unwrap()["model"], "kit.onnx");
        assert!(fs::read_dir(&models).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            !name.contains("partial") && !name.contains(".bak")
        }));
    }

    #[test]
    fn update_uses_best_onnx_when_the_selection_is_not_onnx() {
        let project = TempProject::new("install-best");
        let models = models_dir(project.root());
        let source = project.root().join("export.ONNX");
        let body = onnx_bytes(b"onnx-body");
        fs::write(&source, &body).unwrap();
        fs::write(models.join("best.pt"), b"pt").unwrap();
        fs::write(models.join("best.onnx"), b"previous").unwrap();

        let saved = install_model_at(project.root(), &models, &source, "best.pt").unwrap();
        assert_eq!(saved.model, "best.onnx");
        assert_eq!(saved.active, "best.onnx");
        assert_eq!(
            saved.message,
            format!(
                "Updated {}. Active model is best.onnx.",
                models.join("best.onnx").display()
            )
        );
        assert_eq!(fs::read(models.join("best.onnx")).unwrap(), body);
        assert_eq!(fs::read(models.join("best.pt")).unwrap(), b"pt");
    }

    #[test]
    fn update_creates_best_onnx_when_the_folder_is_empty() {
        let project = TempProject::new("install-empty");
        let models = models_dir(project.root());
        let source = project.root().join("weights.onnx");
        let body = onnx_bytes(b"first");
        fs::write(&source, &body).unwrap();
        let saved = install_model_at(project.root(), &models, &source, "").unwrap();
        assert_eq!(saved.model, "best.onnx");
        assert_eq!(
            saved.message,
            format!(
                "Updated {}. Active model is best.onnx.",
                models.join("best.onnx").display()
            )
        );
        assert_eq!(fs::read(models.join("best.onnx")).unwrap(), body);
    }

    #[test]
    fn update_replaces_bytes_when_the_size_matches() {
        let project = TempProject::new("install-same-size");
        let models = models_dir(project.root());
        fs::write(models.join("best.onnx"), onnx_bytes(b"aaaa")).unwrap();
        let source = project.root().join("next.onnx");
        let body = onnx_bytes(b"bbbb");
        fs::write(&source, &body).unwrap();
        install_model_at(project.root(), &models, &source, "best.onnx").unwrap();
        assert_eq!(fs::read(models.join("best.onnx")).unwrap(), body);
    }

    #[test]
    fn update_rejects_a_non_onnx_file_and_keeps_the_old_bytes() {
        let project = TempProject::new("install-reject");
        let models = models_dir(project.root());
        let source = project.root().join("notes.txt");
        fs::write(&source, b"nope").unwrap();
        fs::write(models.join("best.onnx"), b"keep").unwrap();
        let err = install_model_at(project.root(), &models, &source, "best.onnx").unwrap_err();
        assert!(err.contains(".onnx"), "{err}");
        assert_eq!(fs::read(models.join("best.onnx")).unwrap(), b"keep");
    }

    #[test]
    fn update_rejects_an_onnx_name_that_is_not_a_model() {
        let project = TempProject::new("install-header");
        let models = models_dir(project.root());
        let source = project.root().join("notes.onnx");
        fs::write(&source, b"this is not a model").unwrap();
        let kept = onnx_bytes(b"keep");
        fs::write(models.join("best.onnx"), &kept).unwrap();
        let err = install_model_at(project.root(), &models, &source, "best.onnx").unwrap_err();
        assert_eq!(err, "Selected file is not a valid ONNX model");
        assert_eq!(fs::read(models.join("best.onnx")).unwrap(), kept);
        let reported = report_model_update(project.root(), &models, Err(err)).unwrap();
        assert_eq!(reported.message, "Selected file is not a valid ONNX model");
        assert_eq!(fs::read(models.join("best.onnx")).unwrap(), kept);
        assert_ne!(
            load_document(project.root()).unwrap()["model"],
            "notes.onnx"
        );
    }
}
