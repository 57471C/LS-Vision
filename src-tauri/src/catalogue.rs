//! Part catalogue imported from a `.tar` archive.
//!
//! Settings > Parts > Update references picks a tar. The archive holds
//! `catalogue.json` and JPEG crops under `crops/`. Each crop is embedded with
//! the DINOv2 session that startup already opened. The result is written to
//! `vision/catalogue.json`, beside `vision/config.local.json`, and the JPEGs
//! are copied under `vision/catalogue-crops/`. Startup loads that file.
//! Detection and the camera preview keep using `vision/dinov2-references.json`.
//!
//! A catalogue entry looks like this:
//!
//! ```json
//! {
//!   "parts": [
//!     {
//!       "ic": "2BBMNM08",
//!       "description": "M8 nut",
//!       "type": "fastener",
//!       "dimensions": { "diameterMm": 8, "lengthMm": 12 },
//!       "crops": ["crops/2BBMNM08.jpg"]
//!     }
//!   ]
//! }
//! ```
//!
//! `ic`, `description` (`des`), `type` (`partType`), `dimensions`, and `crops`
//! (`images` or `files`) are read. A top-level array is also accepted.
//! A crop path is `crops/<file>.jpg` or `crops/<IC>/<file>.jpg`. The folder
//! after `crops/` groups those images under that IC. An IC is left out when
//! its crop list is empty or none of the listed files are in the tar.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

const CATALOGUE_FILE: &str = "catalogue.json";
const CROPS_DIR: &str = "catalogue-crops";
const VERSION: u32 = 1;
const MAX_TAR_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 20 * 1024 * 1024;
const MAX_PARTS: usize = 2_000;
const MAX_CROPS: usize = 24;
const MAX_PIXELS: usize = 40_000_000;
const MAX_TEXT: usize = 500;
const THUMB_EDGE: u32 = 160;

struct Part {
    ic: String,
    description: String,
    kind: String,
    dimensions: Value,
    crops: Vec<String>,
    embeddings: Vec<Vec<f32>>,
}

struct Book {
    parts: Vec<Part>,
}

impl Book {
    fn empty() -> Self {
        Self { parts: Vec::new() }
    }
}

struct Built {
    book: Book,
    files: Vec<(String, Vec<u8>)>,
}

static CATALOGUE: LazyLock<Mutex<Book>> = LazyLock::new(|| Mutex::new(Book::empty()));

fn lock(mutex: &Mutex<Book>) -> std::sync::MutexGuard<'_, Book> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn vision_directory() -> Result<PathBuf, String> {
    Ok(crate::local_config::project_root()?.join("vision"))
}

fn catalogue_path(vision: &Path) -> PathBuf {
    vision.join(CATALOGUE_FILE)
}

fn log_catalogue_path(path: &Path) {
    eprintln!("DINOv2 catalogue path: {}", path.display());
}

/// Load `vision/catalogue.json`. A missing file leaves memory as it is.
/// A bad file is a warning and also leaves memory as it is.
pub(crate) fn load_on_startup() {
    if cfg!(test) {
        return;
    }
    let vision = match vision_directory() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("warning: DINOv2 catalogue was not loaded: {err}");
            return;
        }
    };
    let path = catalogue_path(&vision);
    match load_book(&vision) {
        Ok(Some(book)) => {
            let count = book.parts.len();
            *lock(&CATALOGUE) = book;
            eprintln!(
                "DINOv2 catalogue loaded: {count} parts from {}",
                path.display()
            );
        }
        Ok(None) => {}
        Err(err) => eprintln!("warning: {err} In-memory catalogue is unchanged."),
    }
}

#[derive(Clone, Serialize)]
pub struct CatalogueUpdate {
    pub parts: usize,
    pub message: String,
    pub path: String,
}

#[derive(Clone, Serialize)]
pub struct CatalogueView {
    pub ic: String,
    pub description: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub dimensions: String,
    pub thumbs: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct CatalogueList {
    pub path: String,
    pub parts: Vec<CatalogueView>,
}

/// Native tar picker, then embed each crop and replace the saved catalogue.
/// Cancel leaves the saved file and the in-memory catalogue as they are.
/// The dialog runs off the main thread. A panic becomes a status string.
#[tauri::command]
pub async fn import_catalogue(app: AppHandle) -> Result<CatalogueUpdate, String> {
    let vision = vision_directory()?;
    let path = catalogue_path(&vision);
    log_catalogue_path(&path);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            install_picked_catalogue(app, vision)
        }))
    })
    .await;
    match joined {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => Err("Could not update the catalogue".to_string()),
    }
}

