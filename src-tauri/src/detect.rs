//! ONNX detection for the operator page.
//!
//! `detect_image` reads the latest raw frame. A test-mode file is the
//! exception: that call passes the chosen image, and the same letterbox
//! path runs on those pixels. It returns `detections` of
//! `{ ic, confidence, bbox }` and the model's `classes`. The overlay draws
//! those boxes on the still, so this command does not encode a JPEG.
//! This command reads `.onnx` only. A `.pt` name is a training file; the page warns and does not detect.
//!
//! Boxes are checked against Ultralytics `YOLO(best.onnx)`, not `YOLO(best.pt)`.
//! Those two already disagree on the same photo. JPEG bytes are decoded with
//! TurboJPEG so the pixels match `cv2.imdecode`. The resize matches the OpenCV
//! 8-bit `INTER_LINEAR` path.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::frame::{FrameState, RawFrame};
use crate::inference_ep::{self, Provider};
use crate::local_config;

const DEFAULT_CONFIDENCE: f32 = 0.45;
const MIN_CONFIDENCE: f32 = 0.15;
const MAX_CONFIDENCE: f32 = 0.80;
const IOU_THRESHOLD: f32 = 0.7;
const MAX_DET: usize = 300;
const MAX_NMS: usize = 30_000;
const MAX_WH: f32 = 7680.0;
const DEFAULT_IMAGE_SIZE: usize = 640;
const PAD_VALUE: u8 = 114;
const INTER_RESIZE_COEF_SCALE: i32 = 2048;
const MAX_PIXELS: usize = 40_000_000;
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MODEL_MISSING: &str = "Model is missing.";
const MODEL_FAILED: &str = "Model failed to load.";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Detection {
    ic: String,
    confidence: f64,
    bbox: [i32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Roi {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

struct OutputFrame {
    view_w: i32,
    view_h: i32,
    image_size: i32,
    origin_x: i32,
    origin_y: i32,
}

struct Candidate {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    score: f32,
    class_id: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct CacheKey {
    path: PathBuf,
    mtime_ns: u128,
    len: u64,
    requested: Provider,
}

struct LoadedModel {
    key: CacheKey,
    names: Vec<String>,
    image_size: usize,
    end_to_end: bool,
    requested: Provider,
    provider: Provider,
    session: ort::session::Session,
}

static MODEL_CACHE: Mutex<Option<LoadedModel>> = Mutex::new(None);

fn failure(message: &str) -> Value {
    json!({
        "detections": [],
        "image_base64": Value::Null,
        "message": message,
    })
}

fn short_error(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = text.chars().count();
    if count == 0 {
        return "unknown error".to_string();
    }
    if count <= 240 {
        return text;
    }
    let trimmed: String = text.chars().take(237).collect();
    format!("{trimmed}...")
}

/// Python 3 `round` for a value that becomes an integer: half goes to even.
fn py_round(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let floor = value.floor();
    let diff = value - floor;
    let rounded = if diff > 0.5 {
        floor + 1.0
    } else if diff < 0.5 {
        floor
    } else {
        let base = floor as i64;
        if base % 2 == 0 { floor } else { floor + 1.0 }
    };
    if rounded >= i32::MAX as f64 {
        return i32::MAX;
    }
    if rounded <= i32::MIN as f64 {
        return i32::MIN;
    }
    rounded as i32
}

fn round_places(value: f64, places: u32) -> f64 {
    let scale = 10f64.powi(places as i32);
    f64::from(py_round(value * scale)) / scale
}

/// `cvRound` for a non-negative float coefficient. Ties go to even.
fn cv_round_f32(value: f32) -> i32 {
    if !value.is_finite() || value < 0.0 {
        return 0;
    }
    let floor = value.floor();
    let diff = value - floor;
    let rounded = if diff > 0.5 {
        floor + 1.0
    } else if diff < 0.5 {
        floor
    } else {
        let base = floor as i32;
        if base % 2 == 0 { floor } else { floor + 1.0 }
    };
    rounded as i32
}

fn json_f64(value: &Value) -> Option<f64> {
    // JSON true must not become 1. A string is not a fraction.
    let number = value.as_f64()?;
    if number.is_finite() {
        Some(number)
    } else {
        None
    }
}

fn clean_detect_roi(raw: &Value) -> Option<Roi> {
    if raw.is_null() {
        return None;
    }
    let object = raw.as_object()?;
    let x = json_f64(object.get("x")?)?;
    let y = json_f64(object.get("y")?)?;
    let w = json_f64(object.get("w")?)?;
    let h = json_f64(object.get("h")?)?;
    if w < 0.01 || h < 0.01 {
        return None;
    }
    if x < 0.0 || y < 0.0 || w > 1.0 || h > 1.0 || x + w > 1.0001 || y + h > 1.0001 {
        return None;
    }
    let width = w.min(1.0 - x);
    let height = h.min(1.0 - y);
    Some(Roi {
        x: round_places(x, 4),
        y: round_places(y, 4),
        w: round_places(width, 4),
        h: round_places(height, 4),
    })
}

/// Saved `confidence` in vision/config.local.json. Missing or invalid values use 0.45.
fn confidence_value(document: &Value) -> f32 {
    let Some(number) = document.get("confidence").and_then(|value| value.as_f64()) else {
        return DEFAULT_CONFIDENCE;
    };
    if !number.is_finite() {
        return DEFAULT_CONFIDENCE;
    }
    (number as f32).clamp(MIN_CONFIDENCE, MAX_CONFIDENCE)
}

/// `enabled` defaults to true when the field is missing or not a boolean.
fn slot_is_enabled(raw: &Value) -> bool {
    raw.get("enabled").and_then(Value::as_bool).unwrap_or(true)
}

/// Saved part-detection regions, slot 1 through 4.
///
/// `detectRois` is the list when that key is an array, and a null entry is
/// skipped. A box with `enabled: false` is skipped too. A file that only has
/// `detectRoi` still uses that box as slot 1.
fn slots_from_document(document: &Value) -> Vec<(i32, Roi)> {
    if let Some(list) = document.get("detectRois").and_then(Value::as_array) {
        let mut slots = Vec::new();
        for (index, item) in list.iter().take(4).enumerate() {
            if let Some(roi) = clean_detect_roi(item) {
                if slot_is_enabled(item) {
                    slots.push((index as i32 + 1, roi));
                }
            }
        }
        return slots;
    }
    match document.get("detectRoi") {
        Some(raw) => match clean_detect_roi(raw) {
            Some(roi) if slot_is_enabled(raw) => vec![(1, roi)],
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// True when a box is saved, including a box the operator has turned off.
fn any_saved_detect_roi(document: &Value) -> bool {
    if let Some(list) = document.get("detectRois").and_then(Value::as_array) {
        return list
            .iter()
            .take(4)
            .any(|item| clean_detect_roi(item).is_some());
    }
    document
        .get("detectRoi")
        .and_then(clean_detect_roi)
        .is_some()
}

enum DetectPlan {
    FullFrame,
    Idle,
    Slots(Vec<(i32, Roi)>),
}

/// No saved box uses the full frame. Saved boxes that are all off detect nothing.
fn detect_plan(document: &Value) -> DetectPlan {
    let slots = slots_from_document(document);
    if !slots.is_empty() {
        return DetectPlan::Slots(slots);
    }
    if any_saved_detect_roi(document) {
        DetectPlan::Idle
    } else {
        DetectPlan::FullFrame
    }
}

fn roi_slice(width: usize, height: usize, roi: &Roi) -> (usize, usize, usize, usize) {
    if width < 1 || height < 1 {
        return (0, 0, width, height);
    }
    let mut x = py_round(roi.x * width as f64);
    let mut y = py_round(roi.y * height as f64);
    let mut rw = py_round(roi.w * width as f64);
    let mut rh = py_round(roi.h * height as f64);
    x = x.clamp(0, width as i32 - 1);
    y = y.clamp(0, height as i32 - 1);
    rw = rw.max(1).min(width as i32 - x);
    rh = rh.max(1).min(height as i32 - y);
    (x as usize, y as usize, rw as usize, rh as usize)
}

fn crop_rgb(
    rgb: &[u8],
    width: usize,
    height: usize,
    roi: &Roi,
) -> (Vec<u8>, usize, usize, i32, i32) {
    let (x, y, rw, rh) = roi_slice(width, height, roi);
    let mut out = Vec::with_capacity(rw * rh * 3);
    for row in 0..rh {
        let start = ((y + row) * width + x) * 3;
        let end = start + rw * 3;
        out.extend_from_slice(&rgb[start..end]);
    }
    (out, rw, rh, x as i32, y as i32)
}

/// OpenCV 4.10 `cv2.resize(..., INTER_LINEAR)` for 8-bit pixels.
///
/// Horizontal sums stay in `i32` (`pixel * 11-bit coeff`). The 8-bit vertical
/// step is the specialized form, not `(sum + bias) >> 22`:
/// `((b0 * (row >> 4)) >> 16 + (b1 * (row >> 4)) >> 16 + 2) >> 2`.
pub(crate) fn resize_linear(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u8> {
    let channels = 3;
    if src_w == dst_w && src_h == dst_h {
        return src.to_vec();
    }
    let scale_x = 1.0 / (dst_w as f64 / src_w as f64);
    let scale_y = 1.0 / (dst_h as f64 / src_h as f64);
    let mut xofs = vec![0i32; dst_w * channels];
    let mut alpha = vec![0i32; dst_w * channels * 2];
    let mut yofs = vec![0i32; dst_h];
    let mut beta = vec![0i32; dst_h * 2];
    let mut xmax = dst_w;

    for dx in 0..dst_w {
        let mut fx = ((dx as f64 + 0.5) * scale_x - 0.5) as f32;
        let mut sx = fx.floor() as i32;
        fx -= sx as f32;
        if sx < 0 {
            fx = 0.0;
            sx = 0;
        }
        if sx + 1 >= src_w as i32 {
            xmax = xmax.min(dx);
            if sx >= src_w as i32 - 1 {
                fx = 0.0;
                sx = src_w as i32 - 1;
            }
        }
        let base = sx * channels as i32;
        for channel in 0..channels {
            xofs[dx * channels + channel] = base + channel as i32;
        }
        let a0 = cv_round_f32((1.0 - fx) * INTER_RESIZE_COEF_SCALE as f32);
        let a1 = cv_round_f32(fx * INTER_RESIZE_COEF_SCALE as f32);
        for channel in 0..channels {
            let at = (dx * channels + channel) * 2;
            alpha[at] = a0;
            alpha[at + 1] = a1;
        }
    }

    for dy in 0..dst_h {
        let mut fy = ((dy as f64 + 0.5) * scale_y - 0.5) as f32;
        let sy = fy.floor() as i32;
        fy -= sy as f32;
        yofs[dy] = sy;
        beta[dy * 2] = cv_round_f32((1.0 - fy) * INTER_RESIZE_COEF_SCALE as f32);
        beta[dy * 2 + 1] = cv_round_f32(fy * INTER_RESIZE_COEF_SCALE as f32);
    }

    let dwidth = dst_w * channels;
    let xmax_ch = xmax * channels;
    let mut horizontal = vec![0i32; src_h * dwidth];
    for sy in 0..src_h {
        let source = &src[sy * src_w * channels..(sy + 1) * src_w * channels];
        let dest = &mut horizontal[sy * dwidth..(sy + 1) * dwidth];
        for dx in 0..xmax_ch {
            let sx = xofs[dx] as usize;
            dest[dx] = i32::from(source[sx]) * alpha[dx * 2]
                + i32::from(source[sx + channels]) * alpha[dx * 2 + 1];
        }
        for dx in xmax_ch..dwidth {
            dest[dx] = i32::from(source[xofs[dx] as usize]) * INTER_RESIZE_COEF_SCALE;
        }
    }

    let mut out = vec![0u8; dst_h * dwidth];
    for dy in 0..dst_h {
        let sy0 = clip_row(yofs[dy], src_h);
        let sy1 = clip_row(yofs[dy] + 1, src_h);
        let b0 = beta[dy * 2];
        let b1 = beta[dy * 2 + 1];
        let row0 = &horizontal[sy0 * dwidth..(sy0 + 1) * dwidth];
        let row1 = &horizontal[sy1 * dwidth..(sy1 + 1) * dwidth];
        let dest = &mut out[dy * dwidth..(dy + 1) * dwidth];
        for dx in 0..dwidth {
            let left = (b0 * (row0[dx] >> 4)) >> 16;
            let right = (b1 * (row1[dx] >> 4)) >> 16;
            dest[dx] = ((left + right + 2) >> 2) as u8;
        }
    }
    out
}

fn clip_row(index: i32, height: usize) -> usize {
    if index < 0 {
        0
    } else if index as usize >= height {
        height - 1
    } else {
        index as usize
    }
}

pub(crate) struct Letterbox {
    pub(crate) new_w: i32,
    pub(crate) new_h: i32,
    pub(crate) top: i32,
    pub(crate) left: i32,
}

pub(crate) fn letterbox_geometry(width: i32, height: i32, size: i32) -> Letterbox {
    let shape_h = f64::from(height);
    let shape_w = f64::from(width);
    let target = f64::from(size);
    let ratio = (target / shape_h).min(target / shape_w);
    let new_w = py_round(shape_w * ratio);
    let new_h = py_round(shape_h * ratio);
    let dw = (target - f64::from(new_w)) / 2.0;
    let dh = (target - f64::from(new_h)) / 2.0;
    Letterbox {
        new_w,
        new_h,
        top: py_round(dh - 0.1),
        left: py_round(dw - 0.1),
    }
}

fn letterbox(rgb: &[u8], width: usize, height: usize, size: usize) -> Result<Vec<u8>, String> {
    let geometry = letterbox_geometry(width as i32, height as i32, size as i32);
    if geometry.new_w < 1 || geometry.new_h < 1 {
        return Err("Could not read the image".to_string());
    }
    let new_w = geometry.new_w as usize;
    let new_h = geometry.new_h as usize;
    let resized = resize_linear(rgb, width, height, new_w, new_h);
    let mut canvas = vec![PAD_VALUE; size * size * 3];
    let top = geometry.top.max(0) as usize;
    let left = geometry.left.max(0) as usize;
    for y in 0..new_h {
        let dst_y = top + y;
        if dst_y >= size || left >= size {
            break;
        }
        let copy_w = new_w.min(size - left);
        let src_start = y * new_w * 3;
        let dst_start = (dst_y * size + left) * 3;
        canvas[dst_start..dst_start + copy_w * 3]
            .copy_from_slice(&resized[src_start..src_start + copy_w * 3]);
    }
    Ok(canvas)
}

fn nchw(canvas: &[u8], size: usize) -> Vec<f32> {
    let plane = size * size;
    let mut data = vec![0f32; 3 * plane];
    for y in 0..size {
        for x in 0..size {
            let pixel = (y * size + x) * 3;
            let offset = y * size + x;
            data[offset] = f32::from(canvas[pixel]) / 255.0;
            data[plane + offset] = f32::from(canvas[pixel + 1]) / 255.0;
            data[plane * 2 + offset] = f32::from(canvas[pixel + 2]) / 255.0;
        }
    }
    data
}

fn scale_box(x1: f32, y1: f32, x2: f32, y2: f32, img0_h: i32, img0_w: i32, img1: i32) -> [i32; 4] {
    // Ultralytics recomputes the gain from the two shapes. It does not reuse
    // the letterbox offsets. The pad and gain are f64, then applied as f32.
    let img0_h_f = f64::from(img0_h);
    let img0_w_f = f64::from(img0_w);
    let img1_f = f64::from(img1);
    let gain = (img1_f / img0_h_f).min(img1_f / img0_w_f);
    let new_h = py_round(img0_h_f * gain);
    let new_w = py_round(img0_w_f * gain);
    let gain_y = f64::from(new_h) / img0_h_f;
    let gain_x = f64::from(new_w) / img0_w_f;
    let pad_x = py_round((img1_f - f64::from(new_w)) / 2.0 - 0.1);
    let pad_y = py_round((img1_f - f64::from(new_h)) / 2.0 - 0.1);
    let gain_x_f = gain_x as f32;
    let gain_y_f = gain_y as f32;
    let pad_x_f = pad_x as f32;
    let pad_y_f = pad_y as f32;
    let max_x = img0_w as f32;
    let max_y = img0_h as f32;
    let left = ((x1 - pad_x_f) / gain_x_f).clamp(0.0, max_x);
    let top = ((y1 - pad_y_f) / gain_y_f).clamp(0.0, max_y);
    let right = ((x2 - pad_x_f) / gain_x_f).clamp(0.0, max_x);
    let bottom = ((y2 - pad_y_f) / gain_y_f).clamp(0.0, max_y);
    [
        py_round(f64::from(left)),
        py_round(f64::from(top)),
        py_round(f64::from(right)),
        py_round(f64::from(bottom)),
    ]
}

fn nms(mut items: Vec<Candidate>) -> Vec<Candidate> {
    items.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then(left.class_id.cmp(&right.class_id))
    });
    if items.len() > MAX_NMS {
        items.truncate(MAX_NMS);
    }
    let mut suppressed = vec![false; items.len()];
    let mut keep = Vec::new();
    for index in 0..items.len() {
        if suppressed[index] {
            continue;
        }
        keep.push(index);
        if keep.len() == MAX_DET {
            break;
        }
        let offset_i = items[index].class_id as f32 * MAX_WH;
        let ix1 = items[index].x1 + offset_i;
        let iy1 = items[index].y1 + offset_i;
        let ix2 = items[index].x2 + offset_i;
        let iy2 = items[index].y2 + offset_i;
        let area_i = (ix2 - ix1) * (iy2 - iy1);
        for other in (index + 1)..items.len() {
            if suppressed[other] {
                continue;
            }
            let offset_j = items[other].class_id as f32 * MAX_WH;
            let jx1 = items[other].x1 + offset_j;
            let jy1 = items[other].y1 + offset_j;
            let jx2 = items[other].x2 + offset_j;
            let jy2 = items[other].y2 + offset_j;
            let w = (ix2.min(jx2) - ix1.max(jx1)).max(0.0);
            let h = (iy2.min(jy2) - iy1.max(jy1)).max(0.0);
            let inter = w * h;
            if inter == 0.0 {
                continue;
            }
            let area_j = (jx2 - jx1) * (jy2 - jy1);
            let union = area_i + area_j - inter;
            if union > 0.0 && inter / union > IOU_THRESHOLD {
                suppressed[other] = true;
            }
        }
    }
    keep.into_iter()
        .map(|index| {
            let item = &items[index];
            Candidate {
                x1: item.x1,
                y1: item.y1,
                x2: item.x2,
                y2: item.y2,
                score: item.score,
                class_id: item.class_id,
            }
        })
        .collect()
}

fn class_name(names: &[String], class_id: usize) -> String {
    names
        .get(class_id)
        .filter(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| class_id.to_string())
}

fn parse_class_names(text: &str) -> Vec<String> {
    if let Some(names) = parse_json_names(text) {
        return names;
    }
    let bytes = text.as_bytes();
    let mut pairs = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b':' {
            index += 1;
            continue;
        }
        let mut key_end = index;
        while key_end > 0 && bytes[key_end - 1].is_ascii_whitespace() {
            key_end -= 1;
        }
        let mut key_start = key_end;
        while key_start > 0 && bytes[key_start - 1].is_ascii_digit() {
            key_start -= 1;
        }
        if key_start == key_end {
            index += 1;
            continue;
        }
        let Ok(key) = std::str::from_utf8(&bytes[key_start..key_end])
            .unwrap_or("")
            .parse::<usize>()
        else {
            index += 1;
            continue;
        };
        let mut cursor = index + 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || (bytes[cursor] != b'\'' && bytes[cursor] != b'"') {
            index += 1;
            continue;
        }
        let quote = bytes[cursor];
        cursor += 1;
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        if let Ok(name) = std::str::from_utf8(&bytes[start..cursor]) {
            pairs.push((key, name.to_string()));
        }
        index = cursor.saturating_add(1);
    }
    names_from_pairs(pairs)
}

fn parse_json_names(text: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    let mut pairs = Vec::new();
    for (key, item) in object {
        let key = key.parse::<usize>().ok()?;
        let name = item.as_str()?.to_string();
        pairs.push((key, name));
    }
    if pairs.is_empty() {
        return None;
    }
    Some(names_from_pairs(pairs))
}

fn names_from_pairs(pairs: Vec<(usize, String)>) -> Vec<String> {
    let Some(last) = pairs.iter().map(|(key, _)| *key).max() else {
        return Vec::new();
    };
    let mut names = vec![String::new(); last + 1];
    for (key, name) in pairs {
        names[key] = name;
    }
    names
}

const LOAD_CLASSES: &str = "Load a model to choose classes.";
const NO_CLASS_LIST: &str = "This model has no class list.";

#[derive(Debug, Serialize)]
pub struct ModelClasses {
    model: String,
    classes: Vec<String>,
    message: String,
}

fn read_varint(data: &[u8], index: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    while *index < data.len() && shift < 64 {
        let byte = data[*index];
        *index += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

fn protobuf_bytes<'a>(data: &'a [u8], index: &mut usize) -> Option<&'a [u8]> {
    let len = usize::try_from(read_varint(data, index)?).ok()?;
    let end = index.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    let bytes = &data[*index..end];
    *index = end;
    Some(bytes)
}

// ONNX ModelProto stores Ultralytics class names in metadata_props (field 14).
fn onnx_names_metadata(data: &[u8]) -> Option<String> {
    let mut index = 0;
    while index < data.len() {
        let tag = read_varint(data, &mut index)?;
        let field = tag >> 3;
        let wire = tag & 7;
        match wire {
            0 => {
                read_varint(data, &mut index)?;
            }
            1 => index = index.checked_add(8)?,
            5 => index = index.checked_add(4)?,
            2 => {
                let bytes = protobuf_bytes(data, &mut index)?;
                if field == 14 {
                    if let Some(text) = metadata_entry_value(bytes, "names") {
                        return Some(text);
                    }
                }
            }
            _ => return None,
        }
    }
    None
}

fn metadata_entry_value(data: &[u8], wanted: &str) -> Option<String> {
    let mut index = 0;
    let mut key = String::new();
    let mut value = String::new();
    while index < data.len() {
        let tag = read_varint(data, &mut index)?;
        let field = tag >> 3;
        if tag & 7 != 2 {
            return None;
        }
        let bytes = protobuf_bytes(data, &mut index)?;
        let text = String::from_utf8_lossy(bytes).into_owned();
        if field == 1 {
            key = text;
        } else if field == 2 {
            value = text;
        }
    }
    if key == wanted && !value.is_empty() {
        Some(value)
    } else {
        None
    }
}

fn listed_classes(names: Vec<String>) -> Vec<String> {
    let mut listed = Vec::new();
    for name in names {
        if name.is_empty() || listed.iter().any(|item| item == &name) {
            continue;
        }
        listed.push(name);
    }
    listed
}

fn classes_from_metadata(model: &str, data: &[u8]) -> ModelClasses {
    let Some(text) = onnx_names_metadata(data) else {
        return ModelClasses {
            model: model.to_string(),
            classes: Vec::new(),
            message: NO_CLASS_LIST.to_string(),
        };
    };
    let classes = listed_classes(parse_class_names(&text));
    let message = if classes.is_empty() {
        NO_CLASS_LIST.to_string()
    } else {
        String::new()
    };
    ModelClasses {
        model: model.to_string(),
        classes,
        message,
    }
}

fn load_model_prompt(model: &str) -> ModelClasses {
    ModelClasses {
        model: model.to_string(),
        classes: Vec::new(),
        message: LOAD_CLASSES.to_string(),
    }
}

fn model_class_names_now() -> ModelClasses {
    let Ok(root) = local_config::project_root() else {
        return load_model_prompt("");
    };
    let Ok(model) = local_config::configured_model_name(&root) else {
        return load_model_prompt("");
    };
    if !model.to_ascii_lowercase().ends_with(".onnx") {
        return load_model_prompt(&model);
    }
    let Ok(models) = local_config::app_models_dir() else {
        return load_model_prompt(&model);
    };
    let Ok(path) = resolved_onnx_model(&models, &model) else {
        return load_model_prompt(&model);
    };
    let Ok(data) = std::fs::read(&path) else {
        return load_model_prompt(&model);
    };
    classes_from_metadata(&model, &data)
}

#[tauri::command]
pub async fn model_class_names() -> ModelClasses {
    match tauri::async_runtime::spawn_blocking(model_class_names_now).await {
        Ok(found) => found,
        Err(_) => load_model_prompt(""),
    }
}

fn score_at(
    values: &[f32],
    bcn: bool,
    channels: usize,
    boxes: usize,
    class_slot: usize,
    box_index: usize,
) -> f32 {
    if bcn {
        values[class_slot * boxes + box_index]
    } else {
        values[box_index * channels + class_slot]
    }
}

fn candidates_from_output(
    values: &[f32],
    channels: usize,
    boxes: usize,
    bcn: bool,
    conf_threshold: f32,
) -> Vec<Candidate> {
    let class_count = channels.saturating_sub(4);
    let mut found = Vec::new();
    if class_count == 0 {
        return found;
    }
    for box_index in 0..boxes {
        let cx = score_at(values, bcn, channels, boxes, 0, box_index);
        let cy = score_at(values, bcn, channels, boxes, 1, box_index);
        let w = score_at(values, bcn, channels, boxes, 2, box_index);
        let h = score_at(values, bcn, channels, boxes, 3, box_index);
        let mut best_score = f32::NEG_INFINITY;
        let mut best_class = 0;
        for class_id in 0..class_count {
            let score = score_at(values, bcn, channels, boxes, 4 + class_id, box_index);
            if score > best_score {
                best_score = score;
                best_class = class_id;
            }
        }
        if !best_score.is_finite() || best_score <= conf_threshold {
            continue;
        }
        let half_w = w * 0.5;
        let half_h = h * 0.5;
        found.push(Candidate {
            x1: cx - half_w,
            y1: cy - half_h,
            x2: cx + half_w,
            y2: cy + half_h,
            score: best_score,
            class_id: best_class,
        });
    }
    found
}

fn detections_from_output(
    values: &[f32],
    shape: &[i64],
    names: &[String],
    end_to_end: bool,
    frame: OutputFrame,
    conf_threshold: f32,
) -> Result<Vec<Detection>, String> {
    if shape.len() != 3 || shape[0] < 1 {
        return Err("Detect failed: unexpected output shape".to_string());
    }
    let dim1 = shape[1];
    let dim2 = shape[2];
    if dim1 < 1 || dim2 < 1 {
        return Err("Detect failed: unexpected output shape".to_string());
    }
    // Exported YOLOv8 is (1, 4 + classes, boxes). An end-to-end graph is (1, boxes, 6).
    let bcn = dim1 <= dim2;
    let (channels, boxes) = if bcn {
        (dim1 as usize, dim2 as usize)
    } else {
        (dim2 as usize, dim1 as usize)
    };
    if values.len() < channels * boxes {
        return Err("Detect failed: unexpected output shape".to_string());
    }
    let values = &values[..channels * boxes];
    let chosen = if end_to_end {
        end_to_end_candidates(values, channels, boxes, bcn, conf_threshold)
    } else {
        nms(candidates_from_output(
            values,
            channels,
            boxes,
            bcn,
            conf_threshold,
        ))
    };
    let mut detections = Vec::with_capacity(chosen.len());
    for item in chosen {
        let scaled = scale_box(
            item.x1,
            item.y1,
            item.x2,
            item.y2,
            frame.view_h,
            frame.view_w,
            frame.image_size,
        );
        detections.push(Detection {
            ic: class_name(names, item.class_id),
            confidence: round_places(f64::from(item.score), 4),
            bbox: [
                scaled[0] + frame.origin_x,
                scaled[1] + frame.origin_y,
                scaled[2] + frame.origin_x,
                scaled[3] + frame.origin_y,
            ],
        });
    }
    Ok(detections)
}

fn end_to_end_candidates(
    values: &[f32],
    channels: usize,
    boxes: usize,
    bcn: bool,
    conf_threshold: f32,
) -> Vec<Candidate> {
    if channels < 6 {
        return Vec::new();
    }
    let mut found = Vec::new();
    for box_index in 0..boxes {
        let x1 = score_at(values, bcn, channels, boxes, 0, box_index);
        let y1 = score_at(values, bcn, channels, boxes, 1, box_index);
        let x2 = score_at(values, bcn, channels, boxes, 2, box_index);
        let y2 = score_at(values, bcn, channels, boxes, 3, box_index);
        let score = score_at(values, bcn, channels, boxes, 4, box_index);
        let class_id = score_at(values, bcn, channels, boxes, 5, box_index);
        if !score.is_finite() || score <= conf_threshold || class_id < 0.0 {
            continue;
        }
        found.push(Candidate {
            x1,
            y1,
            x2,
            y2,
            score,
            class_id: class_id as usize,
        });
        if found.len() == MAX_DET {
            break;
        }
    }
    found
}

fn decode_rgb(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] == 0xd8 {
        return decode_jpeg(bytes);
    }
    decode_with_image(bytes)
}

fn reject_size(width: u32, height: u32) -> Result<(), String> {
    if width < 1 || height < 1 || (width as usize).saturating_mul(height as usize) > MAX_PIXELS {
        return Err("Could not read the image".to_string());
    }
    Ok(())
}

/// libjpeg-turbo, same family as OpenCV `cv2.imdecode`.
fn decode_jpeg(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let image = turbojpeg::decompress(bytes, turbojpeg::PixelFormat::RGB)
        .map_err(|_| "Could not read the image".to_string())?;
    let width = u32::try_from(image.width).map_err(|_| "Could not read the image".to_string())?;
    let height = u32::try_from(image.height).map_err(|_| "Could not read the image".to_string())?;
    reject_size(width, height)?;
    let row = image.width.saturating_mul(3);
    if image.pitch < row {
        return Err("Could not read the image".to_string());
    }
    if image.pitch == row {
        return Ok((image.pixels, width, height));
    }
    let mut packed = Vec::with_capacity(row.saturating_mul(image.height));
    for y in 0..image.height {
        let start = y.saturating_mul(image.pitch);
        let end = start + row;
        if end > image.pixels.len() {
            return Err("Could not read the image".to_string());
        }
        packed.extend_from_slice(&image.pixels[start..end]);
    }
    Ok((packed, width, height))
}

fn decode_with_image(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let image =
        image::load_from_memory(bytes).map_err(|_| "Could not read the image".to_string())?;
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    reject_size(width, height)?;
    Ok((rgb.into_raw(), width, height))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model.onnx")
        .to_string()
}

fn cache_key(path: &Path, requested: Provider) -> Result<CacheKey, String> {
    let meta = std::fs::metadata(path).map_err(|err| {
        format!(
            "{} failed to load: {}",
            file_name(path),
            short_error(&err.to_string())
        )
    })?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_nanos())
        .unwrap_or(0);
    Ok(CacheKey {
        path: path.to_path_buf(),
        mtime_ns,
        len: meta.len(),
        requested,
    })
}

/// Cargo tests run from `target/debug/deps`. `copy-dylibs` puts ONNX Runtime next
/// to `target/debug` when Windows will not symlink into `deps`.
fn prepare_runtime() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(exe_dir) = exe.parent() else {
        return;
    };
    let Some(parent) = exe_dir.parent() else {
        return;
    };
    if !parent.join("onnxruntime.dll").is_file() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !runtime_dll(name) {
            continue;
        }
        let dest = exe_dir.join(name);
        if !dest.exists() {
            let _ = std::fs::copy(&path, &dest);
        }
    }
}

fn runtime_dll(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".dll") {
        return false;
    }
    [
        "onnxruntime",
        "directml",
        "cublas",
        "cudart",
        "cudnn",
        "cufft",
        "curand",
        "nvrtc",
        "nvinfer",
        "nvonnxparser",
        "zlibwapi",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

pub(crate) fn ensure_runtime() -> bool {
    runtime().is_ok()
}

pub(crate) fn runtime() -> Result<ort::environment::Environment, String> {
    static ENV: Mutex<Option<ort::environment::Environment>> = Mutex::new(None);
    let mut guard = ENV.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(env) = guard.clone() {
        return Ok(env);
    }
    prepare_runtime();
    let env = ort::init().build().map_err(|err| {
        format!(
            "YOLO model is not loaded: {}",
            short_error(&err.to_string())
        )
    })?;
    *guard = Some(env.clone());
    Ok(env)
}

fn input_image_size(session: &ort::session::Session) -> usize {
    let Some(input) = session.inputs().first() else {
        return DEFAULT_IMAGE_SIZE;
    };
    let Some(shape) = input.dtype().tensor_shape() else {
        return DEFAULT_IMAGE_SIZE;
    };
    if shape.len() >= 4 && shape[2] > 0 && shape[2] == shape[3] && shape[2] <= 4096 {
        return shape[2] as usize;
    }
    DEFAULT_IMAGE_SIZE
}

fn execution_provider(provider: Provider) -> ort::ep::ExecutionProviderDispatch {
    match provider {
        Provider::DirectMl => ort::ep::DirectML::default().build(),
        Provider::Cuda => ort::ep::CUDA::default().build(),
        Provider::Cpu => ort::ep::CPU::default().build(),
    }
}

pub(crate) fn open_session(
    env: &ort::environment::Environment,
    path: &Path,
    provider: Provider,
) -> Result<ort::session::Session, String> {
    let name = file_name(path);
    let builder = ort::session::Session::builder(env)
        .map_err(|err| format!("{name} failed to load: {}", short_error(&err.to_string())))?;
    let mut builder = if provider == Provider::Cpu {
        builder
    } else {
        builder
            .with_execution_providers([execution_provider(provider).error_on_failure()])
            .map_err(|err| {
                format!(
                    "{name} failed to load with {}: {}",
                    provider.label(),
                    short_error(&err.to_string())
                )
            })?
    };
    builder.commit_from_file(path).map_err(|err| {
        format!(
            "{name} failed to load with {}: {}",
            provider.label(),
            short_error(&err.to_string())
        )
    })
}

fn finish_model(
    path: &Path,
    key: CacheKey,
    requested: Provider,
    provider: Provider,
    session: ort::session::Session,
) -> Result<LoadedModel, String> {
    let name = file_name(path);
    let (names, end_to_end) = {
        let metadata = session
            .metadata()
            .map_err(|err| format!("{name} failed to load: {}", short_error(&err.to_string())))?;
        let names = metadata
            .custom("names")
            .map(|text| parse_class_names(&text))
            .unwrap_or_default();
        let end_to_end = metadata.custom("end2end").is_some_and(|value| {
            let value = value.trim();
            value.eq_ignore_ascii_case("true") || value == "1"
        });
        (names, end_to_end)
    };
    let image_size = input_image_size(&session);
    let message = if provider == requested {
        String::new()
    } else {
        format!(
            "{} did not start. Using {}.",
            requested.label(),
            provider.label()
        )
    };
    inference_ep::remember(requested, provider, message);
    Ok(LoadedModel {
        key,
        names,
        image_size,
        end_to_end,
        requested,
        provider,
        session,
    })
}

fn load_model(path: &Path, key: CacheKey, requested: Provider) -> Result<LoadedModel, String> {
    let env = runtime()?;
    match open_session(&env, path, requested) {
        Ok(session) => finish_model(path, key, requested, requested, session),
        Err(err) if requested != Provider::Cpu => {
            let session = open_session(&env, path, Provider::Cpu)
                .map_err(|cpu_err| format!("{err} {cpu_err}"))?;
            let mut model = finish_model(path, key, requested, Provider::Cpu, session)?;
            model.key.requested = requested;
            inference_ep::remember(
                requested,
                Provider::Cpu,
                format!(
                    "{} did not start ({}). Using CPU.",
                    requested.label(),
                    short_error(&err)
                ),
            );
            Ok(model)
        }
        Err(err) => Err(err),
    }
}

fn with_model<T>(
    path: &Path,
    run: impl FnOnce(&mut LoadedModel) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = MODEL_CACHE.lock().unwrap_or_else(|err| err.into_inner());
    let requested = inference_ep::selected_provider();
    let key = cache_key(path, requested)?;
    let stale = guard.as_ref().is_none_or(|engine| engine.key != key);
    if stale {
        *guard = Some(load_model(path, key, requested)?);
    }
    let model = guard
        .as_mut()
        .ok_or_else(|| "YOLO model is not loaded".to_string())?;
    run(model)
}

/// Drop the cached session so the next detection reads the model file again.
pub(crate) fn forget_session() {
    let mut guard = MODEL_CACHE.lock().unwrap_or_else(|err| err.into_inner());
    *guard = None;
}

fn model_status_message(err: &str) -> String {
    if err == MODEL_MISSING {
        return MODEL_MISSING.to_string();
    }
    if err.contains("failed to load") || err.contains("not loaded") {
        return MODEL_FAILED.to_string();
    }
    err.to_string()
}

/// The active filename from `config.local.json`, joined to the app data models folder.
/// A missing file is reported before any session is opened.
fn resolved_onnx_model(models: &Path, name: &str) -> Result<PathBuf, String> {
    if !name.to_ascii_lowercase().ends_with(".onnx") {
        return Err(format!("{name} is not an ONNX file."));
    }
    let path = models.join(name);
    if !path.is_file() {
        return Err(MODEL_MISSING.to_string());
    }
    Ok(path)
}

fn active_onnx_path(root: &Path) -> Result<PathBuf, String> {
    let name = local_config::configured_model_name(root)?;
    resolved_onnx_model(&local_config::app_models_dir()?, &name)
}

/// Drop the cached session and load the active `.onnx` model with the saved provider.
pub(crate) fn reload_session() -> Result<(), String> {
    forget_session();
    let requested = inference_ep::selected_provider();
    let Ok(root) = local_config::project_root() else {
        inference_ep::remember(requested, requested, "");
        return Ok(());
    };
    match active_onnx_path(&root) {
        Ok(path) => match with_model(&path, |_| Ok(())) {
            Ok(()) => Ok(()),
            Err(err) => {
                let message = model_status_message(&err);
                inference_ep::remember(requested, requested, message.clone());
                Err(message)
            }
        },
        Err(err) if err == MODEL_MISSING => {
            inference_ep::remember(requested, requested, MODEL_MISSING);
            Ok(())
        }
        Err(_) => {
            inference_ep::remember(requested, requested, "");
            Ok(())
        }
    }
}

fn detect_view(
    rgb: &[u8],
    width: u32,
    height: u32,
    roi: Option<&Roi>,
    image_size: usize,
) -> Result<(Vec<f32>, i32, i32, i32, i32), String> {
    let width = width as usize;
    let height = height as usize;
    let (view, view_w, view_h, origin_x, origin_y) = if let Some(roi) = roi {
        crop_rgb(rgb, width, height, roi)
    } else {
        (rgb.to_vec(), width, height, 0, 0)
    };
    let canvas = letterbox(&view, view_w, view_h, image_size)?;
    Ok((
        nchw(&canvas, image_size),
        view_w as i32,
        view_h as i32,
        origin_x,
        origin_y,
    ))
}

fn infer_detections(
    model: &mut LoadedModel,
    data: Vec<f32>,
    image_size: usize,
    view_w: i32,
    view_h: i32,
    origin_x: i32,
    origin_y: i32,
    conf_threshold: f32,
) -> Result<Vec<Detection>, String> {
    let tensor = ort::value::Tensor::from_array(([1usize, 3, image_size, image_size], data))
        .map_err(|err| format!("Detect failed: {}", short_error(&err.to_string())))?;
    let outputs = model
        .session
        .run(ort::inputs![tensor])
        .map_err(|err| format!("Detect failed: {}", short_error(&err.to_string())))?;
    let (shape, values) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("Detect failed: {}", short_error(&err.to_string())))?;
    detections_from_output(
        values,
        shape,
        &model.names,
        model.end_to_end,
        OutputFrame {
            view_w,
            view_h,
            image_size: image_size as i32,
            origin_x,
            origin_y,
        },
        conf_threshold,
    )
}

fn run_model(
    path: &Path,
    rgb: &[u8],
    width: u32,
    height: u32,
    roi: Option<&Roi>,
    conf_threshold: f32,
) -> Result<(Vec<Detection>, Vec<String>), String> {
    let image_size = with_model(path, |model| Ok(model.image_size))?;
    let prepared = detect_view(rgb, width, height, roi, image_size)?;
    infer_prepared(path, image_size, prepared, conf_threshold)
}

fn infer_prepared(
    path: &Path,
    image_size: usize,
    prepared: (Vec<f32>, i32, i32, i32, i32),
    conf_threshold: f32,
) -> Result<(Vec<Detection>, Vec<String>), String> {
    let (data, view_w, view_h, origin_x, origin_y) = prepared;
    with_model(path, |model| {
        let retry = if model.provider == Provider::Cpu {
            None
        } else {
            Some(data.clone())
        };
        let found = match infer_detections(
            model,
            data,
            image_size,
            view_w,
            view_h,
            origin_x,
            origin_y,
            conf_threshold,
        ) {
            Ok(found) => found,
            Err(message) => {
                let Some(retry) = retry else {
                    return Err(message);
                };
                let failed = model.provider;
                let env = runtime()?;
                model.session = open_session(&env, path, Provider::Cpu)
                    .map_err(|cpu_err| format!("{message} {cpu_err}"))?;
                model.provider = Provider::Cpu;
                let detail = message
                    .strip_prefix("Detect failed: ")
                    .unwrap_or(message.as_str());
                inference_ep::remember(
                    model.requested,
                    Provider::Cpu,
                    format!(
                        "{} failed while detecting ({detail}). Using CPU.",
                        failed.label()
                    ),
                );
                infer_detections(
                    model,
                    retry,
                    image_size,
                    view_w,
                    view_h,
                    origin_x,
                    origin_y,
                    conf_threshold,
                )?
            }
        };
        // The same names this session used to label boxes. Empty gaps are dropped.
        Ok((found, listed_classes(model.names.clone())))
    })
}

#[cfg(test)]
fn detect_on_model(
    bytes: &[u8],
    path: &Path,
    roi: Option<&Roi>,
    conf_threshold: f32,
) -> Result<Value, String> {
    let (rgb, width, height) = decode_rgb(bytes)?;
    let (detections, classes) = run_model(path, &rgb, width, height, roi, conf_threshold)?;
    Ok(json!({
        "detections": detections,
        "classes": classes,
    }))
}

fn payload_bytes(text: &str) -> Result<Vec<u8>, String> {
    let text = text.trim();
    let encoded = if let Some(rest) = text.strip_prefix("data:") {
        let (_, data) = rest
            .split_once(',')
            .ok_or_else(|| "Could not read the image".to_string())?;
        data
    } else {
        text
    };
    let compact: String = encoded.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.is_empty() {
        return Err("Could not read the image".to_string());
    }
    let bytes = STANDARD
        .decode(compact.as_bytes())
        .map_err(|_| "Could not read the image".to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err("Could not read the image".to_string());
    }
    Ok(bytes)
}

fn raw_rgb(frame: &RawFrame) -> Result<(&[u8], u32, u32), String> {
    if frame.channels != 3 || frame.width == 0 || frame.height == 0 {
        return Err("Could not read the image".to_string());
    }
    let pixels = (frame.width as usize).saturating_mul(frame.height as usize);
    if pixels == 0 || pixels > MAX_PIXELS {
        return Err("Could not read the image".to_string());
    }
    let expected = pixels.saturating_mul(3);
    if frame.data.len() < expected {
        return Err("Could not read the image".to_string());
    }
    Ok((&frame.data[..expected], frame.width, frame.height))
}

fn detect_pixels(
    rgb: &[u8],
    width: u32,
    height: u32,
    capture_references: bool,
) -> Result<Value, String> {
    let root = match local_config::project_root() {
        Ok(root) => root,
        Err(message) => return Ok(failure(&message)),
    };
    let path = match active_onnx_path(&root) {
        Ok(path) => path,
        Err(message) => return Ok(failure(&model_status_message(&message))),
    };
    let document = local_config::load_document(&root).ok();
    let confidence = document
        .as_ref()
        .map(confidence_value)
        .unwrap_or(DEFAULT_CONFIDENCE);
    let plan = document
        .as_ref()
        .map(detect_plan)
        .unwrap_or(DetectPlan::FullFrame);
    // No slot is the full frame, in the same object the page already reads.
    // Saved slots that are all off return an empty list, not the full frame.
    let slots = match plan {
        DetectPlan::FullFrame => {
            return match run_model(&path, rgb, width, height, None, confidence) {
                Ok((detections, classes)) => {
                    let dinov2 =
                        log_dinov2_embeddings(rgb, width, height, &detections, capture_references);
                    Ok(json!({
                        "detections": detections,
                        "classes": classes,
                        "dinov2": dinov2,
                    }))
                }
                Err(message) => Ok(failure(&model_status_message(&message))),
            };
        }
        DetectPlan::Idle => return Ok(json!([])),
        DetectPlan::Slots(slots) => slots,
    };
    let mut rows = Vec::with_capacity(slots.len());
    for (slot, roi) in slots {
        match run_model(&path, rgb, width, height, Some(&roi), confidence) {
            Ok((detections, classes)) => {
                let dinov2 =
                    log_dinov2_embeddings(rgb, width, height, &detections, capture_references);
                rows.push(json!({
                    "slot": slot,
                    "detections": detections,
                    "classes": classes,
                    "dinov2": dinov2,
                }));
            }
            Err(message) => return Ok(failure(&model_status_message(&message))),
        }
    }
    Ok(Value::Array(rows))
}

/// Confidence and NMS have already dropped the other boxes. A missing DINOv2
/// session returns without a log and does not change these detections.
fn log_dinov2_embeddings(
    rgb: &[u8],
    width: u32,
    height: u32,
    detections: &[Detection],
    capture_references: bool,
) -> Vec<String> {
    if detections.is_empty() || !crate::dinov2::is_loaded() {
        return Vec::new();
    }
    let crops: Vec<crate::dinov2::DebugCrop<'_>> = detections
        .iter()
        .map(|item| crate::dinov2::DebugCrop {
            class_name: &item.ic,
            confidence: item.confidence,
            bbox: item.bbox,
        })
        .collect();
    crate::dinov2::debug_embed(rgb, width, height, &crops, capture_references)
}

fn detect_raw(frame: Option<RawFrame>, capture_references: bool) -> Result<Value, String> {
    let Some(frame) = frame else {
        return Ok(failure("No frame"));
    };
    let (rgb, width, height) = match raw_rgb(&frame) {
        Ok(rgb) => rgb,
        Err(message) => return Ok(failure(&message)),
    };
    detect_pixels(rgb, width, height, capture_references)
}

fn detect_encoded(text: &str, capture_references: bool) -> Result<Value, String> {
    let bytes = match payload_bytes(text) {
        Ok(bytes) => bytes,
        Err(message) => return Ok(failure(&message)),
    };
    let (rgb, width, height) = match decode_rgb(&bytes) {
        Ok(decoded) => decoded,
        Err(message) => return Ok(failure(&message)),
    };
    detect_pixels(&rgb, width, height, capture_references)
}

#[tauri::command]
pub async fn detect_image(
    state: tauri::State<'_, FrameState>,
    image: Option<String>,
    capture_references: Option<bool>,
) -> Result<Value, String> {
    // Test mode passes the chosen file. A live detect leaves this empty and
    // uses the raw frame stored by capture_snapshot. Saving references stores
    // each surviving box's embedding under its IC. It does not change the result.
    let capture_references = capture_references.unwrap_or(false);
    if let Some(text) = image.filter(|value| !value.trim().is_empty()) {
        return tauri::async_runtime::spawn_blocking(move || {
            detect_encoded(&text, capture_references)
        })
        .await
        .map_err(|_| "Detect failed".to_string())?;
    }
    let frame = {
        let guard = state.latest.lock().unwrap_or_else(|err| err.into_inner());
        guard.clone()
    };
    tauri::async_runtime::spawn_blocking(move || detect_raw(frame, capture_references))
        .await
        .map_err(|_| "Detect failed".to_string())?
}

const BENCHMARK_RUNS: usize = 5;

#[derive(Serialize)]
pub struct DetectBenchmark {
    pub provider: String,
    pub average_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub width: u32,
    pub height: u32,
    pub image: String,
}

/// Five timed runs of the active ONNX session on a fixed test image.
/// Letterbox and drawing are outside the timer. The active provider is the one
/// that finished the runs, including a CPU fallback.
#[tauri::command]
pub async fn benchmark_detect() -> Result<DetectBenchmark, String> {
    tauri::async_runtime::spawn_blocking(run_benchmark)
        .await
        .map_err(|_| "Benchmark failed.".to_string())?
}

fn run_benchmark() -> Result<DetectBenchmark, String> {
    let root = local_config::project_root()?;
    let model_path = active_onnx_path(&root)?;
    benchmark_files(&model_path, &benchmark_image(&root)?).map_err(|err| model_status_message(&err))
}

fn benchmark_image(root: &Path) -> Result<PathBuf, String> {
    pick_benchmark_image(&root.join("vision").join("test-images"))
}

fn benchmark_image_ext(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png")
    )
}

