//! Part catalogue imported from a `.tar` archive.
//!
//! Settings > Parts > Update references picks a tar. The archive holds
//! `catalogue.json` and JPEG crops under `crops/`. Each crop is embedded with
//! the DINOv2 session for the current `dinoModel`. The extract opens that ONNX
//! file and writes vectors of its CLS length. A model change marks stored
//! vectors stale. The next extract writes that model's rows. The matcher reads
//! only the model it loaded, so ViT-S and ViT-B vectors are not mixed.
//! Embeddings and crop images go into the `dino_catalogue` table of the
//! results database. Part text and crop ids go into `app_config` in that same
//! file. Startup loads the database.
//! The default import updates item codes that are in the tar and adds new
//! ones. Parts and crops that are only on this PC stay. Full replace writes
//! the tar as the whole catalogue, including classes over their crop limit.
//! The page shows that choice only after that database write has succeeded.
//! Truncate then removes the extra crops. Keep all stores the higher limit.
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
//!       "type": "nut",
//!       "measurementMethod": "nut",
//!       "thread": "M8",
//!       "af_mm": 13,
//!       "height_mm": 6.5,
//!       "crops": ["crops/2BBMNM08.jpg"]
//!     }
//!   ]
//! }
//! ```
//!
//! `ic`, `description` (`des`), `type` (`partType`), and `crops`
//! (`images` or `files`) are read. `measurementMethod` is optional. Sizes are
//! the structured fields `thread`, `length_mm`, `af_mm`, `width_mm`,
//! `height_mm`, `od_mm`, `id_mm`, and `thickness_mm`, or a `{ name, mm }` list
//! such as `Length under head` and `Thread diameter`. `length_rule` and
//! `diameter_rule`, including the copy under `measurement_rules`, name which
//! of those fields is the length and which is the diameter. The rule note is
//! kept with the part. A Dimensions string is display text and is not stored
//! as a measurement. The page shows the size generated from the ruled fields.
//! A crop path is `crops/<file>.jpg` or `crops/<IC>/<file>.jpg`. The folder
//! after `crops/` groups those images under that IC. An IC is left out when
//! its crop list is empty or none of the listed files are in the tar.
//!
//! The references dialog can delete, reassign, or reorder crops. That only
//! changes the existing `crops` and `embeddings` rows. Per-dimension
//! tolerances are stored in `app_config`, not in the part rows.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

const CATALOGUE_FILE: &str = "catalogue.json";
const CROPS_DIR: &str = "catalogue-crops";
const VERSION: u32 = 1;
const MAX_TAR_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 20 * 1024 * 1024;
const MAX_PARTS: usize = 2_000;
const MAX_CROPS: usize = 24;
const PROMPT_THUMBS: usize = 4;
const CROP_LIMITS_FILE: &str = "catalogue-crop-limits.json";
const TOLERANCES_FILE: &str = "catalogue-tolerances.json";
const MAX_PIXELS: usize = 40_000_000;
const MAX_TEXT: usize = 500;
const THUMB_EDGE: u32 = 160;

struct Part {
    ic: String,
    description: String,
    kind: String,
    /// Empty when the part has no measurement method. Metrology then uses the
    /// generic length and diameter caliper.
    measurement_method: String,
    dimensions: Value,
    crops: Vec<String>,
    embeddings: Vec<Vec<f32>>,
    /// Set when this class may keep more than `MAX_CROPS` crops.
    crop_limit: Option<usize>,
}

struct Book {
    parts: Vec<Part>,
    /// `s_reg` or `b_reg` when these vectors were extracted with that model.
    /// Missing means the vectors came from an older model.
    embedding_model: Option<String>,
    /// Set when the saved vectors must not be matched until the next extract.
    embeddings_stale: bool,
}

impl Book {
    fn empty() -> Self {
        Self {
            parts: Vec::new(),
            embedding_model: None,
            embeddings_stale: false,
        }
    }

    fn from_parts(parts: Vec<Part>) -> Self {
        Self {
            parts,
            embedding_model: None,
            embeddings_stale: false,
        }
    }
}

fn book_has_vectors(book: &Book) -> bool {
    book.parts
        .iter()
        .any(|part| part.embeddings.iter().any(|values| !values.is_empty()))
}

/// Stored vectors can be scored only when they were extracted with `active`
/// and have not been marked stale. A book with no vectors is usable.
pub(crate) fn embeddings_usable(
    embedding_model: Option<&str>,
    embeddings_stale: bool,
    has_vectors: bool,
    active: &str,
) -> bool {
    if !has_vectors {
        return true;
    }
    !embeddings_stale && embedding_model == Some(active)
}

fn model_matches(book: &Book, active: &str) -> bool {
    embeddings_usable(
        book.embedding_model.as_deref(),
        book.embeddings_stale,
        book_has_vectors(book),
        active,
    )
}

fn stamp_extracted(book: &mut Book, active: &str) {
    book.embedding_model = Some(active.to_string());
    book.embeddings_stale = false;
}

/// Replace every stored vector. The caller writes the book after this returns.
fn reembed_book(
    vision: &Path,
    book: &mut Book,
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
    progress: &mut dyn FnMut(&str, usize, usize),
    done: &mut usize,
    total: usize,
) -> Result<(), String> {
    let mut sources = Vec::new();
    for part in &book.parts {
        sources.extend(part.crops.iter().cloned());
    }
    let stored = stored_crops(vision, &sources)?;
    let mut expected_width = None;
    for part in &mut book.parts {
        let mut fresh = Vec::with_capacity(part.crops.len());
        for crop in &part.crops {
            report_extract(progress, &part.ic, *done, total);
            let bytes = jpeg_from_stored(stored.get(crop))?;
            let values =
                embed(&bytes).map_err(|err| format!("Could not embed {}: {err}", part.ic))?;
            if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
                return Err(format!("Could not embed {}.", part.ic));
            }
            remember_width(&mut expected_width, &values)?;
            fresh.push(values);
            *done = done.saturating_add(1);
            report_extract(progress, &part.ic, *done, total);
        }
        part.embeddings = fresh;
    }
    Ok(())
}

fn report_extract(
    progress: &mut dyn FnMut(&str, usize, usize),
    ic: &str,
    done: usize,
    total: usize,
) {
    if total > 0 {
        progress(ic, done, total);
    }
}

fn aligned_stamp(book: &Book, active: &str) -> (Option<String>, bool) {
    if !book_has_vectors(book) || book.embedding_model.as_deref() == Some(active) {
        return (Some(active.to_string()), false);
    }
    (book.embedding_model.clone(), true)
}

/// Mark vectors that belong to another model. Vectors already extracted with
/// `active` stay usable, including a switch back to that model before the
/// next extract. Returns whether matching must skip the stored vectors.
pub(crate) fn align_embeddings_file(vision: &Path, active: &str) -> Result<bool, String> {
    let Some(mut book) = load_book(vision)? else {
        return Ok(false);
    };
    if let Some(switched) = book_for_model(vision, &book, active)? {
        let same = book.embedding_model.as_deref() == Some(active)
            && !book.embeddings_stale
            && book.parts.len() == switched.parts.len()
            && book
                .parts
                .iter()
                .zip(&switched.parts)
                .all(|(left, right)| left.embeddings == right.embeddings);
        if !same {
            persist_book(vision, &switched)?;
        }
        return Ok(false);
    }
    let has_vectors = book_has_vectors(&book);
    let (model, stale) = aligned_stamp(&book, active);
    let skipped = stale && has_vectors;
    if book.embedding_model == model && book.embeddings_stale == stale {
        return Ok(skipped);
    }
    book.embedding_model = model;
    book.embeddings_stale = stale;
    persist_book(vision, &book)?;
    if skipped {
        eprintln!(
            "DINOv2 catalogue embeddings are stale for model {active}. The next extract uses that model."
        );
    }
    Ok(skipped)
}

pub(crate) fn align_saved_embeddings(active: &str) -> Result<bool, String> {
    let vision = vision_directory()?;
    let stale = align_embeddings_file(&vision, active)?;
    if let Some(book) = load_book(&vision)? {
        *lock(&CATALOGUE) = book;
    }
    crate::catalog::reload_installed();
    Ok(stale)
}

pub(crate) fn embeddings_need_extract(active: &str) -> Result<bool, String> {
    let vision = vision_directory()?;
    let Some(book) = load_book(&vision)? else {
        return Ok(false);
    };
    if !model_matches(&book, active) {
        return Ok(true);
    }
    if let Some(width) = crate::dinov2::loaded_cls_width() {
        if book_has_vectors(&book) && !stored_widths_match(&book, width) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn stored_widths_match(book: &Book, width: usize) -> bool {
    book.parts.iter().all(|part| {
        part.embeddings
            .iter()
            .all(|values| values.is_empty() || values.len() == width)
    })
}

/// Re-embed a saved book whose vectors are a different length from the model
/// that just embedded a new crop. Tests that append a vector directly do not
/// call this.
fn rewrite_catalogue_width(vision: &Path, width: usize) -> Result<(), String> {
    let Some(mut book) = load_book(vision)? else {
        return Ok(());
    };
    if !book_has_vectors(&book) || stored_widths_match(&book, width) {
        return Ok(());
    }
    let mut embed = embed_jpeg;
    let mut silent = |_: &str, _: usize, _: usize| {};
    let mut done = 0usize;
    let total = book.parts.iter().map(|part| part.crops.len()).sum();
    reembed_book(vision, &mut book, &mut embed, &mut silent, &mut done, total)?;
    stamp_extracted(&mut book, &crate::dinov2::selected_model_id());
    persist_book(vision, &book)?;
    Ok(())
}

struct Built {
    book: Book,
    files: Vec<(String, Vec<u8>)>,
}

static CATALOGUE: LazyLock<Mutex<Book>> = LazyLock::new(|| Mutex::new(Book::empty()));
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static WAITING: LazyLock<Mutex<Option<WaitingReplace>>> = LazyLock::new(|| Mutex::new(None));

fn lock(mutex: &Mutex<Book>) -> std::sync::MutexGuard<'_, Book> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn vision_directory() -> Result<PathBuf, String> {
    Ok(crate::local_config::project_root()?.join("vision"))
}

fn catalogue_path(vision: &Path) -> PathBuf {
    crate::app_db::database_path(vision)
}

fn catalogue_file(vision: &Path) -> PathBuf {
    vision.join(CATALOGUE_FILE)
}

fn is_repo_vision(vision: &Path) -> bool {
    crate::local_config::project_root()
        .map(|root| vision == root.join("vision"))
        .unwrap_or(false)
}

fn log_catalogue_path(path: &Path) {
    eprintln!("DINOv2 catalogue path: {}", path.display());
}

/// Load the catalogue from the results database. A missing catalogue leaves memory as it is.
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

#[derive(Clone, Debug, Serialize)]
pub struct CropClassPrompt {
    pub ic: String,
    pub count: usize,
    pub limit: usize,
    pub dropped: usize,
    /// Tar timestamps distinguish the crops, so truncate keeps the newest.
    pub recent: bool,
    pub thumbs: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct CatalogueUpdate {
    pub parts: usize,
    pub message: String,
    pub path: String,
    /// Non-zero while a full replace is waiting for crop-limit choices.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub token: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<CropClassPrompt>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

const EXTRACT_EVENT: &str = "catalogue-extract";

#[derive(Clone, Serialize)]
struct CatalogueExtractProgress {
    ic: String,
    done: usize,
    total: usize,
}

fn emit_extract(app: &AppHandle, ic: &str, done: usize, total: usize) {
    let _ = app.emit(
        EXTRACT_EVENT,
        CatalogueExtractProgress {
            ic: ic.to_string(),
            done,
            total,
        },
    );
}

impl CatalogueUpdate {
    fn done(parts: usize, message: impl Into<String>, path: &Path) -> Self {
        Self {
            parts,
            message: message.into(),
            path: path.display().to_string(),
            token: 0,
            classes: Vec::new(),
        }
    }

    fn waiting(
        parts: usize,
        message: impl Into<String>,
        path: &Path,
        token: u64,
        classes: Vec<CropClassPrompt>,
    ) -> Self {
        Self {
            parts,
            message: message.into(),
            path: path.display().to_string(),
            token,
            classes,
        }
    }
}

#[derive(Deserialize)]
pub struct CropChoiceInput {
    pub ic: String,
    pub action: String,
}

#[derive(Clone, Serialize)]
pub struct CatalogueCropView {
    pub index: usize,
    pub src: String,
    pub hash: String,
}

#[derive(Clone, Serialize)]
pub struct CatalogueSizeView {
    pub key: String,
    pub label: String,
    pub value: String,
    #[serde(rename = "toleranceMm", skip_serializing_if = "Option::is_none")]
    pub tolerance_mm: Option<f64>,
}

#[derive(Clone, Serialize)]
pub struct CatalogueView {
    pub ic: String,
    pub description: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub dimensions: String,
    pub thumbs: Vec<String>,
    pub crops: Vec<CatalogueCropView>,
    pub sizes: Vec<CatalogueSizeView>,
    #[serde(rename = "addedUnix", skip_serializing_if = "Option::is_none")]
    pub added_unix: Option<i64>,
}

#[derive(Clone, Serialize)]
pub struct CatalogueList {
    pub path: String,
    pub parts: Vec<CatalogueView>,
}

/// Native tar picker, then embed each crop.
///
/// `replace` false, the default, updates item codes that are in the tar and
/// adds new ones. Parts and crops that are only on this PC stay.
/// `replace` true embeds every crop and writes `catalogue.json` before any
/// crop-limit choice is returned. A failed embed or write returns an error
/// and does not offer truncate or keep.
/// Cancel on the file dialog leaves the saved file as it is.
/// The dialog runs off the main thread. A panic becomes a status string.
#[tauri::command]
pub async fn import_catalogue(
    app: AppHandle,
    replace: Option<bool>,
) -> Result<CatalogueUpdate, String> {
    let replace = replace.unwrap_or(false);
    let vision = vision_directory()?;
    let path = catalogue_path(&vision);
    log_catalogue_path(&path);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            install_picked_catalogue(app, vision, replace)
        }))
    })
    .await;
    match joined {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => Err("Could not update the catalogue".to_string()),
    }
}

fn install_picked_catalogue(
    app: AppHandle,
    vision: PathBuf,
    replace: bool,
) -> Result<CatalogueUpdate, String> {
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
        return Ok(CatalogueUpdate::done(
            lock(&CATALOGUE).parts.len(),
            "No file selected.",
            &path,
        ));
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
    // A full replace embeds every crop from memory. That loop can keep every
    // core busy, so the webview does not paint `catalogue-extract` until this
    // command returns and the page clears the bar. Pause after each new count.
    let mut painted = 0usize;
    let mut progress = |ic: &str, done: usize, total: usize| {
        emit_extract(&app, ic, done, total);
        if replace && done != painted {
            painted = done;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    if !replace {
        let mut embed = embed_jpeg;
        let count = import_tar_reporting(&bytes, &vision, false, &mut embed, &mut progress)?;
        eprintln!(
            "DINOv2 catalogue updated: {count} parts from {}",
            path.display()
        );
        return Ok(CatalogueUpdate::done(
            count,
            format!("Catalogue updated. {count} parts on this PC."),
            &path,
        ));
    }
    let mut draft = inspect_replace(&bytes, &vision)?;
    let mut embed = embed_jpeg;
    let report = match write_full_replace(&vision, &draft, &mut embed, &mut progress) {
        Ok(report) => report,
        Err(err) => {
            clear_waiting();
            return Err(err);
        }
    };
    eprintln!(
        "DINOv2 catalogue replaced: {} parts from {}",
        report.parts,
        path.display()
    );
    if draft.prompts.is_empty() {
        clear_waiting();
        return Ok(CatalogueUpdate::done(report.parts, report.message, &path));
    }
    let classes = draft.prompts.clone();
    eprintln!(
        "DINOv2 catalogue written. {} classes are over their crop limit.",
        classes.len()
    );
    release_draft_jpegs(&mut draft);
    let token = store_waiting(vision, draft);
    Ok(CatalogueUpdate::waiting(
        report.parts,
        report.message,
        &path,
        token,
        classes,
    ))
}

/// Apply truncate or keep-all to a catalogue that has already been written.
/// This does not embed again. A failed trim leaves that catalogue in place
/// and puts the choice back.
#[tauri::command]
pub async fn finish_catalogue_replace(
    token: u64,
    choices: Vec<CropChoiceInput>,
) -> Result<CatalogueUpdate, String> {
    let parsed = parse_choices(&choices)?;
    let waiting = take_waiting(token)?;
    let path = catalogue_path(&waiting.vision);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match apply_crop_choice(&waiting.vision, &waiting.draft, &parsed) {
                Ok(report) => Ok(report),
                Err(err) => Err((err, waiting)),
            }
        }))
    })
    .await;
    match joined {
        Ok(Ok(Ok(report))) => {
            eprintln!(
                "DINOv2 catalogue replaced: {} parts from {}",
                report.parts,
                path.display()
            );
            Ok(CatalogueUpdate::done(report.parts, report.message, &path))
        }
        Ok(Ok(Err((err, waiting)))) => {
            restore_waiting(waiting);
            Err(err)
        }
        Ok(Err(_)) | Err(_) => Err("Could not update the catalogue".to_string()),
    }
}