fn install_picked_catalogue(app: AppHandle, vision: PathBuf) -> Result<CatalogueUpdate, String> {
    let path = catalogue_path(&vision);
    let mut picker = app
        .dialog()
        .file()
        .add_filter("Tar archive", &["tar"])
        .set_title("Update references");
    if let Some(window) = app.get_webview_window("main") {
        picker = picker.set_parent(&window);
    }
    if vision.is_dir() {
        picker = picker.set_directory(&vision);
    }
    let Some(picked) = picker.blocking_pick_file() else {
        return Ok(CatalogueUpdate {
            parts: lock(&CATALOGUE).parts.len(),
            message: "No file selected.".to_string(),
            path: path.display().to_string(),
        });
    };
    let source = picked
        .into_path()
        .map_err(|_| "Could not read the selected file.".to_string())?;
    if !source
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tar"))
    {
        return Err("Selected file is not a tar archive.".to_string());
    }
    let meta =
        fs::metadata(&source).map_err(|err| format!("Could not read the tar archive: {err}"))?;
    if meta.len() > MAX_TAR_BYTES {
        return Err("The tar archive is too large.".to_string());
    }
    let bytes =
        fs::read(&source).map_err(|err| format!("Could not read the tar archive: {err}"))?;
    let count = import_tar_bytes(&bytes, &vision, &mut embed_jpeg)?;
    eprintln!(
        "DINOv2 catalogue loaded: {count} parts from {}",
        path.display()
    );
    Ok(CatalogueUpdate {
        parts: count,
        message: format!("Catalogue loaded {count} parts."),
        path: path.display().to_string(),
    })
}

/// Read the saved catalogue and return one view per part.
/// A missing file is an empty list and does not clear memory.
#[tauri::command]
pub async fn list_catalogue() -> Result<CatalogueList, String> {
    let vision = vision_directory()?;
    let path = catalogue_path(&vision);
    log_catalogue_path(&path);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            catalogue_list_from(&vision)
        }))
    })
    .await;
    match joined {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => Err("Could not load the catalogue".to_string()),
    }
}

fn catalogue_list_from(vision: &Path) -> Result<CatalogueList, String> {
    let path = catalogue_path(vision);
    let loaded = load_book(vision)?;
    let book = match loaded {
        Some(book) => {
            let stored = book;
            let views = views_from(&stored, vision);
            *lock(&CATALOGUE) = stored;
            views
        }
        None => Vec::new(),
    };
    Ok(CatalogueList {
        path: path.display().to_string(),
        parts: book,
    })
}

/// Remove one part from memory, from the catalogue file, and from its crop folder.
#[tauri::command]
pub fn delete_catalogue_part(ic: String) -> Result<(), String> {
    let vision = vision_directory()?;
    remove_part(&vision, &ic)
}

fn embed_jpeg(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let (rgb, width, height) = decode_jpeg(bytes)?;
    // Same 224×224 stretch the box matcher uses, so a stored crop matches itself.
    crate::inference::dino::embed_rgb(&rgb, width, height)
}

fn import_tar_bytes(
    bytes: &[u8],
    vision: &Path,
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
) -> Result<usize, String> {
    let built = build_from_tar(bytes, embed)?;
    let count = built.book.parts.len();
    commit_book(vision, &built)?;
    *lock(&CATALOGUE) = built.book;
    crate::catalog::reload_installed();
    Ok(count)
}

struct Archive {
    catalogue: String,
    files: HashMap<String, Vec<u8>>,
}

fn read_tar(bytes: &[u8]) -> Result<Archive, String> {
    if bytes.len() as u64 > MAX_TAR_BYTES {
        return Err("The tar archive is too large.".to_string());
    }
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let entries = archive
        .entries()
        .map_err(|err| format!("Could not read the tar archive: {err}"))?;
    let mut files = HashMap::new();
    for entry in entries {
        let mut entry = entry.map_err(|err| format!("Could not read the tar archive: {err}"))?;
        let raw = entry
            .path()
            .map_err(|err| format!("Could not read the tar archive: {err}"))?;
        let Some(key) = entry_key(raw.as_ref())? else {
            continue;
        };
        let size = entry.size();
        if size > MAX_ENTRY_BYTES {
            return Err("A file in the archive is too large.".to_string());
        }
        if !entry.header().entry_type().is_file() {
            io::copy(&mut entry, &mut io::sink())
                .map_err(|err| format!("Could not read the tar archive: {err}"))?;
            continue;
        }
        if !is_catalogue_key(&key) && !is_crop_key(&key) {
            io::copy(&mut entry, &mut io::sink())
                .map_err(|err| format!("Could not read the tar archive: {err}"))?;
            continue;
        }
        let mut body = Vec::new();
        entry
            .take(MAX_ENTRY_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|err| format!("Could not read the tar archive: {err}"))?;
        if body.len() as u64 > MAX_ENTRY_BYTES {
            return Err("A file in the archive is too large.".to_string());
        }
        files.insert(key, body);
    }
    let (catalogue_key, prefix) = locate_catalogue(&files)?;
    let catalogue = String::from_utf8(files.remove(&catalogue_key).unwrap_or_default())
        .map_err(|_| "catalogue.json is not valid JSON.".to_string())?;
    let mut crops = HashMap::new();
    for (key, body) in files {
        if let Some(name) = crop_name(&key, &prefix) {
            crops.insert(format!("crops/{name}"), body);
        }
    }
    Ok(Archive {
        catalogue: strip_bom(&catalogue).to_string(),
        files: crops,
    })
}

