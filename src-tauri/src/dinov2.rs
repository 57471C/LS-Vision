//! Optional DINOv2 ONNX session, stored beside the YOLO session.
//!
//! Startup opens `models/dinov2_vits14.onnx` next to the exe when the installer
//! shipped it, and otherwise `src-tauri/models/dinov2_vits14.onnx`.
//! A missing or unloadable file is a warning. Detection keeps using the YOLO session.
//! After YOLO keeps a box, this module can crop it, letterbox it to 224, and log
//! the embedding. A test-mode capture stores that vector under the box's IC.
//! The camera preview can hold the same vectors and store only the boxes an
//! admin selects, appending them or replacing that IC. That same preview detect
//! keeps a copy of the RGB frame. An admin marquee crops that copy and embeds
//! it with DINOv2 only, then stores the one vector under an IC they choose.
//! The map is kept in memory and written to `vision/dinov2-references.json`.
//! A later detection logs the best cosine score against those references.
//! A box counts toward its IC only when that score is above that IC's threshold.
//! An IC with no saved threshold uses the Training match score.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::detect;
use crate::inference_ep::{self, Provider};

const FILE_NAME: &str = "dinov2_vits14.onnx";
const REFERENCES_FILE: &str = "dinov2-references.json";
const REFERENCES_VERSION: u32 = 1;
const MIN_REFERENCE_THRESHOLD: f32 = 0.50;
const MAX_REFERENCE_THRESHOLD: f32 = 0.95;

/// Vectors for each IC, plus an optional threshold that overrides the Training score.
struct ReferenceBook {
    vectors: HashMap<String, Vec<Vec<f32>>>,
    thresholds: HashMap<String, f32>,
}