/// Drop a waiting crop-limit choice. The extracted catalogue stays in place.
#[tauri::command]
pub fn cancel_catalogue_replace(token: u64) -> Result<(), String> {
    let _ = take_waiting(token)?;
    eprintln!("DINOv2 catalogue crop choice cancelled.");
    Ok(())
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

/// Remove the chosen crops from one part. The part stays, including when it
/// has no images left.
#[tauri::command]
pub fn delete_catalogue_crops(ic: String, indexes: Vec<usize>) -> Result<(), String> {
    let vision = vision_directory()?;
    delete_crops(&vision, &ic, &indexes)
}

/// Move the chosen crops, and their embeddings, onto another item code.
/// A new item code is added when it is not already in the catalogue.
#[tauri::command]
pub fn reassign_catalogue_crops(
    ic: String,
    indexes: Vec<usize>,
    target: String,
) -> Result<(), String> {
    let vision = vision_directory()?;
    reassign_crops(&vision, &ic, &indexes, &target)
}

/// Rewrite one part's crop order. The first index is the primary image.
#[tauri::command]
pub fn reorder_catalogue_crops(ic: String, order: Vec<usize>) -> Result<(), String> {
    let vision = vision_directory()?;
    reorder_crops(&vision, &ic, &order)
}

/// Save or clear one dimension tolerance for an item code.
/// `mm` null removes the override. The catalogue file is not rewritten.
#[tauri::command]
pub fn set_catalogue_tolerance(ic: String, key: String, mm: Option<f64>) -> Result<(), String> {
    let vision = vision_directory()?;
    set_tolerance(&vision, &ic, &key, mm)
}

/// Tolerances saved for one item code, keyed by the normalised dimension name.
/// A missing file is an empty map.
pub(crate) fn tolerances_for(ic: &str) -> HashMap<String, f64> {
    let ic = ic.trim();
    if ic.is_empty() {
        return HashMap::new();
    }
    let Ok(vision) = vision_directory() else {
        return HashMap::new();
    };
    read_tolerances(&vision)
        .get(ic)
        .cloned()
        .unwrap_or_default()
}

#[derive(Clone, Serialize)]
pub struct CatalogueCropPreview {
    pub image: String,
    pub width: u32,
    pub height: u32,
}

/// JPEG of one box on the full-resolution frame held by the last preview Detect.
#[tauri::command]
pub fn preview_catalogue_crop(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> Result<CatalogueCropPreview, String> {
    let (rgb, width, height) = held_crop(x1, y1, x2, y2)?;
    let jpeg = encode_rgb(&rgb, width, height)?;
    Ok(CatalogueCropPreview {
        image: format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(jpeg)
        ),
        width,
        height,
    })
}

/// Crop that box, stretch-embed it, and append the JPEG and vector under `ic`.
/// The in-memory matcher reloads, so the next Detect in this session can use it.
#[tauri::command]
pub fn save_catalogue_crop(
    ic: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> Result<CatalogueUpdate, String> {
    if ic.trim().is_empty() {
        return Err("Enter an IC.".to_string());
    }
    let (rgb, width, height) = held_crop(x1, y1, x2, y2)?;
    let embedding = embed_catalogue_image(&rgb, width, height)?;
    let jpeg = encode_rgb(&rgb, width, height)?;
    let vision = vision_directory()?;
    rewrite_catalogue_width(&vision, embedding.len())?;
    let saved_ic = append_catalogue_crop(&vision, &ic, &jpeg, &embedding)?;
    if let Some(book) = load_book(&vision)? {
        *lock(&CATALOGUE) = book;
    }
    crate::catalog::reload_installed();
    let path = catalogue_path(&vision);
    Ok(CatalogueUpdate::done(
        lock(&CATALOGUE).parts.len(),
        format!("Saved {saved_ic}."),
        &path,
    ))
}

fn held_crop(x1: f64, y1: f64, x2: f64, y2: f64) -> Result<(Vec<u8>, u32, u32), String> {
    let bbox = crop_box_arg(x1, y1, x2, y2)?;
    let Some((rgb, width, height)) = crate::dinov2::held_frame_rgb() else {
        return Err("Detect again before saving this part.".to_string());
    };
    crate::catalog::crop_clamped(&rgb, width, height, bbox)
        .map_err(|_| "Could not read that crop.".to_string())
}

fn crop_box_arg(x1: f64, y1: f64, x2: f64, y2: f64) -> Result<[i32; 4], String> {
    if ![x1, y1, x2, y2].into_iter().all(f64::is_finite) {
        return Err("Could not read that crop.".to_string());
    }
    Ok([
        x1.round() as i32,
        y1.round() as i32,
        x2.round() as i32,
        y2.round() as i32,
    ])
}

/// Append one JPEG and its embedding. Other parts, and this part's text, stay.
fn append_catalogue_crop(
    vision: &Path,
    ic: &str,
    jpeg: &[u8],
    embedding: &[f32],
) -> Result<String, String> {
    let ic = clean_ic(ic)?;
    if jpeg.len() < 3 || jpeg[0] != 0xff || jpeg[1] != 0xd8 {
        return Err("Could not save the crop.".to_string());
    }
    if embedding.is_empty() || embedding.iter().any(|value| !value.is_finite()) {
        return Err("Could not embed the crop.".to_string());
    }
    let mut book = load_book(vision)?.unwrap_or_else(Book::empty);
    let active = crate::dinov2::selected_model_id();
    if !model_matches(&book, &active) {
        let mut embed = embed_jpeg;
        let mut silent = |_: &str, _: usize, _: usize| {};
        let mut done = 0usize;
        let total = book.parts.iter().map(|part| part.crops.len()).sum();
        reembed_book(vision, &mut book, &mut embed, &mut silent, &mut done, total)?;
    }
    if book.parts.len() >= MAX_PARTS && book.parts.iter().all(|part| part.ic != ic) {
        return Err("The catalogue has too many parts.".to_string());
    }
    if book
        .parts
        .iter()
        .any(|part| part.ic == ic && part.crops.len() >= MAX_CROPS)
    {
        return Err(format!("Part {ic} has too many crops."));
    }
    let folder = folder_for(book.parts.iter().find(|part| part.ic == ic), &ic);
    let relative = next_crop_source(&book, &folder)?;
    if let Some(part) = book.parts.iter_mut().find(|part| part.ic == ic) {
        part.crops.push(relative.clone());
        part.embeddings.push(embedding.to_vec());
    } else {
        book.parts.push(Part {
            ic: ic.clone(),
            description: String::new(),
            kind: String::new(),
            measurement_method: String::new(),
            dimensions: Value::Null,
            crops: vec![relative.clone()],
            embeddings: vec![embedding.to_vec()],
            crop_limit: None,
        });
    }
    stamp_extracted(&mut book, &active);
    let mut crops = HashMap::new();
    crops.insert(relative, jpeg.to_vec());
    persist_rows(vision, &book, &crops)?;
    Ok(ic)
}

fn folder_for(part: Option<&Part>, ic: &str) -> String {
    if let Some(crop) = part.and_then(|part| part.crops.first()) {
        if let Some(folder) = crop_folder(crop) {
            return folder;
        }
    }
    sanitize_ic(ic)
}

fn crop_folder(crop: &str) -> Option<String> {
    let rest = crop.strip_prefix(&format!("{CROPS_DIR}/"))?;
    let (folder, file) = rest.split_once('/')?;
    if folder.is_empty() || file.is_empty() || folder == "." || folder == ".." {
        return None;
    }
    Some(folder.to_string())
}

fn next_crop_source(book: &Book, folder: &str) -> Result<String, String> {
    for index in 1..10_000 {
        let relative = format!("{CROPS_DIR}/{folder}/crop-{index}.jpg");
        if !safe_crop_path(&relative) {
            return Err("Could not write the catalogue.".to_string());
        }
        let taken = book
            .parts
            .iter()
            .any(|part| part.crops.iter().any(|crop| crop == &relative));
        if !taken {
            return Ok(relative);
        }
    }
    Err("Could not write the catalogue.".to_string())
}

fn encode_rgb(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3);
    if width == 0 || height == 0 || rgb.len() < expected {
        return Err("Could not read that crop.".to_string());
    }
    let image = turbojpeg::Image {
        pixels: &rgb[..expected],
        width: width as usize,
        pitch: (width as usize).saturating_mul(3),
        height: height as usize,
        format: turbojpeg::PixelFormat::RGB,
    };
    let jpeg = turbojpeg::compress(image, 90, turbojpeg::Subsamp::Sub2x2)
        .map_err(|_| "Could not save the crop.".to_string())?;
    Ok(jpeg.as_ref().to_vec())
}

fn embed_jpeg(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let (rgb, width, height) = decode_jpeg(bytes)?;
    embed_catalogue_image(&rgb, width, height)
}

/// Stretch-embed one crop with the ONNX session for the current `dinoModel`.
/// The returned vector is that session's CLS length.
fn embed_catalogue_image(rgb: &[u8], width: u32, height: u32) -> Result<Vec<f32>, String> {
    let model = crate::dinov2::session_for_catalogue()?;
    // Same 224×224 stretch the box matcher uses, so a stored crop matches itself.
    let values = crate::inference::dino::embed_rgb(rgb, width, height)?;
    accept_embedding(values, model.width)
}

fn accept_embedding(values: Vec<f32>, width: usize) -> Result<Vec<f32>, String> {
    if width == 0 || values.len() != width {
        return Err(format!(
            "DINOv2 returned {} values. The loaded model reports {width}.",
            values.len()
        ));
    }
    Ok(values)
}

fn remember_width(expected: &mut Option<usize>, values: &[f32]) -> Result<(), String> {
    match *expected {
        Some(width) if values.len() != width => Err(format!(
            "DINOv2 returned {} values. The loaded model reports {width}.",
            values.len()
        )),
        Some(_) => Ok(()),
        None => {
            *expected = Some(values.len());
            Ok(())
        }
    }
}

#[cfg(test)]
fn import_tar_bytes(
    bytes: &[u8],
    vision: &Path,
    replace: bool,
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
) -> Result<usize, String> {
    let mut silent = |_: &str, _: usize, _: usize| {};
    import_tar_reporting(bytes, vision, replace, embed, &mut silent)
}

fn import_tar_reporting(
    bytes: &[u8],
    vision: &Path,
    replace: bool,
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<usize, String> {
    let active = crate::dinov2::selected_model_id();
    let saved = if replace { None } else { load_book(vision)? };
    let reembed_saved = saved
        .as_ref()
        .is_some_and(|book| !model_matches(book, &active));
    let parts = resolve_tar(bytes, true)?;
    let kept = kept_within_limit(parts)?;
    let tar_total: usize = kept.iter().map(|part| part.crops.len()).sum();
    let saved_total: usize = if reembed_saved {
        saved
            .as_ref()
            .map(|book| book.parts.iter().map(|part| part.crops.len()).sum())
            .unwrap_or(0)
    } else {
        0
    };
    let total = tar_total.saturating_add(saved_total);
    let mut done = 0usize;
    let built = embed_kept(kept, embed, progress, &mut done, total)?;
    let mut built = if let Some(mut existing) = saved {
        if reembed_saved {
            reembed_book(vision, &mut existing, embed, progress, &mut done, total)?;
        }
        merge_loaded(vision, built, existing)?
    } else {
        built
    };
    stamp_extracted(&mut built.book, &active);
    let count = built.book.parts.len();
    commit_book(vision, &built)?;
    *lock(&CATALOGUE) = built.book;
    crate::catalog::reload_installed();
    Ok(count)
}

struct TarFile {
    bytes: Vec<u8>,
    /// Unix mtime from the tar header. Zero when the header has no time.
    mtime: u64,
}

struct Archive {
    catalogue: String,
    files: HashMap<String, TarFile>,
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
        let mtime = entry.header().mtime().unwrap_or(0);
        let mut body = Vec::new();
        entry
            .take(MAX_ENTRY_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|err| format!("Could not read the tar archive: {err}"))?;
        if body.len() as u64 > MAX_ENTRY_BYTES {
            return Err("A file in the archive is too large.".to_string());
        }
        files.insert(key, TarFile { bytes: body, mtime });
    }
    let (catalogue_key, prefix) = locate_catalogue(&files)?;
    let catalogue = String::from_utf8(
        files
            .remove(&catalogue_key)
            .map(|file| file.bytes)
            .unwrap_or_default(),
    )
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

fn locate_catalogue(files: &HashMap<String, TarFile>) -> Result<(String, String), String> {
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
    measurement_method: String,
    dimensions: Value,
    crops: Vec<String>,
}

fn parse_source(text: &str, enforce_crop_limit: bool) -> Result<Vec<SourcePart>, String> {
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
        parts.push(source_part(&map, enforce_crop_limit)?);
    }
    Ok(merge_parts(parts, enforce_crop_limit)?)
}

fn source_part(map: &Map<String, Value>, enforce_crop_limit: bool) -> Result<SourcePart, String> {
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
    let measurement_method = clean_method(&field_string(
        map,
        &["measurementMethod", "measurement_method"],
    ));
    let dimensions = crate::part_size::from_part(&measurement_method, &kind, map);
    if dimensions.to_string().len() > MAX_TEXT {
        return Err(format!("Dimensions for {ic} are too long."));
    }
    let crops = crop_list(map, &ic, enforce_crop_limit)?;
    Ok(SourcePart {
        ic,
        description,
        kind,
        measurement_method,
        dimensions,
        crops,
    })
}

/// `bolt`, `nut`, `washer`, `cable_tie`, `spring`, or `custom`. Anything else
/// is empty, and metrology keeps the generic caliper.
fn clean_method(raw: &str) -> String {
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
        .as_str()
    {
        "bolt" => "bolt".to_string(),
        "nut" => "nut".to_string(),
        "washer" => "washer".to_string(),
        "cable_tie" | "cabletie" => "cable_tie".to_string(),
        "spring" => "spring".to_string(),
        "custom" => "custom".to_string(),
        _ => String::new(),
    }
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

fn crop_list(
    map: &Map<String, Value>,
    ic: &str,
    enforce_crop_limit: bool,
) -> Result<Vec<String>, String> {
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
    if enforce_crop_limit && items.len() > MAX_CROPS {
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

fn merge_parts(
    parts: Vec<SourcePart>,
    enforce_crop_limit: bool,
) -> Result<Vec<SourcePart>, String> {
    let mut merged: Vec<SourcePart> = Vec::new();
    for part in parts {
        if let Some(existing) = merged.iter_mut().find(|item| item.ic == part.ic) {
            if !part.description.is_empty() {
                existing.description = part.description;
            }
            if !part.kind.is_empty() {
                existing.kind = part.kind;
            }
            if !part.measurement_method.is_empty() {
                existing.measurement_method = part.measurement_method;
            }
            if !part.dimensions.is_null() {
                existing.dimensions = part.dimensions;
            }
            existing.crops.extend(part.crops);
            if enforce_crop_limit && existing.crops.len() > MAX_CROPS {
                return Err(format!("Part {} has too many crops.", existing.ic));
            }
        } else {
            merged.push(part);
        }
    }
    Ok(merged)
}

fn kept_within_limit(parts: Vec<ResolvedPart>) -> Result<Vec<KeptPart>, String> {
    let mut kept = Vec::with_capacity(parts.len());
    for part in parts {
        if part.crops.len() > MAX_CROPS {
            return Err(format!("Part {} has too many crops.", part.ic));
        }
        kept.push(KeptPart {
            ic: part.ic,
            description: part.description,
            kind: part.kind,
            measurement_method: part.measurement_method,
            dimensions: part.dimensions,
            crop_limit: None,
            crops: part.crops,
        });
    }
    Ok(kept)
}

#[cfg(test)]
fn build_from_tar(
    bytes: &[u8],
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
) -> Result<Built, String> {
    let parts = resolve_tar(bytes, true)?;
    let kept = kept_within_limit(parts)?;
    embed_kept_quiet(kept, embed)
}

struct WaitingReplace {
    token: u64,
    vision: PathBuf,
    draft: ReplaceDraft,
}

#[derive(Debug)]
struct ReplaceDraft {
    parts: Vec<ResolvedPart>,
    prompts: Vec<CropClassPrompt>,
    limits: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
struct ResolvedCrop {
    key: String,
    jpeg: Vec<u8>,
    mtime: u64,
}

#[derive(Debug)]
struct ResolvedPart {
    ic: String,
    description: String,
    kind: String,
    measurement_method: String,
    dimensions: Value,
    crops: Vec<ResolvedCrop>,
}

struct KeptPart {
    ic: String,
    description: String,
    kind: String,
    measurement_method: String,
    dimensions: Value,
    crop_limit: Option<usize>,
    crops: Vec<ResolvedCrop>,
}

#[derive(Debug)]
struct ImportReport {
    parts: usize,
    message: String,
}

struct Decision {
    ic: String,
    kept: usize,
    dropped: usize,
    overrode: bool,
}

#[derive(Clone, Copy)]
enum CropAction {
    Truncate,
    Override,
}

fn store_waiting(vision: PathBuf, draft: ReplaceDraft) -> u64 {
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let mut slot = WAITING.lock().unwrap_or_else(|err| err.into_inner());
    if slot.is_some() {
        eprintln!("DINOv2 catalogue crop choice replaced a waiting import.");
    }
    *slot = Some(WaitingReplace {
        token,
        vision,
        draft,
    });
    token
}

fn take_waiting(token: u64) -> Result<WaitingReplace, String> {
    let mut slot = WAITING.lock().unwrap_or_else(|err| err.into_inner());
    match slot.as_ref() {
        Some(waiting) if waiting.token == token => Ok(slot.take().expect("waiting token")),
        _ => Err("That import is no longer waiting.".to_string()),
    }
}

fn restore_waiting(waiting: WaitingReplace) {
    let mut slot = WAITING.lock().unwrap_or_else(|err| err.into_inner());
    if slot.is_some() {
        eprintln!("DINOv2 catalogue left the newer crop choice in place.");
        return;
    }
    *slot = Some(waiting);
}

fn clear_waiting() {
    *WAITING.lock().unwrap_or_else(|err| err.into_inner()) = None;
}

fn release_draft_jpegs(draft: &mut ReplaceDraft) {
    for part in &mut draft.parts {
        for crop in &mut part.crops {
            crop.jpeg = Vec::new();
        }
    }
}

fn require_choices(
    draft: &ReplaceDraft,
    choices: &[(String, CropAction)],
) -> Result<HashMap<String, CropAction>, String> {
    let mut chosen = HashMap::new();
    for (ic, action) in choices {
        if chosen.insert(ic.clone(), *action).is_some() {
            return Err(format!("Choose one action for {ic}."));
        }
    }
    for prompt in &draft.prompts {
        if !chosen.contains_key(&prompt.ic) {
            return Err(format!("Choose a crop limit for {}.", prompt.ic));
        }
    }
    for ic in chosen.keys() {
        if !draft.prompts.iter().any(|prompt| &prompt.ic == ic) {
            return Err(format!("{ic} is already within its crop limit."));
        }
    }
    Ok(chosen)
}

fn parse_choices(choices: &[CropChoiceInput]) -> Result<Vec<(String, CropAction)>, String> {
    let mut parsed = Vec::with_capacity(choices.len());
    let mut seen = HashSet::new();
    for choice in choices {
        let ic = clean_ic(&choice.ic)?;
        if !seen.insert(ic.clone()) {
            return Err(format!("Choose one action for {ic}."));
        }
        let action = match choice.action.trim() {
            "truncate" => CropAction::Truncate,
            "override" => CropAction::Override,
            _ => return Err("Choose truncate or keep all for each class.".to_string()),
        };
        parsed.push((ic, action));
    }
    Ok(parsed)
}

/// Read a full-replace tar without embedding. Classes over their limit are
/// listed for the admin. Nothing is written.
fn inspect_replace(bytes: &[u8], vision: &Path) -> Result<ReplaceDraft, String> {
    let parts = resolve_tar(bytes, false)?;
    let limits = known_limits(vision);
    let mut prompts = Vec::new();
    for part in &parts {
        let limit = allowance(&limits, &part.ic);
        if part.crops.len() > limit {
            prompts.push(prompt_for(part, limit));
        }
    }
    Ok(ReplaceDraft {
        parts,
        prompts,
        limits,
    })
}

fn prompt_for(part: &ResolvedPart, limit: usize) -> CropClassPrompt {
    let count = part.crops.len();
    let dropped = count.saturating_sub(limit);
    let recent = mtimes_differ(&part.crops);
    eprintln!(
        "DINOv2 catalogue {ic}: {count} crops, limit {limit}",
        ic = part.ic
    );
    CropClassPrompt {
        ic: part.ic.clone(),
        count,
        limit,
        dropped,
        recent,
        thumbs: prompt_thumbs(part),
    }
}

#[cfg(test)]
fn commit_replace(
    vision: &Path,
    draft: &ReplaceDraft,
    choices: &[(String, CropAction)],
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
) -> Result<ImportReport, String> {
    let chosen = require_choices(draft, choices)?;
    let mut kept = Vec::with_capacity(draft.parts.len());
    let mut notes = Vec::new();
    let mut overrides = Vec::new();
    for part in &draft.parts {
        let limit = draft
            .prompts
            .iter()
            .find(|prompt| prompt.ic == part.ic)
            .map(|prompt| prompt.limit)
            .unwrap_or_else(|| allowance(&draft.limits, &part.ic));
        let (indexes, crop_limit, note) = match chosen.get(&part.ic).copied() {
            None => {
                if part.crops.len() > limit {
                    return Err(format!("Choose a crop limit for {}.", part.ic));
                }
                if part.crops.len() > MAX_CROPS {
                    eprintln!(
                        "DINOv2 catalogue {}: kept {} crops, limit {limit}",
                        part.ic,
                        part.crops.len()
                    );
                }
                (
                    select_indexes(&part.crops, part.crops.len()),
                    stored_allowance(part.crops.len(), limit),
                    None,
                )
            }
            Some(CropAction::Truncate) => {
                let indexes = select_indexes(&part.crops, limit);
                let kept_count = indexes.len();
                let dropped = part.crops.len().saturating_sub(kept_count);
                eprintln!(
                    "DINOv2 catalogue {}: truncated to {kept_count}, dropped {dropped}",
                    part.ic
                );
                let note = Decision {
                    ic: part.ic.clone(),
                    kept: kept_count,
                    dropped,
                    overrode: false,
                };
                (indexes, stored_allowance(kept_count, limit), Some(note))
            }
            Some(CropAction::Override) => {
                let count = part.crops.len();
                eprintln!(
                    "DINOv2 catalogue {}: kept all {count} crops, limit is now {count}",
                    part.ic
                );
                overrides.push((part.ic.clone(), count));
                let note = Decision {
                    ic: part.ic.clone(),
                    kept: count,
                    dropped: 0,
                    overrode: true,
                };
                (select_indexes(&part.crops, count), Some(count), Some(note))
            }
        };
        if let Some(note) = note {
            notes.push(note);
        }
        let crops = indexes
            .into_iter()
            .map(|index| ResolvedCrop {
                key: part.crops[index].key.clone(),
                jpeg: part.crops[index].jpeg.clone(),
                mtime: part.crops[index].mtime,
            })
            .collect();
        kept.push(KeptPart {
            ic: part.ic.clone(),
            description: part.description.clone(),
            kind: part.kind.clone(),
            measurement_method: part.measurement_method.clone(),
            dimensions: part.dimensions.clone(),
            crop_limit,
            crops,
        });
    }
    let mut built = embed_kept_quiet(kept, embed)?;
    stamp_extracted(&mut built.book, &crate::dinov2::selected_model_id());
    let count = built.book.parts.len();
    commit_book(vision, &built)?;
    if let Err(err) = record_crop_limits(vision, &overrides) {
        eprintln!("warning: DINOv2 crop limits were not saved: {err}");
    }
    *lock(&CATALOGUE) = built.book;
    crate::catalog::reload_installed();
    Ok(ImportReport {
        parts: count,
        message: loaded_message(count, &notes),
    })
}

/// Embed every draft crop, write `catalogue.json`, and reload matching.
/// The previous catalogue stays in place when the embed or the write fails.
fn write_full_replace(
    vision: &Path,
    draft: &ReplaceDraft,
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<ImportReport, String> {
    let total: usize = draft.parts.iter().map(|part| part.crops.len()).sum();
    let mut done = 0usize;
    let mut kept = Vec::with_capacity(draft.parts.len());
    for part in &draft.parts {
        let limit = allowance(&draft.limits, &part.ic);
        kept.push(KeptPart {
            ic: part.ic.clone(),
            description: part.description.clone(),
            kind: part.kind.clone(),
            measurement_method: part.measurement_method.clone(),
            dimensions: part.dimensions.clone(),
            crop_limit: stored_allowance(part.crops.len(), limit),
            crops: part.crops.clone(),
        });
    }
    let mut built = embed_kept(kept, embed, progress, &mut done, total)?;
    stamp_extracted(&mut built.book, &crate::dinov2::selected_model_id());
    let count = built.book.parts.len();
    commit_book(vision, &built)?;
    *lock(&CATALOGUE) = built.book;
    crate::catalog::reload_installed();
    Ok(ImportReport {
        parts: count,
        message: format!("Catalogue loaded {count} parts."),
    })
}

fn draft_matches_book(draft: &ReplaceDraft, book: &Book) -> bool {
    if draft.parts.len() != book.parts.len() {
        return false;
    }
    draft.parts.iter().all(|part| {
        book.parts.iter().any(|saved| {
            saved.ic == part.ic
                && saved.crops.len() == part.crops.len()
                && saved.embeddings.len() == part.crops.len()
        })
    })
}

fn part_limit(draft: &ReplaceDraft, ic: &str) -> usize {
    draft
        .prompts
        .iter()
        .find(|prompt| prompt.ic == ic)
        .map(|prompt| prompt.limit)
        .unwrap_or_else(|| allowance(&draft.limits, ic))
}

/// Trim or remember limits on a catalogue `write_full_replace` already wrote.
/// Crop indexes follow that draft order. This does not embed again.
fn apply_crop_choice(
    vision: &Path,
    draft: &ReplaceDraft,
    choices: &[(String, CropAction)],
) -> Result<ImportReport, String> {
    let chosen = require_choices(draft, choices)?;
    let Some(mut book) = load_book(vision)? else {
        return Err("Could not read the catalogue.".to_string());
    };
    let active = crate::dinov2::selected_model_id();
    if !model_matches(&book, &active) || !draft_matches_book(draft, &book) {
        return Err("Could not update the catalogue.".to_string());
    }
    let mut notes = Vec::new();
    let mut overrides = Vec::new();
    for part in &draft.parts {
        let Some(saved) = book.parts.iter_mut().find(|item| item.ic == part.ic) else {
            return Err("Could not update the catalogue.".to_string());
        };
        let limit = part_limit(draft, &part.ic);
        match chosen.get(&part.ic).copied() {
            None => {
                if part.crops.len() > limit {
                    return Err(format!("Choose a crop limit for {}.", part.ic));
                }
            }
            Some(CropAction::Truncate) => {
                let indexes = select_indexes(&part.crops, limit);
                let keep: HashSet<usize> = indexes.into_iter().collect();
                let mut crops = Vec::with_capacity(keep.len());
                let mut embeddings = Vec::with_capacity(keep.len());
                for index in 0..saved.crops.len() {
                    if keep.contains(&index) {
                        crops.push(saved.crops[index].clone());
                        embeddings.push(saved.embeddings[index].clone());
                    }
                }
                if crops.len() != keep.len() {
                    return Err("Could not update the catalogue.".to_string());
                }
                let kept_count = crops.len();
                let dropped = part.crops.len().saturating_sub(kept_count);
                eprintln!(
                    "DINOv2 catalogue {}: truncated to {kept_count}, dropped {dropped}",
                    part.ic
                );
                saved.crops = crops;
                saved.embeddings = embeddings;
                saved.crop_limit = stored_allowance(kept_count, limit);
                notes.push(Decision {
                    ic: part.ic.clone(),
                    kept: kept_count,
                    dropped,
                    overrode: false,
                });
            }
            Some(CropAction::Override) => {
                let count = part.crops.len();
                eprintln!(
                    "DINOv2 catalogue {}: kept all {count} crops, limit is now {count}",
                    part.ic
                );
                saved.crop_limit = Some(count);
                overrides.push((part.ic.clone(), count));
                notes.push(Decision {
                    ic: part.ic.clone(),
                    kept: count,
                    dropped: 0,
                    overrode: true,
                });
            }
        }
    }
    let count = book.parts.len();
    store_book(vision, book)?;
    if let Err(err) = record_crop_limits(vision, &overrides) {
        eprintln!("warning: DINOv2 crop limits were not saved: {err}");
    }
    Ok(ImportReport {
        parts: count,
        message: loaded_message(count, &notes),
    })
}

fn stored_allowance(kept: usize, limit: usize) -> Option<usize> {
    if kept > MAX_CROPS {
        Some(limit.max(kept))
    } else {
        None
    }
}

fn loaded_message(count: usize, notes: &[Decision]) -> String {
    let summary = decision_summary(notes);
    if summary.is_empty() {
        format!("Catalogue loaded {count} parts.")
    } else {
        format!("Catalogue loaded {count} parts. {summary}")
    }
}

fn decision_summary(notes: &[Decision]) -> String {
    if notes.is_empty() {
        return String::new();
    }
    const SHOW: usize = 8;
    let mut parts = Vec::new();
    for note in notes.iter().take(SHOW) {
        if note.overrode {
            parts.push(format!("Kept all crops for {} ({})", note.ic, note.kept));
        } else {
            parts.push(format!(
                "Truncated {} to {} (dropped {})",
                note.ic, note.kept, note.dropped
            ));
        }
    }
    let mut text = parts.join(". ");
    if notes.len() > SHOW {
        text.push_str(&format!(
            ". {} more classes were also updated",
            notes.len() - SHOW
        ));
    }
    text.push('.');
    text
}

fn resolve_tar(bytes: &[u8], enforce_crop_limit: bool) -> Result<Vec<ResolvedPart>, String> {
    let archive = read_tar(bytes)?;
    let sources = parse_source(&archive.catalogue, enforce_crop_limit)?;
    let parts = resolve_sources(&archive, sources);
    if parts.is_empty() {
        return Err("Catalogue has no parts.".to_string());
    }
    Ok(parts)
}

fn resolve_sources(archive: &Archive, sources: Vec<SourcePart>) -> Vec<ResolvedPart> {
    let mut grouped = group_crops(&sources);
    let mut parts = Vec::new();
    for source in sources {
        let keys = grouped.remove(&source.ic).unwrap_or_default();
        let mut crops = Vec::new();
        for key in keys {
            if let Some(file) = archive.files.get(&key) {
                crops.push(ResolvedCrop {
                    key,
                    jpeg: file.bytes.clone(),
                    mtime: file.mtime,
                });
            }
        }
        log_resolved_crops(&source.ic, crops.len());
        if crops.is_empty() {
            continue;
        }
        parts.push(ResolvedPart {
            ic: source.ic,
            description: source.description,
            kind: source.kind,
            measurement_method: source.measurement_method,
            dimensions: source.dimensions,
            crops,
        });
    }
    parts
}

#[cfg(test)]
fn embed_kept_quiet(
    kept: Vec<KeptPart>,
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
) -> Result<Built, String> {
    let total = kept.iter().map(|part| part.crops.len()).sum();
    let mut done = 0usize;
    let mut silent = |_: &str, _: usize, _: usize| {};
    embed_kept(kept, embed, &mut silent, &mut done, total)
}

fn embed_kept(
    kept: Vec<KeptPart>,
    embed: &mut dyn FnMut(&[u8]) -> Result<Vec<f32>, String>,
    progress: &mut dyn FnMut(&str, usize, usize),
    done: &mut usize,
    total: usize,
) -> Result<Built, String> {
    let mut used = HashSet::new();
    let mut parts = Vec::with_capacity(kept.len());
    let mut files = Vec::new();
    let mut expected_width = None;
    for part in kept {
        let folder = allocate_folder(&mut used, &part.ic);
        let mut crops = Vec::with_capacity(part.crops.len());
        let mut embeddings = Vec::with_capacity(part.crops.len());
        for (index, crop) in part.crops.iter().enumerate() {
            report_extract(progress, &part.ic, *done, total);
            let values =
                embed(&crop.jpeg).map_err(|err| format!("Could not embed {}: {err}", part.ic))?;
            if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
                return Err(format!("Could not embed {}.", part.ic));
            }
            remember_width(&mut expected_width, &values)?;
            *done = done.saturating_add(1);
            report_extract(progress, &part.ic, *done, total);
            let file_name = stored_crop_name(&crop.key, index);
            let relative = format!("{CROPS_DIR}/{folder}/{file_name}");
            crops.push(relative.clone());
            files.push((relative, crop.jpeg.clone()));
            embeddings.push(values);
        }
        parts.push(Part {
            ic: part.ic,
            description: part.description,
            kind: part.kind,
            measurement_method: part.measurement_method,
            dimensions: part.dimensions,
            crops,
            embeddings,
            crop_limit: part.crop_limit.filter(|limit| *limit > MAX_CROPS),
        });
    }
    if parts.is_empty() {
        return Err("Catalogue has no parts.".to_string());
    }
    Ok(Built {
        book: Book::from_parts(parts),
        files,
    })
}

fn allowance(limits: &HashMap<String, usize>, ic: &str) -> usize {
    limits
        .get(ic)
        .copied()
        .filter(|count| *count > MAX_CROPS)
        .unwrap_or(MAX_CROPS)
}

fn known_limits(vision: &Path) -> HashMap<String, usize> {
    let mut limits = read_limit_map(vision);
    match load_book(vision) {
        Ok(Some(book)) => {
            for part in book.parts {
                if let Some(count) = part.crop_limit.filter(|count| *count > MAX_CROPS) {
                    let slot = limits.entry(part.ic).or_insert(0);
                    if count > *slot {
                        *slot = count;
                    }
                }
            }
        }
        Ok(None) => {}
        Err(err) => {
            eprintln!("warning: DINOv2 crop limits ignored the saved catalogue: {err}");
        }
    }
    limits.retain(|_, count| *count > MAX_CROPS);
    limits
}

const CROP_LIMITS_KEY: &str = "catalogue.cropLimits";

fn parse_limit_map(text: &str) -> Option<HashMap<String, usize>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let map = value.get("limits")?.as_object()?;
    let mut limits = HashMap::new();
    for (ic, value) in map {
        let Some(count) = value.as_u64().and_then(|count| usize::try_from(count).ok()) else {
            continue;
        };
        if count > MAX_CROPS {
            limits.insert(ic.clone(), count);
        }
    }
    Some(limits)
}

fn limit_document(limits: &HashMap<String, usize>) -> Result<String, String> {
    let mut map = Map::new();
    let mut keys: Vec<&String> = limits.keys().collect();
    keys.sort();
    for key in keys {
        map.insert(key.clone(), serde_json::json!(limits[key]));
    }
    let file = serde_json::json!({
        "version": 1,
        "limits": map,
    });
    let mut bytes = serde_json::to_vec_pretty(&file)
        .map_err(|_| "Could not write the catalogue.".to_string())?;
    bytes.push(b'\n');
    String::from_utf8(bytes).map_err(|_| "Could not write the catalogue.".to_string())
}

fn read_limit_map(vision: &Path) -> HashMap<String, usize> {
    match crate::app_db::get_text(vision, CROP_LIMITS_KEY) {
        Ok(Some(text)) => {
            return parse_limit_map(&text).unwrap_or_else(|| {
                eprintln!("warning: DINOv2 crop limits are not a valid file.");
                HashMap::new()
            });
        }
        Ok(None) => {}
        Err(err) => {
            eprintln!("warning: DINOv2 crop limits were not read: {err}");
            return HashMap::new();
        }
    }
    let path = vision.join(CROP_LIMITS_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return HashMap::new(),
        Err(err) => {
            eprintln!("warning: DINOv2 crop limits were not read: {err}");
            return HashMap::new();
        }
    };
    let Some(limits) = parse_limit_map(&text) else {
        eprintln!("warning: DINOv2 crop limits are not a valid file.");
        return HashMap::new();
    };
    if let Ok(document) = limit_document(&limits)
        && crate::app_db::put_text(vision, CROP_LIMITS_KEY, &document).is_ok()
        && !cfg!(test)
    {
        let _ = fs::remove_file(&path);
    }
    limits
}

fn record_crop_limits(vision: &Path, updates: &[(String, usize)]) -> Result<(), String> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut limits = read_limit_map(vision);
    for (ic, count) in updates {
        if *count > MAX_CROPS {
            limits.insert(ic.clone(), *count);
        }
    }
    let document = limit_document(&limits)?;
    crate::app_db::put_text(vision, CROP_LIMITS_KEY, &document)
        .map_err(|err| format!("Could not write the catalogue: {err}"))
}

fn mtimes_differ(crops: &[ResolvedCrop]) -> bool {
    let Some(first) = crops.first() else {
        return false;
    };
    crops.iter().any(|crop| crop.mtime != first.mtime)
}

fn select_indexes(crops: &[ResolvedCrop], keep: usize) -> Vec<usize> {
    if crops.len() <= keep {
        return (0..crops.len()).collect();
    }
    if mtimes_differ(crops) {
        newest_indexes(crops, keep)
    } else {
        sample_indexes(crops, keep)
    }
}

fn newest_indexes(crops: &[ResolvedCrop], keep: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..crops.len()).collect();
    order.sort_by(|&left, &right| {
        crops[right]
            .mtime
            .cmp(&crops[left].mtime)
            .then(right.cmp(&left))
    });
    order.truncate(keep);
    order.sort_unstable();
    order
}

fn sample_indexes(crops: &[ResolvedCrop], keep: usize) -> Vec<usize> {
    let mut indexes: Vec<usize> = (0..crops.len()).collect();
    let mut state = sample_seed(crops);
    for start in 0..keep {
        let span = indexes.len() - start;
        let offset = (state as usize) % span;
        indexes.swap(start, start + offset);
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
    }
    indexes.truncate(keep);
    indexes.sort_unstable();
    indexes
}

fn sample_seed(crops: &[ResolvedCrop]) -> u64 {
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    state ^= (std::process::id() as u64).wrapping_shl(32);
    for crop in crops {
        state = state.wrapping_mul(0x100_0000_01B3);
        for byte in crop.key.bytes() {
            state = state.wrapping_add(u64::from(byte));
        }
    }
    if state == 0 {
        0xA5A5_A5A5_A5A5_A5A5
    } else {
        state
    }
}

fn prompt_thumbs(part: &ResolvedPart) -> Vec<String> {
    let mut order: Vec<usize> = if mtimes_differ(&part.crops) {
        let mut indexes = newest_indexes(&part.crops, part.crops.len());
        indexes.sort_by(|&left, &right| {
            part.crops[right]
                .mtime
                .cmp(&part.crops[left].mtime)
                .then(right.cmp(&left))
        });
        indexes
    } else {
        (0..part.crops.len()).collect()
    };
    let mut thumbs = Vec::new();
    for index in order.drain(..) {
        if thumbs.len() == PROMPT_THUMBS {
            break;
        }
        if let Some(url) = thumb_jpeg(&part.crops[index].jpeg) {
            thumbs.push(url);
        }
    }
    thumbs
}

fn thumb_jpeg(bytes: &[u8]) -> Option<String> {
    let image = image::load_from_memory(bytes).ok()?;
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

/// Update item codes that are in `incoming` and add the rest.
/// Parts and crops that are only in `existing` stay.
fn merge_loaded(vision: &Path, incoming: Built, existing: Book) -> Result<Built, String> {
    let mut incoming_files: HashMap<String, Vec<u8>> = incoming.files.into_iter().collect();
    let mut indexes = HashMap::new();
    for (index, part) in incoming.book.parts.iter().enumerate() {
        indexes.insert(part.ic.clone(), index);
    }
    let mut needed = Vec::new();
    for old in &existing.parts {
        needed.extend(old.crops.iter().cloned());
    }
    let stored = stored_crops(vision, &needed)?;
    let mut consumed = HashSet::new();
    let mut used = HashSet::new();
    let mut parts = Vec::with_capacity(existing.parts.len() + incoming.book.parts.len());
    let mut files = Vec::new();
    for old in existing.parts {
        if let Some(index) = indexes.get(&old.ic).copied() {
            consumed.insert(index);
            parts.push(fuse_part(
                &old,
                &incoming.book.parts[index],
                &stored,
                &mut incoming_files,
                &mut files,
                &mut used,
            )?);
        } else {
            keep_part_files(&old, &stored, &mut files, &mut used)?;
            parts.push(old);
        }
    }
    for (index, part) in incoming.book.parts.into_iter().enumerate() {
        if consumed.contains(&index) {
            continue;
        }
        for crop in &part.crops {
            let bytes = incoming_files
                .remove(crop)
                .ok_or_else(|| "Could not write the catalogue.".to_string())?;
            push_crop_file(&mut files, &mut used, crop.clone(), bytes)?;
        }
        parts.push(part);
    }
    if parts.len() > MAX_PARTS {
        return Err("The catalogue has too many parts.".to_string());
    }
    Ok(Built {
        book: Book::from_parts(parts),
        files,
    })
}

/// Tar crops replace a saved crop with the same original file name.
/// Any other saved crop on that item code stays, including an admin crop.
fn fuse_part(
    old: &Part,
    new_part: &Part,
    stored: &HashMap<String, crate::app_db::StoredCrop>,
    incoming_files: &mut HashMap<String, Vec<u8>>,
    files: &mut Vec<(String, Vec<u8>)>,
    used: &mut HashSet<String>,
) -> Result<Part, String> {
    let incoming_keys: HashSet<String> = new_part
        .crops
        .iter()
        .map(|crop| crop_match_key(crop))
        .collect();
    let mut crops = Vec::new();
    let mut embeddings = Vec::new();
    for (crop, embedding) in new_part.crops.iter().zip(&new_part.embeddings) {
        let bytes = incoming_files
            .remove(crop)
            .ok_or_else(|| "Could not write the catalogue.".to_string())?;
        crops.push(crop.clone());
        embeddings.push(embedding.clone());
        push_crop_file(files, used, crop.clone(), bytes)?;
    }
    for (crop, embedding) in old.crops.iter().zip(&old.embeddings) {
        if incoming_keys.contains(&crop_match_key(crop)) {
            continue;
        }
        let bytes = jpeg_from_stored(stored.get(crop))?;
        crops.push(crop.clone());
        embeddings.push(embedding.clone());
        push_crop_file(files, used, crop.clone(), bytes)?;
    }
    if crops.len() > MAX_CROPS {
        return Err(format!("Part {} has too many crops.", old.ic));
    }
    let crop_limit = old
        .crop_limit
        .filter(|limit| *limit > MAX_CROPS && crops.len() <= *limit);
    Ok(Part {
        ic: old.ic.clone(),
        description: prefer_text(&new_part.description, &old.description),
        kind: prefer_text(&new_part.kind, &old.kind),
        measurement_method: prefer_text(&new_part.measurement_method, &old.measurement_method),
        dimensions: if new_part.dimensions.is_null() {
            old.dimensions.clone()
        } else {
            new_part.dimensions.clone()
        },
        crops,
        embeddings,
        crop_limit,
    })
}

fn prefer_text(incoming: &str, saved: &str) -> String {
    if incoming.is_empty() {
        saved.to_string()
    } else {
        incoming.to_string()
    }
}

/// `{index}-{original name}` is a crop written by a tar import.
/// `crop-1.jpg` is an admin save and keeps its whole file name.
fn crop_match_key(relative: &str) -> String {
    let file = relative.rsplit(['/', '\\']).next().unwrap_or(relative);
    match file.split_once('-') {
        Some((index, rest))
            if !index.is_empty()
                && !rest.is_empty()
                && index.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            rest.to_string()
        }
        _ => file.to_string(),
    }
}

fn keep_part_files(
    part: &Part,
    stored: &HashMap<String, crate::app_db::StoredCrop>,
    files: &mut Vec<(String, Vec<u8>)>,
    used: &mut HashSet<String>,
) -> Result<(), String> {
    for crop in &part.crops {
        let bytes = jpeg_from_stored(stored.get(crop))?;
        push_crop_file(files, used, crop.clone(), bytes)?;
    }
    Ok(())
}

fn stored_crops(
    vision: &Path,
    crops: &[String],
) -> Result<HashMap<String, crate::app_db::StoredCrop>, String> {
    crate::app_db::crops_for_sources(vision, crops)
        .map_err(|err| format!("Could not read the catalogue: {err}"))
}

fn jpeg_from_stored(image: Option<&crate::app_db::StoredCrop>) -> Result<Vec<u8>, String> {
    let Some(image) = image else {
        return Err("Could not read the catalogue.".to_string());
    };
    if image.bytes.len() < 3 || image.bytes[0] != 0xff || image.bytes[1] != 0xd8 {
        return Err("Could not read the catalogue.".to_string());
    }
    Ok(image.bytes.clone())
}

fn push_crop_file(
    files: &mut Vec<(String, Vec<u8>)>,
    used: &mut HashSet<String>,
    relative: String,
    bytes: Vec<u8>,
) -> Result<(), String> {
    if !used.insert(relative.clone()) {
        return Err("Could not write the catalogue.".to_string());
    }
    files.push((relative, bytes));
    Ok(())
}

fn commit_book(vision: &Path, built: &Built) -> Result<(), String> {
    let crops: HashMap<String, Vec<u8>> = built.files.iter().cloned().collect();
    persist_rows(vision, &built.book, &crops)
}

#[cfg(test)]
fn book_json(book: &Book) -> Result<Vec<u8>, String> {
    let file = StoredFile {
        version: VERSION,
        embedding_model: book.embedding_model.clone(),
        embeddings_stale: book.embeddings_stale,
        parts: book
            .parts
            .iter()
            .map(|part| StoredPart {
                ic: part.ic.clone(),
                description: part.description.clone(),
                kind: part.kind.clone(),
                measurement_method: part.measurement_method.clone(),
                dimensions: part.dimensions.clone(),
                crops: part.crops.clone(),
                embeddings: part.embeddings.clone(),
                crop_limit: part.crop_limit.filter(|limit| *limit > MAX_CROPS),
                extra: Map::new(),
            })
            .collect(),
    };
    let mut bytes =
        serde_json::to_vec(&file).map_err(|_| "Could not write the catalogue.".to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Serialize, Deserialize)]
struct StoredFile {
    version: u32,
    #[serde(
        default,
        rename = "embeddingModel",
        skip_serializing_if = "Option::is_none"
    )]
    embedding_model: Option<String>,
    #[serde(
        default,
        rename = "embeddingsStale",
        skip_serializing_if = "skip_false"
    )]
    embeddings_stale: bool,
    parts: Vec<StoredPart>,
}