fn entry_key(path: &Path) -> Result<Option<String>, String> {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') || text.starts_with("//") {
        return Err("The archive has an unsafe path.".to_string());
    }
    let mut parts = Vec::new();
    for segment in text.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains('\0') {
            return Err("The archive has an unsafe path.".to_string());
        }
        parts.push(segment);
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts.join("/")))
}

fn is_catalogue_key(key: &str) -> bool {
    key == "catalogue.json" || key.ends_with("/catalogue.json")
}

fn is_crop_key(key: &str) -> bool {
    crop_name(key, "").is_some() || key.contains("/crops/")
}

fn crop_name<'a>(key: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = key.strip_prefix(prefix).unwrap_or(key);
    let name = rest.strip_prefix("crops/")?;
    if !jpeg_crop_path(name) {
        return None;
    }
    Some(name)
}

/// `crops/<file>.jpg` and `crops/<IC>/<file>.jpg` are both crop files.
fn jpeg_crop_path(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut file_name = "";
    for part in name.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return false;
        }
        file_name = part;
    }
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".jpg") || lower.ends_with(".jpeg")
}

fn locate_catalogue(files: &HashMap<String, Vec<u8>>) -> Result<(String, String), String> {
    if files.contains_key("catalogue.json") {
        return Ok(("catalogue.json".to_string(), String::new()));
    }
    let mut nested: Vec<String> = files
        .keys()
        .filter(|key| key.ends_with("/catalogue.json") && key.matches('/').count() == 1)
        .cloned()
        .collect();
    nested.sort();
    if nested.len() == 1 {
        let key = nested.remove(0);
        let prefix = key.strip_suffix("catalogue.json").unwrap_or("").to_string();
        return Ok((key, prefix));
    }
    if nested.len() > 1 {
        return Err("The archive has more than one catalogue.json.".to_string());
    }
    Err("The archive has no catalogue.json.".to_string())
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

struct SourcePart {
    ic: String,
    description: String,
    kind: String,
    dimensions: Value,
    crops: Vec<String>,
}

fn parse_source(text: &str) -> Result<Vec<SourcePart>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| "catalogue.json is not valid JSON.".to_string())?;
    let items = match value {
        Value::Array(items) => items,
        Value::Object(map) => {
            let list = map
                .get("parts")
                .or_else(|| map.get("items"))
                .or_else(|| map.get("catalogue"))
                .or_else(|| map.get("entries"))
                .ok_or_else(|| "catalogue.json has no parts.".to_string())?;
            list.as_array()
                .cloned()
                .ok_or_else(|| "catalogue.json has no parts.".to_string())?
        }
        _ => return Err("catalogue.json has no parts.".to_string()),
    };
    if items.is_empty() {
        return Err("Catalogue has no parts.".to_string());
    }
    if items.len() > MAX_PARTS {
        return Err("The catalogue has too many parts.".to_string());
    }
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        let Value::Object(map) = item else {
            return Err("catalogue.json has a part that is not an object.".to_string());
        };
        parts.push(source_part(&map)?);
    }
    Ok(merge_parts(parts)?)
}

fn source_part(map: &Map<String, Value>) -> Result<SourcePart, String> {
    let ic = clean_ic(&field_string(
        map,
        &["ic", "IC", "code", "inventoryCode", "inventory_code"],
    ))?;
    let description = bounded_text(
        &field_string(map, &["description", "des", "Description"]),
        &format!("Description for {ic}"),
    )?;
    let kind = bounded_text(
        &field_string(map, &["type", "partType", "part_type"]),
        &format!("Type for {ic}"),
    )?;
    let dimensions = map
        .get("dimensions")
        .or_else(|| map.get("dimension"))
        .or_else(|| map.get("size"))
        .cloned()
        .unwrap_or(Value::Null);
    if dimensions.to_string().len() > MAX_TEXT {
        return Err(format!("Dimensions for {ic} are too long."));
    }
    let crops = crop_list(map, &ic)?;
    Ok(SourcePart {
        ic,
        description,
        kind,
        dimensions,
        crops,
    })
}

fn field_string(map: &Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        if let Some(Value::String(text)) = map.get(*key) {
            let text = text.trim();
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }
    String::new()
}

fn bounded_text(text: &str, label: &str) -> Result<String, String> {
    let text = text.trim();
    if text.chars().count() > MAX_TEXT {
        return Err(format!("{label} is too long."));
    }
    Ok(text.to_string())
}

fn clean_ic(raw: &str) -> Result<String, String> {
    let ic = raw.trim();
    if ic.is_empty() {
        return Err("A part is missing an IC.".to_string());
    }
    if ic.chars().count() > 80 {
        return Err(format!("IC {ic} is too long."));
    }
    if ic
        .chars()
        .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
    {
        return Err(format!("IC {ic} has an unsupported character."));
    }
    Ok(ic.to_string())
}

