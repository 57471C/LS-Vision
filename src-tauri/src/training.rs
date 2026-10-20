//! Training snapshots from the camera preview window.
//! The default folder is Documents/ls-vision/training even when LocalAppData exists.
//! A save never overwrites: the suggested name is the last base, plus ` (n)` when today's file exists.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::local_config::{self, load_document, save_document};

const MAX_STEM: usize = 80;
const DEFAULT_BASE: &str = "image";

#[derive(Debug, Serialize)]
pub struct TrainingName {
    pub name: String,
    pub folder: String,
    pub day: String,
}

#[derive(Debug, Serialize)]
pub struct TrainingSave {
    pub saved: bool,
    pub name: String,
    pub path: String,
    pub message: String,
}

#[tauri::command]
pub fn default_training_folder() -> Result<String, String> {
    Ok(display_path(&local_config::default_training_dir()?))
}

/// Check a folder from Settings. An empty path uses the Documents default.
/// The folder is created so a later snapshot can write into it.
#[tauri::command]
pub fn check_training_folder(path: String) -> Result<String, String> {
    let dir = resolve_training_dir(&path)?;
    if dir.exists() && !dir.is_dir() {
        return Err("Choose a folder, not a file.".to_string());
    }
    fs::create_dir_all(&dir).map_err(|err| {
        format!(
            "Could not create the training folder {}: {err}",
            dir.display()
        )
    })?;
    if local_config::path_is_program_files(&dir) {
        return Err("Training images cannot be written under Program Files.".to_string());
    }
    Ok(display_path(&dir))
}

/// Folder picker. Cancel returns an empty string.
#[tauri::command]
pub fn pick_training_folder(app: AppHandle) -> Result<String, String> {
    let picked = app
        .dialog()
        .file()
        .set_title("Training image folder")
        .blocking_pick_folder();
    let Some(picked) = picked else {
        return Ok(String::new());
    };
    let path = picked
        .into_path()
        .map_err(|_| "Could not read the selected folder.".to_string())?;
    check_training_folder(display_path(&path))
}

#[tauri::command]
pub fn suggest_training_filename() -> Result<TrainingName, String> {
    let root = local_config::project_root()?;
    suggest_in(&root, &today_folder())
}

#[tauri::command]
pub fn save_training_image(name: String, image: String) -> Result<TrainingSave, String> {
    let bytes = jpeg_bytes(&image)?;
    let root = local_config::project_root()?;
    save_jpeg(&root, &name, &bytes, &today_folder())
}

fn today_folder() -> String {
    chrono::Local::now().format("%d-%m-%Y").to_string()
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

fn resolve_training_dir(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return local_config::default_training_dir();
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err("Choose a full folder path.".to_string());
    }
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("The folder path cannot contain '..'.".to_string());
    }
    if local_config::path_is_program_files(&path) {
        return Err("Training images cannot be written under Program Files.".to_string());
    }
    Ok(path)
}

fn configured_dir(root: &Path) -> Result<PathBuf, String> {
    let doc = load_document(root)?;
    let raw = doc
        .get("trainingDir")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    resolve_training_dir(raw)
}

fn day_folder(day: &str) -> Result<String, String> {
    let bytes = day.as_bytes();
    let ok = bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| {
            if index == 2 || index == 5 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        });
    if ok {
        Ok(day.to_string())
    } else {
        Err("Could not choose today's folder.".to_string())
    }
}

fn suggest_in(root: &Path, day: &str) -> Result<TrainingName, String> {
    let folder = configured_dir(root)?;
    let day = day_folder(day)?;
    let base = stored_base(root)?;
    let name = next_free_name(&folder.join(&day), &base);
    Ok(TrainingName {
        name,
        folder: display_path(&folder),
        day,
    })
}

fn save_jpeg(root: &Path, name: &str, jpeg: &[u8], day: &str) -> Result<TrainingSave, String> {
    let stem = sanitize_stem(name)?;
    let folder = configured_dir(root)?;
    let day = day_folder(day)?;
    let dir = folder.join(&day);
    let path = dir.join(format!("{stem}.jpg"));
    if path.exists() {
        let next = next_free_name(&dir, &remember_base(&stem));
        return Ok(TrainingSave {
            saved: false,
            name: next,
            path: String::new(),
            message: "That file already exists. A free name is filled in.".to_string(),
        });
    }
    if local_config::path_is_program_files(&dir) {
        return Err("Training images cannot be written under Program Files.".to_string());
    }
    fs::create_dir_all(&dir).map_err(|err| {
        format!(
            "Could not create the training folder {}: {err}",
            dir.display()
        )
    })?;
    let opened = OpenOptions::new().write(true).create_new(true).open(&path);
    let mut file = match opened {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let next = next_free_name(&dir, &remember_base(&stem));
            return Ok(TrainingSave {
                saved: false,
                name: next,
                path: String::new(),
                message: "That file already exists. A free name is filled in.".to_string(),
            });
        }
        Err(err) => return Err(format!("Could not save the training image: {err}")),
    };
    file.write_all(jpeg)
        .map_err(|err| format!("Could not save the training image: {err}"))?;
    let base = remember_base(&stem);
    let message = match remember_name(root, &base) {
        Ok(()) => format!("Saved {stem}.jpg"),
        Err(err) => format!("Saved {stem}.jpg. {err}"),
    };
    Ok(TrainingSave {
        saved: true,
        name: stem,
        path: display_path(&path),
        message,
    })
}