fn skip_false(value: &bool) -> bool {
    !*value
}

#[derive(Serialize, Deserialize)]
struct StoredPart {
    ic: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(
        default,
        rename = "measurementMethod",
        skip_serializing_if = "String::is_empty"
    )]
    measurement_method: String,
    #[serde(default)]
    dimensions: Value,
    crops: Vec<String>,
    embeddings: Vec<Vec<f32>>,
    #[serde(default, rename = "cropLimit", skip_serializing_if = "Option::is_none")]
    crop_limit: Option<usize>,
    /// Sizes written beside `dimensions` on an older file. They are folded in
    /// on load and are not written back.
    #[serde(default, flatten, skip_serializing_if = "Map::is_empty")]
    extra: Map<String, Value>,
}

/// Part text stored in `app_config`. Embeddings stay in `dino_catalogue`.
#[derive(Serialize, Deserialize)]
struct PartMeta {
    ic: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(
        default,
        rename = "measurementMethod",
        skip_serializing_if = "String::is_empty"
    )]
    measurement_method: String,
    #[serde(default)]
    dimensions: Value,
    #[serde(default)]
    crops: Vec<String>,
    #[serde(default, rename = "cropLimit", skip_serializing_if = "Option::is_none")]
    crop_limit: Option<usize>,
}

