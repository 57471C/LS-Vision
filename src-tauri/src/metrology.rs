//! Optional caliper pass after a catalogue match.
//!
//! Settings > Capture stores `metrology` in `vision/config.local.json`. When
//! that switch is off, detection is unchanged. When it is on, each box that
//! resolved to a catalogue part (`matched_bom` or `wrong_part`) is cropped
//! from the full-resolution frame, measured, and given a `metrology` flag.
//! An unrecognized box is left alone. The flag does not change the two-pass
//! status, the BoM count, or the YOLO label file.
//!
//! The crop is expanded by the saved margin, then OpenCV turns it grey,
//! thresholds it (Otsu or a fixed cutoff), keeps the largest external contour,
//! and fits a min-area rectangle. The long side is the length and the short
//! side is the diameter. Pixels become millimetres with the camera scale,
//! adjusted by the frame width the same way Measure does.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use opencv::core::{Mat, Point, RotatedRect, Vec3b, Vector};

const DEFAULT_MARGIN: f64 = 5.0;
const DEFAULT_FIXED: u8 = 128;
const DEFAULT_MIN_AREA: f64 = 200.0;
const DEFAULT_LENGTH_TOLERANCE: f64 = 1.5;
const DEFAULT_DIAMETER_TOLERANCE: f64 = 0.8;
/// A contour that fills the crop is the background, not the part.
const MAX_PART_FRACTION: f64 = 0.92;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThresholdMode {
    Otsu,
    Fixed,
}

#[derive(Clone, Debug)]
pub(crate) struct MetrologySettings {
    pub enabled: bool,
    pub margin_percent: f64,
    pub mode: ThresholdMode,
    pub fixed_threshold: u8,
    pub min_contour_area_px: f64,
    pub length_tolerance_mm: f64,
    pub diameter_tolerance_mm: f64,
}

