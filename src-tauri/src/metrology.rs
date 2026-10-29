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
//! adjusted by the frame width the same way Measure does. The same rectangle's
//! two axes are stored as `spans` so the overlay can draw them. The contour,
//! the thresholds, and the millimetre conversion stay as they are.
//!
//! `measurementMethod` on the catalogue part selects bolt, nut, washer,
//! cable tie, or spring sizes. `custom` and a missing method keep the generic
//! length and diameter. YOLO class index 3 is a label and is not measured.
//! The global length and diameter tolerances still apply. The Ø symbol is only
//! used when the size is a diameter.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use opencv::core::{Mat, Point, Point2f, RotatedRect, Vec3b, Vector};
use opencv::prelude::MatTraitConst;

/// YOLO class index for a label. Those boxes are not measured.
const LABEL_CLASS: usize = 3;

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

/// One measured side of the min-area rectangle, in full-frame pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetrologySpan {
    pub kind: String,
    pub text: String,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
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
    /// Empty on a pass, so the mismatch badge is omitted.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Endpoints of the measured sides. Empty when the rectangle could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<MetrologySpan>,
}

#[derive(Clone, Debug)]
pub(crate) struct BoxMeasure {
    pub bbox: [i32; 4],
    pub matched: bool,
    pub ic: String,
    /// YOLO class index. Class 3 is left unmeasured.
    pub class_id: usize,
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
        spans: Vec::new(),
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
            if item.class_id == LABEL_CLASS {
                eprintln!("metrology {}: label class is not measured", item.ic);
                return None;
            }
            let part = crate::catalogue::part_dimensions(&item.ic);
            let description = part
                .as_ref()
                .map(|part| part.description.clone())
                .unwrap_or_default();
            let dimensions = part
                .as_ref()
                .map(|part| part.dimensions.clone())
                .unwrap_or(Value::Null);
            let method = part
                .as_ref()
                .map(|part| part.measurement_method.clone())
                .unwrap_or_default();
            let result = if method.is_empty() || method == "custom" {
                let expected = expected_from(&description, &dimensions);
                measure_rgb(rgb, width, height, item.bbox, &settings, mm, &expected)
            } else {
                measure_method(
                    rgb,
                    width,
                    height,
                    item.bbox,
                    &settings,
                    mm,
                    &description,
                    &dimensions,
                    &method,
                )
            };
            match result {
                Some(result) => {
                    let status = match result.status {
                        MetrologyStatus::Pass => "pass",
                        MetrologyStatus::DimensionMismatch => "DIMENSION_MISMATCH",
                    };
                    if method.is_empty() || method == "custom" {
                        eprintln!(
                            "metrology {} {status} length {:.1} mm diameter {:.1} mm",
                            item.ic, result.length_mm, result.diameter_mm
                        );
                    } else {
                        eprintln!(
                            "metrology {} {method} {status} length {:.1} mm diameter {:.1} mm",
                            item.ic, result.length_mm, result.diameter_mm
                        );
                    }
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
    let hit = caliper(&crop, crop_w, crop_h, settings)
        .map_err(|err| eprintln!("metrology: {err}"))
        .ok()?;
    generic_from_hit(
        hit,
        f64::from(x1),
        f64::from(y1),
        mm_per_px,
        expected,
        settings,
    )
}

fn generic_from_hit(
    hit: CaliperHit,
    origin_x: f64,
    origin_y: f64,
    mm_per_px: f64,
    expected: &ExpectedSize,
    settings: &MetrologySettings,
) -> Option<MetrologyResult> {
    let length_mm = hit.length_px * mm_per_px;
    let diameter_mm = hit.diameter_px * mm_per_px;
    if !length_mm.is_finite() || !diameter_mm.is_finite() {
        return None;
    }
    let mut result = judge(
        length_mm,
        diameter_mm,
        expected,
        settings.length_tolerance_mm,
        settings.diameter_tolerance_mm,
    );
    result.spans = spans_from_rect(
        hit.rect,
        origin_x,
        origin_y,
        result.length_mm,
        result.diameter_mm,
    );
    Some(result)
}

struct Axes {
    long: [f64; 4],
    short: [f64; 4],
    long_px: f64,
    short_px: f64,
}

struct Gauge {
    kind: &'static str,
    px: f64,
    expected: Option<f64>,
    diameter: bool,
    ignored: bool,
    line: [f64; 4],
}

/// Measure one matched box with the catalogue method. `custom` is not passed
/// here. A method that cannot read its sizes falls back to the generic caliper.
fn measure_method(
    rgb: &[u8],
    width: u32,
    height: u32,
    bbox: [i32; 4],
    settings: &MetrologySettings,
    mm_per_px: f64,
    description: &str,
    dimensions: &Value,
    method: &str,
) -> Option<MetrologyResult> {
    if !(mm_per_px.is_finite() && mm_per_px > 0.0) {
        return None;
    }
    let (x1, y1, x2, y2) = expanded_box(bbox, settings.margin_percent, width, height)?;
    let (crop, crop_w, crop_h) = crop_rgb(rgb, width, height, x1, y1, x2, y2)?;
    let hit = caliper(&crop, crop_w, crop_h, settings)
        .map_err(|err| eprintln!("metrology: {err}"))
        .ok()?;
    let axes = axes_of(hit.rect)?;
    let pairs = named_sizes(description, dimensions);
    let gauges = match method {
        "bolt" => bolt_gauges(&crop, crop_w, crop_h, settings, &axes, &pairs),
        "nut" => Some(nut_gauges(&axes, &pairs)),
        "washer" => washer_gauges(&crop, crop_w, crop_h, settings, &axes, &pairs),
        "cable_tie" => Some(cable_gauges(&axes, &pairs)),
        "spring" => Some(spring_gauges(&axes, &pairs)),
        _ => None,
    };
    let origin_x = f64::from(x1);
    let origin_y = f64::from(y1);
    if let Some(gauges) = gauges {
        return result_from_gauges(
            &gauges,
            origin_x,
            origin_y,
            mm_per_px,
            &method_label(method, &pairs),
            settings,
        );
    }
    generic_from_hit(
        hit,
        origin_x,
        origin_y,
        mm_per_px,
        &expected_from(description, dimensions),
        settings,
    )
}

fn axes_of(rect: RotatedRect) -> Option<Axes> {
    let mut corners = [Point2f::new(0.0, 0.0); 4];
    if rect.points(&mut corners).is_err() {
        return None;
    }
    let width_line = line_array(midline(corners[0], corners[1], corners[2], corners[3]));
    let height_line = line_array(midline(corners[1], corners[2], corners[3], corners[0]));
    let width = f64::from(rect.size.width);
    let height = f64::from(rect.size.height);
    if !width.is_finite() || !height.is_finite() || width < 1.0 || height < 1.0 {
        return None;
    }
    if width >= height {
        Some(Axes {
            long: width_line,
            short: height_line,
            long_px: width,
            short_px: height,
        })
    } else {
        Some(Axes {
            long: height_line,
            short: width_line,
            long_px: height,
            short_px: width,
        })
    }
}

fn line_array(line: (f64, f64, f64, f64)) -> [f64; 4] {
    [line.0, line.1, line.2, line.3]
}

fn line_len(line: [f64; 4]) -> f64 {
    let dx = line[2] - line[0];
    let dy = line[3] - line[1];
    (dx * dx + dy * dy).sqrt()
}

fn named_sizes(description: &str, dimensions: &Value) -> Vec<(String, f64)> {
    let mut pairs = Vec::new();
    absorb_named(dimensions, &mut pairs);
    let from_text = match dimensions {
        Value::String(text) => parse_size_text(text),
        _ => None,
    };
    let from_text = from_text.or_else(|| {
        if pairs.is_empty() {
            parse_size_text(description)
        } else {
            None
        }
    });
    if let Some((diameter, length)) = from_text {
        pairs.push(("diameter".to_string(), diameter));
        pairs.push(("length".to_string(), length));
    }
    pairs
}

fn absorb_named(dimensions: &Value, pairs: &mut Vec<(String, f64)>) {
    match dimensions {
        Value::Object(map) => {
            for (key, value) in map {
                if let Some(number) = positive_number(value) {
                    pairs.push((norm_key(key), number));
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                let Some(map) = item.as_object() else {
                    continue;
                };
                if let Some(name) = map.get("name").and_then(Value::as_str) {
                    if let Some(number) = map
                        .get("mm")
                        .and_then(positive_number)
                        .or_else(|| map.get("value").and_then(positive_number))
                    {
                        pairs.push((norm_key(name), number));
                    }
                }
                for (key, value) in map {
                    if key.eq_ignore_ascii_case("name") || key.eq_ignore_ascii_case("mm") {
                        continue;
                    }
                    if let Some(number) = positive_number(value) {
                        pairs.push((norm_key(key), number));
                    }
                }
            }
        }
        _ => {}
    }
}

fn norm_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn find_mm(pairs: &[(String, f64)], keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some((_, number)) = pairs.iter().find(|(name, _)| name == key) {
            return Some(*number);
        }
    }
    None
}

fn bolt_gauges(
    rgb: &[u8],
    width: i32,
    height: i32,
    settings: &MetrologySettings,
    axes: &Axes,
    pairs: &[(String, f64)],
) -> Option<Vec<Gauge>> {
    let mask = part_mask(rgb, width, height, settings).ok()?;
    let stations = width_profile(&mask, width, height, axes);
    let split = bolt_split(&stations, axes)?;
    let length = find_mm(
        pairs,
        &[
            "lengthunderhead",
            "underheadlength",
            "shanklength",
            "griplength",
            "length",
            "lengthmm",
            "len",
            "l",
        ],
    );
    let diameter = find_mm(
        pairs,
        &[
            "threaddiameter",
            "threaddia",
            "nominaldiameter",
            "diameter",
            "diametermm",
            "dia",
        ],
    );
    Some(vec![
        Gauge {
            kind: "length_under_head",
            px: split.under_px,
            expected: length,
            diameter: false,
            ignored: false,
            line: split.length_line,
        },
        Gauge {
            kind: "thread_diameter",
            px: split.thread_px,
            expected: diameter,
            diameter: true,
            ignored: false,
            line: split.diameter_line,
        },
    ])
}

struct Station {
    t: f64,
    width: f64,
    offset: f64,
}

struct BoltSplit {
    under_px: f64,
    thread_px: f64,
    length_line: [f64; 4],
    diameter_line: [f64; 4],
}

fn width_profile(mask: &[u8], width: i32, height: i32, axes: &Axes) -> Vec<Station> {
    let long_len = line_len(axes.long);
    let short_len = line_len(axes.short);
    if long_len < 2.0 || short_len < 1.0 {
        return Vec::new();
    }
    let steps_along = (long_len.round() as usize).clamp(24, 400);
    let step_unit_x = (axes.short[2] - axes.short[0]) / short_len;
    let step_unit_y = (axes.short[3] - axes.short[1]) / short_len;
    let reach = (axes.short_px * 0.5 + 4.0).ceil() as i32;
    let mut stations = Vec::with_capacity(steps_along + 1);
    for index in 0..=steps_along {
        let t = index as f64 / steps_along as f64;
        let center_x = axes.long[0] + (axes.long[2] - axes.long[0]) * t;
        let center_y = axes.long[1] + (axes.long[3] - axes.long[1]) * t;
        let mut min_step: Option<i32> = None;
        let mut max_step: Option<i32> = None;
        for step in -reach..=reach {
            let x = center_x + step_unit_x * f64::from(step);
            let y = center_y + step_unit_y * f64::from(step);
            if mask_on(mask, width, height, x, y) {
                min_step = Some(min_step.map_or(step, |have| have.min(step)));
                max_step = Some(max_step.map_or(step, |have| have.max(step)));
            }
        }
        let (Some(min_step), Some(max_step)) = (min_step, max_step) else {
            stations.push(Station {
                t,
                width: 0.0,
                offset: 0.0,
            });
            continue;
        };
        stations.push(Station {
            t,
            width: f64::from(max_step - min_step) + 1.0,
            offset: f64::from(max_step + min_step) * 0.5,
        });
    }
    stations
}

fn bolt_split(stations: &[Station], axes: &Axes) -> Option<BoltSplit> {
    let active: Vec<&Station> = stations.iter().filter(|item| item.width > 2.0).collect();
    if active.len() < 8 {
        return None;
    }
    let mut widths: Vec<f64> = active.iter().map(|item| item.width).collect();
    widths.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let narrow = widths[widths.len() * 30 / 100];
    let wide = widths[(widths.len() * 90 / 100).min(widths.len() - 1)];
    if !(narrow > 1.0 && wide >= narrow * 1.45) {
        return None;
    }
    let cut = narrow * 1.35;
    let from_start = mean_side(&active, true) >= mean_side(&active, false);
    let boundary = head_boundary(&active, cut, from_start)?;
    let start_t = active.first()?.t;
    let end_t = active.last()?.t;
    let span = (end_t - start_t).abs();
    if span < 1e-6 {
        return None;
    }
    let boundary_t = active[boundary].t;
    let head_span = if from_start {
        boundary_t - start_t
    } else {
        end_t - boundary_t
    };
    let fraction = head_span / span;
    if !(0.06..=0.55).contains(&fraction) {
        return None;
    }
    let mut shank_widths = Vec::new();
    let mut shank_offsets = Vec::new();
    for (index, station) in active.iter().enumerate() {
        let on_shank = if from_start {
            index > boundary
        } else {
            index < boundary
        };
        if on_shank {
            shank_widths.push(station.width);
            shank_offsets.push(station.offset);
        }
    }
    if shank_widths.is_empty() {
        return None;
    }
    shank_widths
        .sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    shank_offsets
        .sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let thread_px = shank_widths[shank_widths.len() / 2];
    let offset = shank_offsets[shank_offsets.len() / 2];
    if thread_px < 1.0 {
        return None;
    }
    let under_px = axes.long_px * (span - head_span) / span;
    if under_px < 1.0 {
        return None;
    }
    let (t_near, t_far) = if from_start {
        (boundary_t, end_t)
    } else {
        (start_t, boundary_t)
    };
    let length_line = [
        point_on(axes, t_near, offset).0,
        point_on(axes, t_near, offset).1,
        point_on(axes, t_far, offset).0,
        point_on(axes, t_far, offset).1,
    ];
    let mid_t = (t_near + t_far) * 0.5;
    let center = point_on(axes, mid_t, offset);
    let short_len = line_len(axes.short);
    let unit_x = (axes.short[2] - axes.short[0]) / short_len;
    let unit_y = (axes.short[3] - axes.short[1]) / short_len;
    let half = thread_px * 0.5;
    let diameter_line = [
        center.0 - unit_x * half,
        center.1 - unit_y * half,
        center.0 + unit_x * half,
        center.1 + unit_y * half,
    ];
    Some(BoltSplit {
        under_px,
        thread_px,
        length_line,
        diameter_line,
    })
}

fn mean_side(stations: &[&Station], from_start: bool) -> f64 {
    let count = ((stations.len() as f64) * 0.18).ceil().max(1.0) as usize;
    let count = count.min(stations.len());
    let slice = if from_start {
        &stations[..count]
    } else {
        &stations[stations.len() - count..]
    };
    slice.iter().map(|item| item.width).sum::<f64>() / count as f64
}

fn head_boundary(stations: &[&Station], cut: f64, from_start: bool) -> Option<usize> {
    let mut last_head = None;
    let mut gap = 0_u32;
    let order: Vec<usize> = if from_start {
        (0..stations.len()).collect()
    } else {
        (0..stations.len()).rev().collect()
    };
    for index in order {
        if stations[index].width > cut {
            last_head = Some(index);
            gap = 0;
        } else if last_head.is_some() {
            gap += 1;
            if gap > 2 {
                break;
            }
        }
    }
    last_head
}

fn point_on(axes: &Axes, t: f64, offset: f64) -> (f64, f64) {
    let short_len = line_len(axes.short).max(1.0);
    let unit_x = (axes.short[2] - axes.short[0]) / short_len;
    let unit_y = (axes.short[3] - axes.short[1]) / short_len;
    let x = axes.long[0] + (axes.long[2] - axes.long[0]) * t + unit_x * offset;
    let y = axes.long[1] + (axes.long[3] - axes.long[1]) * t + unit_y * offset;
    (x, y)
}

fn nut_gauges(axes: &Axes, pairs: &[(String, f64)]) -> Vec<Gauge> {
    let expected = find_mm(pairs, &["acrossflats", "widthacrossflats", "af", "flats"]);
    vec![Gauge {
        kind: "across_flats",
        px: axes.short_px,
        expected,
        diameter: false,
        ignored: false,
        line: axes.short,
    }]
}

fn washer_gauges(
    rgb: &[u8],
    width: i32,
    height: i32,
    settings: &MetrologySettings,
    axes: &Axes,
    pairs: &[(String, f64)],
) -> Option<Vec<Gauge>> {
    let mask = part_mask(rgb, width, height, settings).ok()?;
    let long = scan_axis(&mask, width, height, axes.long)?;
    let short = scan_axis(&mask, width, height, axes.short)?;
    let aspect = axes.long_px / axes.short_px.max(1.0);
    let (od_px, id_px, thickness_px) = if aspect < 1.35 {
        let od = (long.outer + short.outer) * 0.5;
        let id = match (long.gap, short.gap) {
            (Some(left), Some(right)) => Some((left + right) * 0.5),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
        (od, id, None)
    } else {
        (long.outer, long.gap, Some(short.outer))
    };
    if od_px < 1.0 {
        return None;
    }
    let od_expected = find_mm(
        pairs,
        &[
            "outerdiameter",
            "od",
            "outsidediameter",
            "diameter",
            "diametermm",
            "dia",
        ],
    );
    let id_expected = find_mm(pairs, &["innerdiameter", "id", "insidediameter"]);
    let thickness_expected = find_mm(pairs, &["thickness", "thk", "height", "heightmm", "h"]);
    let mut gauges = vec![Gauge {
        kind: "outer_diameter",
        px: od_px,
        expected: od_expected,
        diameter: true,
        ignored: false,
        line: centered_line(axes.long, od_px),
    }];
    if let Some(id_px) = id_px {
        if id_px > 1.0 {
            gauges.push(Gauge {
                kind: "inner_diameter",
                px: id_px,
                expected: id_expected,
                diameter: true,
                ignored: false,
                line: centered_line(axes.long, id_px),
            });
        }
    }
    if let Some(thickness_px) = thickness_px {
        if thickness_px > 1.0 {
            gauges.push(Gauge {
                kind: "thickness",
                px: thickness_px,
                expected: thickness_expected,
                diameter: false,
                ignored: true,
                line: centered_line(axes.short, thickness_px),
            });
        }
    }
    Some(gauges)
}

struct AxisScan {
    outer: f64,
    gap: Option<f64>,
}

fn scan_axis(mask: &[u8], width: i32, height: i32, line: [f64; 4]) -> Option<AxisScan> {
    let len = line_len(line);
    if len < 2.0 {
        return None;
    }
    let steps = len.round().max(2.0) as usize;
    let step = len / steps as f64;
    let mut runs: Vec<(f64, f64)> = Vec::new();
    let mut run_start: Option<usize> = None;
    for index in 0..=steps {
        let t = index as f64 / steps as f64;
        let x = line[0] + (line[2] - line[0]) * t;
        let y = line[1] + (line[3] - line[1]) * t;
        let on = mask_on(mask, width, height, x, y);
        if on && run_start.is_none() {
            run_start = Some(index);
        }
        if !on {
            if let Some(start) = run_start.take() {
                runs.push((start as f64 * step, index as f64 * step));
            }
        }
    }
    if let Some(start) = run_start {
        runs.push((start as f64 * step, (steps as f64 + 1.0) * step));
    }
    let first = runs.first()?;
    let last = runs.last()?;
    let outer = last.1 - first.0;
    if outer < 1.0 {
        return None;
    }
    let mut gap: f64 = 0.0;
    for pair in runs.windows(2) {
        gap = gap.max(pair[1].0 - pair[0].1);
    }
    Some(AxisScan {
        outer,
        gap: if gap > 1.0 { Some(gap) } else { None },
    })
}

fn cable_gauges(axes: &Axes, pairs: &[(String, f64)]) -> Vec<Gauge> {
    let length = find_mm(
        pairs,
        &[
            "length",
            "totallength",
            "overalllength",
            "lengthmm",
            "len",
            "l",
        ],
    );
    let width = find_mm(pairs, &["strapwidth", "width", "bandwidth"]);
    vec![
        Gauge {
            kind: "length",
            px: axes.long_px,
            expected: length,
            diameter: false,
            ignored: false,
            line: axes.long,
        },
        Gauge {
            kind: "width",
            px: axes.short_px,
            expected: width,
            diameter: false,
            ignored: false,
            line: axes.short,
        },
    ]
}

fn spring_gauges(axes: &Axes, pairs: &[(String, f64)]) -> Vec<Gauge> {
    let height = find_mm(
        pairs,
        &[
            "freeheight",
            "height",
            "heightmm",
            "h",
            "length",
            "lengthmm",
            "len",
            "l",
        ],
    );
    let diameter = find_mm(
        pairs,
        &[
            "outerdiameter",
            "coildiameter",
            "od",
            "diameter",
            "diametermm",
            "dia",
        ],
    );
    let height_is_long = match (height, diameter) {
        (Some(height), Some(diameter)) => height >= diameter,
        _ => true,
    };
    let (height_px, height_line, diameter_px, diameter_line) = if height_is_long {
        (axes.long_px, axes.long, axes.short_px, axes.short)
    } else {
        (axes.short_px, axes.short, axes.long_px, axes.long)
    };
    vec![
        Gauge {
            kind: "free_height",
            px: height_px,
            expected: height,
            diameter: false,
            ignored: false,
            line: height_line,
        },
        Gauge {
            kind: "coil_diameter",
            px: diameter_px,
            expected: diameter,
            diameter: true,
            ignored: false,
            line: diameter_line,
        },
    ]
}

fn centered_line(line: [f64; 4], length_px: f64) -> [f64; 4] {
    let len = line_len(line);
    if len < 1.0 || !length_px.is_finite() {
        return line;
    }
    let center_x = (line[0] + line[2]) * 0.5;
    let center_y = (line[1] + line[3]) * 0.5;
    let unit_x = (line[2] - line[0]) / len;
    let unit_y = (line[3] - line[1]) / len;
    let half = length_px * 0.5;
    [
        center_x - unit_x * half,
        center_y - unit_y * half,
        center_x + unit_x * half,
        center_y + unit_y * half,
    ]
}

fn method_label(method: &str, pairs: &[(String, f64)]) -> String {
    match method {
        "bolt" => {
            let length = find_mm(
                pairs,
                &[
                    "lengthunderhead",
                    "underheadlength",
                    "shanklength",
                    "griplength",
                    "length",
                    "lengthmm",
                    "len",
                    "l",
                ],
            );
            let diameter = find_mm(
                pairs,
                &[
                    "threaddiameter",
                    "threaddia",
                    "nominaldiameter",
                    "diameter",
                    "diametermm",
                    "dia",
                ],
            );
            expected_label(&ExpectedSize {
                diameter_mm: diameter,
                length_mm: length,
                description: String::new(),
            })
        }
        "nut" => find_mm(pairs, &["acrossflats", "widthacrossflats", "af", "flats"])
            .map(|across| format!("{} mm", compact_number(across)))
            .unwrap_or_default(),
        "washer" => {
            let outer = find_mm(
                pairs,
                &[
                    "outerdiameter",
                    "od",
                    "outsidediameter",
                    "diameter",
                    "diametermm",
                    "dia",
                ],
            );
            let inner = find_mm(pairs, &["innerdiameter", "id", "insidediameter"]);
            match (outer, inner) {
                (Some(outer), Some(inner)) => {
                    format!("Ø{} × Ø{} mm", compact_number(outer), compact_number(inner))
                }
                (Some(outer), None) => format!("Ø{} mm", compact_number(outer)),
                (None, Some(inner)) => format!("Ø{} mm", compact_number(inner)),
                (None, None) => String::new(),
            }
        }
        "cable_tie" => {
            let length = find_mm(
                pairs,
                &[
                    "length",
                    "totallength",
                    "overalllength",
                    "lengthmm",
                    "len",
                    "l",
                ],
            );
            let width = find_mm(pairs, &["strapwidth", "width", "bandwidth"]);
            match (length, width) {
                (Some(length), Some(width)) => {
                    format!("{} × {} mm", compact_number(length), compact_number(width))
                }
                (Some(length), None) => format!("{} mm", compact_number(length)),
                (None, Some(width)) => format!("{} mm", compact_number(width)),
                (None, None) => String::new(),
            }
        }
        "spring" => {
            let height = find_mm(
                pairs,
                &[
                    "freeheight",
                    "height",
                    "heightmm",
                    "h",
                    "length",
                    "lengthmm",
                    "len",
                    "l",
                ],
            );
            let diameter = find_mm(
                pairs,
                &[
                    "outerdiameter",
                    "coildiameter",
                    "od",
                    "diameter",
                    "diametermm",
                    "dia",
                ],
            );
            match (height, diameter) {
                (Some(height), Some(diameter)) => format!(
                    "{} × Ø{} mm",
                    compact_number(height),
                    compact_number(diameter)
                ),
                (Some(height), None) => format!("{} mm", compact_number(height)),
                (None, Some(diameter)) => format!("Ø{} mm", compact_number(diameter)),
                (None, None) => String::new(),
            }
        }
        _ => String::new(),
    }
}

fn result_from_gauges(
    gauges: &[Gauge],
    origin_x: f64,
    origin_y: f64,
    mm_per_px: f64,
    label: &str,
    settings: &MetrologySettings,
) -> Option<MetrologyResult> {
    if gauges.is_empty() {
        return None;
    }
    let mut bad = Vec::new();
    for gauge in gauges {
        if gauge.ignored {
            continue;
        }
        let Some(want) = gauge.expected else {
            continue;
        };
        let measured = gauge.px * mm_per_px;
        if !measured.is_finite() {
            return None;
        }
        let tolerance = if gauge.diameter {
            settings.diameter_tolerance_mm
        } else {
            settings.length_tolerance_mm
        };
        if (measured - want).abs() > tolerance {
            bad.push(gauge);
        }
    }
    let status = if bad.is_empty() {
        MetrologyStatus::Pass
    } else {
        MetrologyStatus::DimensionMismatch
    };
    let note = if status == MetrologyStatus::Pass {
        String::new()
    } else {
        let measured = bad
            .iter()
            .map(|gauge| {
                format!(
                    "{} {}",
                    gauge_name(gauge.kind),
                    gauge_text(gauge, mm_per_px)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let expected_text = if label.is_empty() {
            "part".to_string()
        } else {
            label.to_string()
        };
        format!("Expected {expected_text}, measured {measured}")
    };
    let (length_mm, expected_length_mm) = primary_linear(gauges, mm_per_px);
    let (diameter_mm, expected_diameter_mm) = primary_diameter(gauges, mm_per_px);
    let mut spans = Vec::new();
    for gauge in gauges {
        let line = (gauge.line[0], gauge.line[1], gauge.line[2], gauge.line[3]);
        if let Some(span) = make_span(
            gauge.kind,
            &gauge_text(gauge, mm_per_px),
            line,
            origin_x,
            origin_y,
        ) {
            spans.push(span);
        }
    }
    Some(MetrologyResult {
        status,
        length_mm,
        diameter_mm,
        expected_length_mm,
        expected_diameter_mm,
        expected_label: label.to_string(),
        note,
        spans,
    })
}

fn primary_linear(gauges: &[Gauge], mm_per_px: f64) -> (f64, Option<f64>) {
    let compared = gauges
        .iter()
        .find(|gauge| !gauge.diameter && !gauge.ignored);
    let gauge = compared.or_else(|| gauges.iter().find(|gauge| !gauge.diameter));
    match gauge {
        Some(gauge) => (
            round_mm(gauge.px * mm_per_px),
            if gauge.ignored { None } else { gauge.expected },
        ),
        None => (0.0, None),
    }
}

fn primary_diameter(gauges: &[Gauge], mm_per_px: f64) -> (f64, Option<f64>) {
    match gauges.iter().find(|gauge| gauge.diameter && !gauge.ignored) {
        Some(gauge) => (round_mm(gauge.px * mm_per_px), gauge.expected),
        None => (0.0, None),
    }
}

fn round_mm(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    (value * 100.0).round() / 100.0
}

fn gauge_text(gauge: &Gauge, mm_per_px: f64) -> String {
    let measured = round_mm(gauge.px * mm_per_px);
    if gauge.diameter {
        format!("Ø {} mm", one_decimal(measured))
    } else {
        format!("{} mm", one_decimal(measured))
    }
}

fn gauge_name(kind: &str) -> &str {
    match kind {
        "length_under_head" => "length under head",
        "thread_diameter" => "thread diameter",
        "across_flats" => "across flats",
        "outer_diameter" => "OD",
        "inner_diameter" => "ID",
        "thickness" => "thickness",
        "width" => "strap width",
        "free_height" => "free height",
        "coil_diameter" => "coil diameter",
        "length" => "length",
        _ => kind,
    }
}

fn part_mask(
    rgb: &[u8],
    width: i32,
    height: i32,
    settings: &MetrologySettings,
) -> Result<Vec<u8>, String> {
    let color = Mat::new_rows_cols_with_bytes::<Vec3b>(height, width, rgb)
        .map_err(|err| err.to_string())?;
    let mut gray = Mat::default();
    opencv::imgproc::cvt_color_def(&color, &mut gray, opencv::imgproc::COLOR_RGB2GRAY)
        .map_err(|err| err.to_string())?;
    let image_area = f64::from(width) * f64::from(height);
    let mut best_area = 0.0;
    let mut best_mask = None;
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
        let mut area_here = 0.0;
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
            if area > area_here {
                area_here = area;
            }
        }
        if area_here > best_area {
            best_area = area_here;
            best_mask = Some(mask_bytes(&binary)?);
        }
    }
    best_mask.ok_or_else(|| "no contour".to_string())
}

fn mask_bytes(binary: &Mat) -> Result<Vec<u8>, String> {
    let rows = binary.rows();
    let cols = binary.cols();
    let mut out = vec![0_u8; (rows as usize).saturating_mul(cols as usize)];
    for row in 0..rows {
        for col in 0..cols {
            let pixel = *binary
                .at_2d::<u8>(row, col)
                .map_err(|err| err.to_string())?;
            if pixel > 0 {
                out[(row * cols + col) as usize] = 1;
            }
        }
    }
    Ok(out)
}

fn mask_on(mask: &[u8], width: i32, height: i32, x: f64, y: f64) -> bool {
    if !x.is_finite() || !y.is_finite() || width < 1 || height < 1 {
        return false;
    }
    let col = x.round() as i32;
    let row = y.round() as i32;
    if col < 0 || row < 0 || col >= width || row >= height {
        return false;
    }
    mask[(row * width + col) as usize] > 0
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

struct CaliperHit {
    length_px: f64,
    diameter_px: f64,
    rect: RotatedRect,
}

fn caliper(
    rgb: &[u8],
    width: i32,
    height: i32,
    settings: &MetrologySettings,
) -> Result<CaliperHit, String> {
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
    Ok(CaliperHit {
        length_px: side_a.max(side_b),
        diameter_px: side_a.min(side_b),
        rect,
    })
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

/// The two centre-lines of the min-area rectangle, shifted into the full frame.
/// Length is the longer side. Diameter is the shorter side. This does not
/// change which side was measured.
fn spans_from_rect(
    rect: RotatedRect,
    origin_x: f64,
    origin_y: f64,
    length_mm: f64,
    diameter_mm: f64,
) -> Vec<MetrologySpan> {
    let mut corners = [Point2f::new(0.0, 0.0); 4];
    if rect.points(&mut corners).is_err() {
        return Vec::new();
    }
    let width_line = midline(corners[0], corners[1], corners[2], corners[3]);
    let height_line = midline(corners[1], corners[2], corners[3], corners[0]);
    let width = f64::from(rect.size.width);
    let height = f64::from(rect.size.height);
    if !width.is_finite() || !height.is_finite() {
        return Vec::new();
    }
    let (length_line, diameter_line) = if width >= height {
        (width_line, height_line)
    } else {
        (height_line, width_line)
    };
    let mut spans = Vec::new();
    if let Some(span) = make_span(
        "length",
        &format!("{} mm", one_decimal(length_mm)),
        length_line,
        origin_x,
        origin_y,
    ) {
        spans.push(span);
    }
    if let Some(span) = make_span(
        "diameter",
        &format!("Ø {} mm", one_decimal(diameter_mm)),
        diameter_line,
        origin_x,
        origin_y,
    ) {
        spans.push(span);
    }
    spans
}

fn midline(a: Point2f, b: Point2f, c: Point2f, d: Point2f) -> (f64, f64, f64, f64) {
    (
        (f64::from(a.x) + f64::from(b.x)) * 0.5,
        (f64::from(a.y) + f64::from(b.y)) * 0.5,
        (f64::from(c.x) + f64::from(d.x)) * 0.5,
        (f64::from(c.y) + f64::from(d.y)) * 0.5,
    )
}

fn make_span(
    kind: &str,
    text: &str,
    line: (f64, f64, f64, f64),
    origin_x: f64,
    origin_y: f64,
) -> Option<MetrologySpan> {
    let (x1, y1, x2, y2) = line;
    let points = [x1 + origin_x, y1 + origin_y, x2 + origin_x, y2 + origin_y];
    if points.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let dx = points[2] - points[0];
    let dy = points[3] - points[1];
    if dx * dx + dy * dy < 1.0 {
        return None;
    }
    Some(MetrologySpan {
        kind: kind.to_string(),
        text: text.to_string(),
        x1: round2(points[0]),
        y1: round2(points[1]),
        x2: round2(points[2]),
        y2: round2(points[3]),
    })
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
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
        assert!(pass.spans.is_empty());
        let value = serde_json::to_value(&pass).unwrap();
        assert!(value.get("note").is_none());
        assert!(value.get("spans").is_none());
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
            class_id: 0,
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
                class_id: 0,
            },
            BoxMeasure {
                bbox: [10, 10, 40, 40],
                matched: false,
                ic: "unknown".to_string(),
                class_id: 0,
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
        let length_span = result
            .spans
            .iter()
            .find(|span| span.kind == "length")
            .expect("length span");
        let diameter_span = result
            .spans
            .iter()
            .find(|span| span.kind == "diameter")
            .expect("diameter span");
        assert_eq!(
            length_span.text,
            format!("{} mm", one_decimal(result.length_mm))
        );
        assert!(diameter_span.text.starts_with("Ø "));
        assert!(diameter_span.text.ends_with(" mm"));
        let length_dx = length_span.x2 - length_span.x1;
        let length_dy = length_span.y2 - length_span.y1;
        let diameter_dx = diameter_span.x2 - diameter_span.x1;
        let diameter_dy = diameter_span.y2 - diameter_span.y1;
        assert!(
            length_dx.abs() > 80.0 && length_dy.abs() < 5.0,
            "length line {length_dx},{length_dy}"
        );
        assert!(
            diameter_dy.abs() > 20.0 && diameter_dx.abs() < 5.0,
            "diameter line {diameter_dx},{diameter_dy}"
        );
        let dot = length_dx * diameter_dx + length_dy * diameter_dy;
        let lengths = (length_dx * length_dx + length_dy * length_dy).sqrt()
            * (diameter_dx * diameter_dx + diameter_dy * diameter_dy).sqrt();
        assert!((dot / lengths).abs() < 0.15, "axes are not perpendicular");
        let mid_x = (length_span.x1 + length_span.x2) / 2.0;
        let mid_y = (length_span.y1 + length_span.y2) / 2.0;
        assert!((mid_x - 90.0).abs() < 8.0 && (mid_y - 65.0).abs() < 8.0);

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
        assert_eq!(result.spans.len(), 2);
        assert!(
            result
                .spans
                .iter()
                .any(|span| span.kind == "diameter" && span.text.starts_with("Ø "))
        );
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
                class_id: 0,
            },
            BoxMeasure {
                bbox: [40, 50, 140, 80],
                matched: true,
                ic: "NOT-IN-CATALOGUE".to_string(),
                class_id: 0,
            },
        ];
        let results = measure_boxes(Some(&document), &rgb, width, height, &boxes);
        assert!(results[0].is_none());
        let matched = results[1].as_ref().expect("matched box");
        assert_eq!(matched.status, MetrologyStatus::Pass);
    }

    fn fill(data: &mut [u8], width: u32, height: u32, x: u32, y: u32, w: u32, h: u32, on: bool) {
        let value = if on { 255 } else { 0 };
        for row in y..(y + h).min(height) {
            for col in x..(x + w).min(width) {
                let index = ((row * width + col) * 3) as usize;
                data[index] = value;
                data[index + 1] = value;
                data[index + 2] = value;
            }
        }
    }

    fn span_length(span: &MetrologySpan) -> f64 {
        let dx = span.x2 - span.x1;
        let dy = span.y2 - span.y1;
        (dx * dx + dy * dy).sqrt()
    }

    #[test]
    fn a_bolt_uses_length_under_the_head() {
        let width = 260_u32;
        let height = 180_u32;
        let mut rgb = vec![0_u8; (width * height * 3) as usize];
        fill(&mut rgb, width, height, 40, 50, 30, 80, true);
        fill(&mut rgb, width, height, 70, 70, 120, 40, true);
        let mut settings = sample_settings(ThresholdMode::Otsu);
        settings.length_tolerance_mm = 0.8;
        settings.diameter_tolerance_mm = 0.8;
        let result = measure_method(
            &rgb,
            width,
            height,
            [40, 50, 190, 130],
            &settings,
            0.1,
            "hex bolt",
            &json!({ "lengthUnderHead": 12, "threadDiameter": 4, "thickness": 8 }),
            "bolt",
        )
        .expect("bolt");
        assert_eq!(result.status, MetrologyStatus::Pass, "{}", result.note);
        assert!(
            (result.length_mm - 12.0).abs() < 0.8,
            "under head {}",
            result.length_mm
        );
        assert!(
            (result.length_mm - 15.0).abs() > 1.5,
            "full length {}",
            result.length_mm
        );
        assert!(
            (result.diameter_mm - 4.0).abs() < 0.8,
            "thread {}",
            result.diameter_mm
        );
        assert!(
            (result.diameter_mm - 8.0).abs() > 1.5,
            "head width {}",
            result.diameter_mm
        );
        let length = result
            .spans
            .iter()
            .find(|span| span.kind == "length_under_head")
            .expect("length under head");
        let thread = result
            .spans
            .iter()
            .find(|span| span.kind == "thread_diameter")
            .expect("thread");
        assert!(!length.text.contains('Ø'), "{}", length.text);
        assert!(thread.text.starts_with("Ø "), "{}", thread.text);
        let drawn = span_length(length);
        assert!(
            (100.0..135.0).contains(&drawn),
            "length line {drawn} px is the full bolt"
        );
    }

    #[test]
    fn a_washer_reports_od_id_and_ignores_thickness() {
        let width = 200_u32;
        let height = 80_u32;
        let mut rgb = vec![0_u8; (width * height * 3) as usize];
        fill(&mut rgb, width, height, 30, 30, 80, 16, true);
        fill(&mut rgb, width, height, 50, 34, 40, 8, false);
        let mut settings = sample_settings(ThresholdMode::Fixed);
        settings.length_tolerance_mm = 0.6;
        settings.diameter_tolerance_mm = 0.6;
        let result = measure_method(
            &rgb,
            width,
            height,
            [30, 30, 110, 46],
            &settings,
            0.1,
            "washer",
            &json!({ "outerDiameter": 8, "innerDiameter": 4, "thickness": 9 }),
            "washer",
        )
        .expect("washer");
        assert_eq!(result.status, MetrologyStatus::Pass, "{}", result.note);
        let outer = result
            .spans
            .iter()
            .find(|span| span.kind == "outer_diameter")
            .expect("OD");
        let inner = result
            .spans
            .iter()
            .find(|span| span.kind == "inner_diameter")
            .expect("ID");
        let thickness = result
            .spans
            .iter()
            .find(|span| span.kind == "thickness")
            .expect("thickness");
        assert!(outer.text.starts_with("Ø "), "{}", outer.text);
        assert!(inner.text.starts_with("Ø "), "{}", inner.text);
        assert!(!thickness.text.contains('Ø'), "{}", thickness.text);
        assert!(
            thickness.text.starts_with("1.") || thickness.text.starts_with("2."),
            "{}",
            thickness.text
        );
        let missed = measure_method(
            &rgb,
            width,
            height,
            [30, 30, 110, 46],
            &settings,
            0.1,
            "washer",
            &json!({ "outerDiameter": 14, "innerDiameter": 4, "thickness": 1.6 }),
            "washer",
        )
        .expect("od miss");
        assert_eq!(missed.status, MetrologyStatus::DimensionMismatch);
        assert!(missed.note.contains("OD"), "{}", missed.note);
    }

    #[test]
    fn a_part_without_a_method_keeps_the_generic_caliper() {
        let width = 220_u32;
        let height = 140_u32;
        let rgb = white_on_black(width, height, 40, 50, 100, 30);
        let result = measure_method(
            &rgb,
            width,
            height,
            [40, 50, 140, 80],
            &sample_settings(ThresholdMode::Otsu),
            0.1,
            "",
            &json!({ "diameterMm": 3, "lengthMm": 10 }),
            "custom",
        )
        .expect("generic");
        assert_eq!(result.status, MetrologyStatus::Pass, "{}", result.note);
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
        assert!(result.spans.iter().any(|span| span.kind == "length"
            && !span.text.contains('Ø')
            && span.text.ends_with(" mm")));
        assert!(
            result
                .spans
                .iter()
                .any(|span| span.kind == "diameter" && span.text.starts_with("Ø "))
        );
    }

    #[test]
    fn a_label_class_is_not_measured() {
        let width = 220_u32;
        let height = 140_u32;
        let rgb = white_on_black(width, height, 40, 50, 100, 30);
        let document = json!({
            "metrology": {
                "enabled": true,
                "marginPercent": 5,
                "thresholdMode": "otsu",
                "minContourAreaPx": 200,
                "lengthToleranceMm": 0.5,
                "diameterToleranceMm": 0.5
            },
            "camera": {
                "index": 0,
                "scale": { "mmPerPx": 0.1, "width": width, "height": height }
            }
        });
        let label = vec![BoxMeasure {
            bbox: [40, 50, 140, 80],
            matched: true,
            ic: "LABEL".to_string(),
            class_id: 3,
        }];
        let skipped = measure_boxes(Some(&document), &rgb, width, height, &label);
        assert!(skipped[0].is_none());
        let part = vec![BoxMeasure {
            bbox: [40, 50, 140, 80],
            matched: true,
            ic: "NOT-IN-CATALOGUE".to_string(),
            class_id: 0,
        }];
        let measured = measure_boxes(Some(&document), &rgb, width, height, &part);
        assert!(measured[0].is_some());
    }

    #[test]
    fn a_nut_reports_across_flats_without_a_diameter_symbol() {
        let width = 160_u32;
        let height = 140_u32;
        let rgb = white_on_black(width, height, 20, 20, 50, 80);
        let mut settings = sample_settings(ThresholdMode::Otsu);
        settings.length_tolerance_mm = 0.5;
        let result = measure_method(
            &rgb,
            width,
            height,
            [20, 20, 70, 100],
            &settings,
            0.1,
            "nut",
            &json!({ "acrossFlats": 5, "thickness": 20 }),
            "nut",
        )
        .expect("nut");
        assert_eq!(result.status, MetrologyStatus::Pass, "{}", result.note);
        assert!((result.length_mm - 5.0).abs() < 0.5, "{}", result.length_mm);
        assert!(
            (result.length_mm - 8.0).abs() > 1.0,
            "thickness {}",
            result.length_mm
        );
        assert_eq!(result.spans.len(), 1);
        assert_eq!(result.spans[0].kind, "across_flats");
        assert!(
            !result.spans[0].text.contains('Ø'),
            "{}",
            result.spans[0].text
        );
    }

    #[test]
    fn a_cable_tie_measures_length_and_strap_width() {
        let width = 180_u32;
        let height = 80_u32;
        let rgb = white_on_black(width, height, 20, 30, 100, 16);
        let mut settings = sample_settings(ThresholdMode::Otsu);
        settings.length_tolerance_mm = 0.5;
        settings.diameter_tolerance_mm = 0.05;
        let result = measure_method(
            &rgb,
            width,
            height,
            [20, 30, 120, 46],
            &settings,
            0.1,
            "cable tie",
            &json!({ "length": 10, "strapWidth": 1.2 }),
            "cable_tie",
        )
        .expect("cable tie");
        assert_eq!(result.status, MetrologyStatus::Pass, "{}", result.note);
        assert_eq!(result.spans.len(), 2);
        assert!(
            result.spans.iter().all(|span| !span.text.contains('Ø')),
            "{:?}",
            result.spans
        );
        assert!(result.spans.iter().any(|span| span.kind == "length"));
        assert!(result.spans.iter().any(|span| span.kind == "width"));
    }

    #[test]
    fn a_spring_marks_only_the_coil_as_a_diameter() {
        let width = 180_u32;
        let height = 80_u32;
        let rgb = white_on_black(width, height, 20, 20, 80, 30);
        let mut settings = sample_settings(ThresholdMode::Otsu);
        settings.length_tolerance_mm = 0.5;
        settings.diameter_tolerance_mm = 0.5;
        let result = measure_method(
            &rgb,
            width,
            height,
            [20, 20, 100, 50],
            &settings,
            0.1,
            "spring",
            &json!({ "freeHeight": 3, "outerDiameter": 8 }),
            "spring",
        )
        .expect("spring");
        assert_eq!(result.status, MetrologyStatus::Pass, "{}", result.note);
        let height_span = result
            .spans
            .iter()
            .find(|span| span.kind == "free_height")
            .expect("height");
        let coil = result
            .spans
            .iter()
            .find(|span| span.kind == "coil_diameter")
            .expect("coil");
        assert!(!height_span.text.contains('Ø'), "{}", height_span.text);
        assert!(coil.text.starts_with("Ø "), "{}", coil.text);
        assert!(span_length(coil) > span_length(height_span));
    }
}