fn row_model_name(model: Option<&str>) -> &'static str {
    match model {
        Some("s_reg") => "s_reg",
        Some("b_reg") => "b_reg",
        _ => "legacy",
    }
}

fn parts_metadata(book: &Book) -> Result<String, String> {
    let parts: Vec<PartMeta> = book
        .parts
        .iter()
        .map(|part| PartMeta {
            ic: part.ic.clone(),
            description: part.description.clone(),
            kind: part.kind.clone(),
            measurement_method: part.measurement_method.clone(),
            dimensions: part.dimensions.clone(),
            crops: part.crops.clone(),
            crop_limit: part.crop_limit.filter(|limit| *limit > MAX_CROPS),
        })
        .collect();
    serde_json::to_string(&parts).map_err(|_| "Could not write the catalogue.".to_string())
}

fn persist_book(vision: &Path, book: &Book) -> Result<(), String> {
    persist_rows(vision, book, &HashMap::new())
}

fn persist_rows(
    vision: &Path,
    book: &Book,
    crops: &HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    if book.parts.len() > MAX_PARTS {
        return Err("The catalogue has too many parts.".to_string());
    }
    let parts_json = parts_metadata(book)?;
    let model = row_model_name(book.embedding_model.as_deref());
    let mut rows = Vec::new();
    let mut live = Vec::new();
    for part in &book.parts {
        if part.crops.len() != part.embeddings.len() {
            return Err("Could not write the catalogue.".to_string());
        }
        for (crop, values) in part.crops.iter().zip(&part.embeddings) {
            if !safe_crop_path(crop)
                || values.is_empty()
                || values.iter().any(|value| !value.is_finite())
            {
                return Err("Could not write the catalogue.".to_string());
            }
            live.push((part.ic.clone(), crop.clone()));
            let image = crops.get(crop).filter(|bytes| !bytes.is_empty()).cloned();
            rows.push(crate::app_db::EmbeddingRow {
                ic: part.ic.clone(),
                source: crop.clone(),
                values: values.clone(),
                crop: image,
            });
        }
    }
    crate::app_db::write_catalogue(
        vision,
        &crate::app_db::CatalogueMeta {
            embedding_model: book.embedding_model.clone(),
            embeddings_stale: book.embeddings_stale,
            parts_json,
        },
        model,
        &rows,
        &live,
    )
    .map_err(|err| format!("Could not write the catalogue: {err}"))
}