fn stored_base(root: &Path) -> Result<String, String> {
    let doc = load_document(root)?;
    let raw = doc
        .get("trainingImageName")
        .and_then(Value::as_str)
        .unwrap_or("");
    Ok(base_from_raw(raw))
}

fn remember_name(root: &Path, base: &str) -> Result<(), String> {
    let mut doc = load_document(root)?;
    let object = doc
        .as_object_mut()
        .ok_or_else(|| "Could not remember the file name.".to_string())?;
    object.insert(
        "trainingImageName".to_string(),
        Value::String(base.to_string()),
    );
    save_document(root, &doc)
}

fn base_from_raw(raw: &str) -> String {
    let Ok(stem) = sanitize_stem(raw) else {
        return DEFAULT_BASE.to_string();
    };
    let base = remember_base(&stem);
    if base.is_empty() {
        DEFAULT_BASE.to_string()
    } else {
        base
    }
}

fn remember_base(stem: &str) -> String {
    let base = strip_copy_suffix(stem).trim().trim_end_matches(['.', ' ']);
    if base.is_empty() || is_reserved(base) {
        DEFAULT_BASE.to_string()
    } else {
        base.to_string()
    }
}

/// Smallest free name in `dir`. The plain base is used when `{base}.jpg` is absent.
/// Otherwise the name is `{base} (n)` for the smallest n >= 2.
fn next_free_name(dir: &Path, base: &str) -> String {
    let base = if base.is_empty() || is_reserved(base) {
        DEFAULT_BASE
    } else {
        base
    };
    if !dir.join(format!("{base}.jpg")).exists() {
        return base.to_string();
    }
    for index in 2..10_000 {
        let name = format!("{base} ({index})");
        if !dir.join(format!("{name}.jpg")).exists() {
            return name;
        }
    }
    format!("{base} (10000)")
}

fn strip_copy_suffix(stem: &str) -> &str {
    let Some(open) = stem.rfind(" (") else {
        return stem;
    };
    if !stem.ends_with(')') {
        return stem;
    }
    let digits = &stem[open + 2..stem.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return stem;
    }
    let base = stem[..open].trim_end();
    if base.is_empty() { stem } else { base }
}

fn strip_jpeg_ext(text: &str) -> String {
    let mut name = text.trim().to_string();
    loop {
        let lower = name.to_ascii_lowercase();
        let cut = if lower.ends_with(".jpeg") {
            5
        } else if lower.ends_with(".jpg") {
            4
        } else {
            break;
        };
        name = name[..name.len() - cut]
            .trim()
            .trim_end_matches(['.', ' '])
            .to_string();
        if name.is_empty() {
            break;
        }
    }
    name
}

fn sanitize_stem(raw: &str) -> Result<String, String> {
    let stripped = strip_jpeg_ext(raw);
    let mut cleaned = String::new();
    for ch in stripped.chars() {
        if ch.is_control() || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            continue;
        }
        cleaned.push(ch);
    }
    let clipped: String = cleaned
        .trim()
        .trim_end_matches(['.', ' '])
        .chars()
        .take(MAX_STEM)
        .collect();
    let clipped = clipped.trim().trim_end_matches(['.', ' ']).to_string();
    if clipped.is_empty() || is_reserved(&clipped) {
        return Err("Enter a file name.".to_string());
    }
    Ok(clipped)
}

fn is_reserved(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let bytes = upper.as_bytes();
    bytes.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && bytes[3].is_ascii_digit()
        && bytes[3] != b'0'
}