fn crop_list(map: &Map<String, Value>, ic: &str) -> Result<Vec<String>, String> {
    let value = map
        .get("crops")
        .or_else(|| map.get("images"))
        .or_else(|| map.get("files"))
        .or_else(|| map.get("crop"))
        .or_else(|| map.get("image"));
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items: Vec<String> = match value {
        Value::String(text) => vec![text.clone()],
        Value::Array(list) => list
            .iter()
            .map(|item| match item {
                Value::String(text) => Ok(text.clone()),
                _ => Err(format!("Part {ic} has a crop that is not a file name.")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(format!("Part {ic} has no crops.")),
    };
    if items.is_empty() {
        return Ok(Vec::new());
    }
    if items.len() > MAX_CROPS {
        return Err(format!("Part {ic} has too many crops."));
    }
    let mut crops = Vec::with_capacity(items.len());
    for item in items {
        crops.push(normalise_crop(&item, ic)?);
    }
    Ok(crops)
}

fn normalise_crop(raw: &str, ic: &str) -> Result<String, String> {
    let text = raw.trim().replace('\\', "/");
    let text = text.trim_start_matches("./");
    if text.is_empty() || text.contains('\0') || text.split('/').any(|part| part == "..") {
        return Err(format!("Part {ic} names a crop outside crops/."));
    }
    let name = if let Some(rest) = text.strip_prefix("crops/") {
        rest
    } else if !text.contains('/') {
        text
    } else {
        return Err(format!("Part {ic} names a crop outside crops/."));
    };
    if !jpeg_crop_path(name) {
        let label = name.rsplit('/').next().unwrap_or(name);
        if label.is_empty() || name.split('/').any(|part| part.is_empty() || part == ".") {
            return Err(format!("Part {ic} names a crop outside crops/."));
        }
        return Err(format!("Crop {label} is not a JPEG."));
    }
    Ok(format!("crops/{name}"))
}

/// Folder after `crops/` when the file lives at `crops/<IC>/<file>`.
/// A file directly in `crops/` stays with the part that listed it.
fn crop_group(key: &str, part_ic: &str) -> String {
    let Some(rest) = key.strip_prefix("crops/") else {
        return part_ic.to_string();
    };
    match rest.split_once('/') {
        Some((folder, file)) if !folder.is_empty() && !file.is_empty() => folder.to_string(),
        _ => part_ic.to_string(),
    }
}

fn group_crops(sources: &[SourcePart]) -> HashMap<String, Vec<String>> {
    let known: HashSet<&str> = sources.iter().map(|part| part.ic.as_str()).collect();
    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
    for source in sources {
        grouped.entry(source.ic.clone()).or_default();
        for key in &source.crops {
            let folder = crop_group(key, &source.ic);
            let group = if known.contains(folder.as_str()) {
                folder
            } else {
                source.ic.clone()
            };
            grouped.entry(group).or_default().push(key.clone());
        }
    }
    grouped
}

fn log_resolved_crops(ic: &str, count: usize) {
    eprintln!("DINOv2 catalogue {ic}: {count} crops");
}

fn merge_parts(parts: Vec<SourcePart>) -> Result<Vec<SourcePart>, String> {
    let mut merged: Vec<SourcePart> = Vec::new();
    for part in parts {
        if let Some(existing) = merged.iter_mut().find(|item| item.ic == part.ic) {
            if !part.description.is_empty() {
                existing.description = part.description;
            }
            if !part.kind.is_empty() {
                existing.kind = part.kind;
            }
            if !part.dimensions.is_null() {
                existing.dimensions = part.dimensions;
            }
            existing.crops.extend(part.crops);
            if existing.crops.len() > MAX_CROPS {
                return Err(format!("Part {} has too many crops.", existing.ic));
            }
        } else {
            merged.push(part);
        }
    }
    Ok(merged)
}

fn build_from_tar(
    bytes: &[u8],
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
) -> Result<Built, String> {
    let archive = read_tar(bytes)?;
    let sources = parse_source(&archive.catalogue)?;
    let mut grouped = group_crops(&sources);
    let mut used = HashSet::new();
    let mut parts = Vec::with_capacity(sources.len());
    let mut files = Vec::new();
    for source in sources {
        let keys = grouped.remove(&source.ic).unwrap_or_default();
        let mut resolved = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(jpeg) = archive.files.get(&key) {
                resolved.push((key, jpeg.clone()));
            }
        }
        log_resolved_crops(&source.ic, resolved.len());
        if resolved.is_empty() {
            continue;
        }
        if resolved.len() > MAX_CROPS {
            return Err(format!("Part {} has too many crops.", source.ic));
        }
        let folder = allocate_folder(&mut used, &source.ic);
        let mut crops = Vec::with_capacity(resolved.len());
        let mut embeddings = Vec::with_capacity(resolved.len());
        for (index, (key, jpeg)) in resolved.iter().enumerate() {
            let values =
                embed(jpeg).map_err(|err| format!("Could not embed {}: {err}", source.ic))?;
            if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
                return Err(format!("Could not embed {}.", source.ic));
            }
            let file_name = stored_crop_name(key, index);
            let relative = format!("{CROPS_DIR}/{folder}/{file_name}");
            crops.push(relative.clone());
            files.push((relative, jpeg.clone()));
            embeddings.push(values);
        }
        parts.push(Part {
            ic: source.ic,
            description: source.description,
            kind: source.kind,
            dimensions: source.dimensions,
            crops,
            embeddings,
        });
    }
    if parts.is_empty() {
        return Err("Catalogue has no parts.".to_string());
    }
    Ok(Built {
        book: Book { parts },
        files,
    })
}

fn allocate_folder(used: &mut HashSet<String>, ic: &str) -> String {
    let base = sanitize_ic(ic);
    if used.insert(base.clone()) {
        return base;
    }
    for index in 2..10_000 {
        let name = format!("{base}-{index}");
        if used.insert(name.clone()) {
            return name;
        }
    }
    let name = format!("{base}-part");
    used.insert(name.clone());
    name
}

fn sanitize_ic(ic: &str) -> String {
    let mut out = String::new();
    for ch in ic.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let out: String = out.chars().take(60).collect();
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "part".to_string()
    } else {
        out
    }
}