impl ReferenceBook {
    fn empty() -> Self {
        Self {
            vectors: HashMap::new(),
            thresholds: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dinov2Port {
    pub name: String,
    pub shape: Vec<i64>,
    pub dtype: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dinov2Status {
    pub loaded: bool,
    pub path: String,
    pub inputs: Vec<Dinov2Port>,
    pub outputs: Vec<Dinov2Port>,
    pub message: String,
}

struct State {
    session: Option<ort::session::Session>,
    status: Dinov2Status,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static REFERENCES: LazyLock<Mutex<ReferenceBook>> =
    LazyLock::new(|| Mutex::new(ReferenceBook::empty()));
static HELD: LazyLock<Mutex<Vec<Option<HeldEmbed>>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static FRAME: LazyLock<Mutex<Option<HeldFrame>>> = LazyLock::new(|| Mutex::new(None));
static MARQUEE: LazyLock<Mutex<Option<Vec<f32>>>> = LazyLock::new(|| Mutex::new(None));
static MARQUEE_TOKEN: AtomicU64 = AtomicU64::new(0);
static FRAME_GEN: AtomicU64 = AtomicU64::new(0);

/// RGB copy of the preview detect that asked to hold references.
/// A marquee crops this still, so a later camera frame cannot change it.
#[derive(Clone)]
struct HeldFrame {
    rgb: Vec<u8>,
    width: u32,
    height: u32,
}

/// One crop embedding kept from the latest preview detect. `None` means that
/// box had no vector, so it cannot be saved.
#[derive(Clone)]
pub(crate) struct HeldEmbed {
    pub ic: String,
    pub values: Vec<f32>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// On Windows the resource directory is the exe directory, so the installer
/// copy is `models/dinov2_vits14.onnx` beside `ls-vision.exe`.
/// Dev and tests use `src-tauri/models/dinov2_vits14.onnx` when that bundled file is absent.
pub(crate) fn model_path() -> PathBuf {
    resolve_model_path(
        std::env::current_exe().ok().as_deref(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
}

fn resolve_model_path(exe: Option<&Path>, manifest_dir: &Path) -> PathBuf {
    if let Some(dir) = exe.and_then(|path| path.parent()) {
        let bundled = dir.join("models").join(FILE_NAME);
        if bundled.is_file() {
            return bundled;
        }
    }
    manifest_dir.join("models").join(FILE_NAME)
}

/// Open the DINOv2 session once. A missing file does not touch the YOLO cache.
/// Saved references load even when the model file is missing.
/// Use OpenVINO, when saved, is decided before this session opens. Off leaves
/// the ONNX Runtime session path unchanged.
pub(crate) fn load_on_startup() {
    if inference_ep::read_use_openvino() {
        inference_ep::prepare_openvino();
        let _ = detect::reload_session();
    }
    load_at(&model_path());
    load_saved_references();
}

/// Reopen the DINOv2 file with the current backend. References stay in memory.
pub(crate) fn reload_session() {
    load_at(&model_path());
}

pub(crate) fn load_at(path: &Path) -> Dinov2Status {
    let status = match open_held(path) {
        Ok((session, status)) => {
            *lock(&STATE) = Some(State {
                session: Some(session),
                status: status.clone(),
            });
            status
        }
        Err(message) => {
            let status = Dinov2Status {
                loaded: false,
                path: path.display().to_string(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                message,
            };
            *lock(&STATE) = Some(State {
                session: None,
                status: status.clone(),
            });
            status
        }
    };
    log_status(&status);
    status
}

/// Whether the DINOv2 session loaded, plus its input and output names and shapes.
#[tauri::command]
pub fn dinov2_status() -> Dinov2Status {
    let guard = lock(&STATE);
    let Some(state) = guard.as_ref() else {
        return idle_status();
    };
    let Some(session) = state.session.as_ref() else {
        return state.status.clone();
    };
    let mut status = state.status.clone();
    status.inputs = ports(session.inputs());
    status.outputs = ports(session.outputs());
    status
}

fn idle_status() -> Dinov2Status {
    Dinov2Status {
        loaded: false,
        path: model_path().display().to_string(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        message: "DINOv2 session has not been loaded.".to_string(),
    }
}

fn open_held(path: &Path) -> Result<(ort::session::Session, Dinov2Status), String> {
    if !path.is_file() {
        return Err(format!(
            "DINOv2 model is missing at {}. YOLO detection is unchanged.",
            path.display()
        ));
    }
    let (session, fallback) = open_dinov2(path)?;
    let mut message = format!("DINOv2 session is ready. Loaded from {}.", path.display());
    if let Some(note) = fallback {
        message.push(' ');
        message.push_str(&note);
    }
    let status = Dinov2Status {
        loaded: true,
        path: path.display().to_string(),
        inputs: ports(session.inputs()),
        outputs: ports(session.outputs()),
        message,
    };
    Ok((session, status))
}

/// Same execution provider as YOLO, with a CPU fallback that does not change the saved provider.
fn open_dinov2(path: &Path) -> Result<(ort::session::Session, Option<String>), String> {
    let env = detect::runtime().map_err(|err| {
        format!("DINOv2 model failed to load ({err}). YOLO detection is unchanged.")
    })?;
    let requested = inference_ep::selected_provider();
    if inference_ep::wants_openvino() {
        match detect::open_openvino_session(&env, path) {
            Ok(session) => {
                inference_ep::mark_openvino_started();
                return Ok((session, None));
            }
            Err(err) => {
                let was_active = inference_ep::openvino_is_active();
                inference_ep::mark_openvino_failed(&err);
                if was_active {
                    detect::forget_session();
                }
            }
        }
    }
    match detect::open_session(&env, path, requested) {
        Ok(session) => Ok((session, None)),
        Err(err) if requested != Provider::Cpu => {
            match detect::open_session(&env, path, Provider::Cpu) {
                Ok(session) => Ok((
                    session,
                    Some(format!(
                        "{} did not start for DINOv2. Using CPU.",
                        requested.label()
                    )),
                )),
                Err(cpu_err) => Err(format!(
                    "DINOv2 model failed to load ({err} {cpu_err}). YOLO detection is unchanged."
                )),
            }
        }
        Err(err) => Err(format!(
            "DINOv2 model failed to load ({err}). YOLO detection is unchanged."
        )),
    }
}

fn ports(outlets: &[ort::value::Outlet]) -> Vec<Dinov2Port> {
    outlets
        .iter()
        .map(|outlet| Dinov2Port {
            name: outlet.name().to_string(),
            shape: outlet
                .dtype()
                .tensor_shape()
                .map(|shape| shape.to_vec())
                .unwrap_or_default(),
            dtype: outlet.dtype().to_string(),
        })
        .collect()
}

const EMBED_SIZE: usize = 224;
const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// One YOLO box that already survived the confidence threshold and NMS.
pub(crate) struct DebugCrop<'a> {
    pub class_name: &'a str,
    pub confidence: f64,
    pub bbox: [i32; 4],
}

pub(crate) fn is_loaded() -> bool {
    lock(&STATE)
        .as_ref()
        .is_some_and(|state| state.session.is_some())
}

pub(crate) struct EmbedDebug {
    pub lines: Vec<String>,
    /// One entry per crop. `None` means this box has no reference to compare.
    /// `Some` is whether the best cosine is above the threshold.
    pub counted: Vec<Option<bool>>,
    /// The vector for each crop. `None` when that crop did not embed.
    pub vectors: Vec<Option<Vec<f32>>>,
}

/// Crop, letterbox, and embed each box. A missing session returns no lines and
/// prints nothing. Each returned line is also written to stderr.
/// `store` keeps the vector under the box IC for a later cosine match.
pub(crate) fn debug_embed(
    rgb: &[u8],
    width: u32,
    height: u32,
    crops: &[DebugCrop<'_>],
    store: bool,
    threshold: f32,
) -> EmbedDebug {
    let mut guard = lock(&STATE);
    let Some(session) = guard.as_mut().and_then(|state| state.session.as_mut()) else {
        return EmbedDebug {
            lines: Vec::new(),
            counted: Vec::new(),
            vectors: Vec::new(),
        };
    };
    embed_crops(session, rgb, width, height, crops, store, threshold)
}

/// One crop through the loaded session. Used to compare a known box with a reference.
#[cfg(test)]
pub(crate) fn embed_debug_crop(
    rgb: &[u8],
    width: u32,
    height: u32,
    crop: &DebugCrop<'_>,
) -> Result<Vec<f32>, String> {
    let mut guard = lock(&STATE);
    let Some(session) = guard.as_mut().and_then(|state| state.session.as_mut()) else {
        return Err("DINOv2 session is not loaded".to_string());
    };
    embed_one(session, rgb, width, height, crop)
}

/// Run one NCHW buffer through the session opened at startup.
/// `data` is the flat `[1, 3, 224, 224]` tensor. The returned vector is not normalised.
pub(crate) fn forward_nchw(data: Vec<f32>) -> Result<Vec<f32>, String> {
    const PLANE: usize = 224 * 224;
    if data.len() != 3 * PLANE {
        return Err("DINOv2 input was the wrong size.".to_string());
    }
    let mut guard = lock(&STATE);
    let Some(session) = guard.as_mut().and_then(|state| state.session.as_mut()) else {
        return Err("DINOv2 model is not loaded.".to_string());
    };
    let tensor = ort::value::Tensor::from_array(([1usize, 3, 224, 224], data))
        .map_err(|err| format!("embed failed: {}", brief(&err.to_string())))?;
    let outputs = session
        .run(ort::inputs![tensor])
        .map_err(|err| format!("embed failed: {}", brief(&err.to_string())))?;
    let (_shape, values) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("embed failed: {}", brief(&err.to_string())))?;
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return Err("DINOv2 returned an empty embedding.".to_string());
    }
    Ok(values.to_vec())
}

#[cfg(test)]
pub(crate) fn clear_references() {
    *lock(&REFERENCES) = ReferenceBook::empty();
}

/// Replace the vectors held for a later admin save. One entry per detection.
pub(crate) fn hold_embeddings(items: Vec<Option<HeldEmbed>>) {
    *lock(&HELD) = items;
}

/// Keep the RGB frame from a preview detect. Clears any marquee vector, because
/// that vector belonged to the previous still.
pub(crate) fn hold_frame(rgb: &[u8], width: u32, height: u32) {
    invalidate_marquee();
    FRAME_GEN.fetch_add(1, Ordering::SeqCst);
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3);
    if width == 0 || height == 0 || expected == 0 || rgb.len() < expected {
        *lock(&FRAME) = None;
        return;
    }
    *lock(&FRAME) = Some(HeldFrame {
        rgb: rgb[..expected].to_vec(),
        width,
        height,
    });
}

/// Drop the held still and any marquee vector. Box embeddings are left as they are.
pub(crate) fn clear_held_frame() {
    invalidate_marquee();
    FRAME_GEN.fetch_add(1, Ordering::SeqCst);
    *lock(&FRAME) = None;
}

fn invalidate_marquee() {
    MARQUEE_TOKEN.fetch_add(1, Ordering::SeqCst);
    *lock(&MARQUEE) = None;
}

/// The latest marquee claim. An older embed sees a different token and does not store.
fn claim_marquee() -> u64 {
    let token = MARQUEE_TOKEN.fetch_add(1, Ordering::SeqCst) + 1;
    *lock(&MARQUEE) = None;
    token
}

#[cfg(test)]
pub(crate) fn clear_held() {
    lock(&HELD).clear();
}

#[cfg(test)]
pub(crate) fn hold_marquee(values: Vec<f32>) {
    *lock(&MARQUEE) = Some(values);
}

/// Append one reference. Returns how many vectors are now stored for this IC.
pub(crate) fn remember(class_name: &str, values: &[f32]) -> usize {
    let mut guard = lock(&REFERENCES);
    let list = guard.vectors.entry(class_name.to_string()).or_default();
    list.push(values.to_vec());
    list.len()
}

/// `Some(true)` when the score is above the threshold. `None` when there is no score.
pub(crate) fn reference_counts(score: Option<f32>, threshold: f32) -> Option<bool> {
    let score = score?;
    if !score.is_finite() || !threshold.is_finite() {
        return None;
    }
    Some(score > threshold)
}

/// The threshold the next detection uses for this IC.
/// A saved value overrides the Training match score passed as `fallback`.
pub(crate) fn threshold_for(class_name: &str, fallback: f32) -> f32 {
    lock(&REFERENCES)
        .thresholds
        .get(class_name)
        .copied()
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

/// Best cosine similarity against the references stored for this IC.
pub(crate) fn best_similarity(class_name: &str, values: &[f32]) -> Option<f32> {
    let guard = lock(&REFERENCES);
    let list = guard.vectors.get(class_name)?;
    let mut best: Option<f32> = None;
    for reference in list {
        let Some(score) = cosine(values, reference) else {
            continue;
        };
        best = Some(best.map(|current| current.max(score)).unwrap_or(score));
    }
    best
}

#[derive(Serialize, Deserialize)]
struct ReferenceFile {
    version: u32,
    references: HashMap<String, Vec<Vec<f32>>>,
    /// Per-IC overrides. Missing means the Training match score.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    thresholds: HashMap<String, f32>,
}

/// Keep a finite score inside 0.50 to 0.95, rounded to two decimal places.
fn stored_threshold(value: f32) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    let clamped = value.clamp(MIN_REFERENCE_THRESHOLD, MAX_REFERENCE_THRESHOLD);
    let rounded = (clamped * 100.0).round() / 100.0;
    if rounded.is_finite() {
        Some(rounded)
    } else {
        None
    }
}

fn references_path() -> Result<PathBuf, String> {
    let root = crate::local_config::project_root()?;
    Ok(root.join("vision").join(REFERENCES_FILE))
}

fn log_references_path(path: &Path) {
    eprintln!("DINOv2 references path: {}", path.display());
}

/// Load the file next to config.local.json. A missing file leaves memory as it is.
/// A bad file is a warning and also leaves memory as it is.
pub(crate) fn load_saved_references() {
    let path = match references_path() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("warning: DINOv2 references were not loaded: {err}");
            return;
        }
    };
    match load_references_from(&path) {
        Ok(true) => {
            let classes = lock(&REFERENCES).vectors.len();
            eprintln!(
                "DINOv2 references loaded: {classes} classes from {}",
                path.display()
            );
        }
        Ok(false) => {}
        Err(err) => eprintln!("warning: {err} In-memory references are unchanged."),
    }
}

fn parse_reference_map(text: &str) -> Result<ReferenceBook, &'static str> {
    let file: ReferenceFile = serde_json::from_str(text).map_err(|_| "json")?;
    if file.version != REFERENCES_VERSION {
        return Err("version");
    }
    let mut vectors = HashMap::new();
    for (name, list) in file.references {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let kept: Vec<Vec<f32>> = list
            .into_iter()
            .filter(|values| !values.is_empty() && values.iter().all(|value| value.is_finite()))
            .collect();
        if kept.is_empty() {
            continue;
        }
        vectors.insert(name.to_string(), kept);
    }
    let mut thresholds = HashMap::new();
    for (name, value) in file.thresholds {
        let name = name.trim();
        if name.is_empty() || !vectors.contains_key(name) {
            continue;
        }
        let Some(stored) = stored_threshold(value) else {
            continue;
        };
        thresholds.insert(name.to_string(), stored);
    }
    Ok(ReferenceBook {
        vectors,
        thresholds,
    })
}

/// `Ok(true)` when a file was read. `Ok(false)` when the file is absent.
pub(crate) fn load_references_from(path: &Path) -> Result<bool, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(format!("Could not read DINOv2 references: {err}")),
    };
    let clean = match parse_reference_map(&text) {
        Ok(clean) => clean,
        Err("version") => {
            return Err(format!(
                "DINOv2 references at {} use an unknown version.",
                path.display()
            ));
        }
        Err(_) => {
            return Err(format!(
                "DINOv2 references at {} are not valid JSON.",
                path.display()
            ));
        }
    };
    *lock(&REFERENCES) = clean;
    Ok(true)
}

fn replace_reference_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Could not write DINOv2 references.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("Could not write DINOv2 references: {err}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes).map_err(|err| format!("Could not write DINOv2 references: {err}"))?;
    if fs::rename(&tmp, path).is_err() {
        if path.exists() {
            fs::remove_file(path).map_err(|err| {
                let _ = fs::remove_file(&tmp);
                format!("Could not replace DINOv2 references: {err}")
            })?;
        }
        fs::rename(&tmp, path).map_err(|err| {
            let _ = fs::remove_file(&tmp);
            format!("Could not replace DINOv2 references: {err}")
        })?;
    }
    Ok(())
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Copy a valid reference file over `dest` and replace the in-memory map.
/// A file that does not parse leaves both the destination and memory unchanged.
/// The returned count is the number of ICs now stored.
pub(crate) fn install_reference_file(source: &Path, dest: &Path) -> Result<usize, String> {
    let bytes =
        fs::read(source).map_err(|err| format!("Could not read DINOv2 references: {err}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| "Selected file is not a valid DINOv2 reference file.".to_string())?;
    let clean = parse_reference_map(&text)
        .map_err(|_| "Selected file is not a valid DINOv2 reference file.".to_string())?;
    if !same_file(source, dest) {
        replace_reference_bytes(dest, text.as_bytes())?;
    }
    let count = clean.vectors.len();
    *lock(&REFERENCES) = clean;
    Ok(count)
}

pub(crate) fn references_loaded_message(classes: usize) -> String {
    format!("References loaded {classes} ICs.")
}

#[derive(Clone, Serialize)]
pub struct ReferenceUpdate {
    pub classes: usize,
    pub message: String,
    pub path: String,
}

fn reference_update(path: &Path, classes: usize, message: String) -> ReferenceUpdate {
    ReferenceUpdate {
        classes,
        message,
        path: path.display().to_string(),
    }
}

/// Native JSON picker, then replace the saved file and the in-memory book.
/// The dialog runs off the main thread. A panic becomes a status string.
fn install_picked_references(app: AppHandle, dest: PathBuf) -> Result<ReferenceUpdate, String> {
    let mut picker = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_title("Update references");
    if let Some(window) = app.get_webview_window("main") {
        picker = picker.set_parent(&window);
    }
    if let Some(dir) = dest.parent() {
        if dir.is_dir() {
            picker = picker.set_directory(dir);
        }
    }
    let picked = picker.blocking_pick_file();
    let Some(picked) = picked else {
        return Ok(reference_update(
            &dest,
            lock(&REFERENCES).vectors.len(),
            "No file selected.".to_string(),
        ));
    };
    let source = match picked.into_path() {
        Ok(path) => path,
        Err(_) => return Err("Could not read the selected file.".to_string()),
    };
    if !source
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        return Err("Selected file is not a valid DINOv2 reference file.".to_string());
    }
    let classes = install_reference_file(&source, &dest)?;
    eprintln!(
        "DINOv2 references loaded: {classes} classes from {}",
        dest.display()
    );
    Ok(reference_update(
        &dest,
        classes,
        references_loaded_message(classes),
    ))
}

/// Replace `vision/dinov2-references.json` with the chosen file and load it.
/// Cancel leaves the saved file and the in-memory map as they are.
/// The next detection uses the loaded embeddings. The app does not need a restart.
///
/// The picker is the native file dialog and accepts `.json`. It does not run on
/// the main thread: a sync command blocks that thread, and the dialog waits for it.
#[tauri::command]
pub async fn update_references(app: AppHandle) -> Result<ReferenceUpdate, String> {
    let dest = references_path()?;
    log_references_path(&dest);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            install_picked_references(app, dest)
        }))
    })
    .await;
    match joined {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => Err("Could not update the references".to_string()),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ReferenceRow {
    pub ic: String,
    /// Saved override. Absent when this IC uses the Training match score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
    pub count: usize,
}

/// One row per saved IC, sorted by IC.
pub(crate) fn reference_rows() -> Vec<ReferenceRow> {
    let guard = lock(&REFERENCES);
    let mut rows: Vec<ReferenceRow> = guard
        .vectors
        .iter()
        .map(|(ic, vectors)| ReferenceRow {
            ic: ic.clone(),
            threshold: guard.thresholds.get(ic).copied(),
            count: vectors.len(),
        })
        .collect();
    rows.sort_by(|left, right| left.ic.cmp(&right.ic));
    rows
}

/// Store one IC's threshold and write the reference file.
/// The next detection reads this value from memory.
pub(crate) fn apply_reference_threshold(ic: &str, threshold: f32) -> Result<ReferenceRow, String> {
    let ic = ic.trim();
    if ic.is_empty() {
        return Err("That IC is not saved.".to_string());
    }
    let Some(stored) = stored_threshold(threshold) else {
        return Err("Enter a threshold from 0.50 to 0.95.".to_string());
    };
    let row = {
        let mut guard = lock(&REFERENCES);
        if !guard.vectors.contains_key(ic) {
            return Err("That IC is not saved.".to_string());
        }
        guard.thresholds.insert(ic.to_string(), stored);
        ReferenceRow {
            ic: ic.to_string(),
            threshold: Some(stored),
            count: guard.vectors.get(ic).map(Vec::len).unwrap_or(0),
        }
    };
    persist_references();
    Ok(row)
}

/// Remove one IC from memory and from the reference file.
pub(crate) fn remove_reference(ic: &str) -> Result<(), String> {
    let ic = ic.trim();
    {
        let mut guard = lock(&REFERENCES);
        if guard.vectors.remove(ic).is_none() {
            return Err("That IC is not saved.".to_string());
        }
        guard.thresholds.remove(ic);
    }
    persist_references();
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct ReferenceList {
    pub path: String,
    pub rows: Vec<ReferenceRow>,
}

/// Read one reference file into memory and return its rows.
/// A missing file is an empty list and does not clear memory.
/// Invalid JSON and permission errors are returned and leave memory unchanged.
pub(crate) fn reference_list_from(path: &Path) -> Result<ReferenceList, String> {
    let rows = match load_references_from(path)? {
        true => reference_rows(),
        false => Vec::new(),
    };
    Ok(ReferenceList {
        path: path.display().to_string(),
        rows,
    })
}

#[tauri::command]
pub async fn list_references() -> Result<ReferenceList, String> {
    let path = references_path()?;
    log_references_path(&path);
    let joined = tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reference_list_from(&path)))
    })
    .await;
    match joined {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => Err("Could not load the references".to_string()),
    }
}