impl MetrologySettings {
    fn disabled() -> Self {
        Self {
            enabled: false,
            margin_percent: DEFAULT_MARGIN,
            mode: ThresholdMode::Otsu,
            fixed_threshold: DEFAULT_FIXED,
            min_contour_area_px: DEFAULT_MIN_AREA,
            length_tolerance_mm: DEFAULT_LENGTH_TOLERANCE,
            diameter_tolerance_mm: DEFAULT_DIAMETER_TOLERANCE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetrologyStatus {
    #[serde(rename = "pass")]
    Pass,
    #[serde(rename = "DIMENSION_MISMATCH")]
    DimensionMismatch,
}

/// Measured length and diameter for one matched box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetrologyResult {
    pub status: MetrologyStatus,
    pub length_mm: f64,
    pub diameter_mm: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_length_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_diameter_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expected_label: String,
    /// Empty on a pass, so the overlay adds nothing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug)]
pub(crate) struct BoxMeasure {
    pub bbox: [i32; 4],
    pub matched: bool,
    pub ic: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ExpectedSize {
    pub diameter_mm: Option<f64>,
    pub length_mm: Option<f64>,
    pub description: String,
}

struct CameraScale {
    mm_per_px: f64,
    width: f64,
}

pub(crate) fn settings_from(document: Option<&Value>) -> MetrologySettings {
    let Some(value) = document.and_then(|document| document.get("metrology")) else {
        return MetrologySettings::disabled();
    };
    parse_settings(value)
}

fn parse_settings(value: &Value) -> MetrologySettings {
    let Some(map) = value.as_object() else {
        return MetrologySettings::disabled();
    };
    let mode = map
        .get("thresholdMode")
        .and_then(Value::as_str)
        .map(|text| text.trim().to_ascii_lowercase());
    MetrologySettings {
        enabled: map.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        margin_percent: clamp_number(map.get("marginPercent"), 0.0, 100.0, DEFAULT_MARGIN),
        mode: if mode.as_deref() == Some("fixed") {
            ThresholdMode::Fixed
        } else {
            ThresholdMode::Otsu
        },
        fixed_threshold: clamp_u8(map.get("fixedThreshold"), DEFAULT_FIXED),
        min_contour_area_px: clamp_number(
            map.get("minContourAreaPx"),
            0.0,
            1_000_000.0,
            DEFAULT_MIN_AREA,
        ),
        length_tolerance_mm: clamp_number(
            map.get("lengthToleranceMm"),
            0.0,
            1_000.0,
            DEFAULT_LENGTH_TOLERANCE,
        ),
        diameter_tolerance_mm: clamp_number(
            map.get("diameterToleranceMm"),
            0.0,
            1_000.0,
            DEFAULT_DIAMETER_TOLERANCE,
        ),
    }
}

fn clamp_number(value: Option<&Value>, min: f64, max: f64, fallback: f64) -> f64 {
    let Some(number) = value.and_then(Value::as_f64) else {
        return fallback;
    };
    if !number.is_finite() {
        return fallback;
    }
    number.clamp(min, max)
}

fn clamp_u8(value: Option<&Value>, fallback: u8) -> u8 {
    let Some(number) = value.and_then(Value::as_f64) else {
        return fallback;
    };
    if !number.is_finite() {
        return fallback;
    }
    let rounded = number.round();
    if !(0.0..=255.0).contains(&rounded) {
        return fallback;
    }
    rounded as u8
}

#[derive(Clone, Copy)]
enum Axis {
    Diameter,
    InnerDiameter,
    Length,
}

/// Catalogue dimensions for one item code. A string such as `M6x40` is read
/// as diameter then length. An object uses `diameterMm` and `lengthMm`.
/// A list of `{ "name", "mm" }` uses OD for the diameter and h or length
/// for the length, which is how the imported catalogue stores them.
pub(crate) fn expected_from(description: &str, dimensions: &Value) -> ExpectedSize {
    let mut diameter = None;
    let mut length = None;
    match dimensions {
        Value::Object(map) => absorb_map(map, &mut diameter, &mut length),
        Value::Array(items) => {
            for item in items {
                if let Some(map) = item.as_object() {
                    absorb_map(map, &mut diameter, &mut length);
                }
            }
        }
        Value::String(text) => {
            if let Some((next_diameter, next_length)) = parse_size_text(text) {
                diameter = Some(next_diameter);
                length = Some(next_length);
            }
        }
        _ => {}
    }
    if diameter.is_none() && length.is_none() {
        if let Some((next_diameter, next_length)) = parse_size_text(description) {
            diameter = Some(next_diameter);
            length = Some(next_length);
        }
    }
    ExpectedSize {
        diameter_mm: diameter,
        length_mm: length,
        description: description.trim().to_string(),
    }
}

fn absorb_map(map: &Map<String, Value>, diameter: &mut Option<f64>, length: &mut Option<f64>) {
    if let Some(name) = map.get("name").and_then(Value::as_str) {
        if let Some(number) = map
            .get("mm")
            .and_then(positive_number)
            .or_else(|| map.get("value").and_then(positive_number))
        {
            keep_axis(name, number, diameter, length);
        }
    }
    for (key, value) in map {
        if key.eq_ignore_ascii_case("name") || key.eq_ignore_ascii_case("mm") {
            continue;
        }
        if let Some(number) = positive_number(value) {
            keep_axis(key, number, diameter, length);
        }
    }
}

fn keep_axis(name: &str, number: f64, diameter: &mut Option<f64>, length: &mut Option<f64>) {
    match dimension_axis(name) {
        Some(Axis::Diameter) if diameter.is_none() => *diameter = Some(number),
        Some(Axis::InnerDiameter) if diameter.is_none() => *diameter = Some(number),
        Some(Axis::Length) if length.is_none() => *length = Some(number),
        _ => {}
    }
}

fn dimension_axis(name: &str) -> Option<Axis> {
    let lower: String = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    match lower.as_str() {
        "diametermm" | "diameter" | "dia" | "od" | "outsidediameter" => Some(Axis::Diameter),
        "id" | "innerdiameter" | "insidediameter" => Some(Axis::InnerDiameter),
        "lengthmm" | "length" | "len" | "l" | "h" | "height" | "heightmm" | "thickness" => {
            Some(Axis::Length)
        }
        _ => None,
    }
}

fn positive_number(value: &Value) -> Option<f64> {
    if let Some(number) = value.as_f64() {
        return finite_positive(number);
    }
    if let Some(text) = value.as_str() {
        return leading_number(text.trim());
    }
    None
}

fn finite_positive(number: f64) -> Option<f64> {
    if number.is_finite() && number > 0.0 {
        Some(number)
    } else {
        None
    }
}

/// `M6x40`, `6 x 40`, or `6×40 mm`. The first number is the diameter.
fn parse_size_text(text: &str) -> Option<(f64, f64)> {
    let lower = text.trim().to_ascii_lowercase();
    let index = lower.find(['x', '×'])?;
    let left = lower[..index].trim().trim_start_matches('m').trim();
    let right = lower[index + lower[index..].chars().next()?.len_utf8()..].trim();
    Some((leading_number(left)?, leading_number(right)?))
}

fn leading_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut end = 0;
    let mut dot = false;
    for (index, ch) in text.char_indices() {
        if ch.is_ascii_digit() {
            end = index + 1;
            continue;
        }
        if ch == '.' && !dot {
            dot = true;
            end = index + 1;
            continue;
        }
        break;
    }
    if end == 0 {
        return None;
    }
    finite_positive(text[..end].parse().ok()?)
}

fn expected_label(expected: &ExpectedSize) -> String {
    match (expected.diameter_mm, expected.length_mm) {
        (Some(diameter), Some(length)) => {
            format!("M{}x{}", compact_number(diameter), compact_number(length))
        }
        (None, Some(length)) => format!("{} mm", compact_number(length)),
        (Some(diameter), None) => format!("Ø{} mm", compact_number(diameter)),
        (None, None) if !expected.description.is_empty() => expected.description.clone(),
        (None, None) => String::new(),
    }
}

fn compact_number(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if (rounded - rounded.round()).abs() < 1e-6 {
        return format!("{}", rounded.round() as i64);
    }
    format!("{rounded:.2}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn one_decimal(value: f64) -> String {
    format!("{value:.1}")
}

fn judge(
    length_mm: f64,
    diameter_mm: f64,
    expected: &ExpectedSize,
    length_tolerance: f64,
    diameter_tolerance: f64,
) -> MetrologyResult {
    let length_bad = expected
        .length_mm
        .is_some_and(|want| (length_mm - want).abs() > length_tolerance);
    let diameter_bad = expected
        .diameter_mm
        .is_some_and(|want| (diameter_mm - want).abs() > diameter_tolerance);
    let status = if length_bad || diameter_bad {
        MetrologyStatus::DimensionMismatch
    } else {
        MetrologyStatus::Pass
    };
    let label = expected_label(expected);
    let note = if status == MetrologyStatus::Pass {
        String::new()
    } else {
        let measured = if length_bad && diameter_bad {
            format!(
                "{} × {} mm",
                one_decimal(length_mm),
                one_decimal(diameter_mm)
            )
        } else if length_bad {
            format!("{} mm", one_decimal(length_mm))
        } else {
            format!("diameter {} mm", one_decimal(diameter_mm))
        };
        let expected_text = if label.is_empty() {
            "part".to_string()
        } else {
            label.clone()
        };
        format!("Expected {expected_text}, measured {measured}")
    };
    MetrologyResult {
        status,
        length_mm: (length_mm * 100.0).round() / 100.0,
        diameter_mm: (diameter_mm * 100.0).round() / 100.0,
        expected_length_mm: expected.length_mm,
        expected_diameter_mm: expected.diameter_mm,
        expected_label: label,
        note,
    }
}

fn read_scale(value: Option<&Value>) -> Option<CameraScale> {
    let map = value?.as_object()?;
    let mm_per_px = map.get("mmPerPx")?.as_f64()?;
    let width = map.get("width")?.as_f64()?;
    let height = map.get("height")?.as_f64()?;
    if !(mm_per_px.is_finite() && mm_per_px > 0.0 && mm_per_px <= 1000.0) {
        return None;
    }
    if !(width.is_finite() && (1.0..=20000.0).contains(&width)) {
        return None;
    }
    if !(height.is_finite() && (1.0..=20000.0).contains(&height)) {
        return None;
    }
    Some(CameraScale { mm_per_px, width })
}

fn scale_for(camera: &Map<String, Value>, index: i64) -> Option<CameraScale> {
    if let Some(by_index) = camera.get("byIndex").and_then(Value::as_object) {
        let key = index.to_string();
        if let Some(record) = by_index.get(&key) {
            return read_scale(record.get("scale"));
        }
    }
    let active = camera.get("index").and_then(Value::as_i64);
    if active == Some(index) {
        return read_scale(camera.get("scale"));
    }
    None
}

/// Millimetres per pixel of this frame. The stored factor belongs to the
/// image width it was drawn on, and is scaled when this frame is wider.
fn mm_per_px(document: &Value, frame_width: u32) -> Option<f64> {
    if frame_width == 0 {
        return None;
    }
    let camera = document.get("camera")?.as_object()?;
    let index = camera.get("index").and_then(Value::as_i64).unwrap_or(0);
    let scale = scale_for(camera, index)?;
    let mm = scale.mm_per_px * (scale.width / f64::from(frame_width));
    if mm.is_finite() && mm > 0.0 {
        Some(mm)
    } else {
        None
    }
}

/// Measure every catalogue match. Unrecognized boxes stay `None`.
/// A missing scale, or a missing OpenCV DLL, leaves every box unset.
pub(crate) fn measure_boxes(
    document: Option<&Value>,
    rgb: &[u8],
    width: u32,
    height: u32,
    boxes: &[BoxMeasure],
) -> Vec<Option<MetrologyResult>> {
    let settings = settings_from(document);
    if !settings.enabled {
        return vec![None; boxes.len()];
    }
    if !boxes.iter().any(|item| item.matched) {
        return vec![None; boxes.len()];
    }
    let Some(document) = document else {
        return vec![None; boxes.len()];
    };
    let Some(mm) = mm_per_px(document, width) else {
        eprintln!("metrology skipped: no camera scale");
        return vec![None; boxes.len()];
    };
    if !opencv_ready() {
        eprintln!("metrology skipped: OpenCV is not available");
        return vec![None; boxes.len()];
    }
    boxes
        .iter()
        .map(|item| {
            if !item.matched {
                return None;
            }
            let expected = crate::catalogue::part_dimensions(&item.ic)
                .map(|(description, dimensions)| expected_from(&description, &dimensions))
                .unwrap_or_default();
            match measure_rgb(rgb, width, height, item.bbox, &settings, mm, &expected) {
                Some(result) => {
                    let status = match result.status {
                        MetrologyStatus::Pass => "pass",
                        MetrologyStatus::DimensionMismatch => "DIMENSION_MISMATCH",
                    };
                    eprintln!(
                        "metrology {} {status} length {:.1} mm diameter {:.1} mm",
                        item.ic, result.length_mm, result.diameter_mm
                    );
                    Some(result)
                }
                None => {
                    eprintln!("metrology {}: no measurement", item.ic);
                    None
                }
            }
        })
        .collect()
}

pub(crate) fn measure_rgb(
    rgb: &[u8],
    width: u32,
    height: u32,
    bbox: [i32; 4],
    settings: &MetrologySettings,
    mm_per_px: f64,
    expected: &ExpectedSize,
) -> Option<MetrologyResult> {
    if !(mm_per_px.is_finite() && mm_per_px > 0.0) {
        return None;
    }
    let (x1, y1, x2, y2) = expanded_box(bbox, settings.margin_percent, width, height)?;
    let (crop, crop_w, crop_h) = crop_rgb(rgb, width, height, x1, y1, x2, y2)?;
    let (length_px, diameter_px) = caliper(&crop, crop_w, crop_h, settings)
        .map_err(|err| eprintln!("metrology: {err}"))
        .ok()?;
    let length_mm = length_px * mm_per_px;
    let diameter_mm = diameter_px * mm_per_px;
    if !length_mm.is_finite() || !diameter_mm.is_finite() {
        return None;
    }
    Some(judge(
        length_mm,
        diameter_mm,
        expected,
        settings.length_tolerance_mm,
        settings.diameter_tolerance_mm,
    ))
}

fn expanded_box(
    bbox: [i32; 4],
    margin_percent: f64,
    width: u32,
    height: u32,
) -> Option<(i32, i32, i32, i32)> {
    let x1 = bbox[0].min(bbox[2]);
    let y1 = bbox[1].min(bbox[3]);
    let x2 = bbox[0].max(bbox[2]);
    let y2 = bbox[1].max(bbox[3]);
    if x2 <= x1 || y2 <= y1 {
        return None;
    }
    let grow_x = f64::from(x2 - x1) * margin_percent / 100.0;
    let grow_y = f64::from(y2 - y1) * margin_percent / 100.0;
    let frame_w = width as i32;
    let frame_h = height as i32;
    let left = (f64::from(x1) - grow_x).floor() as i32;
    let top = (f64::from(y1) - grow_y).floor() as i32;
    let right = (f64::from(x2) + grow_x).ceil() as i32;
    let bottom = (f64::from(y2) + grow_y).ceil() as i32;
    let left = left.clamp(0, frame_w);
    let top = top.clamp(0, frame_h);
    let right = right.clamp(0, frame_w);
    let bottom = bottom.clamp(0, frame_h);
    if right - left < 2 || bottom - top < 2 {
        return None;
    }
    Some((left, top, right, bottom))
}

fn crop_rgb(
    rgb: &[u8],
    width: u32,
    height: u32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
) -> Option<(Vec<u8>, i32, i32)> {
    let frame_w = width as i32;
    let frame_h = height as i32;
    if x1 < 0 || y1 < 0 || x2 > frame_w || y2 > frame_h || x2 <= x1 || y2 <= y1 {
        return None;
    }
    let crop_w = x2 - x1;
    let crop_h = y2 - y1;
    let row_bytes = (crop_w as usize).checked_mul(3)?;
    let mut out = vec![0_u8; row_bytes.checked_mul(crop_h as usize)?];
    let stride = (width as usize).checked_mul(3)?;
    for row in 0..crop_h {
        let src = ((y1 + row) as usize)
            .checked_mul(stride)?
            .checked_add((x1 as usize).checked_mul(3)?)?;
        let dst = (row as usize).checked_mul(row_bytes)?;
        out[dst..dst + row_bytes].copy_from_slice(rgb.get(src..src + row_bytes)?);
    }
    Some((out, crop_w, crop_h))
}

fn caliper(
    rgb: &[u8],
    width: i32,
    height: i32,
    settings: &MetrologySettings,
) -> Result<(f64, f64), String> {
    let color = Mat::new_rows_cols_with_bytes::<Vec3b>(height, width, rgb)
        .map_err(|err| err.to_string())?;
    let mut gray = Mat::default();
    opencv::imgproc::cvt_color_def(&color, &mut gray, opencv::imgproc::COLOR_RGB2GRAY)
        .map_err(|err| err.to_string())?;
    let mut best: Option<(f64, Vector<Point>)> = None;
    for invert in [false, true] {
        let flags = threshold_flags(settings.mode, invert);
        let cutoff = match settings.mode {
            ThresholdMode::Otsu => 0.0,
            ThresholdMode::Fixed => f64::from(settings.fixed_threshold),
        };
        let mut binary = Mat::default();
        opencv::imgproc::threshold(&gray, &mut binary, cutoff, 255.0, flags)
            .map_err(|err| err.to_string())?;
        let mut contours: Vector<Vector<Point>> = Vector::new();
        opencv::imgproc::find_contours(
            &binary,
            &mut contours,
            opencv::imgproc::RETR_EXTERNAL,
            opencv::imgproc::CHAIN_APPROX_SIMPLE,
            Point::new(0, 0),
        )
        .map_err(|err| err.to_string())?;
        let image_area = f64::from(width) * f64::from(height);
        for index in 0..contours.len() {
            let contour = contours.get(index).map_err(|err| err.to_string())?;
            let area = opencv::imgproc::contour_area(&contour, false)
                .map_err(|err| err.to_string())?
                .abs();
            if area < settings.min_contour_area_px {
                continue;
            }
            if image_area > 0.0 && area > image_area * MAX_PART_FRACTION {
                continue;
            }
            if best.as_ref().is_none_or(|(have, _)| area > *have) {
                best = Some((area, contour));
            }
        }
    }
    let (_, contour) = best.ok_or_else(|| "no contour".to_string())?;
    let rect = opencv::imgproc::min_area_rect(&contour).map_err(|err| err.to_string())?;
    let (side_a, side_b) = rect_sides(&rect)?;
    if side_a < 1.0 || side_b < 1.0 {
        return Err("contour is too small".to_string());
    }
    Ok((side_a.max(side_b), side_a.min(side_b)))
}

fn threshold_flags(mode: ThresholdMode, invert: bool) -> i32 {
    let base = if invert {
        opencv::imgproc::THRESH_BINARY_INV
    } else {
        opencv::imgproc::THRESH_BINARY
    };
    if mode == ThresholdMode::Otsu {
        base | opencv::imgproc::THRESH_OTSU
    } else {
        base
    }
}

fn rect_sides(rect: &RotatedRect) -> Result<(f64, f64), String> {
    let width = f64::from(rect.size.width);
    let height = f64::from(rect.size.height);
    if !width.is_finite() || !height.is_finite() {
        return Err("min-area rect is empty".to_string());
    }
    Ok((width, height))
}

fn opencv_ready() -> bool {
    let Some(name) = option_env!("LS_VISION_OPENCV_DLL") else {
        return true;
    };
    if name.is_empty() {
        return true;
    }
    library_loads(name)
}

fn library_loads(name: &str) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::FreeLibrary;
        use windows::Win32::System::LibraryLoader::LoadLibraryW;
        use windows::core::PCWSTR;

        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let Ok(module) = LoadLibraryW(PCWSTR(wide.as_ptr())) else {
                return false;
            };
            let _ = FreeLibrary(module);
            true
        }
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn white_on_black(width: u32, height: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
        let mut data = vec![0_u8; (width * height * 3) as usize];
        for row in y..y + h {
            for col in x..x + w {
                let index = ((row * width + col) * 3) as usize;
                data[index] = 255;
                data[index + 1] = 255;
                data[index + 2] = 255;
            }
        }
        data
    }

    fn sample_settings(mode: ThresholdMode) -> MetrologySettings {
        MetrologySettings {
            enabled: true,
            margin_percent: 5.0,
            mode,
            fixed_threshold: 128,
            min_contour_area_px: 200.0,
            length_tolerance_mm: 0.5,
            diameter_tolerance_mm: 0.5,
        }
    }

    #[test]
    fn missing_settings_stay_off() {
        let settings = settings_from(None);
        assert!(!settings.enabled);
        assert_eq!(settings.margin_percent, 5.0);
        assert_eq!(settings.mode, ThresholdMode::Otsu);
        assert_eq!(settings.fixed_threshold, 128);
        assert_eq!(settings.min_contour_area_px, 200.0);
        assert_eq!(settings.length_tolerance_mm, 1.5);
        assert_eq!(settings.diameter_tolerance_mm, 0.8);
    }

    #[test]
    fn saved_settings_are_clamped() {
        let settings = settings_from(Some(&json!({
            "metrology": {
                "enabled": true,
                "marginPercent": 250,
                "thresholdMode": "Fixed",
                "fixedThreshold": 400,
                "minContourAreaPx": -4,
                "lengthToleranceMm": 2.25,
                "diameterToleranceMm": 0.8
            }
        })));
        assert!(settings.enabled);
        assert_eq!(settings.margin_percent, 100.0);
        assert_eq!(settings.mode, ThresholdMode::Fixed);
        assert_eq!(settings.fixed_threshold, 128);
        assert_eq!(settings.min_contour_area_px, 0.0);
        assert_eq!(settings.length_tolerance_mm, 2.25);
    }

    #[test]
    fn catalogue_size_reads_m6x40() {
        let from_object = expected_from("bolt", &json!({ "diameterMm": 6, "lengthMm": 40 }));
        assert_eq!(from_object.diameter_mm, Some(6.0));
        assert_eq!(from_object.length_mm, Some(40.0));
        assert_eq!(expected_label(&from_object), "M6x40");

        let from_text = expected_from("", &json!("M6x40"));
        assert_eq!(from_text.diameter_mm, Some(6.0));
        assert_eq!(from_text.length_mm, Some(40.0));

        let from_description = expected_from("M6.5x12 bolt", &Value::Null);
        assert_eq!(from_description.diameter_mm, Some(6.5));
        assert_eq!(from_description.length_mm, Some(12.0));
        assert_eq!(expected_label(&from_description), "M6.5x12");

        let from_list = expected_from(
            "Grommet Rubber Buffer M22",
            &json!([{ "name": "h", "mm": 8.35 }, { "name": "OD", "mm": 22 }]),
        );
        assert_eq!(from_list.diameter_mm, Some(22.0));
        assert_eq!(from_list.length_mm, Some(8.35));
    }

    #[test]
    fn mismatch_note_names_the_expected_size() {
        let expected = expected_from("", &json!({ "diameterMm": 6, "lengthMm": 40 }));
        let result = judge(25.2, 6.0, &expected, 1.5, 0.8);
        assert_eq!(result.status, MetrologyStatus::DimensionMismatch);
        assert_eq!(result.note, "Expected M6x40, measured 25.2 mm");
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["status"], "DIMENSION_MISMATCH");

        let pass = judge(40.1, 6.2, &expected, 1.5, 0.8);
        assert_eq!(pass.status, MetrologyStatus::Pass);
        assert!(pass.note.is_empty());
        assert!(serde_json::to_value(&pass).unwrap().get("note").is_none());
    }

    #[test]
    fn scale_follows_the_active_camera_width() {
        let document = json!({
            "camera": {
                "index": 1,
                "scale": { "mmPerPx": 0.5, "width": 1000, "height": 800 },
                "byIndex": {
                    "1": {
                        "scale": { "mmPerPx": 0.2, "width": 2000, "height": 1000 }
                    }
                }
            }
        });
        let mm = mm_per_px(&document, 4000).unwrap();
        assert!((mm - 0.1).abs() < 1e-9);
    }

    #[test]
    fn disabled_and_unrecognized_boxes_are_not_measured() {
        let document = json!({
            "metrology": { "enabled": false },
            "camera": {
                "index": 0,
                "scale": { "mmPerPx": 0.1, "width": 200, "height": 120 }
            }
        });
        let boxes = vec![BoxMeasure {
            bbox: [10, 10, 40, 40],
            matched: true,
            ic: "FAST-1".to_string(),
        }];
        let skipped = measure_boxes(Some(&document), &[0, 0, 0], 1, 1, &boxes);
        assert!(skipped.iter().all(|item| item.is_none()));

        let mut on = document;
        on["metrology"]["enabled"] = json!(true);
        let mixed = vec![
            BoxMeasure {
                bbox: [10, 10, 40, 40],
                matched: false,
                ic: String::new(),
            },
            BoxMeasure {
                bbox: [10, 10, 40, 40],
                matched: false,
                ic: "unknown".to_string(),
            },
        ];
        let results = measure_boxes(Some(&on), &[0, 0, 0], 1, 1, &mixed);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|item| item.is_none()));
    }

    #[test]
    fn a_white_rectangle_matches_its_catalogue_size() {
        let width = 220_u32;
        let height = 140_u32;
        let rgb = white_on_black(width, height, 40, 50, 100, 30);
        let expected = expected_from("bolt", &json!({ "diameterMm": 3, "lengthMm": 10 }));
        let result = measure_rgb(
            &rgb,
            width,
            height,
            [40, 50, 140, 80],
            &sample_settings(ThresholdMode::Otsu),
            0.1,
            &expected,
        )
        .expect("rectangle");
        assert_eq!(result.status, MetrologyStatus::Pass);
        assert!(result.note.is_empty());
        assert!(
            (result.length_mm - 10.0).abs() < 0.5,
            "{}",
            result.length_mm
        );
        assert!(
            (result.diameter_mm - 3.0).abs() < 0.5,
            "{}",
            result.diameter_mm
        );

        let fixed = measure_rgb(
            &rgb,
            width,
            height,
            [40, 50, 140, 80],
            &sample_settings(ThresholdMode::Fixed),
            0.1,
            &expected,
        )
        .expect("fixed threshold");
        assert_eq!(fixed.status, MetrologyStatus::Pass);
    }

    #[test]
    fn a_short_rectangle_is_a_dimension_mismatch() {
        let width = 220_u32;
        let height = 140_u32;
        let rgb = white_on_black(width, height, 40, 50, 100, 30);
        let expected = expected_from("", &json!({ "diameterMm": 6, "lengthMm": 40 }));
        let mut settings = sample_settings(ThresholdMode::Otsu);
        settings.length_tolerance_mm = 1.5;
        settings.diameter_tolerance_mm = 0.8;
        let result = measure_rgb(
            &rgb,
            width,
            height,
            [40, 50, 140, 80],
            &settings,
            0.1,
            &expected,
        )
        .expect("mismatch");
        assert_eq!(result.status, MetrologyStatus::DimensionMismatch);
        assert!(
            result.note.starts_with("Expected M6x40, measured "),
            "{}",
            result.note
        );
        assert!(result.note.contains("mm"));
    }

    #[test]
    fn a_speck_below_the_minimum_area_is_ignored() {
        let rgb = white_on_black(80, 80, 30, 30, 8, 8);
        let result = measure_rgb(
            &rgb,
            80,
            80,
            [28, 28, 40, 40],
            &sample_settings(ThresholdMode::Otsu),
            0.1,
            &ExpectedSize::default(),
        );
        assert!(result.is_none());
    }

    #[test]
    fn measure_boxes_skips_an_unrecognized_neighbour() {
        let width = 220_u32;
        let height = 140_u32;
        let rgb = white_on_black(width, height, 40, 50, 100, 30);
        let document = json!({
            "metrology": {
                "enabled": true,
                "marginPercent": 5,
                "thresholdMode": "otsu",
                "fixedThreshold": 128,
                "minContourAreaPx": 200,
                "lengthToleranceMm": 0.5,
                "diameterToleranceMm": 0.5
            },
            "camera": {
                "index": 0,
                "byIndex": {
                    "0": {
                        "scale": { "mmPerPx": 0.1, "width": width, "height": height }
                    }
                }
            }
        });
        let boxes = vec![
            BoxMeasure {
                bbox: [40, 50, 140, 80],
                matched: false,
                ic: String::new(),
            },
            BoxMeasure {
                bbox: [40, 50, 140, 80],
                matched: true,
                ic: "NOT-IN-CATALOGUE".to_string(),
            },
        ];
        let results = measure_boxes(Some(&document), &rgb, width, height, &boxes);
        assert!(results[0].is_none());
        let matched = results[1].as_ref().expect("matched box");
        assert_eq!(matched.status, MetrologyStatus::Pass);
    }
}
