//! Ultralytics square letterbox for the YOLO ONNX input.
//!
//! The unpadded size is rounded first. The leftover pixels are split with the
//! same `(pad - 0.1).round()` top/left assignment Ultralytics uses. Boxes are
//! mapped back with that gain. Millimetres are computed only from native
//! sensor pixels, after `unpad_*`.

use serde::{Deserialize, Serialize};

pub const PAD_RGB: u8 = 114;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LetterboxTransform {
    pub orig_w: f32,
    pub orig_h: f32,
    pub target_size: f32, // e.g. 1280.0
    pub gain: f32,
    pub pad_x: f32, // Left padding in 1280x1280 tensor space
    pub pad_y: f32, // Top padding in 1280x1280 tensor space
    pub scaled_w: f32,
    pub scaled_h: f32,
}

impl LetterboxTransform {
    pub fn new(orig_w: u32, orig_h: u32, target_size: f32) -> Self {
        let (w, h) = (orig_w as f32, orig_h as f32);

        // 1. Uniform scaling ratio
        let gain = (target_size / w).min(target_size / h);

        // 2. Unpadded scaled dimensions (round first, matching Ultralytics)
        let scaled_w = (w * gain).round();
        let scaled_h = (h * gain).round();

        // 3. Half-padding computation
        let dw = (target_size - scaled_w) / 2.0;
        let dh = (target_size - scaled_h) / 2.0;

        // 4. Asymmetric split rounding (-0.1 / +0.1) to match Ultralytics top/left assignment
        let pad_x = (dw - 0.1).round().max(0.0);
        let pad_y = (dh - 0.1).round().max(0.0);

        Self {
            orig_w: w,
            orig_h: h,
            target_size,
            gain,
            pad_x,
            pad_y,
            scaled_w,
            scaled_h,
        }
    }

    /// Map a point (x, y) from 1280x1280 tensor space back to native camera pixel space.
    /// Strictly clamped to [0, orig_w] and [0, orig_h].
    #[inline]
    pub fn unpad_point(&self, x: f32, y: f32) -> (f32, f32) {
        let native_x = ((x - self.pad_x) / self.gain).clamp(0.0, self.orig_w);
        let native_y = ((y - self.pad_y) / self.gain).clamp(0.0, self.orig_h);
        (native_x, native_y)
    }

    /// Map a bounding box [x1, y1, x2, y2] back to native camera space.
    pub fn unpad_bbox(&self, bbox: [f32; 4]) -> [f32; 4] {
        let (x1, y1) = self.unpad_point(bbox[0], bbox[1]);
        let (x2, y2) = self.unpad_point(bbox[2], bbox[3]);
        [x1, y1, x2, y2]
    }

    /// Map polygon points [[x, y], ...] back to native camera space.
    pub fn unpad_polygon(&self, poly: &[[f32; 2]]) -> Vec<[f32; 2]> {
        poly.iter()
            .map(|pt| {
                let (nx, ny) = self.unpad_point(pt[0], pt[1]);
                [nx, ny]
            })
            .collect()
    }
}

/// How RGB bytes become the float32 NCHW tensor Ultralytics exports expect.
/// The default is [`TensorNormalization::ZeroToOne`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TensorNormalization {
    /// `val / 255.0`. Standard Ultralytics export.
    #[default]
    ZeroToOne,
    /// `val`. Models that normalise inside the graph.
    ZeroTo255,
}

impl TensorNormalization {
    /// `zeroToOne` / `0-1`, or `zeroTo255` / `0-255`. Anything else is ignored.
    pub fn parse(value: &str) -> Option<Self> {
        let key = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        match key.as_str() {
            "zerotoone" | "zero-to-one" | "0-1" | "0to1" | "unit" | "/255" | "div255"
            | "ultralytics" => Some(Self::ZeroToOne),
            "zeroto255" | "zero-to-255" | "0-255" | "0to255" | "uint8" | "pixel" => {
                Some(Self::ZeroTo255)
            }
            _ => None,
        }
    }

    fn limits(self) -> (f32, f32) {
        match self {
            Self::ZeroToOne => (0.0, 1.0),
            Self::ZeroTo255 => (0.0, 255.0),
        }
    }
}

/// Metadata wins only when it names a known mode. A missing key stays `[0, 1]`.
pub fn normalization_from_metadata<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> TensorNormalization {
    for (key, value) in entries {
        let key = key.trim().to_ascii_lowercase().replace(['_', ' '], "");
        if (key == "normalization" || key == "tensornormalization")
            && let Some(mode) = TensorNormalization::parse(value)
        {
            return mode;
        }
    }
    TensorNormalization::ZeroToOne
}

/// Millimetres from a length that is already in native sensor pixels.
/// Tensor-space distances are not a valid input. Unpad first.
#[cfg(test)]
pub(crate) fn millimetres_from_sensor_px(native_px: f32, mm_per_px: f32) -> Option<f32> {
    if !native_px.is_finite() || !mm_per_px.is_finite() || mm_per_px <= 0.0 {
        return None;
    }
    let mm = native_px.abs() * mm_per_px;
    if mm.is_finite() { Some(mm) } else { None }
}