#[tauri::command]
pub fn set_reference_threshold(ic: String, threshold: f64) -> Result<ReferenceRow, String> {
    apply_reference_threshold(&ic, threshold as f32)
}

#[tauri::command]
pub fn delete_reference(ic: String) -> Result<(), String> {
    remove_reference(&ic)
}

pub(crate) fn save_references_to(path: &Path) -> Result<(), String> {
    let guard = lock(&REFERENCES);
    let file = ReferenceFile {
        version: REFERENCES_VERSION,
        references: guard.vectors.clone(),
        thresholds: guard.thresholds.clone(),
    };
    drop(guard);
    let mut text = serde_json::to_string(&file)
        .map_err(|_| "Could not write DINOv2 references.".to_string())?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    replace_reference_bytes(path, text.as_bytes())
}

fn persist_references() {
    // Unit tests store vectors in memory and write only to a temp path.
    if cfg!(test) {
        return;
    }
    let path = match references_path() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("warning: DINOv2 references stayed in memory: {err}");
            return;
        }
    };
    if let Err(err) = save_references_to(&path) {
        eprintln!("warning: {err} In-memory references are unchanged.");
    }
}

fn cosine(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (a, b) in left.iter().zip(right) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    let scale = left_norm.sqrt() * right_norm.sqrt();
    if !scale.is_finite() || scale == 0.0 {
        return None;
    }
    let score = dot / scale;
    if score.is_finite() { Some(score) } else { None }
}