/// Vectors already stored for `active` replace the in-memory set.
/// `None` means that model does not cover every saved crop.
fn book_for_model(vision: &Path, book: &Book, active: &str) -> Result<Option<Book>, String> {
    if active != "s_reg" && active != "b_reg" {
        return Ok(None);
    }
    let rows = crate::app_db::embeddings_for_model(vision, active)
        .map_err(|err| format!("Could not read the catalogue: {err}"))?;
    let mut by_source = HashMap::new();
    for row in rows {
        by_source.insert((row.ic, row.source), row.values);
    }
    let mut parts = Vec::with_capacity(book.parts.len());
    for part in &book.parts {
        let mut embeddings = Vec::with_capacity(part.crops.len());
        for crop in &part.crops {
            let Some(values) = by_source.get(&(part.ic.clone(), crop.clone())) else {
                return Ok(None);
            };
            if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
                return Ok(None);
            }
            embeddings.push(values.clone());
        }
        parts.push(Part {
            ic: part.ic.clone(),
            description: part.description.clone(),
            kind: part.kind.clone(),
            measurement_method: part.measurement_method.clone(),
            dimensions: part.dimensions.clone(),
            crops: part.crops.clone(),
            embeddings,
            crop_limit: part.crop_limit,
        });
    }
    if book_has_vectors(book) && parts.iter().all(|part| part.embeddings.is_empty()) {
        return Ok(None);
    }
    Ok(Some(Book {
        parts,
        embedding_model: Some(active.to_string()),
        embeddings_stale: false,
    }))
}

enum CatalogueMatch {
    Missing,
    Stale,
    Ready(Book),
}

fn match_load(vision: &Path, active: &str) -> Result<CatalogueMatch, String> {
    let Some(book) = load_book(vision)? else {
        return Ok(CatalogueMatch::Missing);
    };
    if book_has_vectors(&book) && !model_matches(&book, active) {
        return Ok(CatalogueMatch::Stale);
    }
    Ok(CatalogueMatch::Ready(book))
}

pub(crate) fn matching_entries(active: &str) -> Vec<crate::catalog::CatalogEntry> {
    let vision = match vision_directory() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("warning: {err} Catalogue matching has no parts.");
            return Vec::new();
        }
    };
    let path = catalogue_path(&vision);
    let book = match match_load(&vision, active) {
        Ok(CatalogueMatch::Missing) => {
            eprintln!(
                "warning: catalogue is missing at {}. Catalogue matching has no parts.",
                path.display()
            );
            return Vec::new();
        }
        Ok(CatalogueMatch::Stale) => {
            eprintln!(
                "warning: catalogue embeddings at {} do not match DINOv2 model {active}. Matching skips them until the next extract.",
                path.display()
            );
            return Vec::new();
        }
        Ok(CatalogueMatch::Ready(book)) => book,
        Err(err) => {
            eprintln!("warning: {err} Catalogue matching has no parts.");
            return Vec::new();
        }
    };
    if let Some(width) = crate::dinov2::loaded_cls_width()
        && book_has_vectors(&book)
        && !stored_widths_match(&book, width)
    {
        eprintln!(
            "warning: catalogue embeddings at {} do not match DINOv2 model {active}. Matching skips them until the next extract.",
            path.display()
        );
        return Vec::new();
    }
    let entries = entries_of(&book);
    if entries.is_empty() {
        eprintln!(
            "warning: catalogue at {} has no parts. Catalogue matching has no parts.",
            path.display()
        );
    }
    entries
}

fn entries_of(book: &Book) -> Vec<crate::catalog::CatalogEntry> {
    let mut entries = Vec::new();
    for part in &book.parts {
        for values in &part.embeddings {
            let Ok(embedding) = crate::inference::dino::l2_normalize(values) else {
                continue;
            };
            entries.push(crate::catalog::CatalogEntry {
                ic: part.ic.clone(),
                description: part.description.trim().to_string(),
                part_type: part.kind.trim().to_string(),
                size: crate::part_size::canonical(
                    &part.measurement_method,
                    &part.kind,
                    &part.dimensions,
                ),
                embedding,
            });
        }
    }
    entries
}

/// `Ok(None)` when the database and the old JSON file are both absent.
fn load_book(vision: &Path) -> Result<Option<Book>, String> {
    if let Some(book) = load_book_db(vision)? {
        return Ok(Some(book));
    }
    if cfg!(test) && is_repo_vision(vision) {
        return Ok(None);
    }
    let Some(mut book) = load_book_file(vision)? else {
        return Ok(None);
    };
    // An unstamped book was extracted by a model this app no longer selects.
    if !matches!(book.embedding_model.as_deref(), Some("s_reg" | "b_reg")) {
        book.embedding_model = None;
        if book_has_vectors(&book) {
            book.embeddings_stale = true;
        }
    }
    persist_book(vision, &book)?;
    if !cfg!(test) {
        let _ = fs::remove_file(catalogue_file(vision));
        let _ = fs::remove_file(vision.join("catalogue.json.tmp"));
    }
    Ok(Some(book))
}

fn load_book_db(vision: &Path) -> Result<Option<Book>, String> {
    let Some(meta) = crate::app_db::read_catalogue_meta(vision)
        .map_err(|err| format!("Could not read the catalogue: {err}"))?
    else {
        return Ok(None);
    };
    let stored: Vec<PartMeta> = serde_json::from_str(&meta.parts_json)
        .map_err(|_| "Could not read the catalogue.".to_string())?;
    if stored.len() > MAX_PARTS {
        return Err("The catalogue has too many parts.".to_string());
    }
    let model = row_model_name(meta.embedding_model.as_deref());
    let rows = crate::app_db::embeddings_for_model(vision, model)
        .map_err(|err| format!("Could not read the catalogue: {err}"))?;
    let mut by_source = HashMap::new();
    for row in rows {
        by_source.insert((row.ic, row.source), row.values);
    }
    let mut parts = Vec::with_capacity(stored.len());
    for part in stored {
        parts.push(part_from_meta(part, &by_source)?);
    }
    Ok(Some(Book {
        parts,
        embedding_model: meta.embedding_model,
        embeddings_stale: meta.embeddings_stale,
    }))
}

fn part_from_meta(
    part: PartMeta,
    by_source: &HashMap<(String, String), Vec<f32>>,
) -> Result<Part, String> {
    let invalid = || "Could not read the catalogue.".to_string();
    let ic = clean_ic(&part.ic).map_err(|_| invalid())?;
    let crop_limit = part.crop_limit.filter(|limit| *limit > MAX_CROPS);
    let allowed = crop_limit.unwrap_or(MAX_CROPS);
    if part.crops.len() > allowed {
        return Err(invalid());
    }
    let mut embeddings = Vec::with_capacity(part.crops.len());
    for crop in &part.crops {
        if !safe_crop_path(crop) {
            return Err(invalid());
        }
        let Some(values) = by_source.get(&(ic.clone(), crop.clone())) else {
            return Err(invalid());
        };
        if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
            return Err(invalid());
        }
        embeddings.push(values.clone());
    }
    let kind = part.kind.trim().to_string();
    let measurement_method = clean_method(&part.measurement_method);
    Ok(Part {
        ic,
        description: part.description.trim().to_string(),
        kind: kind.clone(),
        measurement_method: measurement_method.clone(),
        dimensions: crate::part_size::normalize(&measurement_method, &kind, &part.dimensions),
        crops: part.crops,
        embeddings,
        crop_limit,
    })
}

/// `Ok(None)` when the old JSON file is absent.
fn load_book_file(vision: &Path) -> Result<Option<Book>, String> {
    let path = catalogue_file(vision);
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
    Ok(Some(Book {
        parts,
        embedding_model: file.embedding_model,
        embeddings_stale: file.embeddings_stale,
    }))
}