fn jpeg_bytes(raw: &str) -> Result<Vec<u8>, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("Could not read the snapshot.".to_string());
    }
    let payload = text.split_once(',').map(|(_, data)| data).unwrap_or(text);
    let compact: String = payload.chars().filter(|ch| !ch.is_whitespace()).collect();
    let bytes = STANDARD
        .decode(compact)
        .map_err(|_| "Could not read the snapshot.".to_string())?;
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Ok(bytes);
    }
    Err("The snapshot is not a JPEG.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder, RgbImage};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "ls-vision-training-{name}-{id}-{}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("vision")).unwrap();
            Self { root }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn tiny_jpeg(red: u8) -> Vec<u8> {
        let image = RgbImage::from_pixel(1, 1, image::Rgb([red, 20, 30]));
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 80)
            .write_image(image.as_raw(), 1, 1, ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    fn point_at(root: &Path, folder: &Path, name: &str) {
        let config = serde_json::json!({
            "trainingDir": folder,
            "trainingImageName": name,
            "source": "json"
        });
        save_document(root, &config).unwrap();
    }

    #[test]
    fn default_folder_is_under_documents() {
        let home = Path::new(r"C:\Users\operator");
        let dir = local_config::training_dir_for_home(home);
        assert_eq!(
            dir,
            PathBuf::from(r"C:\Users\operator\Documents\ls-vision\training")
        );
        let live = local_config::default_training_dir().unwrap();
        let text = live
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        assert!(text.ends_with("\\documents\\ls-vision\\training"), "{text}");
        assert!(!text.contains("\\appdata\\local\\"), "{text}");
        assert!(!local_config::path_is_program_files(&live), "{text}");
    }

    #[test]
    fn folder_rejects_program_files_and_parent_dirs() {
        assert!(
            resolve_training_dir(r"C:\Program Files\ls-vision\training")
                .unwrap_err()
                .contains("Program Files")
        );
        assert!(
            resolve_training_dir(r"C:\Users\operator\Documents\ls-vision\training\..\..")
                .unwrap_err()
                .contains("..")
        );
        assert!(resolve_training_dir("Documents\\ls-vision\\training").is_err());
    }

    #[test]
    fn names_drop_one_copy_suffix_and_illegal_characters() {
        assert_eq!(base_from_raw(""), "image");
        assert_eq!(base_from_raw("bracket (2).jpg"), "bracket");
        assert_eq!(base_from_raw("bracket (2) (3).jpeg"), "bracket (2)");
        assert_eq!(sanitize_stem("bad:name*?.jpg").unwrap(), "badname");
        assert!(sanitize_stem("CON.jpg").is_err());
        assert!(sanitize_stem("   ").is_err());
        assert!(sanitize_stem("LPT1").is_err());
    }

    #[test]
    fn suggestion_uses_the_base_until_today_has_that_file() {
        let project = TempProject::new("suggest");
        let folder = project.root.join("shots");
        point_at(&project.root, &folder, "bracket");
        let day = "20-10-2026";
        let first = suggest_in(&project.root, day).unwrap();
        assert_eq!(first.name, "bracket");
        assert_eq!(first.day, day);

        let dir = folder.join(day);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bracket.jpg"), b"a").unwrap();
        assert_eq!(suggest_in(&project.root, day).unwrap().name, "bracket (2)");

        fs::write(dir.join("bracket (2).jpg"), b"b").unwrap();
        assert_eq!(suggest_in(&project.root, day).unwrap().name, "bracket (3)");

        let other = "21-10-2026";
        assert_eq!(suggest_in(&project.root, other).unwrap().name, "bracket");
    }

    #[test]
    fn save_writes_the_date_folder_and_does_not_overwrite() {
        let project = TempProject::new("save");
        let folder = project.root.join("shots");
        point_at(&project.root, &folder, "");
        let day = "20-10-2026";
        let jpeg = tiny_jpeg(10);
        let saved = save_jpeg(&project.root, "bracket", &jpeg, day).unwrap();
        assert!(saved.saved);
        let path = folder.join(day).join("bracket.jpg");
        assert_eq!(PathBuf::from(&saved.path), path);
        assert_eq!(fs::read(&path).unwrap(), jpeg);
        let doc = load_document(&project.root).unwrap();
        assert_eq!(doc["trainingImageName"], "bracket");
        assert_eq!(doc["source"], "json");

        let again = save_jpeg(&project.root, "bracket", &tiny_jpeg(99), day).unwrap();
        assert!(!again.saved);
        assert_eq!(again.name, "bracket (2)");
        assert_eq!(fs::read(&path).unwrap(), jpeg);

        let copy = save_jpeg(&project.root, "bracket (2)", &tiny_jpeg(40), day).unwrap();
        assert!(copy.saved);
        assert!(folder.join(day).join("bracket (2).jpg").is_file());
        assert_eq!(
            load_document(&project.root).unwrap()["trainingImageName"],
            "bracket"
        );
    }
}