fn pick_benchmark_image(dir: &Path) -> Result<PathBuf, String> {
    let preferred = dir.join("Example3.jpg");
    if preferred.is_file() {
        return Ok(preferred);
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Err("No test image in vision/test-images.".to_string());
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && benchmark_image_ext(&path) {
            found.push(path);
        }
    }
    found.sort_by(|left, right| {
        let left_name = left
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let right_name = right
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        left_name
            .to_ascii_lowercase()
            .cmp(&right_name.to_ascii_lowercase())
    });
    found
        .into_iter()
        .next()
        .ok_or_else(|| "No test image in vision/test-images.".to_string())
}

fn summarize_times(times: &[f64]) -> Result<(f64, f64, f64), String> {
    let Some(first) = times.first().copied() else {
        return Err("Benchmark failed.".to_string());
    };
    if times.iter().any(|time| !time.is_finite() || *time < 0.0) {
        return Err("Benchmark failed.".to_string());
    }
    let mut min_ms = first;
    let mut max_ms = first;
    let mut sum = 0.0;
    for time in times {
        min_ms = min_ms.min(*time);
        max_ms = max_ms.max(*time);
        sum += time;
    }
    Ok((sum / times.len() as f64, min_ms, max_ms))
}

/// Session run plus the copy back to CPU. Tensor build is outside the timer.
fn timed_session(model: &mut LoadedModel, data: &[f32], image_size: usize) -> Result<f64, String> {
    let tensor =
        ort::value::Tensor::from_array(([1usize, 3, image_size, image_size], data.to_vec()))
            .map_err(|err| format!("Detect failed: {}", short_error(&err.to_string())))?;
    let started = std::time::Instant::now();
    let outputs = model
        .session
        .run(ort::inputs![tensor])
        .map_err(|err| format!("Detect failed: {}", short_error(&err.to_string())))?;
    let _values = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("Detect failed: {}", short_error(&err.to_string())))?;
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn benchmark_files(model_path: &Path, image_path: &Path) -> Result<DetectBenchmark, String> {
    let bytes = std::fs::read(image_path)
        .map_err(|_| format!("Could not read {}.", file_name(image_path)))?;
    let (rgb, width, height) = decode_rgb(&bytes)?;
    let image_size = with_model(model_path, |model| Ok(model.image_size))?;
    let (data, _view_w, _view_h, _origin_x, _origin_y) =
        detect_view(&rgb, width, height, None, image_size)?;
    let (provider, times) = with_model(model_path, |model| {
        let mut times = Vec::with_capacity(BENCHMARK_RUNS);
        while times.len() < BENCHMARK_RUNS {
            match timed_session(model, &data, image_size) {
                Ok(ms) => times.push(ms),
                Err(message) if model.provider != Provider::Cpu => {
                    let failed = model.provider;
                    let env = runtime()?;
                    model.session = open_session(&env, model_path, Provider::Cpu)
                        .map_err(|cpu_err| format!("{message} {cpu_err}"))?;
                    model.provider = Provider::Cpu;
                    let detail = message
                        .strip_prefix("Detect failed: ")
                        .unwrap_or(message.as_str());
                    inference_ep::remember(
                        model.requested,
                        Provider::Cpu,
                        format!(
                            "{} failed while detecting ({detail}). Using CPU.",
                            failed.label()
                        ),
                    );
                    times.clear();
                    times.push(timed_session(model, &data, image_size)?);
                }
                Err(message) => return Err(message),
            }
        }
        Ok((model.provider, times))
    })?;
    let (average_ms, min_ms, max_ms) = summarize_times(&times)?;
    Ok(DetectBenchmark {
        provider: provider.id().to_string(),
        average_ms,
        min_ms,
        max_ms,
        width,
        height,
        image: file_name(image_path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forget_session_drops_the_cached_model() {
        forget_session();
        let guard = MODEL_CACHE.lock().unwrap_or_else(|err| err.into_inner());
        assert!(guard.is_none());
    }

    #[test]
    fn py_round_matches_python_3_half_to_even() {
        assert_eq!(py_round(-0.5), 0);
        assert_eq!(py_round(0.5), 0);
        assert_eq!(py_round(1.5), 2);
        assert_eq!(py_round(2.5), 2);
        assert_eq!(py_round(-1.5), -2);
        assert_eq!(py_round(-2.5), -2);
        assert_eq!(py_round(79.9), 80);
        assert_eq!(py_round(-0.1), 0);
        assert_eq!(py_round(0.1), 0);
    }

    #[test]
    fn letterbox_of_example3_is_640_by_480_with_80_pixel_bars() {
        // Example3.jpg is 2016 wide and 1512 tall.
        let geometry = letterbox_geometry(2016, 1512, 640);
        assert_eq!(geometry.new_w, 640);
        assert_eq!(geometry.new_h, 480);
        assert_eq!(geometry.top, 80);
        assert_eq!(geometry.left, 0);
    }

    #[test]
    fn letterbox_of_a_wide_short_image_pads_the_top() {
        let geometry = letterbox_geometry(100, 50, 640);
        assert_eq!(geometry.new_w, 640);
        assert_eq!(geometry.new_h, 320);
        assert_eq!(geometry.top, 160);
        assert_eq!(geometry.left, 0);
    }

    #[test]
    fn scale_boxes_undoes_the_letterbox_pad() {
        // img0 is 100 tall and 200 wide. img1 is the 640 square.
        let scaled = scale_box(0.0, 160.0, 10.0, 170.0, 100, 200, 640);
        assert_eq!(scaled, [0, 0, 3, 3]);
    }

    #[test]
    fn nms_keeps_the_higher_score_and_a_different_class() {
        let kept = nms(vec![
            Candidate {
                x1: 0.0,
                y1: 0.0,
                x2: 10.0,
                y2: 10.0,
                score: 0.9,
                class_id: 0,
            },
            Candidate {
                x1: 0.0,
                y1: 0.0,
                x2: 10.0,
                y2: 10.0,
                score: 0.8,
                class_id: 0,
            },
            Candidate {
                x1: 0.0,
                y1: 0.0,
                x2: 10.0,
                y2: 10.0,
                score: 0.7,
                class_id: 1,
            },
        ]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].class_id, 0);
        assert!((kept[0].score - 0.9).abs() < f32::EPSILON);
        assert_eq!(kept[1].class_id, 1);
    }

    #[test]
    fn confidence_at_the_threshold_is_dropped() {
        let values = [
            10.0, 10.0, 4.0, 4.0, 0.25, 0.1, // box 0, conf == 0.25
            20.0, 20.0, 4.0, 4.0, 0.26, 0.1, // box 1
        ];
        // Layout (channels, boxes) = (6, 2) would be BCN. This slice is BNC, two rows.
        let found = candidates_from_output(&values, 6, 2, false, 0.25);
        assert_eq!(found.len(), 1);
        assert!((found[0].score - 0.26).abs() < 0.0001);
    }

    #[test]
    fn confidence_defaults_to_0_45_and_clamps_to_the_slider_range() {
        assert!((confidence_value(&Value::Null) - 0.45).abs() < 0.0001);
        assert!((confidence_value(&json!({"confidence": 0.30})) - 0.30).abs() < 0.0001);
        assert!((confidence_value(&json!({"confidence": 0.10})) - 0.15).abs() < 0.0001);
        assert!((confidence_value(&json!({"confidence": 0.99})) - 0.80).abs() < 0.0001);
        assert!((confidence_value(&json!({"confidence": "nope"})) - 0.45).abs() < 0.0001);
    }

    #[test]
    fn saved_confidence_drops_boxes_at_or_below_it() {
        let values = [
            10.0, 10.0, 4.0, 4.0, 0.40, 0.1, // box 0
            20.0, 20.0, 4.0, 4.0, 0.46, 0.1, // box 1
        ];
        let strict = candidates_from_output(&values, 6, 2, false, 0.45);
        assert_eq!(strict.len(), 1);
        assert!((strict[0].score - 0.46).abs() < 0.0001);
        let loose = candidates_from_output(&values, 6, 2, false, 0.25);
        assert_eq!(loose.len(), 2);
    }

    #[test]
    fn class_names_ignore_digits_inside_the_part_code() {
        let names = parse_class_names("{0: '2BBMNM08', 1: '2BB06B2SZ'}");
        assert_eq!(names, vec!["2BBMNM08".to_string(), "2BB06B2SZ".to_string()]);
        let later = parse_class_names("{0: '2BB10D2HZ', 12: '60ZCL11001'}");
        assert_eq!(later[0], "2BB10D2HZ");
        assert_eq!(later[12], "60ZCL11001");
        assert!(later[1].is_empty());
    }

    #[test]
    fn class_names_accept_a_json_object() {
        let names = parse_class_names(r#"{"0":"2BBMNM08","1":"2BB06B2SZ"}"#);
        assert_eq!(names, vec!["2BBMNM08".to_string(), "2BB06B2SZ".to_string()]);
    }

    fn proto_varint(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = value;
        loop {
            let mut byte = (rest & 0x7f) as u8;
            rest >>= 7;
            if rest != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if rest == 0 {
                break;
            }
        }
        out
    }

    fn proto_bytes(field: u32, bytes: &[u8]) -> Vec<u8> {
        let mut out = proto_varint((u64::from(field) << 3) | 2);
        out.extend(proto_varint(bytes.len() as u64));
        out.extend(bytes);
        out
    }

    fn proto_entry(key: &str, value: &str) -> Vec<u8> {
        let mut body = proto_bytes(1, key.as_bytes());
        body.extend(proto_bytes(2, value.as_bytes()));
        proto_bytes(14, &body)
    }

    #[test]
    fn onnx_metadata_lists_hinge_plate_and_logo() {
        let mut model = proto_varint(8);
        model.push(10);
        model.extend(proto_bytes(7, &[1, 2, 3, 4]));
        model.extend(proto_entry(
            "names",
            "{0: 'Hinge_Plate', 1: 'P02_Logo', 3: 'bolt'}",
        ));
        model.extend(proto_entry("args", "{'batch': 1}"));
        let found = classes_from_metadata("demo.onnx", &model);
        assert_eq!(found.message, "");
        assert_eq!(
            found.classes,
            vec![
                "Hinge_Plate".to_string(),
                "P02_Logo".to_string(),
                "bolt".to_string()
            ]
        );
    }

    #[test]
    fn onnx_without_class_metadata_has_no_class_list() {
        let model = proto_bytes(7, b"graph");
        let found = classes_from_metadata("plain.onnx", &model);
        assert!(found.classes.is_empty());
        assert_eq!(found.message, NO_CLASS_LIST);
        assert!(onnx_names_metadata(&[0xff, 0xff, 0xff]).is_none());
    }

    #[test]
    fn installed_onnx_exposes_its_class_list_when_present() {
        let Ok(appdata) = std::env::var("APPDATA") else {
            return;
        };
        let path = std::path::PathBuf::from(appdata)
            .join("studio.lean.ls-vision")
            .join("models")
            .join("best.onnx");
        if !path.is_file() {
            return;
        }
        let data = std::fs::read(&path).unwrap();
        let found = classes_from_metadata("best.onnx", &data);
        assert!(found.message.is_empty(), "{}", found.message);
        assert!(found.classes.len() >= 2, "{:?}", found.classes);
        assert!(found.classes.iter().any(|name| name == "2QHP01"));
        assert!(found.classes.iter().any(|name| name == "2QHP02"));
    }

    #[test]
    fn detect_roi_rejects_a_bool_and_clips_the_box() {
        assert!(clean_detect_roi(&Value::Null).is_none());
        assert!(clean_detect_roi(&json!({"x": true, "y": 0, "w": 0.5, "h": 0.5})).is_none());
        assert!(clean_detect_roi(&json!({"x": 0.0, "y": 0.0, "w": 0.001, "h": 0.5})).is_none());
        let roi = clean_detect_roi(&json!({"x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5})).unwrap();
        assert_eq!(
            roi,
            Roi {
                x: 0.25,
                y: 0.25,
                w: 0.5,
                h: 0.5
            }
        );
        assert_eq!(roi_slice(100, 200, &roi), (25, 50, 50, 100));
        // 0.125 * 8 = 1.0 exactly. 12.5 pixels on a 100-wide image rounds to 12.
        let half = Roi {
            x: 0.125,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        };
        assert_eq!(roi_slice(100, 100, &half).0, 12);
    }

    #[test]
    fn detect_slots_keep_a_single_roi_and_skip_blanks() {
        assert!(slots_from_document(&json!({})).is_empty());
        assert!(slots_from_document(&json!({ "detectRoi": null })).is_empty());
        let one = slots_from_document(&json!({
            "detectRoi": { "x": 0.1, "y": 0.2, "w": 0.3, "h": 0.4 }
        }));
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].0, 1);
        assert_eq!(one[0].1.x, 0.1);
        let many = slots_from_document(&json!({
            "detectRois": [
                { "x": 0.0, "y": 0.0, "w": 0.2, "h": 0.2 },
                null,
                { "x": 0.5, "y": 0.5, "w": 0.2, "h": 0.2 },
                { "x": true, "y": 0.0, "w": 0.2, "h": 0.2 }
            ],
            "detectRoi": { "x": 0.0, "y": 0.0, "w": 0.5, "h": 0.5 }
        }));
        assert_eq!(
            many.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
            vec![1, 3]
        );
        let only_second = slots_from_document(&json!({
            "detectRois": [null, { "x": 0.2, "y": 0.2, "w": 0.2, "h": 0.2 }, null, null]
        }));
        assert_eq!(only_second.len(), 1);
        assert_eq!(only_second[0].0, 2);
        let enabled = slots_from_document(&json!({
            "detectRois": [
                { "x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2, "enabled": false },
                { "x": 0.4, "y": 0.4, "w": 0.2, "h": 0.2, "enabled": true },
                { "x": 0.6, "y": 0.1, "w": 0.2, "h": 0.2 }
            ]
        }));
        assert_eq!(
            enabled.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(matches!(
            detect_plan(&json!({
                "detectRois": [
                    { "x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2, "enabled": false },
                    null,
                    null,
                    null
                ]
            })),
            DetectPlan::Idle
        ));
        assert!(matches!(
            detect_plan(&json!({ "detectRois": [null, null, null, null] })),
            DetectPlan::FullFrame
        ));
    }

    #[test]
    fn linear_resize_matches_the_opencv_tiny_golden() {
        let mut src = vec![0u8; 4 * 4 * 3];
        for (index, byte) in src.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let resized = resize_linear(&src, 4, 4, 3, 2);
        assert_eq!(
            resized,
            vec![
                6, 7, 8, 11, 12, 13, 15, 16, 17, 30, 31, 32, 35, 36, 37, 39, 40, 41
            ]
        );
    }

    #[test]
    fn payload_bytes_reads_a_data_url() {
        let bytes = payload_bytes("data:image/jpeg;base64,AQID").unwrap();
        assert_eq!(bytes, vec![1, 2, 3]);
        assert!(payload_bytes("  ").is_err());
        assert!(payload_bytes("data:image/jpeg;base64,@@@").is_err());
    }

    #[test]
    fn example3_onnx_boxes_match_python() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.parent().expect("project root");
        let model = root.join("vision").join("models").join("best.onnx");
        let image = root.join("vision").join("test-images").join("Example3.jpg");
        if !model.is_file() || !image.is_file() {
            eprintln!("skip: best.onnx or Example3.jpg is not in the checkout");
            return;
        }
        let bytes = std::fs::read(&image).unwrap();
        let (rgb, width, height) = decode_rgb(&bytes).unwrap();
        let sum: i64 = rgb.iter().map(|byte| i64::from(*byte)).sum();
        assert_eq!(
            (width, height, sum),
            (2016, 1512, 1_263_775_576),
            "JPEG decode does not match cv2 on Example3.jpg"
        );
        // The fixture was exported at conf 0.25. Live detection uses the saved value.
        let payload = detect_on_model(&bytes, &model, None, 0.25).unwrap();
        assert!(payload["classes"].is_array());
        let got: Vec<Detection> = serde_json::from_value(payload["detections"].clone()).unwrap();
        let fixture = manifest.join("tests").join("example3-onnx-detections.json");
        let expected: Vec<Detection> =
            serde_json::from_str(&std::fs::read_to_string(&fixture).unwrap()).unwrap();
        let mut got = got;
        let mut expected = expected;
        fn detection_key(item: &Detection) -> (String, [i32; 4], i64) {
            (
                item.ic.clone(),
                item.bbox,
                (item.confidence * 10_000.0).round() as i64,
            )
        }
        got.sort_by_key(detection_key);
        expected.sort_by_key(detection_key);
        assert_eq!(
            got.len(),
            expected.len(),
            "rust {} python {}",
            got.len(),
            expected.len()
        );
        let mut mismatches = Vec::new();
        for (index, (left, right)) in got.iter().zip(expected.iter()).enumerate() {
            let conf_delta = (left.confidence - right.confidence).abs();
            if (left.ic != right.ic || left.bbox != right.bbox || conf_delta > 0.001)
                && mismatches.len() < 8
            {
                mismatches.push(format!(
                    "{index}: rust {} {:?} {} python {} {:?} {}",
                    left.ic, left.bbox, left.confidence, right.ic, right.bbox, right.confidence
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} boxes differ, first: {mismatches:?}",
            mismatches.len().min(8)
        );
    }

    #[test]
    fn benchmark_image_prefers_example3_then_sorted_name() {
        let dir = std::env::temp_dir().join(format!("ls-vision-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let missing = pick_benchmark_image(&dir).unwrap_err();
        assert_eq!(missing, "No test image in vision/test-images.");
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("b.png"), b"x").unwrap();
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        let picked = pick_benchmark_image(&dir).unwrap();
        assert_eq!(picked.file_name().unwrap(), "a.jpg");
        std::fs::write(dir.join("Example3.jpg"), b"x").unwrap();
        let picked = pick_benchmark_image(&dir).unwrap();
        assert_eq!(picked.file_name().unwrap(), "Example3.jpg");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn benchmark_summary_is_average_min_and_max() {
        let (average, min_ms, max_ms) = summarize_times(&[10.0, 30.0, 20.0]).unwrap();
        assert_eq!(average, 20.0);
        assert_eq!(min_ms, 10.0);
        assert_eq!(max_ms, 30.0);
    }

    #[test]
    fn model_path_is_resolved_before_a_missing_file_can_run() {
        let dir = std::env::temp_dir().join(format!(
            "ls-vision-model-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let missing = resolved_onnx_model(&dir, "best.onnx").unwrap_err();
        assert_eq!(missing, MODEL_MISSING);
        std::fs::write(dir.join("best.onnx"), [0x08, 0x08, 0x12]).unwrap();
        let path = resolved_onnx_model(&dir, "best.onnx").unwrap();
        assert_eq!(path, dir.join("best.onnx"));
        let pt = resolved_onnx_model(&dir, "best.pt").unwrap_err();
        assert!(pt.contains("not an ONNX"), "{pt}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn model_status_message_stays_short() {
        assert_eq!(model_status_message(MODEL_MISSING), MODEL_MISSING);
        assert_eq!(
            model_status_message("best.onnx failed to load: bad protobuf"),
            MODEL_FAILED
        );
        assert_eq!(
            model_status_message("YOLO model is not loaded"),
            MODEL_FAILED
        );
        assert_eq!(
            model_status_message("Detect failed: unexpected output shape"),
            "Detect failed: unexpected output shape"
        );
    }

    #[test]
    fn roi_crop_is_applied_before_the_inference_tensor() {
        let width = 4usize;
        let height = 2usize;
        let mut rgb = vec![0u8; width * height * 3];
        for y in 0..height {
            for x in 2..width {
                let index = (y * width + x) * 3;
                rgb[index] = 255;
                rgb[index + 1] = 255;
                rgb[index + 2] = 255;
            }
        }
        let roi = Roi {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        };
        let (data, view_w, view_h, origin_x, origin_y) =
            detect_view(&rgb, width as u32, height as u32, Some(&roi), 8).unwrap();
        assert_eq!((view_w, view_h, origin_x, origin_y), (2, 2, 0, 0));
        assert_eq!(data.len(), 8 * 8 * 3);
        assert!(
            data.iter().all(|value| *value < 0.5),
            "the tensor still contains the uncropped half"
        );
    }

    #[test]
    fn benchmark_example3_reports_resolution_and_times() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.parent().expect("project root");
        let model = root.join("vision").join("models").join("best.onnx");
        let image = root.join("vision").join("test-images").join("Example3.jpg");
        if !model.is_file() || !image.is_file() {
            eprintln!("skip: best.onnx or Example3.jpg is not in the checkout");
            return;
        }
        let report = benchmark_files(&model, &image).unwrap();
        assert_eq!(report.image, "Example3.jpg");
        assert_eq!((report.width, report.height), (2016, 1512));
        assert!(report.min_ms <= report.average_ms && report.average_ms <= report.max_ms);
        assert!(report.average_ms > 0.0);
        assert!(matches!(
            report.provider.as_str(),
            "cpu" | "directml" | "cuda"
        ));
        eprintln!(
            "benchmark provider={} average_ms={:.1} min_ms={:.1} max_ms={:.1} {}x{}",
            report.provider,
            report.average_ms,
            report.min_ms,
            report.max_ms,
            report.width,
            report.height
        );
    }

    #[test]
    fn example3_logs_one_dinov2_line_per_surviving_box() {
        let _hold = crate::dinov2::lock_for_test();
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.parent().expect("project root");
        let model = root.join("vision").join("models").join("best.onnx");
        let image = root.join("vision").join("test-images").join("Example3.jpg");
        let dino = manifest.join("models").join("dinov2_vits14.onnx");
        if !model.is_file() || !image.is_file() || !dino.is_file() {
            eprintln!("skip: best.onnx, Example3.jpg, or dinov2_vits14.onnx is missing");
            return;
        }
        crate::dinov2::load_at(&dino);
        assert!(crate::dinov2::is_loaded());
        let bytes = std::fs::read(&image).unwrap();
        let (rgb, width, height) = decode_rgb(&bytes).unwrap();
        let (detections, _) = run_model(&model, &rgb, width, height, None, 0.45).unwrap();
        assert!(!detections.is_empty(), "Example3 produced no boxes at 0.45");
        let crops: Vec<crate::dinov2::DebugCrop<'_>> = detections
            .iter()
            .map(|item| crate::dinov2::DebugCrop {
                class_name: &item.ic,
                confidence: item.confidence,
                bbox: item.bbox,
            })
            .collect();
        let lines = crate::dinov2::debug_embed(&rgb, width, height, &crops, false);
        assert_eq!(lines.len(), detections.len());
        for (line, item) in lines.iter().zip(&detections) {
            let prefix = format!("DINOv2 {} conf {:.4} norm ", item.ic, item.confidence);
            assert!(line.starts_with(&prefix), "{line}");
            assert!(!line.contains("embed failed"), "{line}");
        }
    }

    fn inset_rgb(rgb: &[u8], width: u32, height: u32, margin: u32) -> (Vec<u8>, u32, u32) {
        if margin == 0 {
            return (rgb.to_vec(), width, height);
        }
        let x0 = margin as usize;
        let y0 = margin as usize;
        let crop_w = width as usize - margin as usize * 2;
        let crop_h = height as usize - margin as usize * 2;
        let frame_w = width as usize;
        let mut pixels = Vec::with_capacity(crop_w * crop_h * 3);
        for row in 0..crop_h {
            let start = ((y0 + row) * frame_w + x0) * 3;
            pixels.extend_from_slice(&rgb[start..start + crop_w * 3]);
        }
        (pixels, crop_w as u32, crop_h as u32)
    }

    fn jpeg_rgb(rgb: &[u8], width: u32, height: u32, quality: i32) -> Vec<u8> {
        let image = turbojpeg::Image {
            pixels: rgb,
            width: width as usize,
            pitch: (width as usize).saturating_mul(3),
            height: height as usize,
            format: turbojpeg::PixelFormat::RGB,
        };
        turbojpeg::compress(image, quality, turbojpeg::Subsamp::Sub2x2)
            .expect("jpeg")
            .as_ref()
            .to_vec()
    }

    fn logged_match(line: &str, class_name: &str) -> Option<f32> {
        let head = format!("DINOv2 {class_name} ");
        if !line.starts_with(&head) {
            return None;
        }
        let token = line.split(" match ").nth(1)?.split_whitespace().next()?;
        token.parse().ok()
    }

    #[test]
    fn example3_reference_scores_the_same_class_above_a_different_one() {
        struct ClearReferences;
        impl Drop for ClearReferences {
            fn drop(&mut self) {
                crate::dinov2::clear_references();
            }
        }
        let _hold = crate::dinov2::lock_for_test();
        let _clear = ClearReferences;
        crate::dinov2::clear_references();

        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.parent().expect("project root");
        let model = root.join("vision").join("models").join("best.onnx");
        let image = root.join("vision").join("test-images").join("Example3.jpg");
        let dino = manifest.join("models").join("dinov2_vits14.onnx");
        if !model.is_file() || !image.is_file() || !dino.is_file() {
            eprintln!("skip: best.onnx, Example3.jpg, or dinov2_vits14.onnx is missing");
            return;
        }
        crate::dinov2::load_at(&dino);
        assert!(crate::dinov2::is_loaded());
        let bytes = std::fs::read(&image).unwrap();
        let (rgb, width, height) = decode_rgb(&bytes).unwrap();
        let (detections, _) = run_model(&model, &rgb, width, height, None, 0.45).unwrap();
        assert!(
            detections.len() >= 2,
            "Example3 produced fewer than two boxes at 0.45"
        );

        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for item in &detections {
            *counts.entry(item.ic.as_str()).or_default() += 1;
        }
        let reference_ic = counts
            .iter()
            .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
            .map(|(name, _)| (*name).to_string())
            .expect("a class");
        let reference = detections
            .iter()
            .filter(|item| item.ic == reference_ic)
            .max_by(|left, right| left.confidence.total_cmp(&right.confidence))
            .expect("reference box");
        let wrong = detections
            .iter()
            .filter(|item| item.ic != reference_ic)
            .max_by(|left, right| left.confidence.total_cmp(&right.confidence))
            .expect("Example3 needs a second class");

        let stored = crate::dinov2::debug_embed(
            &rgb,
            width,
            height,
            &[crate::dinov2::DebugCrop {
                class_name: &reference.ic,
                confidence: reference.confidence,
                bbox: reference.bbox,
            }],
            true,
        );
        assert!(
            stored
                .iter()
                .any(|line| line == &format!("DINOv2 reference {reference_ic} stored 1")),
            "{stored:?}"
        );

        let wrong_values = crate::dinov2::embed_debug_crop(
            &rgb,
            width,
            height,
            &crate::dinov2::DebugCrop {
                class_name: &wrong.ic,
                confidence: wrong.confidence,
                bbox: wrong.bbox,
            },
        )
        .expect("wrong-class embed");
        let wrong_score = crate::dinov2::best_similarity(&reference_ic, &wrong_values)
            .expect("reference was stored");

        // Same kit, a second file: inset the photo, then fall back to a recompress.
        let attempts = [(48_u32, 80_i32), (16, 90), (0, 75)];
        let mut best_same: Option<f32> = None;
        for (margin, quality) in attempts {
            if margin > 0 && (width <= margin * 2 + 32 || height <= margin * 2 + 32) {
                continue;
            }
            let (crop, crop_w, crop_h) = inset_rgb(&rgb, width, height, margin);
            let jpeg = jpeg_rgb(&crop, crop_w, crop_h, quality);
            let (second, second_w, second_h) = decode_rgb(&jpeg).expect("second image");
            let (found, _) =
                run_model(&model, &second, second_w, second_h, None, 0.45).expect("second detect");
            if !found.iter().any(|item| item.ic == reference_ic) {
                eprintln!("DINOv2 second image margin {margin} lost {reference_ic}");
                continue;
            }
            let crops: Vec<crate::dinov2::DebugCrop<'_>> = found
                .iter()
                .map(|item| crate::dinov2::DebugCrop {
                    class_name: &item.ic,
                    confidence: item.confidence,
                    bbox: item.bbox,
                })
                .collect();
            let lines = crate::dinov2::debug_embed(&second, second_w, second_h, &crops, false);
            best_same = lines
                .iter()
                .filter_map(|line| logged_match(line, &reference_ic))
                .reduce(f32::max);
            if best_same.is_some() {
                eprintln!("DINOv2 second image margin {margin} quality {quality}");
                break;
            }
        }
        let best_same = best_same.expect("the second image logged no match for the reference IC");
        eprintln!(
            "DINOv2 match {reference_ic} same {best_same:.4} other {} {wrong_score:.4}",
            wrong.ic
        );
        assert!(
            best_same > wrong_score,
            "same-class {best_same:.4} did not beat {reference_ic} vs {} {wrong_score:.4}",
            wrong.ic
        );
    }
}