fn embed_crops(
    session: &mut ort::session::Session,
    rgb: &[u8],
    width: u32,
    height: u32,
    crops: &[DebugCrop<'_>],
    store: bool,
    threshold: f32,
) -> EmbedDebug {
    let mut lines = Vec::with_capacity(crops.len());
    let mut counted = Vec::with_capacity(crops.len());
    let mut vectors = Vec::with_capacity(crops.len());
    let mut stored: HashMap<String, usize> = HashMap::new();
    for crop in crops {
        let (line, counts) = match embed_one(session, rgb, width, height, crop) {
            Ok(values) => {
                // Score against the references already stored, then append.
                let matched = best_similarity(crop.class_name, &values);
                let cutoff = threshold_for(crop.class_name, threshold);
                let counts = reference_counts(matched, cutoff);
                if store {
                    let count = remember(crop.class_name, &values);
                    stored.insert(crop.class_name.to_string(), count);
                }
                let line = embed_line(crop.class_name, crop.confidence, &values, matched);
                vectors.push(Some(values));
                (line, counts)
            }
            Err(err) => {
                vectors.push(None);
                (
                    format!(
                        "DINOv2 {} conf {:.4} embed failed: {err}",
                        crop.class_name, crop.confidence
                    ),
                    None,
                )
            }
        };
        eprintln!("{line}");
        lines.push(line);
        counted.push(counts);
    }
    if store {
        let mut names: Vec<String> = stored.keys().cloned().collect();
        names.sort();
        for name in names {
            let count = stored[&name];
            let line = format!("DINOv2 reference {name} stored {count}");
            eprintln!("{line}");
            lines.push(line);
        }
        persist_references();
    }
    EmbedDebug {
        lines,
        counted,
        vectors,
    }
}

/// Store the selected held crops under their class names.
/// Append adds the vectors. Replace keeps only the selected vectors for each
/// selected class and leaves every other class as it was.
/// The returned pairs are the class and how many vectors it now has, sorted by class.
pub(crate) fn apply_selected_references(
    indices: &[usize],
    replace: bool,
) -> Result<Vec<(String, usize)>, String> {
    if indices.is_empty() {
        return Err("Select a box.".to_string());
    }
    let held = lock(&HELD).clone();
    let mut picked: Vec<(String, Vec<f32>)> = Vec::new();
    let mut seen = HashSet::new();
    for index in indices {
        if !seen.insert(*index) {
            continue;
        }
        let Some(slot) = held.get(*index) else {
            return Err("Detect again before saving a reference.".to_string());
        };
        let Some(item) = slot else {
            return Err("That box has no DINOv2 embedding.".to_string());
        };
        let ic = item.ic.trim();
        if ic.is_empty() {
            return Err("That box has no class.".to_string());
        }
        if item.values.is_empty() || item.values.iter().any(|value| !value.is_finite()) {
            return Err("That box has no DINOv2 embedding.".to_string());
        }
        picked.push((ic.to_string(), item.values.clone()));
    }
    if picked.is_empty() {
        return Err("Select a box.".to_string());
    }
    let mut grouped: HashMap<String, Vec<Vec<f32>>> = HashMap::new();
    let mut order = Vec::new();
    for (ic, values) in picked {
        if !grouped.contains_key(&ic) {
            order.push(ic.clone());
        }
        grouped.entry(ic).or_default().push(values);
    }
    Ok(store_grouped(grouped, order, replace))
}

/// Append or replace the vectors for each listed IC. Other ICs stay.
fn store_grouped(
    mut grouped: HashMap<String, Vec<Vec<f32>>>,
    order: Vec<String>,
    replace: bool,
) -> Vec<(String, usize)> {
    let mut guard = lock(&REFERENCES);
    for ic in &order {
        let Some(vectors) = grouped.remove(ic) else {
            continue;
        };
        if replace {
            guard.vectors.insert(ic.clone(), vectors);
        } else {
            guard.vectors.entry(ic.clone()).or_default().extend(vectors);
        }
    }
    let mut rows: Vec<(String, usize)> = order
        .into_iter()
        .map(|ic| {
            let count = guard.vectors.get(&ic).map(Vec::len).unwrap_or(0);
            (ic, count)
        })
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

fn saved_message(rows: &[(String, usize)], replace: bool) -> String {
    let verb = if replace { "Replaced" } else { "Saved" };
    let body = rows
        .iter()
        .map(|(ic, count)| format!("{ic} ({count})"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{verb} {body}.")
}

/// Write the in-memory map. Unit tests keep the file write on an explicit path.
pub(crate) fn write_references() -> Result<(), String> {
    if cfg!(test) {
        return Ok(());
    }
    let path = references_path()?;
    save_references_to(&path)
}

/// Save the selected boxes from the latest preview detect that asked to hold them.
/// `replace` swaps the stored vectors for each selected class. Otherwise the
/// vectors are appended. Other classes are left as they are.
#[tauri::command]
pub fn save_detection_references(
    indices: Vec<usize>,
    replace: Option<bool>,
) -> Result<String, String> {
    let replace = replace.unwrap_or(false);
    let rows = apply_selected_references(&indices, replace)?;
    write_references()?;
    for (name, count) in &rows {
        eprintln!("DINOv2 reference {name} stored {count}");
    }
    Ok(saved_message(&rows, replace))
}

fn marquee_bbox(x: f64, y: f64, w: f64, h: f64) -> Result<[i32; 4], String> {
    if !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() {
        return Err("Draw a larger region.".to_string());
    }
    if w < 1.0 || h < 1.0 || x.abs() > 100_000.0 || y.abs() > 100_000.0 {
        return Err("Draw a larger region.".to_string());
    }
    let x1 = x.round() as i32;
    let y1 = y.round() as i32;
    let x2 = (x + w).round() as i32;
    let y2 = (y + h).round() as i32;
    if x2 <= x1 || y2 <= y1 {
        return Err("Draw a larger region.".to_string());
    }
    Ok([x1, y1, x2, y2])
}

/// Crop the held preview frame and run DINOv2 only. YOLO is not used.
/// The vector stays unlabelled until `save_marquee_reference` assigns an IC.
#[tauri::command]
pub fn embed_marquee(x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    let bbox = marquee_bbox(x, y, w, h)?;
    let token = claim_marquee();
    let frame_gen = FRAME_GEN.load(Ordering::SeqCst);
    let frame = lock(&FRAME).clone();
    let Some(frame) = frame else {
        return Err("Detect again before saving a reference.".to_string());
    };
    let values = {
        let mut guard = lock(&STATE);
        let Some(session) = guard.as_mut().and_then(|state| state.session.as_mut()) else {
            return Err("DINOv2 is not loaded.".to_string());
        };
        let crop = DebugCrop {
            class_name: "",
            confidence: 0.0,
            bbox,
        };
        embed_one(session, &frame.rgb, frame.width, frame.height, &crop).map_err(|err| {
            if err == "empty crop" {
                "Draw a larger region.".to_string()
            } else {
                err
            }
        })?
    };
    if MARQUEE_TOKEN.load(Ordering::SeqCst) != token
        || FRAME_GEN.load(Ordering::SeqCst) != frame_gen
    {
        return Err("Detect again before saving a reference.".to_string());
    }
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return Err("That region has no DINOv2 embedding.".to_string());
    }
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    eprintln!("DINOv2 marquee norm {norm:.4}");
    *lock(&MARQUEE) = Some(values);
    Ok(())
}

/// Store the pending marquee vector under `ic`.
/// Append adds it. Replace swaps that IC only. Other ICs stay.
pub(crate) fn apply_marquee_reference(
    ic: &str,
    replace: bool,
) -> Result<Vec<(String, usize)>, String> {
    let ic = ic.trim();
    if ic.is_empty() {
        return Err("Enter an IC.".to_string());
    }
    let values = lock(&MARQUEE)
        .clone()
        .ok_or_else(|| "Draw a region before saving a reference.".to_string())?;
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return Err("That region has no DINOv2 embedding.".to_string());
    }
    let rows = store_grouped(
        HashMap::from([(ic.to_string(), vec![values.clone()])]),
        vec![ic.to_string()],
        replace,
    );
    let mut guard = lock(&MARQUEE);
    if guard.as_ref() == Some(&values) {
        *guard = None;
    }
    Ok(rows)
}

/// Save the marquee crop under the IC the admin typed or picked.
#[tauri::command]
pub fn save_marquee_reference(ic: String, replace: Option<bool>) -> Result<String, String> {
    let replace = replace.unwrap_or(false);
    let rows = apply_marquee_reference(&ic, replace)?;
    write_references()?;
    for (name, count) in &rows {
        eprintln!("DINOv2 reference {name} stored {count}");
    }
    Ok(saved_message(&rows, replace))
}

/// Drop a marquee vector the preview discarded. The held frame stays.
#[tauri::command]
pub fn clear_marquee_reference() {
    invalidate_marquee();
}

/// Drop the held still when the preview overlay closes.
#[tauri::command]
pub fn release_preview_frame() {
    clear_held_frame();
}

fn embed_one(
    session: &mut ort::session::Session,
    rgb: &[u8],
    width: u32,
    height: u32,
    crop: &DebugCrop<'_>,
) -> Result<Vec<f32>, String> {
    let (pixels, crop_w, crop_h) = crop_box(rgb, width, height, crop.bbox)?;
    let data = imagenet_letterbox(&pixels, crop_w, crop_h)?;
    let tensor = ort::value::Tensor::from_array(([1usize, 3, EMBED_SIZE, EMBED_SIZE], data))
        .map_err(|err| format!("embed failed: {}", brief(&err.to_string())))?;
    let outputs = session
        .run(ort::inputs![tensor])
        .map_err(|err| format!("embed failed: {}", brief(&err.to_string())))?;
    let (_shape, values) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("embed failed: {}", brief(&err.to_string())))?;
    if values.len() < 4 {
        return Err("embedding was shorter than 4 values".to_string());
    }
    Ok(values.to_vec())
}

fn crop_box(
    rgb: &[u8],
    width: u32,
    height: u32,
    bbox: [i32; 4],
) -> Result<(Vec<u8>, usize, usize), String> {
    let width = width as i32;
    let height = height as i32;
    if width < 1 || height < 1 {
        return Err("empty crop".to_string());
    }
    let x1 = bbox[0].min(bbox[2]).clamp(0, width - 1);
    let y1 = bbox[1].min(bbox[3]).clamp(0, height - 1);
    let mut x2 = bbox[0].max(bbox[2]).clamp(0, width);
    let mut y2 = bbox[1].max(bbox[3]).clamp(0, height);
    if x2 <= x1 {
        x2 = (x1 + 1).min(width);
    }
    if y2 <= y1 {
        y2 = (y1 + 1).min(height);
    }
    let crop_w = (x2 - x1) as usize;
    let crop_h = (y2 - y1) as usize;
    let frame_w = width as usize;
    let expected = frame_w.saturating_mul(height as usize).saturating_mul(3);
    if rgb.len() < expected || crop_w == 0 || crop_h == 0 {
        return Err("empty crop".to_string());
    }
    let mut pixels = Vec::with_capacity(crop_w * crop_h * 3);
    for row in 0..crop_h {
        let y = y1 as usize + row;
        let start = (y * frame_w + x1 as usize) * 3;
        let end = start + crop_w * 3;
        pixels.extend_from_slice(&rgb[start..end]);
    }
    Ok((pixels, crop_w, crop_h))
}

/// Pad the crop to a square, resize to 224, then apply ImageNet mean and std.
/// Padding is exact zero in normalised space, which is the ImageNet mean.
fn imagenet_letterbox(rgb: &[u8], width: usize, height: usize) -> Result<Vec<f32>, String> {
    let geometry = detect::letterbox_geometry(width as i32, height as i32, EMBED_SIZE as i32);
    if geometry.new_w < 1 || geometry.new_h < 1 {
        return Err("empty crop".to_string());
    }
    let new_w = geometry.new_w as usize;
    let new_h = geometry.new_h as usize;
    let resized = detect::resize_linear(rgb, width, height, new_w, new_h);
    let plane = EMBED_SIZE * EMBED_SIZE;
    let mut data = vec![0f32; 3 * plane];
    let top = geometry.top.max(0) as usize;
    let left = geometry.left.max(0) as usize;
    for y in 0..new_h {
        let dst_y = top + y;
        if dst_y >= EMBED_SIZE {
            break;
        }
        for x in 0..new_w {
            let dst_x = left + x;
            if dst_x >= EMBED_SIZE {
                break;
            }
            let src = (y * new_w + x) * 3;
            let dst = dst_y * EMBED_SIZE + dst_x;
            for channel in 0..3 {
                let value = f32::from(resized[src + channel]) / 255.0;
                data[channel * plane + dst] =
                    (value - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
            }
        }
    }
    Ok(data)
}

fn embed_line(class_name: &str, confidence: f64, values: &[f32], matched: Option<f32>) -> String {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    let at = |index| values.get(index).copied().unwrap_or(0.0);
    let score = matched
        .map(|value| format!(" match {value:.4}"))
        .unwrap_or_default();
    format!(
        "DINOv2 {class_name} conf {confidence:.4} norm {norm:.4}{score} {:+.4} {:+.4} {:+.4} {:+.4}",
        at(0),
        at(1),
        at(2),
        at(3)
    )
}

fn brief(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= 160 {
        return text;
    }
    let trimmed: String = text.chars().take(157).collect();
    format!("{trimmed}...")
}

fn log_status(status: &Dinov2Status) {
    if !status.loaded {
        eprintln!("warning: {}", status.message);
        return;
    }
    eprintln!("{}", status.message);
    for port in &status.inputs {
        eprintln!("DINOv2 input {} {}", port.name, port.dtype);
    }
    for port in &status.outputs {
        eprintln!("DINOv2 output {} {}", port.name, port.dtype);
    }
}

#[cfg(test)]
pub(crate) fn lock_for_test() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_model_next_to_the_exe_wins_over_the_crate_copy() {
        let root =
            std::env::temp_dir().join(format!("ls-vision-dino-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let exe_dir = root.join("app");
        let manifest = root.join("crate");
        std::fs::create_dir_all(exe_dir.join("models")).unwrap();
        std::fs::create_dir_all(manifest.join("models")).unwrap();
        let bundled = exe_dir.join("models").join(FILE_NAME);
        let crate_copy = manifest.join("models").join(FILE_NAME);
        std::fs::write(&crate_copy, b"crate").unwrap();
        assert_eq!(
            resolve_model_path(Some(&exe_dir.join("ls-vision.exe")), &manifest),
            crate_copy
        );
        std::fs::write(&bundled, b"bundled").unwrap();
        assert_eq!(
            resolve_model_path(Some(&exe_dir.join("ls-vision.exe")), &manifest),
            bundled
        );
        assert_eq!(resolve_model_path(None, &manifest), crate_copy);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_model_warns_and_a_present_model_reports_shapes() {
        let _hold = lock_for_test();
        let missing = std::env::temp_dir().join("ls-vision-dinov2-missing.onnx");
        let _ = std::fs::remove_file(&missing);
        let status = load_at(&missing);
        assert!(!status.loaded);
        assert!(status.message.contains("missing"), "{}", status.message);
        assert!(
            status.message.contains("YOLO detection is unchanged."),
            "{}",
            status.message
        );
        assert!(status.inputs.is_empty());
        assert!(status.outputs.is_empty());
        assert_eq!(dinov2_status(), status);

        let path = model_path();
        if !path.is_file() {
            eprintln!("skip: {FILE_NAME} is not in src-tauri/models");
            return;
        }
        let status = load_at(&path);
        assert!(status.loaded, "{}", status.message);
        assert!(
            status.message.contains("DINOv2 session is ready"),
            "{}",
            status.message
        );
        let input = status
            .inputs
            .iter()
            .find(|port| port.name == "input")
            .expect("input");
        assert_eq!(input.shape, vec![1, 3, 224, 224]);
        let output = status
            .outputs
            .iter()
            .find(|port| port.name == "output")
            .expect("output");
        assert_eq!(output.shape, vec![1, 384]);
        assert_eq!(dinov2_status().inputs, status.inputs);
        assert_eq!(dinov2_status().outputs, status.outputs);
        assert!(dinov2_status().loaded);
    }

    #[test]
    fn missing_session_skips_embed_without_a_line() {
        let _hold = lock_for_test();
        let missing = std::env::temp_dir().join("ls-vision-dinov2-embed-missing.onnx");
        let _ = std::fs::remove_file(&missing);
        load_at(&missing);
        assert!(!is_loaded());
        let crops = [DebugCrop {
            class_name: "2BBMNM08",
            confidence: 0.5,
            bbox: [0, 0, 2, 2],
        }];
        let rgb = vec![0u8; 4 * 4 * 3];
        assert!(debug_embed(&rgb, 4, 4, &crops, false, 0.7).lines.is_empty());
    }

    #[test]
    fn letterbox_pads_with_imagenet_mean_and_does_not_stretch() {
        // 2 wide by 8 tall. The square pad is top and bottom, so the corner stays 0.
        let mut rgb = vec![0u8; 2 * 8 * 3];
        for pixel in rgb.chunks_exact_mut(3) {
            pixel.copy_from_slice(&[255, 255, 255]);
        }
        let data = imagenet_letterbox(&rgb, 2, 8).unwrap();
        assert_eq!(data.len(), 3 * EMBED_SIZE * EMBED_SIZE);
        assert_eq!(data[0], 0.0);
        let geometry = detect::letterbox_geometry(2, 8, EMBED_SIZE as i32);
        assert!(geometry.new_w < EMBED_SIZE as i32);
        assert_eq!(geometry.new_h, EMBED_SIZE as i32);
        let x = geometry.left.max(0) as usize;
        let white = (1.0 - IMAGENET_MEAN[0]) / IMAGENET_STD[0];
        let got = data[x];
        assert!(
            (got - white).abs() < 0.02,
            "content pixel {got} was stretched or not normalised"
        );
    }

    #[test]
    fn embed_line_reports_class_confidence_norm_and_four_values() {
        let values = [0.1_f32, -0.2, 0.3, -0.4, 9.0];
        let line = embed_line("2BBMNM08", 0.8123, &values, None);
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert_eq!(
            line,
            format!("DINOv2 2BBMNM08 conf 0.8123 norm {norm:.4} +0.1000 -0.2000 +0.3000 -0.4000")
        );
    }

    #[test]
    fn cosine_match_is_one_for_the_same_vector_and_lower_for_another() {
        let _hold = lock_for_test();
        clear_references();
        remember("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]),
            Some(1.0)
        );
        let wrong = best_similarity("2BBMNM08", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        assert!(wrong.abs() < 0.001, "{wrong}");
        assert!(best_similarity("OTHER", &[1.0, 0.0, 0.0, 0.0]).is_none());
        let line = embed_line("2BBMNM08", 0.5, &[1.0, 0.0, 0.0, 0.0], Some(0.81234));
        assert!(line.contains(" match 0.8123"), "{line}");
        clear_references();
    }

    #[test]
    fn a_score_above_the_threshold_counts_and_the_threshold_does_not() {
        assert_eq!(reference_counts(Some(0.7001), 0.7), Some(true));
        assert_eq!(reference_counts(Some(0.7), 0.7), Some(false));
        assert_eq!(reference_counts(Some(0.26), 0.7), Some(false));
        assert_eq!(reference_counts(None, 0.7), None);
        assert_eq!(reference_counts(Some(f32::NAN), 0.7), None);
    }

    #[test]
    fn saved_references_load_after_memory_is_cleared() {
        let _hold = lock_for_test();
        clear_references();
        remember("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]);
        let path =
            std::env::temp_dir().join(format!("ls-vision-dino-refs-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        save_references_to(&path).unwrap();
        clear_references();
        assert!(best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert_eq!(load_references_from(&path).unwrap(), true);
        assert_eq!(
            best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]),
            Some(1.0)
        );
        std::fs::write(&path, b"not json").unwrap();
        let err = load_references_from(&path).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        assert_eq!(
            best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]),
            Some(1.0)
        );
        let missing = std::env::temp_dir().join(format!(
            "ls-vision-dino-refs-missing-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        assert_eq!(load_references_from(&missing).unwrap(), false);
        assert_eq!(
            best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]),
            Some(1.0)
        );
        clear_references();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn selected_boxes_append_or_replace_one_class_and_reload() {
        let _hold = lock_for_test();
        clear_references();
        clear_held();
        remember("OTHER", &[0.0, 1.0, 0.0, 0.0]);
        remember("2BBMNM08", &[0.0, 1.0, 0.0, 0.0]);
        hold_embeddings(vec![
            Some(HeldEmbed {
                ic: "2BBMNM08".to_string(),
                values: vec![1.0, 0.0, 0.0, 0.0],
            }),
            Some(HeldEmbed {
                ic: "6PBB33".to_string(),
                values: vec![0.0, 0.0, 1.0, 0.0],
            }),
            None,
        ]);
        let rows = apply_selected_references(&[0], false).unwrap();
        assert_eq!(rows, vec![("2BBMNM08".to_string(), 2)]);
        assert_eq!(
            best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(best_similarity("OTHER", &[0.0, 1.0, 0.0, 0.0]), Some(1.0));
        assert!(best_similarity("6PBB33", &[0.0, 0.0, 1.0, 0.0]).is_none());

        let rows = apply_selected_references(&[0, 0], true).unwrap();
        assert_eq!(rows, vec![("2BBMNM08".to_string(), 1)]);
        let replaced = best_similarity("2BBMNM08", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        assert!(replaced.abs() < 0.001, "{replaced}");
        assert_eq!(
            best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(best_similarity("OTHER", &[0.0, 1.0, 0.0, 0.0]), Some(1.0));

        let rows = apply_selected_references(&[0, 1], false).unwrap();
        assert_eq!(
            rows,
            vec![("2BBMNM08".to_string(), 2), ("6PBB33".to_string(), 1)]
        );

        let path = std::env::temp_dir().join(format!(
            "ls-vision-dino-selected-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        save_references_to(&path).unwrap();
        clear_references();
        assert!(best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert_eq!(load_references_from(&path).unwrap(), true);
        assert_eq!(
            best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(best_similarity("6PBB33", &[0.0, 0.0, 1.0, 0.0]), Some(1.0));
        assert_eq!(best_similarity("OTHER", &[0.0, 1.0, 0.0, 0.0]), Some(1.0));

        assert_eq!(
            saved_message(
                &[("2BBMNM08".to_string(), 2), ("6PBB33".to_string(), 1)],
                false
            ),
            "Saved 2BBMNM08 (2), 6PBB33 (1)."
        );
        assert_eq!(
            saved_message(&[("2BBMNM08".to_string(), 1)], true),
            "Replaced 2BBMNM08 (1)."
        );
        assert!(apply_selected_references(&[], false).is_err());
        let missing = apply_selected_references(&[9], false).unwrap_err();
        assert!(missing.contains("Detect again"), "{missing}");
        let empty = apply_selected_references(&[2], false).unwrap_err();
        assert!(empty.contains("no DINOv2"), "{empty}");

        clear_references();
        clear_held();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marquee_vector_appends_or_replaces_one_class_and_reloads() {
        let _hold = lock_for_test();
        clear_references();
        clear_held();
        clear_held_frame();
        remember("OTHER", &[0.0, 1.0, 0.0, 0.0]);
        remember("2BBMNM08", &[0.0, 1.0, 0.0, 0.0]);
        hold_embeddings(vec![Some(HeldEmbed {
            ic: "2BBMNM08".to_string(),
            values: vec![1.0, 0.0, 0.0, 0.0],
        })]);
        hold_marquee(vec![1.0, 0.0, 0.0, 0.0]);
        let rows = apply_marquee_reference("2BBMNM08", false).unwrap();
        assert_eq!(rows, vec![("2BBMNM08".to_string(), 2)]);
        assert!(lock(&MARQUEE).is_none());
        assert_eq!(
            best_similarity("2BBMNM08", &[1.0, 0.0, 0.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(
            best_similarity("2BBMNM08", &[0.0, 1.0, 0.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(best_similarity("OTHER", &[0.0, 1.0, 0.0, 0.0]), Some(1.0));
        assert_eq!(
            apply_selected_references(&[0], false).unwrap(),
            vec![("2BBMNM08".to_string(), 3)]
        );

        hold_marquee(vec![0.0, 0.0, 1.0, 0.0]);
        let rows = apply_marquee_reference("2BBMNM08", true).unwrap();
        assert_eq!(rows, vec![("2BBMNM08".to_string(), 1)]);
        assert!(
            best_similarity("2BBMNM08", &[0.0, 1.0, 0.0, 0.0])
                .unwrap()
                .abs()
                < 0.001
        );
        assert_eq!(
            best_similarity("2BBMNM08", &[0.0, 0.0, 1.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(best_similarity("OTHER", &[0.0, 1.0, 0.0, 0.0]), Some(1.0));

        hold_marquee(vec![1.0, 0.0, 0.0, 0.0]);
        let rows = apply_marquee_reference(" 6PBB33 ", false).unwrap();
        assert_eq!(rows, vec![("6PBB33".to_string(), 1)]);
        assert_eq!(saved_message(&rows, false), "Saved 6PBB33 (1).");

        let path = std::env::temp_dir().join(format!(
            "ls-vision-dino-marquee-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        save_references_to(&path).unwrap();
        clear_references();
        assert!(best_similarity("6PBB33", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert_eq!(load_references_from(&path).unwrap(), true);
        assert_eq!(best_similarity("6PBB33", &[1.0, 0.0, 0.0, 0.0]), Some(1.0));
        assert_eq!(
            best_similarity("2BBMNM08", &[0.0, 0.0, 1.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(best_similarity("OTHER", &[0.0, 1.0, 0.0, 0.0]), Some(1.0));

        hold_marquee(vec![1.0, 0.0, 0.0, 0.0]);
        let blank = apply_marquee_reference("  ", false).unwrap_err();
        assert!(blank.contains("Enter an IC"), "{blank}");
        assert!(lock(&MARQUEE).is_some());
        clear_marquee_reference();
        let missing = apply_marquee_reference("2BBMNM08", false).unwrap_err();
        assert!(missing.contains("Draw a region"), "{missing}");

        hold_marquee(vec![f32::NAN, 1.0]);
        let bad = apply_marquee_reference("2BBMNM08", false).unwrap_err();
        assert!(bad.contains("no DINOv2"), "{bad}");

        hold_marquee(vec![1.0, 0.0, 0.0, 0.0]);
        hold_frame(&[9, 9, 9, 8, 8, 8, 7, 7, 7, 6, 6, 6], 2, 2);
        assert!(lock(&MARQUEE).is_none());
        assert!(lock(&FRAME).is_some());
        hold_frame(&[], 0, 0);
        assert!(lock(&FRAME).is_none());
        assert_eq!(marquee_bbox(1.2, 3.4, 10.0, 12.0).unwrap(), [1, 3, 11, 15]);
        assert!(marquee_bbox(0.0, 0.0, 0.4, 10.0).is_err());
        assert!(marquee_bbox(f64::NAN, 0.0, 10.0, 10.0).is_err());

        clear_references();
        clear_held();
        clear_held_frame();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn imported_file_replaces_memory_and_the_saved_references() {
        let _hold = lock_for_test();
        clear_references();
        remember("OLD", &[0.0, 1.0, 0.0, 0.0]);
        let dir = std::env::temp_dir().join(format!(
            "ls-vision-dino-import-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("dinov2-references.json");
        let dest = dir.join("installed").join("dinov2-references.json");
        let body = concat!(
            r#"{"version":1,"references":{"#,
            r#"" ":[[1.0,0.0]],"#,
            r#""OK":[[],[1.0,0.0,0.0,0.0]],"#,
            r#""6PBB33":[[0.0,0.0,1.0,0.0]]"#,
            "}}"
        );
        std::fs::write(&source, body).unwrap();
        let count = install_reference_file(&source, &dest).unwrap();
        assert_eq!(count, 2);
        assert_eq!(references_loaded_message(count), "References loaded 2 ICs.");
        assert!(best_similarity("OLD", &[0.0, 1.0, 0.0, 0.0]).is_none());
        assert_eq!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]), Some(1.0));
        assert_eq!(best_similarity("6PBB33", &[0.0, 0.0, 1.0, 0.0]), Some(1.0));
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            std::fs::read(&source).unwrap()
        );

        clear_references();
        assert!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert_eq!(load_references_from(&dest).unwrap(), true);
        assert_eq!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]), Some(1.0));

        let kept = std::fs::read(&dest).unwrap();
        let bad = dir.join("bad.json");
        std::fs::write(&bad, b"not json").unwrap();
        let err = install_reference_file(&bad, &dest).unwrap_err();
        assert!(err.contains("not a valid"), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), kept);
        assert_eq!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]), Some(1.0));
        std::fs::write(
            &bad,
            r#"{"version":2,"references":{"NEW":[[1.0,0.0,0.0,0.0]]}}"#,
        )
        .unwrap();
        assert!(install_reference_file(&bad, &dest).is_err());
        assert!(best_similarity("NEW", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert_eq!(std::fs::read(&dest).unwrap(), kept);

        let only = dir.join("one.json");
        std::fs::write(
            &only,
            r#"{"version":1,"references":{"2BBMNM08":[[0.0,1.0,0.0,0.0]]}}"#,
        )
        .unwrap();
        assert_eq!(install_reference_file(&only, &dest).unwrap(), 1);
        assert!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert_eq!(
            best_similarity("2BBMNM08", &[0.0, 1.0, 0.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(references_loaded_message(1), "References loaded 1 ICs.");

        clear_references();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn per_ic_threshold_is_saved_and_gates_the_next_count() {
        let _hold = lock_for_test();
        clear_references();
        remember("OK", &[1.0, 0.0, 0.0, 0.0]);
        remember("OK", &[0.0, 1.0, 0.0, 0.0]);
        remember("6PBB33", &[0.0, 0.0, 1.0, 0.0]);
        apply_reference_threshold("OLD", 0.8).unwrap_err();
        let row = apply_reference_threshold(" OK ", 0.91).unwrap();
        assert_eq!(row.ic, "OK");
        assert_eq!(row.threshold, Some(0.91));
        assert_eq!(row.count, 2);
        assert!((threshold_for("OK", 0.70) - 0.91).abs() < 0.0001);
        assert!((threshold_for("6PBB33", 0.70) - 0.70).abs() < 0.0001);
        assert_eq!(
            reference_counts(Some(0.80), threshold_for("OK", 0.70)),
            Some(false)
        );
        assert_eq!(
            reference_counts(Some(0.80), threshold_for("6PBB33", 0.70)),
            Some(true)
        );
        assert_eq!(
            apply_reference_threshold("OK", 0.10).unwrap().threshold,
            Some(0.50)
        );
        assert_eq!(
            apply_reference_threshold("OK", 1.2).unwrap().threshold,
            Some(0.95)
        );
        let kept = apply_reference_threshold("OK", f32::NAN).unwrap_err();
        assert!(kept.contains("0.50"), "{kept}");
        assert!((threshold_for("OK", 0.70) - 0.95).abs() < 0.0001);

        let path = std::env::temp_dir().join(format!(
            "ls-vision-dino-threshold-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        save_references_to(&path).unwrap();
        clear_references();
        assert!((threshold_for("OK", 0.70) - 0.70).abs() < 0.0001);
        assert_eq!(load_references_from(&path).unwrap(), true);
        assert!((threshold_for("OK", 0.70) - 0.95).abs() < 0.0001);
        assert_eq!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]), Some(1.0));
        assert_eq!(reference_rows().len(), 2);
        let listed = reference_rows();
        assert_eq!(listed[0].ic, "6PBB33");
        assert_eq!(listed[0].threshold, None);
        assert_eq!(listed[0].count, 1);
        assert_eq!(listed[1].ic, "OK");
        assert_eq!(listed[1].threshold, Some(0.95));

        remove_reference("OK").unwrap();
        assert!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert!((threshold_for("OK", 0.70) - 0.70).abs() < 0.0001);
        assert_eq!(best_similarity("6PBB33", &[0.0, 0.0, 1.0, 0.0]), Some(1.0));
        let missing = remove_reference("OK").unwrap_err();
        assert!(missing.contains("not saved"), "{missing}");
        save_references_to(&path).unwrap();
        clear_references();
        assert_eq!(load_references_from(&path).unwrap(), true);
        assert!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert_eq!(best_similarity("6PBB33", &[0.0, 0.0, 1.0, 0.0]), Some(1.0));
        assert_eq!(reference_rows().len(), 1);

        let dir = path.parent().unwrap().join(format!(
            "ls-vision-dino-threshold-import-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("dinov2-references.json");
        let dest = dir.join("installed").join("dinov2-references.json");
        std::fs::write(
            &source,
            concat!(
                r#"{"version":1,"references":{"#,
                r#""OK":[[1.0,0.0,0.0,0.0]],"#,
                r#""6PBB33":[[0.0,0.0,1.0,0.0]],"#,
                r#"" ":[[1.0]]"#,
                r#"},"thresholds":{"OK":0.91,"GONE":0.8,"OK ":null,"6PBB33":"nope"}}"#,
            ),
        )
        .unwrap();
        // A bad threshold value makes the whole file invalid, so memory stays.
        assert!(install_reference_file(&source, &dest).is_err());
        assert_eq!(best_similarity("6PBB33", &[0.0, 0.0, 1.0, 0.0]), Some(1.0));

        std::fs::write(
            &source,
            concat!(
                r#"{"version":1,"references":{"#,
                r#""OK":[[1.0,0.0,0.0,0.0],[0.0,1.0,0.0,0.0]],"#,
                r#""6PBB33":[[0.0,0.0,1.0,0.0]]"#,
                r#"},"thresholds":{"OK":0.82,"6PBB33":1.4,"GONE":1.4," ":0.6}}"#,
            ),
        )
        .unwrap();
        assert_eq!(install_reference_file(&source, &dest).unwrap(), 2);
        assert!((threshold_for("OK", 0.70) - 0.82).abs() < 0.0001);
        assert!((threshold_for("6PBB33", 0.70) - 0.95).abs() < 0.0001);
        assert!((threshold_for("GONE", 0.70) - 0.70).abs() < 0.0001);
        assert_eq!(
            reference_counts(Some(0.80), threshold_for("OK", 0.70)),
            Some(false)
        );
        assert_eq!(
            reference_counts(Some(0.96), threshold_for("6PBB33", 0.70)),
            Some(true)
        );

        let plain = dir.join("plain.json");
        std::fs::write(
            &plain,
            r#"{"version":1,"references":{"6PBB33":[[0.0,0.0,1.0,0.0]]}}"#,
        )
        .unwrap();
        assert_eq!(install_reference_file(&plain, &dest).unwrap(), 1);
        assert!(best_similarity("OK", &[1.0, 0.0, 0.0, 0.0]).is_none());
        assert!((threshold_for("OK", 0.70) - 0.70).abs() < 0.0001);
        assert!((threshold_for("6PBB33", 0.70) - 0.70).abs() < 0.0001);

        clear_references();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reference_list_reads_the_same_file_and_reports_a_missing_or_invalid_one() {
        let _hold = lock_for_test();
        clear_references();
        let dir = std::env::temp_dir().join(format!("ls-vision-dino-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dinov2-references.json");

        let missing = reference_list_from(&path).unwrap();
        assert!(missing.rows.is_empty());
        assert!(missing.path.ends_with("dinov2-references.json"));

        std::fs::write(&path, "{not json").unwrap();
        let err = reference_list_from(&path).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        assert!(reference_rows().is_empty());

        std::fs::write(&path, r#"{"version":1,"references":{"OK":[[1.0,0.0]]}}"#).unwrap();
        let list = reference_list_from(&path).unwrap();
        assert_eq!(list.rows.len(), 1);
        assert_eq!(list.rows[0].ic, "OK");
        assert_eq!(list.rows[0].count, 1);

        std::fs::write(&path, "{").unwrap();
        let err = reference_list_from(&path).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        assert_eq!(reference_rows().len(), 1);

        std::fs::write(&path, r#"{"version":1,"references":{}}"#).unwrap();
        let empty = reference_list_from(&path).unwrap();
        assert!(empty.rows.is_empty());

        clear_references();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