fn stored_part(part: StoredPart, path: &Path) -> Result<Part, String> {
    let invalid = || {
        format!(
            "DINOv2 catalogue at {} is not a valid catalogue.",
            path.display()
        )
    };
    let ic = clean_ic(&part.ic).map_err(|_| invalid())?;
    let crop_limit = part.crop_limit.filter(|limit| *limit > MAX_CROPS);
    let allowed = crop_limit.unwrap_or(MAX_CROPS);
    if part.crops.len() != part.embeddings.len() || part.crops.len() > allowed {
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
    let kind = part.kind.trim().to_string();
    let measurement_method = clean_method(&part.measurement_method);
    let mut sizes = part.extra;
    sizes.insert("dimensions".to_string(), part.dimensions);
    Ok(Part {
        ic,
        description: part.description.trim().to_string(),
        kind: kind.clone(),
        measurement_method: measurement_method.clone(),
        dimensions: crate::part_size::from_part(&measurement_method, &kind, &sizes),
        crops: part.crops,
        embeddings: part.embeddings,
        crop_limit,
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
    book.parts.remove(index);
    persist_book(vision, &book)?;
    *lock(&CATALOGUE) = book;
    crate::catalog::reload_installed();
    drop_tolerance(vision, ic);
    Ok(())
}

fn store_book(vision: &Path, book: Book) -> Result<(), String> {
    persist_book(vision, &book)?;
    *lock(&CATALOGUE) = book;
    crate::catalog::reload_installed();
    Ok(())
}

fn unique_indexes(indexes: &[usize], len: usize) -> Result<Vec<usize>, String> {
    if indexes.is_empty() {
        return Err("Choose an image.".to_string());
    }
    let mut unique = Vec::new();
    for index in indexes {
        if *index >= len {
            return Err("That image is not in the catalogue.".to_string());
        }
        if !unique.contains(index) {
            unique.push(*index);
        }
    }
    unique.sort_unstable();
    Ok(unique)
}

fn permutation(order: &[usize], len: usize) -> Result<Vec<usize>, String> {
    if order.len() != len {
        return Err("Could not reorder the images.".to_string());
    }
    let mut seen = vec![false; len];
    for index in order {
        if *index >= len || seen[*index] {
            return Err("Could not reorder the images.".to_string());
        }
        seen[*index] = true;
    }
    Ok(order.to_vec())
}

fn delete_crops(vision: &Path, ic: &str, indexes: &[usize]) -> Result<(), String> {
    let ic = clean_ic(ic)?;
    let Some(mut book) = load_book(vision)? else {
        return Err("That part is not in the catalogue.".to_string());
    };
    if book.parts.iter().all(|part| part.ic != ic) {
        return Err("That part is not in the catalogue.".to_string());
    }
    {
        let part = book
            .parts
            .iter_mut()
            .find(|part| part.ic == ic)
            .expect("part");
        let unique = unique_indexes(indexes, part.crops.len())?;
        for index in unique.into_iter().rev() {
            part.crops.remove(index);
            part.embeddings.remove(index);
        }
    }
    store_book(vision, book)?;
    Ok(())
}

fn reassign_crops(vision: &Path, ic: &str, indexes: &[usize], target: &str) -> Result<(), String> {
    let ic = clean_ic(ic)?;
    let target = clean_ic(target)?;
    if ic == target {
        return Err("Choose a different IC.".to_string());
    }
    let Some(mut book) = load_book(vision)? else {
        return Err("That part is not in the catalogue.".to_string());
    };
    let source_index = book
        .parts
        .iter()
        .position(|part| part.ic == ic)
        .ok_or_else(|| "That part is not in the catalogue.".to_string())?;
    let unique = unique_indexes(indexes, book.parts[source_index].crops.len())?;
    let moving = unique.len();
    if let Some(target_index) = book.parts.iter().position(|part| part.ic == target) {
        let allowed = book.parts[target_index].crop_limit.unwrap_or(MAX_CROPS);
        if book.parts[target_index].crops.len() + moving > allowed {
            return Err(format!("Part {target} has too many crops."));
        }
    } else if book.parts.len() >= MAX_PARTS {
        return Err("The catalogue has too many parts.".to_string());
    }
    let mut moved_crops = Vec::with_capacity(moving);
    let mut moved_embeddings = Vec::with_capacity(moving);
    for index in unique.into_iter().rev() {
        moved_crops.push(book.parts[source_index].crops.remove(index));
        moved_embeddings.push(book.parts[source_index].embeddings.remove(index));
    }
    moved_crops.reverse();
    moved_embeddings.reverse();
    if let Some(target_index) = book.parts.iter().position(|part| part.ic == target) {
        book.parts[target_index].crops.extend(moved_crops);
        book.parts[target_index].embeddings.extend(moved_embeddings);
    } else {
        book.parts.push(Part {
            ic: target,
            description: String::new(),
            kind: String::new(),
            measurement_method: String::new(),
            dimensions: Value::Null,
            crops: moved_crops,
            embeddings: moved_embeddings,
            crop_limit: None,
        });
    }
    store_book(vision, book)
}

fn reorder_crops(vision: &Path, ic: &str, order: &[usize]) -> Result<(), String> {
    let ic = clean_ic(ic)?;
    let Some(mut book) = load_book(vision)? else {
        return Err("That part is not in the catalogue.".to_string());
    };
    {
        let Some(part) = book.parts.iter_mut().find(|part| part.ic == ic) else {
            return Err("That part is not in the catalogue.".to_string());
        };
        let order = permutation(order, part.crops.len())?;
        let crops: Vec<String> = order
            .iter()
            .map(|index| part.crops[*index].clone())
            .collect();
        let embeddings: Vec<Vec<f32>> = order
            .iter()
            .map(|index| part.embeddings[*index].clone())
            .collect();
        part.crops = crops;
        part.embeddings = embeddings;
    }
    store_book(vision, book)
}

fn set_tolerance(vision: &Path, ic: &str, key: &str, mm: Option<f64>) -> Result<(), String> {
    let ic = clean_ic(ic)?;
    let key = norm_size_key(key);
    if key.is_empty() || key.len() > 80 {
        return Err("That dimension is not on this part.".to_string());
    }
    let Some(book) = load_book(vision)? else {
        return Err("That part is not in the catalogue.".to_string());
    };
    let Some(part) = book.parts.iter().find(|part| part.ic == ic) else {
        return Err("That part is not in the catalogue.".to_string());
    };
    if !size_rows(&part.dimensions).iter().any(|row| row.key == key) {
        return Err("That dimension is not on this part.".to_string());
    }
    let mut saved = read_tolerances(vision);
    match mm {
        None => {
            if let Some(entry) = saved.get_mut(&ic) {
                entry.remove(&key);
                if entry.is_empty() {
                    saved.remove(&ic);
                }
            }
        }
        Some(mm) => {
            if !mm.is_finite() || !(0.0..=1000.0).contains(&mm) {
                return Err("Enter a tolerance from 0 to 1000 mm.".to_string());
            }
            saved.entry(ic).or_default().insert(key, mm);
        }
    }
    write_tolerances(vision, &saved)
}

fn drop_tolerance(vision: &Path, ic: &str) {
    let mut saved = read_tolerances(vision);
    if saved.remove(ic).is_some() {
        let _ = write_tolerances(vision, &saved);
    }
}

#[derive(Serialize, Deserialize, Default)]
struct ToleranceFile {
    version: u32,
    #[serde(default)]
    parts: HashMap<String, HashMap<String, f64>>,
}

const TOLERANCES_KEY: &str = "catalogue.tolerances";

fn tolerances_path(vision: &Path) -> PathBuf {
    vision.join(TOLERANCES_FILE)
}

fn parse_tolerances(text: &str) -> Option<HashMap<String, HashMap<String, f64>>> {
    serde_json::from_str::<ToleranceFile>(text)
        .ok()
        .map(|file| file.parts)
}

fn read_tolerances(vision: &Path) -> HashMap<String, HashMap<String, f64>> {
    match crate::app_db::get_text(vision, TOLERANCES_KEY) {
        Ok(Some(text)) => return parse_tolerances(&text).unwrap_or_default(),
        Ok(None) => {}
        Err(err) => {
            eprintln!("warning: DINOv2 tolerances were not read: {err}");
            return HashMap::new();
        }
    }
    let text = match fs::read_to_string(tolerances_path(vision)) {
        Ok(text) => text,
        Err(_) => return HashMap::new(),
    };
    let Some(parts) = parse_tolerances(&text) else {
        return HashMap::new();
    };
    if write_tolerances(vision, &parts).is_ok() && !cfg!(test) {
        let _ = fs::remove_file(tolerances_path(vision));
    }
    parts
}

fn write_tolerances(
    vision: &Path,
    parts: &HashMap<String, HashMap<String, f64>>,
) -> Result<(), String> {
    let file = ToleranceFile {
        version: 1,
        parts: parts.clone(),
    };
    let text =
        serde_json::to_string(&file).map_err(|_| "Could not save the tolerance.".to_string())?;
    crate::app_db::put_text(vision, TOLERANCES_KEY, &text)
        .map_err(|err| format!("Could not save the tolerance: {err}"))
}

fn views_from(book: &Book, vision: &Path) -> Vec<CatalogueView> {
    let tolerances = read_tolerances(vision);
    let mut sources = Vec::new();
    for part in &book.parts {
        sources.extend(part.crops.iter().cloned());
    }
    let images = match crate::app_db::crops_for_sources(vision, &sources) {
        Ok(images) => images,
        Err(err) => {
            eprintln!("warning: DINOv2 crops were not read: {err}");
            HashMap::new()
        }
    };
    book.parts
        .iter()
        .map(|part| {
            let saved = tolerances.get(&part.ic);
            let mut thumbs = Vec::new();
            let mut crops = Vec::new();
            for (index, crop) in part.crops.iter().enumerate() {
                let (src, hash) = crop_preview(images.get(crop));
                if src.starts_with("data:image/") {
                    thumbs.push(src.clone());
                }
                crops.push(CatalogueCropView { index, src, hash });
            }
            let sizes = size_rows(&part.dimensions)
                .into_iter()
                .map(|row| CatalogueSizeView {
                    tolerance_mm: saved.and_then(|entry| entry.get(&row.key).copied()),
                    key: row.key,
                    label: row.label,
                    value: row.value,
                })
                .collect();
            CatalogueView {
                ic: part.ic.clone(),
                description: part.description.clone(),
                kind: part.kind.clone(),
                dimensions: crate::part_size::canonical(
                    &part.measurement_method,
                    &part.kind,
                    &part.dimensions,
                ),
                thumbs,
                crops,
                sizes,
                added_unix: oldest_stamp(&images, &part.crops),
            }
        })
        .collect()
}

struct SizeRow {
    key: String,
    label: String,
    value: String,
}

fn size_rows(dimensions: &Value) -> Vec<SizeRow> {
    crate::part_size::rows(dimensions)
        .into_iter()
        .map(|(key, label, value)| SizeRow {
            key: key.to_string(),
            label: label.to_string(),
            value,
        })
        .collect()
}

fn norm_size_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn crop_preview(image: Option<&crate::app_db::StoredCrop>) -> (String, String) {
    let Some(image) = image else {
        return (String::new(), String::new());
    };
    if image.bytes.is_empty() {
        return (String::new(), String::new());
    }
    let hash = sha256_hex(&image.bytes);
    let src = thumb_jpeg(&image.bytes).unwrap_or_default();
    (src, hash)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn oldest_stamp(
    images: &HashMap<String, crate::app_db::StoredCrop>,
    crops: &[String],
) -> Option<i64> {
    let mut oldest: Option<i64> = None;
    for crop in crops {
        let Some(secs) = images.get(crop).and_then(|image| image.created_unix) else {
            continue;
        };
        oldest = Some(oldest.map_or(secs, |have| have.min(secs)));
    }
    oldest
}

/// Structured dimensions and measurement method for one catalogue item code.
/// Detection reads these when metrology is on. The catalogue file is unchanged.
pub(crate) struct CatalogueSize {
    pub dimensions: Value,
    pub measurement_method: String,
    pub kind: String,
}

pub(crate) fn part_dimensions(ic: &str) -> Option<CatalogueSize> {
    let wanted = ic.trim();
    if wanted.is_empty() {
        return None;
    }
    let book = lock(&CATALOGUE);
    let part = book.parts.iter().find(|part| part.ic == wanted)?;
    Some(CatalogueSize {
        dimensions: part.dimensions.clone(),
        measurement_method: part.measurement_method.clone(),
        kind: part.kind.clone(),
    })
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
    use std::collections::HashSet;

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

    fn stored_catalogue(dir: &Path) -> Vec<u8> {
        book_json(&load_book(dir).unwrap().unwrap()).unwrap()
    }

    fn stored_limits(dir: &Path) -> Value {
        let text = crate::app_db::get_text(dir, "catalogue.cropLimits")
            .unwrap()
            .expect("crop limits");
        serde_json::from_str(&text).unwrap()
    }

    fn keep_only_model(dir: &Path, model: &str) {
        let connection = rusqlite::Connection::open(catalogue_path(dir)).unwrap();
        connection
            .execute("DELETE FROM dino_catalogue WHERE model != ?1", [model])
            .unwrap();
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

    fn pack_owned(files: &[(String, Vec<u8>, u64)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, bytes, mtime) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(*mtime);
            header.set_cksum();
            builder.append(&header, bytes.as_slice()).unwrap();
        }
        builder.into_inner().unwrap()
    }

    /// One class with `count` crops, plus a single OK crop.
    fn oversized_tar(ic: &str, count: usize, timed: bool, jpeg: &[u8]) -> Vec<u8> {
        let mut names = Vec::new();
        let mut files = Vec::new();
        for index in 0..count {
            let name = format!("crops/{ic}/c{index:02}.jpg");
            names.push(name.clone());
            files.push((
                name,
                jpeg.to_vec(),
                if timed { (index as u64) + 1 } else { 0 },
            ));
        }
        let ok = "crops/OK/ok.jpg".to_string();
        files.push((ok.clone(), jpeg.to_vec(), 1));
        let catalogue = serde_json::json!({
            "parts": [
                { "ic": ic, "description": "Hinge", "crops": names },
                { "ic": "OK", "description": "Ok", "crops": [ok] }
            ]
        });
        files.insert(
            0,
            (
                "catalogue.json".to_string(),
                serde_json::to_vec(&catalogue).unwrap(),
                1,
            ),
        );
        pack_owned(&files)
    }

    fn crop_names(part: &Part) -> HashSet<String> {
        part.crops.iter().map(|crop| crop_match_key(crop)).collect()
    }

    fn stored_jpeg(dir: &Path, source: &str) -> Option<Vec<u8>> {
        crate::app_db::crop_bytes(dir, source).unwrap()
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
        let count = import_tar_bytes(&bytes, &dir, false, &mut stub_embed).unwrap();
        assert_eq!(count, 2);
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts.len(), 2);
        assert_eq!(book.parts[0].ic, "2BBMNM08");
        assert_eq!(book.parts[0].description, "M8 nyloc");
        assert_eq!(book.parts[0].kind, "fastener");
        assert_eq!(book.parts[0].dimensions["thread"], "M8");
        assert_eq!(book.parts[0].dimensions["thread_mm"].as_f64(), Some(8.0));
        assert_eq!(book.parts[0].dimensions["length_mm"].as_f64(), Some(12.0));
        assert_eq!(book.parts[0].crops.len(), 3);
        assert_eq!(book.parts[0].embeddings.len(), 3);
        assert!(book.parts[0].crops[0].starts_with("catalogue-crops/2BBMNM08/"));
        assert!(stored_jpeg(&dir, &book.parts[0].crops[0]).is_some());
        assert!(!dir.join("catalogue-crops").exists());
        assert_eq!(book.parts[1].ic, "2ECT");
        assert_eq!(book.parts[1].kind, "label");
        assert!(book.parts[1].dimensions.is_null());
        let listed = catalogue_list_from(&dir).unwrap();
        assert_eq!(listed.parts.len(), 2);
        assert_eq!(listed.parts[0].description, "M8 nyloc");
        assert_eq!(listed.parts[0].kind, "fastener");
        assert_eq!(listed.parts[0].dimensions, "M8x12");
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
        import_tar_bytes(&first, &dir, false, &mut stub_embed).unwrap();
        let before = stored_catalogue(&dir);
        let second = pack(&[
            (
                "pack/catalogue.json",
                br#"{"parts":[{"ic":"NEW","crops":["crops/b.jpg"]}]}"#,
            ),
            ("pack/crops/b.jpg", jpeg.as_slice()),
        ]);
        let err = import_tar_bytes(&second, &dir, false, &mut |_| {
            Err("DINOv2 model is not loaded.".to_string())
        })
        .unwrap_err();
        assert!(err.contains("DINOv2 model is not loaded"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
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
        assert!(built.book.parts[0].dimensions.is_null());
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
    fn an_appended_crop_keeps_the_part_and_matches_itself() {
        let dir = temp_dir("append");
        let existing = serde_json::json!({
            "version": 1,
            "parts": [{
                "ic": "FAST-1",
                "description": "Washer",
                "type": "washer",
                "dimensions": { "diameterMm": 8 },
                "crops": [],
                "embeddings": []
            }]
        });
        fs::write(
            dir.join("catalogue.json"),
            serde_json::to_vec(&existing).unwrap(),
        )
        .unwrap();
        let jpeg = tiny_jpeg();
        let embedding = vec![3.0_f32, 4.0];
        let ic = append_catalogue_crop(&dir, "FAST-1", &jpeg, &embedding).unwrap();
        assert_eq!(ic, "FAST-1");
        let book = load_book(&dir).unwrap().unwrap();
        assert!(book.parts[0].crops[0].ends_with("crop-1.jpg"));
        assert_eq!(stored_jpeg(&dir, &book.parts[0].crops[0]).unwrap(), jpeg);
        assert!(!dir.join("catalogue-crops").exists());
        assert_eq!(book.parts.len(), 1);
        assert_eq!(book.parts[0].description, "Washer");
        assert_eq!(book.parts[0].kind, "washer");
        assert_eq!(book.parts[0].crops.len(), 1);
        assert_eq!(book.parts[0].embeddings, vec![embedding.clone()]);
        assert!(!dir.join("dinov2-references.json").exists());
        assert!(
            fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| entry.path().extension().and_then(|ext| ext.to_str()) != Some("txt"))
        );

        let again = append_catalogue_crop(&dir, "FAST-1", &jpeg, &embedding).unwrap();
        assert_eq!(again, "FAST-1");
        let book = load_book(&dir).unwrap().unwrap();
        assert!(
            book.parts[0]
                .crops
                .iter()
                .any(|crop| crop.ends_with("crop-2.jpg"))
        );
        assert!(
            book.parts[0]
                .crops
                .iter()
                .all(|crop| { stored_jpeg(&dir, crop).as_deref() == Some(jpeg.as_slice()) })
        );
        assert_eq!(book.parts[0].crops.len(), 2);

        let entries = entries_of(&load_book(&dir).unwrap().unwrap());
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.ic == "FAST-1"));
        let query = entries[0].embedding.clone();
        let matched = crate::catalog::score_query(&entries, &query);
        assert_eq!(
            matched.top.as_ref().map(|item| item.ic.as_str()),
            Some("FAST-1")
        );
        assert!(matched.top.unwrap().score > 0.99);

        let fresh = append_catalogue_crop(&dir, "NEW-9", &jpeg, &embedding).unwrap();
        assert_eq!(fresh, "NEW-9");
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts.len(), 2);
        let fresh_crop = book
            .parts
            .iter()
            .find(|part| part.ic == "NEW-9")
            .unwrap()
            .crops[0]
            .clone();
        assert!(fresh_crop.ends_with("crop-1.jpg"));
        assert_eq!(stored_jpeg(&dir, &fresh_crop).unwrap(), jpeg);

        let err = append_catalogue_crop(&dir, "  ", &jpeg, &embedding).unwrap_err();
        assert!(err.to_ascii_lowercase().contains("ic"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_extract_rejects_a_vector_that_is_not_the_loaded_width() {
        let err = accept_embedding(vec![0.0; 384], 768).unwrap_err();
        assert!(err.contains("384"), "{err}");
        assert!(err.contains("768"), "{err}");
        assert_eq!(accept_embedding(vec![0.25; 768], 768).unwrap().len(), 768);
        assert_eq!(accept_embedding(vec![0.25; 384], 384).unwrap().len(), 384);
        assert!(accept_embedding(vec![0.25; 384], 0).is_err());
    }

    #[test]
    fn an_extract_does_not_write_when_the_embedding_length_changes() {
        let dir = temp_dir("width-change");
        let jpeg = tiny_jpeg();
        let bytes = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"A","crops":["crops/a.jpg"]},{"ic":"B","crops":["crops/b.jpg"]}]}"#,
            ),
            ("crops/a.jpg", jpeg.as_slice()),
            ("crops/b.jpg", jpeg.as_slice()),
        ]);
        let mut n = 0;
        let err = import_tar_bytes(&bytes, &dir, true, &mut |_| {
            n += 1;
            if n == 1 {
                Ok(vec![1.0; 768])
            } else {
                Ok(vec![1.0; 384])
            }
        })
        .unwrap_err();
        assert!(err.contains("384"), "{err}");
        assert!(err.contains("768"), "{err}");
        assert!(load_book(&dir).unwrap().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stored_vectors_of_another_width_do_not_match() {
        let book = Book {
            parts: vec![Part {
                ic: "A".to_string(),
                description: String::new(),
                kind: String::new(),
                measurement_method: String::new(),
                dimensions: Value::Null,
                crops: vec!["catalogue-crops/A/a.jpg".to_string()],
                embeddings: vec![vec![0.0; 384]],
                crop_limit: None,
            }],
            embedding_model: Some("b_reg".to_string()),
            embeddings_stale: false,
        };
        assert!(model_matches(&book, "b_reg"));
        assert!(!stored_widths_match(&book, 768));
        assert!(stored_widths_match(&book, 384));
    }

    #[test]
    fn a_model_change_marks_embeddings_stale_and_the_next_extract_replaces_them() {
        let dir = temp_dir("dino-model");
        let jpeg = tiny_jpeg();
        let original = vec![1.0_f32, 0.0, 0.0, 1.0];
        append_catalogue_crop(&dir, "FAST-1", &jpeg, &original).unwrap();
        let mut book = load_book(&dir).unwrap().unwrap();
        book.embedding_model = Some("s_reg".to_string());
        book.embeddings_stale = false;
        persist_book(&dir, &book).unwrap();
        keep_only_model(&dir, "s_reg");

        assert!(align_embeddings_file(&dir, "b_reg").unwrap());
        let stale = load_book(&dir).unwrap().unwrap();
        assert!(stale.embeddings_stale);
        assert_eq!(stale.embedding_model.as_deref(), Some("s_reg"));
        assert_eq!(stale.parts[0].embeddings, vec![original.clone()]);

        assert!(!align_embeddings_file(&dir, "s_reg").unwrap());
        let restored = load_book(&dir).unwrap().unwrap();
        assert!(!restored.embeddings_stale);
        assert_eq!(restored.parts[0].embeddings, vec![original.clone()]);

        assert!(align_embeddings_file(&dir, "b_reg").unwrap());
        let mut next = load_book(&dir).unwrap().unwrap();
        let fresh = vec![9.0_f32, 8.0, 7.0];
        let mut done = 0usize;
        let total = next.parts.iter().map(|part| part.crops.len()).sum();
        let mut silent = |_: &str, _: usize, _: usize| {};
        reembed_book(
            &dir,
            &mut next,
            &mut |_bytes| Ok(fresh.clone()),
            &mut silent,
            &mut done,
            total,
        )
        .unwrap();
        assert_eq!(next.parts[0].embeddings, vec![fresh.clone()]);
        assert!(
            next.parts[0]
                .embeddings
                .iter()
                .all(|values| values != &original)
        );
        stamp_extracted(&mut next, "b_reg");
        persist_book(&dir, &next).unwrap();
        let saved = load_book(&dir).unwrap().unwrap();
        assert!(!saved.embeddings_stale);
        assert_eq!(saved.embedding_model.as_deref(), Some("b_reg"));
        assert_eq!(saved.parts[0].embeddings, vec![fresh]);
        assert!(!model_matches(&saved, "s_reg"));
        assert!(model_matches(&saved, "b_reg"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_matcher_uses_rows_for_the_selected_model() {
        let dir = temp_dir("model-rows");
        let jpeg = tiny_jpeg();
        let short = vec![1.0_f32, 0.0];
        append_catalogue_crop(&dir, "FAST-1", &jpeg, &short).unwrap();
        let mut book = load_book(&dir).unwrap().unwrap();
        book.embedding_model = Some("b_reg".to_string());
        book.embeddings_stale = false;
        persist_book(&dir, &book).unwrap();
        match match_load(&dir, "b_reg").unwrap() {
            CatalogueMatch::Ready(loaded) => {
                assert!(!loaded.embeddings_stale);
                assert_eq!(loaded.embedding_model.as_deref(), Some("b_reg"));
                assert_eq!(loaded.parts[0].embeddings, vec![short]);
            }
            CatalogueMatch::Missing => panic!("missing catalogue"),
            CatalogueMatch::Stale => panic!("b_reg rows were stale"),
        }
        assert!(matches!(
            match_load(&dir, "s_reg").unwrap(),
            CatalogueMatch::Stale
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_legacy_catalogue_file_is_not_treated_as_the_selected_model() {
        let dir = temp_dir("legacy-file");
        let legacy = serde_json::json!({
            "version": 1,
            "parts": [{
                "ic": "OLD-1",
                "crops": ["catalogue-crops/OLD-1/crop-1.jpg"],
                "embeddings": [[1.0, 0.0]]
            }]
        });
        let _ = fs::remove_file(catalogue_path(&dir));
        fs::write(
            dir.join("catalogue.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        assert!(align_embeddings_file(&dir, "s_reg").unwrap());
        let legacy_book = load_book(&dir).unwrap().unwrap();
        assert!(legacy_book.embeddings_stale);
        assert!(legacy_book.embedding_model.is_none());
        assert_eq!(legacy_book.parts[0].embeddings, vec![vec![1.0, 0.0]]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_part_rejects_another_crop() {
        let dir = temp_dir("full");
        let jpeg = tiny_jpeg();
        for _ in 0..MAX_CROPS {
            append_catalogue_crop(&dir, "FAST-1", &jpeg, &[1.0, 0.0]).unwrap();
        }
        let err = append_catalogue_crop(&dir, "FAST-1", &jpeg, &[1.0, 0.0]).unwrap_err();
        assert!(err.contains("too many crops"));
        assert_eq!(
            load_book(&dir).unwrap().unwrap().parts[0].crops.len(),
            MAX_CROPS
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_catalogue_file_lists_nothing() {
        let dir = temp_dir("missing");
        let listed = catalogue_list_from(&dir).unwrap();
        assert!(listed.parts.is_empty());
        assert!(listed.path.ends_with("results.sqlite"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn measurement_method_round_trips_and_keeps_expected_sizes() {
        let dir = temp_dir("method");
        let stored = serde_json::json!({
            "version": 1,
            "parts": [{
                "ic": "BOLT-1",
                "description": "M6x40",
                "type": "bolt",
                "measurementMethod": "Bolt",
                "dimensions": { "headHeight": 5 },
                "lengthUnderHead": 40,
                "threadDiameter": 6,
                "crops": [],
                "embeddings": []
            }]
        });
        fs::write(
            dir.join("catalogue.json"),
            serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts[0].measurement_method, "bolt");
        assert!(book.parts[0].dimensions.get("headHeight").is_none());
        assert_eq!(book.parts[0].dimensions["thread"], "M6");
        assert_eq!(book.parts[0].dimensions["thread_mm"].as_f64(), Some(6.0));
        assert_eq!(book.parts[0].dimensions["length_mm"].as_f64(), Some(40.0));
        let bytes = book_json(&book).unwrap();
        let saved: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved["parts"][0]["measurementMethod"], "bolt");
        assert!(saved["parts"][0].get("lengthUnderHead").is_none());

        let jpeg = tiny_jpeg();
        let catalogue = br#"{
            "parts": [{
                "ic": "W-1",
                "description": "Washer",
                "measurementMethod": "cable-tie",
                "dimensions": [{ "name": "OD", "mm": 12 }],
                "outerDiameter": 16,
                "thickness": 1.5,
                "crops": ["crops/w.jpg"]
            }]
        }"#;
        let packed = pack(&[
            ("catalogue.json", catalogue),
            ("crops/w.jpg", jpeg.as_slice()),
        ]);
        let built = build_from_tar(&packed, &mut stub_embed).unwrap();
        assert_eq!(built.book.parts[0].measurement_method, "cable_tie");
        assert_eq!(built.book.parts[0].dimensions["od_mm"].as_f64(), Some(16.0));
        assert_eq!(
            built.book.parts[0].dimensions["thickness_mm"].as_f64(),
            Some(1.5)
        );

        let plain = serde_json::json!({
            "version": 1,
            "parts": [{
                "ic": "PLAIN",
                "description": "No method",
                "type": "other",
                "dimensions": "M6x40",
                "crops": [],
                "embeddings": []
            }]
        });
        let plain_dir = temp_dir("method-plain");
        fs::write(
            plain_dir.join("catalogue.json"),
            serde_json::to_vec(&plain).unwrap(),
        )
        .unwrap();
        let plain_book = load_book(&plain_dir).unwrap().unwrap();
        assert!(plain_book.parts[0].measurement_method.is_empty());
        assert!(plain_book.parts[0].dimensions.is_null());
        let plain_json: Value = serde_json::from_slice(&book_json(&plain_book).unwrap()).unwrap();
        assert!(plain_json["parts"][0].get("measurementMethod").is_none());
        let _ = fs::remove_dir_all(&plain_dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_keeps_local_parts_and_updates_matching_codes() {
        let dir = temp_dir("merge");
        let jpeg = tiny_jpeg();
        let mut updated = jpeg.clone();
        updated.push(1);
        let rules = br#"{"partRules":[{"parent":"nut"}]}"#;
        let references = br#"{"version":1,"vectors":{}}"#;
        fs::write(dir.join("config.local.json"), rules).unwrap();
        fs::write(dir.join("dinov2-references.json"), references).unwrap();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{
                    "parts": [
                        {"ic":"KEEP","description":"Local only","crops":["crops/keep.jpg"]},
                        {
                            "ic":"BOTH",
                            "description":"Old text",
                            "type":"nut",
                            "dimensions": { "acrossFlats": 13 },
                            "crops":["crops/both.jpg"]
                        },
                        {"ic":"SPARSE","description":"Keep me","crops":["crops/sparse.jpg"]}
                    ]
                }"#,
            ),
            ("crops/keep.jpg", jpeg.as_slice()),
            ("crops/both.jpg", jpeg.as_slice()),
            ("crops/sparse.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, true, &mut stub_embed).unwrap();
        append_catalogue_crop(&dir, "BOTH", &jpeg, &[9.0, 0.0]).unwrap();
        append_catalogue_crop(&dir, "ADMIN", &jpeg, &[8.0, 0.0]).unwrap();

        let second = pack(&[
            (
                "catalogue.json",
                br#"{
                    "parts": [
                        {"ic":"BOTH","description":"New text","crops":["crops/both.jpg"]},
                        {"ic":"SPARSE","description":"","crops":["crops/sparse.jpg"]},
                        {"ic":"NEW","description":"Added","crops":["crops/new.jpg"]}
                    ]
                }"#,
            ),
            ("crops/both.jpg", updated.as_slice()),
            ("crops/sparse.jpg", jpeg.as_slice()),
            ("crops/new.jpg", jpeg.as_slice()),
        ]);
        let count = import_tar_bytes(&second, &dir, false, &mut stub_embed).unwrap();
        assert_eq!(count, 5);
        let book = load_book(&dir).unwrap().unwrap();
        let ics: Vec<_> = book.parts.iter().map(|part| part.ic.as_str()).collect();
        assert_eq!(ics, vec!["KEEP", "BOTH", "SPARSE", "ADMIN", "NEW"]);
        assert_eq!(book.parts[0].description, "Local only");
        assert!(stored_jpeg(&dir, &book.parts[0].crops[0]).is_some());

        let both = &book.parts[1];
        assert_eq!(both.description, "New text");
        assert_eq!(both.kind, "nut");
        assert_eq!(both.dimensions["af_mm"].as_f64(), Some(13.0));
        assert_eq!(both.crops.len(), 2);
        assert!(both.crops[0].ends_with("both.jpg"));
        assert!(both.crops[1].ends_with("crop-1.jpg"));
        assert_eq!(both.embeddings[0][3], updated.len() as f32);
        assert_eq!(both.embeddings[1], vec![9.0, 0.0]);
        assert!(stored_jpeg(&dir, &both.crops[0]).is_some());
        assert!(stored_jpeg(&dir, &both.crops[1]).is_some());
        assert_eq!(book.parts[2].description, "Keep me");
        assert_eq!(book.parts[3].ic, "ADMIN");
        assert_eq!(book.parts[3].embeddings, vec![vec![8.0, 0.0]]);
        assert!(stored_jpeg(&dir, &book.parts[3].crops[0]).is_some());
        assert_eq!(book.parts[4].description, "Added");

        let again = import_tar_bytes(&second, &dir, false, &mut stub_embed).unwrap();
        assert_eq!(again, 5);
        let both_again = &load_book(&dir).unwrap().unwrap().parts[1];
        assert_eq!(both_again.crops.len(), 2);
        assert_eq!(both_again.embeddings[1], vec![9.0, 0.0]);
        assert_eq!(fs::read(dir.join("config.local.json")).unwrap(), rules);
        assert_eq!(
            fs::read(dir.join("dinov2-references.json")).unwrap(),
            references
        );
        assert!(!dir.join("catalogue-crops").exists());
        assert!(!dir.join("catalogue-crops.incoming").exists());
        assert!(!dir.join("catalogue-crops.previous").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_removes_parts_missing_from_the_tar() {
        let dir = temp_dir("replace");
        let jpeg = tiny_jpeg();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{
                    "parts": [
                        {"ic":"KEEP","description":"Local","crops":["crops/keep.jpg"]},
                        {"ic":"BOTH","description":"Stay","crops":["crops/both.jpg"]}
                    ]
                }"#,
            ),
            ("crops/keep.jpg", jpeg.as_slice()),
            ("crops/both.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, true, &mut stub_embed).unwrap();
        append_catalogue_crop(&dir, "ADMIN", &jpeg, &[8.0, 0.0]).unwrap();
        let saved = load_book(&dir).unwrap().unwrap();
        let keep_source = saved.parts[0].crops[0].clone();
        let admin_source = saved.parts[2].crops[0].clone();
        assert!(stored_jpeg(&dir, &keep_source).is_some());
        assert!(stored_jpeg(&dir, &admin_source).is_some());

        let second = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"BOTH","description":"Only","crops":["crops/both.jpg"]}]}"#,
            ),
            ("crops/both.jpg", jpeg.as_slice()),
        ]);
        let count = import_tar_bytes(&second, &dir, true, &mut stub_embed).unwrap();
        assert_eq!(count, 1);
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts.len(), 1);
        assert_eq!(book.parts[0].ic, "BOTH");
        assert_eq!(book.parts[0].description, "Only");
        assert!(stored_jpeg(&dir, &keep_source).is_none());
        assert!(stored_jpeg(&dir, &admin_source).is_none());
        assert!(stored_jpeg(&dir, &book.parts[0].crops[0]).is_some());
        assert!(!dir.join("catalogue-crops").exists());
        assert!(!dir.join("dinov2-references.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_local_crop_leaves_the_saved_catalogue() {
        let dir = temp_dir("missing-crop");
        let jpeg = tiny_jpeg();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"KEEP","description":"Stay","crops":["crops/keep.jpg"]}]}"#,
            ),
            ("crops/keep.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, true, &mut stub_embed).unwrap();
        let before = stored_catalogue(&dir);
        let source = load_book(&dir).unwrap().unwrap().parts[0].crops[0].clone();
        {
            let connection =
                rusqlite::Connection::open(crate::app_db::database_path(&dir)).unwrap();
            connection
                .execute(
                    "UPDATE dino_catalogue SET crop = NULL WHERE source = ?1",
                    [&source],
                )
                .unwrap();
        }
        let second = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"NEW","crops":["crops/new.jpg"]}]}"#,
            ),
            ("crops/new.jpg", jpeg.as_slice()),
        ]);
        let err = import_tar_bytes(&second, &dir, false, &mut stub_embed).unwrap_err();
        assert!(err.contains("Could not read the catalogue"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
        assert!(!dir.join("catalogue-crops.incoming").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_refuses_too_many_crops_and_leaves_the_catalogue() {
        let dir = temp_dir("merge-full");
        let jpeg = tiny_jpeg();
        for _ in 0..MAX_CROPS {
            append_catalogue_crop(&dir, "FAST-1", &jpeg, &[1.0, 0.0]).unwrap();
        }
        let before = stored_catalogue(&dir);
        let packed = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"FAST-1","description":"Bolt","crops":["crops/extra.jpg"]}]}"#,
            ),
            ("crops/extra.jpg", jpeg.as_slice()),
        ]);
        let err = import_tar_bytes(&packed, &dir, false, &mut stub_embed).unwrap_err();
        assert!(err.contains("too many crops"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
        assert_eq!(
            load_book(&dir).unwrap().unwrap().parts[0].crops.len(),
            MAX_CROPS
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_refuses_too_many_parts_and_leaves_the_catalogue() {
        let dir = temp_dir("merge-parts");
        let mut parts = Vec::new();
        for index in 0..MAX_PARTS {
            parts.push(serde_json::json!({
                "ic": format!("P{index}"),
                "description": "Saved",
                "crops": [],
                "embeddings": []
            }));
        }
        let stored = serde_json::json!({ "version": 1, "parts": parts });
        fs::write(
            dir.join("catalogue.json"),
            serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();
        let before = stored_catalogue(&dir);
        let jpeg = tiny_jpeg();
        let packed = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"NEW","crops":["crops/new.jpg"]}]}"#,
            ),
            ("crops/new.jpg", jpeg.as_slice()),
        ]);
        let err = import_tar_bytes(&packed, &dir, false, &mut stub_embed).unwrap_err();
        assert!(err.contains("too many parts"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_prompts_then_truncates_to_the_newest_crops() {
        let dir = temp_dir("replace-trim");
        let jpeg = tiny_jpeg();
        let rules = br#"{"partRules":[{"parent":"nut"}]}"#;
        fs::write(dir.join("config.local.json"), rules).unwrap();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"KEEP","description":"Local","crops":["crops/keep.jpg"]}]}"#,
            ),
            ("crops/keep.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, true, &mut stub_embed).unwrap();
        let before = stored_catalogue(&dir);
        let count = MAX_CROPS + 6;
        let bytes = oversized_tar("ACH01", count, true, &jpeg);
        let draft = inspect_replace(&bytes, &dir).unwrap();
        assert_eq!(stored_catalogue(&dir), before);
        assert!(!dir.join(CROP_LIMITS_FILE).exists());
        assert_eq!(draft.prompts.len(), 1);
        assert_eq!(draft.prompts[0].ic, "ACH01");
        assert_eq!(draft.prompts[0].count, count);
        assert_eq!(draft.prompts[0].limit, MAX_CROPS);
        assert_eq!(draft.prompts[0].dropped, 6);
        assert!(draft.prompts[0].recent);
        assert!(!draft.prompts[0].thumbs.is_empty());
        assert!(draft.prompts[0].thumbs.len() <= PROMPT_THUMBS);
        let err = commit_replace(&dir, &draft, &[], &mut stub_embed).unwrap_err();
        assert!(err.contains("Choose a crop limit"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);

        let report = commit_replace(
            &dir,
            &draft,
            &[("ACH01".to_string(), CropAction::Truncate)],
            &mut stub_embed,
        )
        .unwrap();
        assert_eq!(report.parts, 2);
        assert!(
            report
                .message
                .starts_with("Catalogue loaded 2 parts. Truncated ACH01 to 24 (dropped 6)."),
            "{}",
            report.message
        );
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts.len(), 2);
        assert!(book.parts.iter().all(|part| part.ic != "KEEP"));
        let ach = book.parts.iter().find(|part| part.ic == "ACH01").unwrap();
        let mut expected = HashSet::new();
        for index in 6..count {
            expected.insert(format!("c{index:02}.jpg"));
        }
        assert_eq!(crop_names(ach), expected);
        assert!(ach.crop_limit.is_none());
        let ok = book.parts.iter().find(|part| part.ic == "OK").unwrap();
        assert_eq!(ok.crops.len(), 1);
        assert!(!dir.join(CROP_LIMITS_FILE).exists());
        assert_eq!(fs::read(dir.join("config.local.json")).unwrap(), rules);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_override_is_remembered_and_a_later_replace_uses_it() {
        let dir = temp_dir("replace-override");
        let jpeg = tiny_jpeg();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"KEEP","crops":["crops/keep.jpg"]}]}"#,
            ),
            ("crops/keep.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, true, &mut stub_embed).unwrap();
        let before = stored_catalogue(&dir);
        let wide = oversized_tar("ACH01", 30, true, &jpeg);
        let draft = inspect_replace(&wide, &dir).unwrap();
        let err = commit_replace(
            &dir,
            &draft,
            &[("ACH01".to_string(), CropAction::Override)],
            &mut |_| Err("DINOv2 model is not loaded.".to_string()),
        )
        .unwrap_err();
        assert!(err.contains("DINOv2 model is not loaded"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
        assert!(!dir.join(CROP_LIMITS_FILE).exists());

        let report = commit_replace(
            &dir,
            &draft,
            &[("ACH01".to_string(), CropAction::Override)],
            &mut stub_embed,
        )
        .unwrap();
        assert!(
            report.message.contains("Kept all crops for ACH01 (30)."),
            "{}",
            report.message
        );
        let book = load_book(&dir).unwrap().unwrap();
        let ach = book.parts.iter().find(|part| part.ic == "ACH01").unwrap();
        assert_eq!(ach.crops.len(), 30);
        assert_eq!(ach.crop_limit, Some(30));
        let limits: Value = stored_limits(&dir);
        assert_eq!(limits["limits"]["ACH01"], serde_json::json!(30));

        let within = oversized_tar("ACH01", 28, true, &jpeg);
        let again = inspect_replace(&within, &dir).unwrap();
        assert!(again.prompts.is_empty());
        let kept = commit_replace(&dir, &again, &[], &mut stub_embed).unwrap();
        assert_eq!(kept.message, "Catalogue loaded 2 parts.");
        let book = load_book(&dir).unwrap().unwrap();
        let ach = book.parts.iter().find(|part| part.ic == "ACH01").unwrap();
        assert_eq!(ach.crops.len(), 28);
        assert_eq!(ach.crop_limit, Some(30));
        let limits: Value = stored_limits(&dir);
        assert_eq!(limits["limits"]["ACH01"], serde_json::json!(30));

        let larger = oversized_tar("ACH01", 40, true, &jpeg);
        let prompt = inspect_replace(&larger, &dir).unwrap();
        assert_eq!(prompt.prompts.len(), 1);
        assert_eq!(prompt.prompts[0].limit, 30);
        assert_eq!(prompt.prompts[0].dropped, 10);
        commit_replace(
            &dir,
            &prompt,
            &[("ACH01".to_string(), CropAction::Truncate)],
            &mut stub_embed,
        )
        .unwrap();
        let book = load_book(&dir).unwrap().unwrap();
        let ach = book.parts.iter().find(|part| part.ic == "ACH01").unwrap();
        assert_eq!(ach.crops.len(), 30);
        assert_eq!(ach.crop_limit, Some(30));
        let mut expected = HashSet::new();
        for index in 10..40 {
            expected.insert(format!("c{index:02}.jpg"));
        }
        assert_eq!(crop_names(ach), expected);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_samples_when_the_archive_has_no_crop_dates() {
        let dir = temp_dir("replace-sample");
        let jpeg = tiny_jpeg();
        let bytes = oversized_tar("ACH01", MAX_CROPS + 6, false, &jpeg);
        let draft = inspect_replace(&bytes, &dir).unwrap();
        assert_eq!(draft.prompts.len(), 1);
        assert!(!draft.prompts[0].recent);
        let report = commit_replace(
            &dir,
            &draft,
            &[("ACH01".to_string(), CropAction::Truncate)],
            &mut stub_embed,
        )
        .unwrap();
        assert!(report.message.contains("dropped 6"), "{}", report.message);
        let book = load_book(&dir).unwrap().unwrap();
        let ach = book.parts.iter().find(|part| part.ic == "ACH01").unwrap();
        let names = crop_names(ach);
        assert_eq!(names.len(), MAX_CROPS);
        assert_eq!(ach.crops.len(), MAX_CROPS);
        for name in &names {
            assert!(name.starts_with('c') && name.ends_with(".jpg"), "{name}");
        }
        assert!(book.parts.iter().any(|part| part.ic == "OK"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_replace_writes_before_the_crop_choice_and_keeps_those_vectors() {
        let dir = temp_dir("replace-write-first");
        let jpeg = tiny_jpeg();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"KEEP","crops":["crops/keep.jpg"]}]}"#,
            ),
            ("crops/keep.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, true, &mut stub_embed).unwrap();
        let mut seeded = load_book(&dir).unwrap().unwrap();
        seeded.embeddings_stale = true;
        seeded.embedding_model = None;
        persist_book(&dir, &seeded).unwrap();
        let before = stored_catalogue(&dir);
        assert!(load_book(&dir).unwrap().unwrap().embeddings_stale);

        let count = MAX_CROPS + 6;
        let wide = oversized_tar("ACH01", count, true, &jpeg);
        let mut draft = inspect_replace(&wide, &dir).unwrap();
        assert_eq!(draft.prompts.len(), 1);
        assert_eq!(stored_catalogue(&dir), before);

        let err = write_full_replace(
            &dir,
            &draft,
            &mut |_| Err("embed failed".to_string()),
            &mut |_: &str, _: usize, _: usize| {},
        )
        .unwrap_err();
        assert!(err.contains("Could not embed"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
        assert!(load_book(&dir).unwrap().unwrap().embeddings_stale);
        assert!(!model_matches(
            &load_book(&dir).unwrap().unwrap(),
            &crate::dinov2::selected_model_id()
        ));
        assert!(!dir.join(CROP_LIMITS_FILE).exists());
        assert!(!dir.join("catalogue-crops.incoming").exists());
        let err = apply_crop_choice(&dir, &draft, &[("ACH01".to_string(), CropAction::Truncate)])
            .unwrap_err();
        assert!(err.contains("Could not update the catalogue"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);

        let mut n = 0u32;
        let mut ticks = Vec::new();
        let report = write_full_replace(
            &dir,
            &draft,
            &mut |bytes: &[u8]| {
                if bytes.len() < 2 || bytes[0] != 0xff {
                    return Err("Could not read the image.".to_string());
                }
                n += 1;
                let mut values = vec![0.25_f32; 768];
                values[0] = n as f32;
                Ok(values)
            },
            &mut |ic: &str, done: usize, total: usize| {
                ticks.push((ic.to_string(), done, total));
            },
        )
        .unwrap();
        assert_eq!(report.message, "Catalogue loaded 2 parts.");
        let expected_total = count + 1;
        let last = ticks.last().expect("progress");
        assert_eq!(last.1, expected_total);
        assert_eq!(last.2, expected_total);
        assert!(ticks.iter().any(|tick| tick.1 < tick.2));
        assert!(ticks.iter().any(|tick| tick.0 == "ACH01"));
        let active = crate::dinov2::selected_model_id();
        let book = load_book(&dir).unwrap().unwrap();
        assert!(!book.embeddings_stale);
        assert_eq!(book.embedding_model.as_deref(), Some(active.as_str()));
        assert!(model_matches(&book, &active));
        let ach = book.parts.iter().find(|part| part.ic == "ACH01").unwrap();
        assert_eq!(ach.crops.len(), count);
        assert_eq!(ach.crop_limit, Some(count));
        assert!(ach.embeddings.iter().all(|values| values.len() == 768));
        assert_eq!(ach.embeddings[0][0], 1.0);
        assert_eq!(ach.embeddings[count - 1][0], count as f32);
        assert!(!dir.join(CROP_LIMITS_FILE).exists());

        let err = apply_crop_choice(&dir, &draft, &[]).unwrap_err();
        assert!(err.contains("Choose a crop limit"), "{err}");
        assert_eq!(
            load_book(&dir)
                .unwrap()
                .unwrap()
                .parts
                .iter()
                .find(|part| part.ic == "ACH01")
                .unwrap()
                .crops
                .len(),
            count
        );

        release_draft_jpegs(&mut draft);
        let trimmed =
            apply_crop_choice(&dir, &draft, &[("ACH01".to_string(), CropAction::Truncate)])
                .unwrap();
        assert!(
            trimmed
                .message
                .starts_with("Catalogue loaded 2 parts. Truncated ACH01 to 24 (dropped 6)."),
            "{}",
            trimmed.message
        );
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.embedding_model.as_deref(), Some(active.as_str()));
        assert!(!book.embeddings_stale);
        let ach = book.parts.iter().find(|part| part.ic == "ACH01").unwrap();
        let mut expected = HashSet::new();
        for index in 6..count {
            expected.insert(format!("c{index:02}.jpg"));
        }
        assert_eq!(crop_names(ach), expected);
        assert!(ach.crop_limit.is_none());
        let heads: Vec<f32> = ach.embeddings.iter().map(|values| values[0]).collect();
        let wanted: Vec<f32> = (7..=count).map(|value| value as f32).collect();
        assert_eq!(heads, wanted);
        assert!(ach.embeddings.iter().all(|values| values.len() == 768));
        let ok = book.parts.iter().find(|part| part.ic == "OK").unwrap();
        assert_eq!(ok.embeddings[0].len(), 768);
        assert_eq!(ok.embeddings[0][0], (count + 1) as f32);
        assert!(!dir.join(CROP_LIMITS_FILE).exists());

        let again = oversized_tar("ACH01", count, true, &jpeg);
        let prompt = inspect_replace(&again, &dir).unwrap();
        assert_eq!(prompt.prompts.len(), 1);
        write_full_replace(
            &dir,
            &prompt,
            &mut |_bytes: &[u8]| {
                let mut values = vec![0.5_f32; 768];
                values[0] = 4.0;
                Ok(values)
            },
            &mut |_: &str, _: usize, _: usize| {},
        )
        .unwrap();
        let kept = apply_crop_choice(
            &dir,
            &prompt,
            &[("ACH01".to_string(), CropAction::Override)],
        )
        .unwrap();
        assert!(
            kept.message.contains("Kept all crops for ACH01 (30)."),
            "{}",
            kept.message
        );
        let book = load_book(&dir).unwrap().unwrap();
        let ach = book.parts.iter().find(|part| part.ic == "ACH01").unwrap();
        assert_eq!(ach.crops.len(), count);
        assert_eq!(ach.crop_limit, Some(count));
        assert!(
            ach.embeddings
                .iter()
                .all(|values| values.len() == 768 && values[0] == 4.0)
        );
        let limits: Value = stored_limits(&dir);
        assert_eq!(limits["limits"]["ACH01"], serde_json::json!(count));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_still_rejects_a_class_over_the_crop_limit() {
        let dir = temp_dir("merge-over");
        let jpeg = tiny_jpeg();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"KEEP","crops":["crops/keep.jpg"]}]}"#,
            ),
            ("crops/keep.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, true, &mut stub_embed).unwrap();
        let before = stored_catalogue(&dir);
        let bytes = oversized_tar("ACH01", MAX_CROPS + 1, false, &jpeg);
        let err = import_tar_bytes(&bytes, &dir, false, &mut stub_embed).unwrap_err();
        assert!(err.contains("too many crops"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
        assert_eq!(load_book(&dir).unwrap().unwrap().parts[0].ic, "KEEP");
        assert!(!dir.join(CROP_LIMITS_FILE).exists());

        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut files = vec![("catalogue.json".to_string(), Vec::new(), 1_u64)];
        for index in 0..13 {
            let name = format!("crops/ACH01/a{index:02}.jpg");
            left.push(name.clone());
            files.push((name, jpeg.clone(), 0));
        }
        for index in 0..12 {
            let name = format!("crops/ACH01/b{index:02}.jpg");
            right.push(name.clone());
            files.push((name, jpeg.clone(), 0));
        }
        let catalogue = serde_json::json!({
            "parts": [
                { "ic": "ACH01", "crops": left },
                { "ic": "ACH01", "description": "Later", "crops": right }
            ]
        });
        files[0].1 = serde_json::to_vec(&catalogue).unwrap();
        let duplicate = pack_owned(&files);
        let err = import_tar_bytes(&duplicate, &dir, false, &mut stub_embed).unwrap_err();
        assert!(err.contains("too many crops"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
        let draft = inspect_replace(&duplicate, &dir).unwrap();
        assert_eq!(draft.prompts.len(), 1);
        assert_eq!(draft.prompts[0].count, 25);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_still_rejects_corrupt_data_before_a_choice() {
        let dir = temp_dir("replace-bad");
        let jpeg = tiny_jpeg();
        let first = pack(&[
            (
                "catalogue.json",
                br#"{"parts":[{"ic":"KEEP","crops":["crops/keep.jpg"]}]}"#,
            ),
            ("crops/keep.jpg", jpeg.as_slice()),
        ]);
        import_tar_bytes(&first, &dir, true, &mut stub_embed).unwrap();
        let before = stored_catalogue(&dir);
        let bad = pack(&[("catalogue.json", b"{"), ("crops/a.jpg", jpeg.as_slice())]);
        let err = inspect_replace(&bad, &dir).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        let mut names = Vec::new();
        let mut files = vec![("catalogue.json".to_string(), Vec::new(), 1_u64)];
        for index in 0..(MAX_CROPS + 2) {
            let name = format!("crops/ACH01/c{index:02}.jpg");
            names.push(name.clone());
            files.push((name, jpeg.clone(), (index as u64) + 1));
        }
        let catalogue = serde_json::json!({
            "parts": [
                { "ic": "ACH01", "crops": names },
                { "ic": "BAD", "crops": ["../secret.jpg"] }
            ]
        });
        files[0].1 = serde_json::to_vec(&catalogue).unwrap();
        let outside = pack_owned(&files);
        let err = inspect_replace(&outside, &dir).unwrap_err();
        assert!(err.contains("outside crops"), "{err}");
        assert_eq!(stored_catalogue(&dir), before);
        assert!(!dir.join(CROP_LIMITS_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    fn color_jpeg(pixels: &[u8]) -> Vec<u8> {
        let image = turbojpeg::Image {
            pixels,
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

    #[test]
    fn references_can_drop_move_and_reorder_without_changing_the_schema() {
        let jpeg = tiny_jpeg();
        let other = color_jpeg(&[0, 0, 255, 0, 0, 200, 255, 255, 255, 10, 20, 30]);
        let catalogue = br#"{
            "parts": [{
                "ic": "2BBMNM08",
                "description": "M8 nut",
                "type": "fastener",
                "dimensions": { "acrossFlats": 13, "h": 6.5 },
                "crops": ["crops/a.jpg", "crops/b.jpg", "crops/a-copy.jpg"]
            }]
        }"#;
        let bytes = pack(&[
            ("catalogue.json", catalogue),
            ("crops/a.jpg", jpeg.as_slice()),
            ("crops/b.jpg", other.as_slice()),
            ("crops/a-copy.jpg", jpeg.as_slice()),
        ]);
        let dir = temp_dir("refs");
        import_tar_bytes(&bytes, &dir, true, &mut stub_embed).unwrap();
        let listed = catalogue_list_from(&dir).unwrap();
        assert_eq!(listed.parts.len(), 1);
        assert_eq!(listed.parts[0].crops.len(), 3);
        assert_eq!(listed.parts[0].thumbs.len(), 3);
        assert_eq!(listed.parts[0].crops[0].index, 0);
        assert_eq!(listed.parts[0].crops[0].hash.len(), 64);
        assert_eq!(listed.parts[0].crops[0].hash, listed.parts[0].crops[2].hash);
        assert_ne!(listed.parts[0].crops[0].hash, listed.parts[0].crops[1].hash);
        assert!(listed.parts[0].added_unix.is_some());
        let keys: Vec<&str> = listed.parts[0]
            .sizes
            .iter()
            .map(|row| row.key.as_str())
            .collect();
        assert!(keys.contains(&"afmm"), "{keys:?}");
        assert!(keys.contains(&"heightmm"), "{keys:?}");
        assert!(
            listed.parts[0]
                .sizes
                .iter()
                .all(|row| row.tolerance_mm.is_none())
        );

        let before = stored_catalogue(&dir);
        set_tolerance(&dir, "2BBMNM08", "heightmm", Some(0.25)).unwrap();
        assert_eq!(stored_catalogue(&dir), before);
        assert!(!String::from_utf8_lossy(&before).contains("tolerance"));
        let saved = catalogue_list_from(&dir).unwrap();
        let height = saved.parts[0]
            .sizes
            .iter()
            .find(|row| row.key == "heightmm")
            .unwrap();
        assert_eq!(height.tolerance_mm, Some(0.25));
        assert!(set_tolerance(&dir, "2BBMNM08", "nope", Some(1.0)).is_err());
        assert!(set_tolerance(&dir, "2BBMNM08", "h", Some(1001.0)).is_err());
        assert_eq!(stored_catalogue(&dir), before);

        let book = load_book(&dir).unwrap().unwrap();
        let unique = listed.parts[0]
            .crops
            .iter()
            .position(|crop| {
                listed.parts[0]
                    .crops
                    .iter()
                    .filter(|other| other.hash == crop.hash)
                    .count()
                    == 1
            })
            .unwrap();
        let gone = book.parts[0].crops[unique].clone();
        delete_crops(&dir, "2BBMNM08", &[unique]).unwrap();
        assert!(stored_jpeg(&dir, &gone).is_none());
        let left = load_book(&dir).unwrap().unwrap();
        assert_eq!(left.parts[0].crops.len(), 2);
        assert_eq!(left.parts[0].embeddings.len(), 2);
        assert_eq!(left.parts[0].description, "M8 nut");
        let listed = catalogue_list_from(&dir).unwrap();
        assert!(
            listed.parts[0]
                .crops
                .iter()
                .all(|crop| crop.hash == listed.parts[0].crops[0].hash)
        );

        let book = load_book(&dir).unwrap().unwrap();
        let first = book.parts[0].crops[0].clone();
        let second = book.parts[0].crops[1].clone();
        let first_embed = book.parts[0].embeddings[0].clone();
        reorder_crops(&dir, "2BBMNM08", &[1, 0]).unwrap();
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts[0].crops[0], second);
        assert_eq!(book.parts[0].crops[1], first);
        assert_eq!(book.parts[0].embeddings[1], first_embed);
        assert!(reorder_crops(&dir, "2BBMNM08", &[0]).is_err());

        reassign_crops(&dir, "2BBMNM08", &[0], "NEW-1").unwrap();
        let book = load_book(&dir).unwrap().unwrap();
        assert_eq!(book.parts[0].crops.len(), 1);
        assert_eq!(book.parts[1].ic, "NEW-1");
        assert_eq!(book.parts[1].embeddings.len(), 1);
        let moved = book.parts[1].crops[0].clone();
        assert!(stored_jpeg(&dir, &moved).is_some());
        let listed = catalogue_list_from(&dir).unwrap();
        let moved_part = listed.parts.iter().find(|part| part.ic == "NEW-1").unwrap();
        assert!(moved_part.thumbs[0].starts_with("data:image/jpeg;base64,"));
        assert!(!dir.join("catalogue-crops").exists());
        assert!(reassign_crops(&dir, "NEW-1", &[0], "NEW-1").is_err());
        assert!(delete_crops(&dir, "NEW-1", &[]).is_err());

        remove_part(&dir, "2BBMNM08").unwrap();
        assert!(read_tolerances(&dir).get("2BBMNM08").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