fn stored_crop_name(key: &str, index: usize) -> String {
    let base = key.rsplit('/').next().unwrap_or("crop.jpg");
    let mut clean = String::new();
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            clean.push(ch);
        } else {
            clean.push('_');
        }
    }
    let lower = clean.to_ascii_lowercase();
    if !lower.ends_with(".jpg") && !lower.ends_with(".jpeg") {
        clean.push_str(".jpg");
    }
    if clean.chars().count() > 80 {
        clean = clean
            .chars()
            .rev()
            .take(80)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
    }
    format!("{index}-{clean}")
}

fn commit_book(vision: &Path, built: &Built) -> Result<(), String> {
    fs::create_dir_all(vision).map_err(|err| format!("Could not write the catalogue: {err}"))?;
    let incoming = vision.join(format!("{CROPS_DIR}.incoming"));
    let dest = vision.join(CROPS_DIR);
    let backup = vision.join(format!("{CROPS_DIR}.previous"));
    let json_path = catalogue_path(vision);
    let json_tmp = vision.join("catalogue.json.tmp");
    let _ = fs::remove_dir_all(&incoming);
    let _ = fs::remove_dir_all(&backup);
    fs::create_dir_all(&incoming).map_err(|err| format!("Could not write the catalogue: {err}"))?;
    for (relative, bytes) in &built.files {
        let name = relative
            .strip_prefix(&format!("{CROPS_DIR}/"))
            .ok_or_else(|| "Could not write the catalogue.".to_string())?;
        let path = incoming.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("Could not write the catalogue: {err}"))?;
        }
        fs::write(&path, bytes).map_err(|err| format!("Could not write the catalogue: {err}"))?;
    }
    let text = book_json(&built.book)?;
    fs::write(&json_tmp, text).map_err(|err| format!("Could not write the catalogue: {err}"))?;
    let had_dest = dest.exists();
    if had_dest {
        fs::rename(&dest, &backup).map_err(|err| {
            let _ = fs::remove_file(&json_tmp);
            let _ = fs::remove_dir_all(&incoming);
            format!("Could not write the catalogue: {err}")
        })?;
    }
    if let Err(err) = fs::rename(&incoming, &dest) {
        if had_dest && backup.exists() && !dest.exists() {
            let _ = fs::rename(&backup, &dest);
        }
        let _ = fs::remove_file(&json_tmp);
        let _ = fs::remove_dir_all(&incoming);
        return Err(format!("Could not write the catalogue: {err}"));
    }
    if let Err(err) = replace_file(&json_tmp, &json_path) {
        let _ = fs::remove_dir_all(&dest);
        if had_dest && backup.exists() {
            let _ = fs::rename(&backup, &dest);
        }
        return Err(err);
    }
    let _ = fs::remove_dir_all(&backup);
    Ok(())
}

fn book_json(book: &Book) -> Result<Vec<u8>, String> {
    let file = StoredFile {
        version: VERSION,
        parts: book
            .parts
            .iter()
            .map(|part| StoredPart {
                ic: part.ic.clone(),
                description: part.description.clone(),
                kind: part.kind.clone(),
                dimensions: part.dimensions.clone(),
                crops: part.crops.clone(),
                embeddings: part.embeddings.clone(),
            })
            .collect(),
    };
    let mut bytes =
        serde_json::to_vec(&file).map_err(|_| "Could not write the catalogue.".to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn replace_file(tmp: &Path, path: &Path) -> Result<(), String> {
    if fs::rename(tmp, path).is_ok() {
        return Ok(());
    }
    if path.exists() {
        fs::remove_file(path).map_err(|err| {
            let _ = fs::remove_file(tmp);
            format!("Could not replace the catalogue: {err}")
        })?;
    }
    fs::rename(tmp, path).map_err(|err| {
        let _ = fs::remove_file(tmp);
        format!("Could not replace the catalogue: {err}")
    })
}

#[derive(Serialize, Deserialize)]
struct StoredFile {
    version: u32,
    parts: Vec<StoredPart>,
}

#[derive(Serialize, Deserialize)]
struct StoredPart {
    ic: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    dimensions: Value,
    crops: Vec<String>,
    embeddings: Vec<Vec<f32>>,
}

/// `Ok(None)` when the file is absent.
fn load_book(vision: &Path) -> Result<Option<Book>, String> {
    let path = catalogue_path(vision);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("Could not read the catalogue: {err}")),
    };
    let file: StoredFile = serde_json::from_str(&text)
        .map_err(|_| format!("DINOv2 catalogue at {} is not valid JSON.", path.display()))?;
    if file.version != VERSION {
        return Err(format!(
            "DINOv2 catalogue at {} is not a valid catalogue.",
            path.display()
        ));
    }
    if file.parts.len() > MAX_PARTS {
        return Err("The catalogue has too many parts.".to_string());
    }
    let mut parts = Vec::with_capacity(file.parts.len());
    for part in file.parts {
        parts.push(stored_part(part, &path)?);
    }
    Ok(Some(Book { parts }))
}

