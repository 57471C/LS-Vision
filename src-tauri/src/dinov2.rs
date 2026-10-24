//! Optional DINOv2 ONNX session, stored beside the YOLO session.
//!
//! Startup opens `src-tauri/models/dinov2_vits14.onnx` when that file is present.
//! A missing or unloadable file is a warning. Detection keeps using the YOLO session.
//! After YOLO keeps a box, this module can crop it, letterbox it to 224, and log
//! the embedding. A test-mode capture stores that vector under the box's IC.
//! The map is kept in memory and written to `vision/dinov2-references.json`.
//! A later detection logs the best cosine score against those references.
//! A box counts toward its IC only when that score is above the saved threshold.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::detect;
use crate::inference_ep::{self, Provider};

const FILE_NAME: &str = "dinov2_vits14.onnx";
const REFERENCES_FILE: &str = "dinov2-references.json";
const REFERENCES_VERSION: u32 = 1;

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
static REFERENCES: LazyLock<Mutex<HashMap<String, Vec<Vec<f32>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// `src-tauri/models/dinov2_vits14.onnx`, from the crate directory baked in at compile time.
pub(crate) fn model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join(FILE_NAME)
}

/// Open the DINOv2 session once. A missing file does not touch the YOLO cache.
/// Saved references load even when the model file is missing.
pub(crate) fn load_on_startup() {
    load_at(&model_path());
    load_saved_references();
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

#[cfg(test)]
pub(crate) fn clear_references() {
    lock(&REFERENCES).clear();
}

/// Append one reference. Returns how many vectors are now stored for this IC.
pub(crate) fn remember(class_name: &str, values: &[f32]) -> usize {
    let mut guard = lock(&REFERENCES);
    let list = guard.entry(class_name.to_string()).or_default();
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

/// Best cosine similarity against the references stored for this IC.
pub(crate) fn best_similarity(class_name: &str, values: &[f32]) -> Option<f32> {
    let guard = lock(&REFERENCES);
    let list = guard.get(class_name)?;
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
}

fn references_path() -> Result<PathBuf, String> {
    let root = crate::local_config::project_root()?;
    Ok(root.join("vision").join(REFERENCES_FILE))
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
            let classes = lock(&REFERENCES).len();
            eprintln!(
                "DINOv2 references loaded: {classes} classes from {}",
                path.display()
            );
        }
        Ok(false) => {}
        Err(err) => eprintln!("warning: {err} In-memory references are unchanged."),
    }
}

/// `Ok(true)` when a file was read. `Ok(false)` when the file is absent.
pub(crate) fn load_references_from(path: &Path) -> Result<bool, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(format!("Could not read DINOv2 references: {err}")),
    };
    let file: ReferenceFile = serde_json::from_str(&text).map_err(|_| {
        format!(
            "DINOv2 references at {} are not valid JSON.",
            path.display()
        )
    })?;
    if file.version != REFERENCES_VERSION {
        return Err(format!(
            "DINOv2 references at {} use an unknown version.",
            path.display()
        ));
    }
    let mut clean = HashMap::new();
    for (name, vectors) in file.references {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let kept: Vec<Vec<f32>> = vectors
            .into_iter()
            .filter(|values| !values.is_empty() && values.iter().all(|value| value.is_finite()))
            .collect();
        if kept.is_empty() {
            continue;
        }
        clean.insert(name.to_string(), kept);
    }
    *lock(&REFERENCES) = clean;
    Ok(true)
}

pub(crate) fn save_references_to(path: &Path) -> Result<(), String> {
    let file = ReferenceFile {
        version: REFERENCES_VERSION,
        references: lock(&REFERENCES).clone(),
    };
    let mut text = serde_json::to_string(&file)
        .map_err(|_| "Could not write DINOv2 references.".to_string())?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Could not write DINOv2 references.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("Could not write DINOv2 references: {err}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text.as_bytes())
        .map_err(|err| format!("Could not write DINOv2 references: {err}"))?;
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
    let mut stored: HashMap<String, usize> = HashMap::new();
    for crop in crops {
        let (line, counts) = match embed_one(session, rgb, width, height, crop) {
            Ok(values) => {
                // Score against the references already stored, then append.
                let matched = best_similarity(crop.class_name, &values);
                let counts = reference_counts(matched, threshold);
                if store {
                    let count = remember(crop.class_name, &values);
                    stored.insert(crop.class_name.to_string(), count);
                }
                (
                    embed_line(crop.class_name, crop.confidence, &values, matched),
                    counts,
                )
            }
            Err(err) => (
                format!(
                    "DINOv2 {} conf {:.4} embed failed: {err}",
                    crop.class_name, crop.confidence
                ),
                None,
            ),
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
    EmbedDebug { lines, counted }
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
}
