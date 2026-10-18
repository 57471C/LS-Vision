//! ONNX detection for the operator page.
//!
//! `detect_image` returns the same JSON as POST /detect: `detections` of
//! `{ ic, confidence, bbox }` and `image_base64` of the original frame.
//! This command reads `.onnx` only. A `.pt` name is a training file; the page warns and does not detect.
//!
//! Boxes are checked against Ultralytics `YOLO(best.onnx)`, not `YOLO(best.pt)`.
//! Those two already disagree on the same photo. JPEG bytes are decoded with
//! TurboJPEG so the pixels match `cv2.imdecode`. The resize matches the OpenCV
//! 8-bit `INTER_LINEAR` path used by the installed sidecar.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::inference_ep::{self, Provider};
use crate::local_config;

const CONF_THRESHOLD: f32 = 0.25;
const IOU_THRESHOLD: f32 = 0.7;
const MAX_DET: usize = 300;
const MAX_NMS: usize = 30_000;
const MAX_WH: f32 = 7680.0;
const DEFAULT_IMAGE_SIZE: usize = 640;
const PAD_VALUE: u8 = 114;
const INTER_RESIZE_COEF_SCALE: i32 = 2048;
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PIXELS: usize = 40_000_000;

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
    } else if (floor as i32) % 2 == 0 {
        floor
    } else {
        floor + 1.0
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

fn configured_roi(root: &Path) -> Option<Roi> {
    let document = local_config::load_document(root).ok()?;
    document.get("detectRoi").and_then(clean_detect_roi)
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

/// OpenCV 4.10 / this sidecar's `cv2.resize(..., INTER_LINEAR)` for 8-bit pixels.
///
/// Horizontal sums stay in `i32` (`pixel * 11-bit coeff`). The 8-bit vertical
/// step is the specialized form, not `(sum + bias) >> 22`:
/// `((b0 * (row >> 4)) >> 16 + (b1 * (row >> 4)) >> 16 + 2) >> 2`.
fn resize_linear(src: &[u8], src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Vec<u8> {
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

struct Letterbox {
    new_w: i32,
    new_h: i32,
    top: i32,
    left: i32,
}

fn letterbox_geometry(width: i32, height: i32, size: i32) -> Letterbox {
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
        if !(best_score > CONF_THRESHOLD) || !best_score.is_finite() {
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
    view_w: i32,
    view_h: i32,
    image_size: i32,
    origin_x: i32,
    origin_y: i32,
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
        end_to_end_candidates(values, channels, boxes, bcn)
    } else {
        nms(candidates_from_output(values, channels, boxes, bcn))
    };
    let mut detections = Vec::with_capacity(chosen.len());
    for item in chosen {
        let scaled = scale_box(
            item.x1, item.y1, item.x2, item.y2, view_h, view_w, image_size,
        );
        detections.push(Detection {
            ic: class_name(names, item.class_id),
            confidence: round_places(f64::from(item.score), 4),
            bbox: [
                scaled[0] + origin_x,
                scaled[1] + origin_y,
                scaled[2] + origin_x,
                scaled[3] + origin_y,
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
        if !(score > CONF_THRESHOLD) || !score.is_finite() || class_id < 0.0 {
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

/// libjpeg-turbo, same family as the sidecar's `cv2.imdecode`.
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

fn encode_jpeg(rgb: &[u8], width: u32, height: u32) -> Result<String, String> {
    let mut bytes = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut bytes, 80);
    encoder
        .write_image(rgb, width, height, ExtendedColorType::Rgb8)
        .map_err(|_| "Could not encode the image".to_string())?;
    Ok(STANDARD.encode(bytes))
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

fn runtime() -> Result<ort::environment::Environment, String> {
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

fn open_session(
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

/// Drop the cached session and load the active `.onnx` model with the saved provider.
pub(crate) fn reload_session() -> Result<(), String> {
    {
        let mut guard = MODEL_CACHE.lock().unwrap_or_else(|err| err.into_inner());
        *guard = None;
    }
    let requested = inference_ep::selected_provider();
    let Ok(root) = local_config::project_root() else {
        inference_ep::remember(requested, requested, "");
        return Ok(());
    };
    let name = match local_config::configured_model_name(&root) {
        Ok(name) => name,
        Err(_) => {
            inference_ep::remember(requested, requested, "");
            return Ok(());
        }
    };
    if !name.to_ascii_lowercase().ends_with(".onnx") {
        inference_ep::remember(requested, requested, "");
        return Ok(());
    }
    let path = local_config::vision_models_dir(&root).join(&name);
    if !path.is_file() {
        inference_ep::remember(requested, requested, "");
        return Ok(());
    }
    with_model(&path, |_| Ok(()))
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
        view_w,
        view_h,
        image_size as i32,
        origin_x,
        origin_y,
    )
}

fn run_model(
    path: &Path,
    rgb: &[u8],
    width: u32,
    height: u32,
    roi: Option<&Roi>,
) -> Result<Vec<Detection>, String> {
    let image_size = with_model(path, |model| Ok(model.image_size))?;
    let (data, view_w, view_h, origin_x, origin_y) =
        detect_view(rgb, width, height, roi, image_size)?;
    with_model(path, |model| {
        let retry = if model.provider == Provider::Cpu {
            None
        } else {
            Some(data.clone())
        };
        match infer_detections(model, data, image_size, view_w, view_h, origin_x, origin_y) {
            Ok(found) => Ok(found),
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
                infer_detections(model, retry, image_size, view_w, view_h, origin_x, origin_y)
            }
        }
    })
}

fn detect_on_model(bytes: &[u8], path: &Path, roi: Option<&Roi>) -> Result<Value, String> {
    let (rgb, width, height) = decode_rgb(bytes)?;
    let detections = run_model(path, &rgb, width, height, roi)?;
    let encoded = encode_jpeg(&rgb, width, height)?;
    Ok(json!({
        "detections": detections,
        "image_base64": encoded,
    }))
}

#[tauri::command]
pub fn detect_image(image: Option<String>) -> Result<Value, String> {
    let root = match local_config::project_root() {
        Ok(root) => root,
        Err(message) => return Ok(failure(&message)),
    };
    let name = match local_config::configured_model_name(&root) {
        Ok(name) => name,
        Err(message) => return Ok(failure(&message)),
    };
    if !name.to_ascii_lowercase().ends_with(".onnx") {
        return Ok(failure(&format!("{name} is not an ONNX file.")));
    }
    let path = local_config::vision_models_dir(&root).join(&name);
    if !path.is_file() {
        return Ok(failure(&format!("{name} was not found in vision/models.")));
    }
    let Some(text) = image.filter(|value| !value.trim().is_empty()) else {
        return Ok(failure("No image and no snapshot"));
    };
    let bytes = match payload_bytes(&text) {
        Ok(bytes) => bytes,
        Err(message) => return Ok(failure(&message)),
    };
    let roi = configured_roi(&root);
    match detect_on_model(&bytes, &path, roi.as_ref()) {
        Ok(payload) => Ok(payload),
        Err(message) => Ok(failure(&message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let found = candidates_from_output(&values, 6, 2, false);
        assert_eq!(found.len(), 1);
        assert!((found[0].score - 0.26).abs() < 0.0001);
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
        let payload = detect_on_model(&bytes, &model, None).unwrap();
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
            if left.ic != right.ic || left.bbox != right.bbox || conf_delta > 0.001 {
                if mismatches.len() < 8 {
                    mismatches.push(format!(
                        "{index}: rust {} {:?} {} python {} {:?} {}",
                        left.ic, left.bbox, left.confidence, right.ic, right.bbox, right.confidence
                    ));
                }
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} boxes differ, first: {mismatches:?}",
            mismatches.len().min(8)
        );
    }
}