fn stored_part(part: StoredPart, path: &Path) -> Result<Part, String> {
    let invalid = || {
        format!(
            "DINOv2 catalogue at {} is not a valid catalogue.",
            path.display()
        )
    };
    let ic = clean_ic(&part.ic).map_err(|_| invalid())?;
    if part.crops.len() != part.embeddings.len() || part.crops.len() > MAX_CROPS {
        return Err(invalid());
    }
    for crop in &part.crops {
        if !safe_crop_path(crop) {
            return Err(invalid());
        }
    }
    for values in &part.embeddings {
        if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
            return Err(invalid());
        }
    }
    Ok(Part {
        ic,
        description: part.description.trim().to_string(),
        kind: part.kind.trim().to_string(),
        dimensions: part.dimensions,
        crops: part.crops,
        embeddings: part.embeddings,
    })
}

fn safe_crop_path(path: &str) -> bool {
    if !path.starts_with(&format!("{CROPS_DIR}/")) || path.contains('\\') || path.contains('\0') {
        return false;
    }
    !path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
}

fn remove_part(vision: &Path, ic: &str) -> Result<(), String> {
    let ic = ic.trim();
    let Some(mut book) = load_book(vision)? else {
        return Err("That part is not in the catalogue.".to_string());
    };
    let Some(index) = book.parts.iter().position(|part| part.ic == ic) else {
        return Err("That part is not in the catalogue.".to_string());
    };
    let removed = book.parts.remove(index);
    let bytes = book_json(&book)?;
    replace_file_bytes(&catalogue_path(vision), &bytes)?;
    *lock(&CATALOGUE) = book;
    crate::catalog::reload_installed();
    for crop in &removed.crops {
        if let Some(path) = crop_file(vision, crop) {
            let _ = fs::remove_file(&path);
            if let Some(parent) = path.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }
    Ok(())
}

fn replace_file_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Could not write the catalogue: {err}"))?;
    }
    fs::write(&tmp, bytes).map_err(|err| format!("Could not write the catalogue: {err}"))?;
    replace_file(&tmp, path)
}

fn views_from(book: &Book, vision: &Path) -> Vec<CatalogueView> {
    book.parts
        .iter()
        .map(|part| CatalogueView {
            ic: part.ic.clone(),
            description: part.description.clone(),
            kind: part.kind.clone(),
            dimensions: dimensions_text(&part.dimensions),
            thumbs: part
                .crops
                .iter()
                .filter_map(|crop| thumb_url(vision, crop))
                .collect(),
        })
        .collect()
}

