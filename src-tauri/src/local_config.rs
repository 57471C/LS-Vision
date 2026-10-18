//! Read and write `vision/config.local.json`, and list the weight files in `vision/models`.
//! These commands do not load a model or open a camera. Detection loads the saved `.onnx` file in Rust.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

const CONFIG_FILE: &str = "config.local.json";
const LEGACY_FILE: &str = "bom-source.json";
const DEFAULT_MODEL_NAME: &str = "best.pt";
const DEFAULT_SQL_DRIVER: &str = "ODBC Driver 18 for SQL Server";
const MAX_MODEL_NAME: usize = 180;

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

fn vision_dir(root: &Path) -> PathBuf {
    root.join("vision")
}

fn config_file(root: &Path) -> PathBuf {
    vision_dir(root).join(CONFIG_FILE)
}

fn legacy_file(root: &Path) -> PathBuf {
    vision_dir(root).join(LEGACY_FILE)
}

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
            "key": ""
        },
        "erp": {
            "name": "",
            "url": ""
        },
        "scanSeconds": 8,
        "testMode": false,
        "detectRoi": null,
        "barcodeRoi": null,
        "model": "",
        "inferenceEp": "directml",
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
        .map_err(|err| format!("Could not read vision/models: {err}"))?;
    let entries =
        fs::read_dir(&root).map_err(|err| format!("Could not read vision/models: {err}"))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("Could not read vision/models: {err}"))?;
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

fn list_models(root: &Path) -> Result<Vec<String>, String> {
    list_model_files(&models_dir(root))
}

fn select_model_name(raw: &str, files: &[String]) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    let cleaned = clean_model_name(raw)
        .ok_or_else(|| "Choose a .pt or .onnx file in vision/models.".to_string())?;
    match_model(&cleaned, files).ok_or_else(|| format!("{cleaned} was not found in vision/models."))
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
    let files = list_models(root)?;
    Ok(active_name(stored, &files))
}

pub(crate) fn vision_models_dir(root: &Path) -> PathBuf {
    models_dir(root)
}

fn store_active_model(root: &Path, filename: &str) -> Result<ActiveModel, String> {
    let files = list_models(root)?;
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
    list_models(&project_root()?)
}

#[tauri::command]
pub fn set_active_model(filename: String) -> Result<ActiveModel, String> {
    store_active_model(&project_root()?, &filename)
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
            list_models(project.root()).unwrap(),
            vec!["A.pt".to_string(), "b.onnx".to_string()]
        );
    }

    #[test]
    fn missing_models_directory_is_an_empty_list() {
        let project = TempProject::new("nomodels");
        fs::remove_dir_all(models_dir(project.root())).unwrap();
        assert!(list_models(project.root()).unwrap().is_empty());
    }

    #[test]
    fn missing_files_load_the_default_document() {
        let project = TempProject::new("default");
        let document = load_document(project.root()).unwrap();
        assert_eq!(document["source"], "json");
        assert_eq!(document["model"], "");
        assert_eq!(document["inferenceEp"], "directml");
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
        assert!(store_active_model(project.root(), "best.pt").is_err());
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

        let saved = store_active_model(project.root(), "best.pt").unwrap();
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

        let missing = store_active_model(project.root(), "nope.onnx").unwrap_err();
        assert!(missing.contains("was not found"));
        let path = store_active_model(project.root(), r"vision/models/best.pt").unwrap_err();
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
        let saved = store_active_model(project.root(), "  ").unwrap();
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
        let saved = store_active_model(project.root(), "").unwrap();
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
        let saved = store_active_model(project.root(), "kit.onnx").unwrap();
        assert_eq!(saved.model, "kit.onnx");
        let document = load_document(project.root()).unwrap();
        assert_eq!(document["source"], "api");
        assert_eq!(document["api"]["key"], "hidden");
        assert_eq!(document["model"], "kit.onnx");
        assert!(config_file(project.root()).is_file());
        assert!(legacy_file(project.root()).is_file());
    }
}