fn rounded_pixels(value: f32) -> Result<usize, String> {
    if !value.is_finite() || !(1.0..=100_000.0).contains(&value) {
        return Err("Could not read the image".to_string());
    }
    Ok(value.round() as usize)
}

/// Gray canvas, then the resized frame pasted at `(pad_x, pad_y)`.
pub fn paint_letterbox(
    resized: &[u8],
    scaled_w: u32,
    scaled_h: u32,
    transform: &LetterboxTransform,
) -> Result<Vec<u8>, String> {
    let size = rounded_pixels(transform.target_size)?;
    if scaled_w < 1 || scaled_h < 1 {
        return Err("Could not read the image".to_string());
    }
    let new_w = scaled_w as usize;
    let new_h = scaled_h as usize;
    let expected = new_w.saturating_mul(new_h).saturating_mul(3);
    if resized.len() < expected || size > 4096 {
        return Err("Could not read the image".to_string());
    }
    let mut canvas = vec![PAD_RGB; size * size * 3];
    let top = transform.pad_y.max(0.0) as usize;
    let left = transform.pad_x.max(0.0) as usize;
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

/// Float32 planar CHW. Non-finite values and values outside the chosen range are refused.
pub fn planar_tensor(
    canvas: &[u8],
    target_size: u32,
    normalization: TensorNormalization,
) -> Result<Vec<f32>, String> {
    let size = target_size as usize;
    let plane = size.saturating_mul(size);
    if target_size < 1 || canvas.len() < plane.saturating_mul(3) {
        return Err("Could not read the image".to_string());
    }
    let scale = match normalization {
        TensorNormalization::ZeroToOne => 1.0 / 255.0,
        TensorNormalization::ZeroTo255 => 1.0,
    };
    let mut data = vec![0f32; 3 * plane];
    for index in 0..plane {
        let pixel = index * 3;
        for channel in 0..3 {
            let value = f32::from(canvas[pixel + channel]) * scale;
            data[channel * plane + index] = value;
        }
    }
    verify_input_tensor(&data, size, normalization)?;
    Ok(data)
}

/// `[1, 3, S, S]` float32 in the range the loaded export expects.
pub fn verify_input_tensor(
    data: &[f32],
    target_size: usize,
    normalization: TensorNormalization,
) -> Result<(), String> {
    let plane = target_size.saturating_mul(target_size);
    let expected = plane.saturating_mul(3);
    if target_size < 1 || data.len() != expected {
        return Err("Detect failed: input tensor shape is not 1x3xSxS".to_string());
    }
    if data.iter().any(|value| !value.is_finite()) {
        return Err("Detect failed: input tensor is not finite".to_string());
    }
    let (low, high) = normalization.limits();
    let eps = 1e-4;
    if data
        .iter()
        .any(|value| *value < low - eps || *value > high + eps)
    {
        return Err("Detect failed: input tensor is outside the model range".to_string());
    }
    Ok(())
}

/// Letterbox `rgb`, then build the ONNX tensor. `resize` is bilinear (OpenCV `INTER_LINEAR`).
pub fn yolo_tensor(
    rgb: &[u8],
    width: u32,
    height: u32,
    target_size: u32,
    normalization: TensorNormalization,
    resize: impl FnOnce(&[u8], usize, usize, usize, usize) -> Vec<u8>,
) -> Result<(Vec<f32>, LetterboxTransform), String> {
    if !(1..=4096).contains(&target_size) || width < 1 || height < 1 {
        return Err("Could not read the image".to_string());
    }
    let pixels = (width as usize).saturating_mul(height as usize);
    if rgb.len() < pixels.saturating_mul(3) {
        return Err("Could not read the image".to_string());
    }
    let transform = LetterboxTransform::new(width, height, target_size as f32);
    let scaled_w = rounded_pixels(transform.scaled_w)?;
    let scaled_h = rounded_pixels(transform.scaled_h)?;
    let resized = resize(rgb, width as usize, height as usize, scaled_w, scaled_h);
    let canvas = paint_letterbox(&resized, scaled_w as u32, scaled_h as u32, &transform)?;
    let data = planar_tensor(&canvas, target_size, normalization)?;
    Ok((data, transform))
}

/// One detection after unpad, in native sensor pixels.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DetectedItem {
    pub ic: String,
    pub confidence: f64,
    pub bbox: [f32; 4],
    /// Polygon points in the same sensor pixel space. Empty when the head has no mask.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub polygon: Vec<[f32; 2]>,
}

/// IPC body for one letterboxed view. `items` are already in sensor pixels.
#[derive(Clone, Debug, Serialize)]
pub struct DetectionResponse {
    pub items: Vec<DetectedItem>,
    pub transform: LetterboxTransform,
}

#[cfg(test)]
#[path = "tests/letterbox_tests.rs"]
mod tests;