fn dimensions_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Array(items) => items
            .iter()
            .map(dimensions_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            keys.into_iter()
                .filter_map(|key| {
                    let text = dimensions_text(&map[key]);
                    if text.is_empty() {
                        None
                    } else {
                        Some(format!("{key} {text}"))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

fn thumb_url(vision: &Path, crop: &str) -> Option<String> {
    let path = crop_file(vision, crop)?;
    let bytes = fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let thumb = image.thumbnail(THUMB_EDGE, THUMB_EDGE);
    let mut encoded = Vec::new();
    thumb
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Jpeg,
        )
        .ok()?;
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(encoded)
    ))
}

fn crop_file(vision: &Path, crop: &str) -> Option<PathBuf> {
    if !safe_crop_path(crop) {
        return None;
    }
    let relative = crop.strip_prefix(&format!("{CROPS_DIR}/"))?;
    let path = vision.join(CROPS_DIR).join(relative);
    Some(path)
}

fn decode_jpeg(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    if bytes.len() < 2 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return Err("Could not read the image.".to_string());
    }
    let image = turbojpeg::decompress(bytes, turbojpeg::PixelFormat::RGB)
        .map_err(|_| "Could not read the image.".to_string())?;
    let width = u32::try_from(image.width).map_err(|_| "Could not read the image.".to_string())?;
    let height =
        u32::try_from(image.height).map_err(|_| "Could not read the image.".to_string())?;
    let pixels = (width as usize).saturating_mul(height as usize);
    if width < 1 || height < 1 || pixels > MAX_PIXELS {
        return Err("Could not read the image.".to_string());
    }
    let row = image.width.saturating_mul(3);
    if image.pitch < row {
        return Err("Could not read the image.".to_string());
    }
    if image.pitch == row {
        return Ok((image.pixels, width, height));
    }
    let mut packed = Vec::with_capacity(row.saturating_mul(image.height));
    for y in 0..image.height {
        let start = y.saturating_mul(image.pitch);
        let end = start + row;
        if end > image.pixels.len() {
            return Err("Could not read the image.".to_string());
        }
        packed.extend_from_slice(&image.pixels[start..end]);
    }
    Ok((packed, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ls-vision-catalogue-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tiny_jpeg() -> Vec<u8> {
        let image = turbojpeg::Image {
            pixels: &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0][..],
            width: 2,
            pitch: 6,
            height: 2,
            format: turbojpeg::PixelFormat::RGB,
        };
        turbojpeg::compress(image, 90, turbojpeg::Subsamp::Sub2x2)
            .expect("jpeg")
            .as_ref()
            .to_vec()
    }

    fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, *bytes).unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn stub_embed(bytes: &[u8]) -> Result<Vec<f32>, String> {
        if bytes.len() < 2 || bytes[0] != 0xff {
            return Err("Could not read the image.".to_string());
        }
        Ok(vec![1.0, 0.0, 0.0, bytes.len() as f32])
    }

    #[test]
    fn unsafe_archive_paths_are_refused() {
        assert!(entry_key(Path::new("crops/../../x.jpg")).is_err());
        assert!(entry_key(Path::new("/etc/passwd")).is_err());
        assert!(entry_key(Path::new("crops/../catalogue.json")).is_err());
        assert_eq!(
            entry_key(Path::new("./crops/a.jpg")).unwrap().unwrap(),
            "crops/a.jpg"
        );
    }

    #[test]
    fn tar_import_stores_parts_crops_and_embeddings() {
        let jpeg = tiny_jpeg();
        let other = tiny_jpeg();
        let catalogue = br#"{
            "parts": [
                {
                    "ic": "2BBMNM08",
                    "description": "M8 nut",
                    "type": "fastener",
                    "dimensions": { "diameterMm": 8, "lengthMm": 12 },
                    "crops": ["crops/nut.jpg", "side.jpg"]
                },
                {
                    "IC": "2BBMNM08",
                    "des": "M8 nyloc",
                    "crops": ["crops/nut-b.jpeg"]
                },
                {
                    "code": "2ECT",
                    "partType": "label",
                    "size": "20 mm",
                    "images": ["crops/label.jpg"]
                }
            ]
        }"#;
        let bytes = pack(&[
            ("catalogue.json", catalogue),
            ("crops/nut.jpg", &jpeg),
            ("crops/side.jpg", &other),
            ("crops/nut-b.jpeg", &jpeg),
            ("crops/label.jpg", &other),
            ("readme.txt", b"ignore me"),
        ]);
        let dir = temp_dir("import");
        let count = import_tar_bytes(&bytes, &dir, &mut stub_embed).unwrap();
        assert_eq!(count, 2);
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts.len(), 2);
        assert_eq!(book.parts[0].ic, "2BBMNM08");
        assert_eq!(book.parts[0].description, "M8 nyloc");
        assert_eq!(book.parts[0].kind, "fastener");
        assert_eq!(
            dimensions_text(&book.parts[0].dimensions),
            "diameterMm 8, lengthMm 12"
        );
        assert_eq!(book.parts[0].crops.len(), 3);
        assert_eq!(book.parts[0].embeddings.len(), 3);
        assert!(book.parts[0].crops[0].starts_with("catalogue-crops/2BBMNM08/"));
        assert!(dir.join(&book.parts[0].crops[0]).is_file());
        assert_eq!(book.parts[1].ic, "2ECT");
        assert_eq!(book.parts[1].kind, "label");
        assert_eq!(book.parts[1].dimensions, Value::String("20 mm".to_string()));
        let listed = catalogue_list_from(&dir).unwrap();
        assert_eq!(listed.parts.len(), 2);
        assert_eq!(listed.parts[0].description, "M8 nyloc");
        assert_eq!(listed.parts[0].kind, "fastener");
        assert_eq!(listed.parts[0].dimensions, "diameterMm 8, lengthMm 12");
        assert_eq!(listed.parts[0].thumbs.len(), 3);
        assert!(listed.parts[0].thumbs[0].starts_with("data:image/jpeg;base64,"));

        remove_part(&dir, "2ECT").unwrap();
        let left = load_book(&dir).unwrap().unwrap();
        assert_eq!(left.parts.len(), 1);
        assert_eq!(left.parts[0].ic, "2BBMNM08");
        assert!(remove_part(&dir, "2ECT").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_embed_leaves_the_saved_catalogue() {
        let dir = temp_dir("keep");
        let jpeg = tiny_jpeg();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"KEEP","crops":["crops/a.jpg"]}]}"#,
            ),
            ("crops/a.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, &mut stub_embed).unwrap();
        let before = fs::read(catalogue_path(&dir)).unwrap();
        let second = pack(&[
            (
                "pack/catalogue.json",
                br#"{"parts":[{"ic":"NEW","crops":["crops/b.jpg"]}]}"#,
            ),
            ("pack/crops/b.jpg", jpeg.as_slice()),
        ]);
        let err = import_tar_bytes(&second, &dir, &mut |_| {
            Err("DINOv2 model is not loaded.".to_string())
        })
        .unwrap_err();
        assert!(err.contains("DINOv2 model is not loaded"), "{err}");
        assert_eq!(fs::read(catalogue_path(&dir)).unwrap(), before);
        assert!(!dir.join("catalogue-crops.incoming").exists());
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts[0].ic, "KEEP");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_catalogue_is_refused() {
        let jpeg = tiny_jpeg();
        let missing = pack(&[("crops/a.jpg", jpeg.as_slice())]);
        let missing_err = match read_tar(&missing) {
            Err(err) => err,
            Ok(_) => panic!("expected a missing catalogue"),
        };
        assert!(missing_err.contains("no catalogue.json"), "{missing_err}");
        let bad = pack(&[("catalogue.json", b"{"), ("crops/a.jpg", jpeg.as_slice())]);
        let bad_err = match build_from_tar(&bad, &mut stub_embed) {
            Err(err) => err,
            Ok(_) => panic!("expected invalid JSON"),
        };
        assert!(bad_err.contains("not valid JSON"), "{bad_err}");
        let outside = pack(&[(
            "catalogue.json",
            br#"{"parts":[{"ic":"X","crops":["../secret.jpg"]}]}"#,
        )]);
        let outside_err = match build_from_tar(&outside, &mut stub_embed) {
            Err(err) => err,
            Ok(_) => panic!("expected a crop outside crops/"),
        };
        assert!(outside_err.contains("outside crops"), "{outside_err}");
        let empty = pack(&[("catalogue.json", br#"{"parts":[]}"#)]);
        let empty_err = match build_from_tar(&empty, &mut stub_embed) {
            Err(err) => err,
            Ok(_) => panic!("expected no parts"),
        };
        assert!(empty_err.contains("no parts"), "{empty_err}");
    }

    #[test]
    fn ic_folder_crops_are_imported_and_empty_ics_are_skipped() {
        let jpeg = tiny_jpeg();
        let catalogue = br#"{
            "parts": [
                {
                    "ic": "2ERGM22",
                    "description": "Hinge plate",
                    "type": "plate",
                    "dimensions": "10 mm",
                    "crops": [
                        "crops/2ERGM22/000001_crop_007.jpg",
                        "crops/2ERGM22/000001_crop_008.jpg"
                    ]
                },
                { "ic": "EMPTY", "description": "No crops", "crops": [] },
                {
                    "ic": "GONE",
                    "description": "Missing file",
                    "crops": ["crops/GONE/absent.jpg"]
                },
                { "ic": "FLAT", "description": "Flat crop", "crops": ["crops/flat.jpg"] },
                {
                    "ic": "OTHER",
                    "description": "Listed under the hinge folder",
                    "crops": ["crops/2ERGM22/000001_crop_009.jpg"]
                }
            ]
        }"#;
        let bytes = pack(&[
            ("catalogue.json", catalogue),
            ("crops/2ERGM22/000001_crop_007.jpg", &jpeg),
            ("crops/2ERGM22/000001_crop_008.jpg", &jpeg),
            ("crops/2ERGM22/000001_crop_009.jpg", &jpeg),
            ("crops/flat.jpg", &jpeg),
        ]);
        let built = build_from_tar(&bytes, &mut stub_embed).unwrap();
        assert_eq!(built.book.parts.len(), 2);
        assert_eq!(built.book.parts[0].ic, "2ERGM22");
        assert_eq!(built.book.parts[0].description, "Hinge plate");
        assert_eq!(built.book.parts[0].kind, "plate");
        assert_eq!(
            built.book.parts[0].dimensions,
            Value::String("10 mm".to_string())
        );
        assert_eq!(built.book.parts[0].crops.len(), 3);
        assert_eq!(built.book.parts[0].embeddings.len(), 3);
        assert!(built.book.parts[0].crops[0].ends_with("000001_crop_007.jpg"));
        assert!(built.book.parts[0].crops[1].ends_with("000001_crop_008.jpg"));
        assert!(built.book.parts[0].crops[2].ends_with("000001_crop_009.jpg"));
        assert_eq!(built.book.parts[1].ic, "FLAT");
        assert_eq!(built.book.parts[1].crops.len(), 1);
        assert_eq!(built.book.parts[1].embeddings.len(), 1);

        let packed = pack(&[
            (
                "pack/catalogue.json",
                br#"{"parts":[{"ic":"2ERGM22","description":"Hinge","crops":["crops/2ERGM22/000001_crop_007.jpg"]}]}"#,
            ),
            ("pack/crops/2ERGM22/000001_crop_007.jpg", jpeg.as_slice()),
        ]);
        let nested = build_from_tar(&packed, &mut stub_embed).unwrap();
        assert_eq!(nested.book.parts.len(), 1);
        assert_eq!(nested.book.parts[0].description, "Hinge");
        assert_eq!(nested.book.parts[0].crops.len(), 1);
        assert!(nested.book.parts[0].crops[0].ends_with("000001_crop_007.jpg"));
        assert_eq!(nested.book.parts[0].embeddings.len(), 1);
    }

    #[test]
    fn a_missing_catalogue_file_lists_nothing() {
        let dir = temp_dir("missing");
        let listed = catalogue_list_from(&dir).unwrap();
        assert!(listed.parts.is_empty());
        assert!(listed.path.ends_with("catalogue.json"));
        let _ = fs::remove_dir_all(&dir);
    }
}
