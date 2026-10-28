//! In-memory catalogue used to name a YOLO box.
//!
//! Startup reads `vision/catalogue.json` into Tauri state. Each stored
//! embedding is scaled to unit length. A missing or empty file logs a warning
//! and leaves the list empty. Matching does not write the file and does not
//! change its schema.
//!
//! A box is given as fractions of the frame. Those fractions become pixels,
//! then each edge is clamped to the frame. The crop is embedded with DINOv2
//! and scored against every catalogue vector with a dot product. The best IC
//! is kept, plus the next three different ICs. A score below 0.50, or an empty
//! catalogue, is "No Match".

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::frame::FrameState;
use crate::inference::dino;

/// A score below this is not a named part.
pub(crate) const MATCH_THRESHOLD: f32 = 0.50;
const RUNNERS_UP: usize = 3;
const CATALOGUE_VERSION: u32 = 1;

/// One catalogue crop after its embedding has been L2-normalised.
#[derive(Clone, Debug)]
pub(crate) struct CatalogEntry {
    pub id: String,
    pub ic: String,
    pub description: String,
    pub part_type: String,
    pub embedding: Vec<f32>,
}

/// One ranked IC. `score` is the cosine similarity, already a dot product.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCandidate {
    pub ic: String,
    pub description: String,
    pub part_type: String,
    pub score: f32,
}

/// The best catalogue IC for one box, plus up to three other ICs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogMatch {
    pub recognized: bool,
    pub label: String,
    pub top: Option<CatalogCandidate>,
    pub runners_up: Vec<CatalogCandidate>,
}

#[derive(Clone)]
pub struct CatalogStore {
    entries: Arc<Mutex<Arc<Vec<CatalogEntry>>>>,
}

static INSTALLED: LazyLock<Mutex<Option<CatalogStore>>> = LazyLock::new(|| Mutex::new(None));

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

impl CatalogStore {
    /// Read the catalogue beside `vision/config.local.json`.
    pub fn open() -> Self {
        let store = Self::from_entries(read_installed_file());
        *lock(&INSTALLED) = Some(store.clone());
        store
    }

    fn from_entries(entries: Vec<CatalogEntry>) -> Self {
        Self {
            entries: Arc::new(Mutex::new(Arc::new(entries))),
        }
    }

    pub fn snapshot(&self) -> Arc<Vec<CatalogEntry>> {
        lock(&self.entries).clone()
    }

    fn replace(&self, entries: Vec<CatalogEntry>) {
        *lock(&self.entries) = Arc::new(entries);
    }
}

/// The list held by [`CatalogStore::open`]. Empty when startup has not run.
pub(crate) fn snapshot() -> Arc<Vec<CatalogEntry>> {
    let guard = lock(&INSTALLED);
    if let Some(store) = guard.as_ref() {
        return store.snapshot();
    }
    drop(guard);
    if cfg!(test) {
        return Arc::new(Vec::new());
    }
    CatalogStore::open().snapshot()
}

/// Read the catalogue again after Settings imports or deletes a part.
pub(crate) fn reload_installed() {
    let Some(store) = lock(&INSTALLED).clone() else {
        return;
    };
    store.replace(read_installed_file());
}

fn catalogue_file() -> Result<PathBuf, String> {
    Ok(crate::local_config::project_root()?
        .join("vision")
        .join("catalogue.json"))
}

fn read_installed_file() -> Vec<CatalogEntry> {
    match catalogue_file() {
        Ok(path) => read_catalogue(&path),
        Err(err) => {
            eprintln!("warning: {err} Catalogue matching has no parts.");
            Vec::new()
        }
    }
}

/// Parse one catalogue file. A missing, empty, or unreadable file is an empty list.
pub(crate) fn read_catalogue(path: &Path) -> Vec<CatalogEntry> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            eprintln!(
                "warning: catalogue.json is missing at {}. Catalogue matching has no parts.",
                path.display()
            );
            return Vec::new();
        }
        Err(err) => {
            eprintln!(
                "warning: could not read catalogue.json at {} ({err}). Catalogue matching has no parts.",
                path.display()
            );
            return Vec::new();
        }
    };
    if text.trim().is_empty() {
        eprintln!(
            "warning: catalogue.json at {} is empty. Catalogue matching has no parts.",
            path.display()
        );
        return Vec::new();
    }
    let file: StoredFile = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(err) => {
            eprintln!(
                "warning: catalogue.json at {} is not a valid catalogue ({err}). Catalogue matching has no parts.",
                path.display()
            );
            return Vec::new();
        }
    };
    if file.version != Some(CATALOGUE_VERSION) {
        eprintln!(
            "warning: catalogue.json at {} is not a valid catalogue. Catalogue matching has no parts.",
            path.display()
        );
        return Vec::new();
    }
    let entries = entries_from_parts(file.parts);
    if entries.is_empty() {
        eprintln!(
            "warning: catalogue.json at {} has no parts. Catalogue matching has no parts.",
            path.display()
        );
    }
    entries
}

fn entries_from_parts(parts: Vec<StoredPart>) -> Vec<CatalogEntry> {
    let mut entries = Vec::new();
    for part in parts {
        let ic = part.ic.trim();
        if ic.is_empty() {
            continue;
        }
        for (index, values) in part.embeddings.into_iter().enumerate() {
            let Some(embedding) = unit_embedding(&values) else {
                continue;
            };
            entries.push(CatalogEntry {
                id: format!("{ic}#{index}"),
                ic: ic.to_string(),
                description: part.description.trim().to_string(),
                part_type: part.part_type.trim().to_string(),
                embedding,
            });
        }
    }
    entries
}

fn unit_embedding(values: &[f32]) -> Option<Vec<f32>> {
    dino::l2_normalize(values).ok()
}

#[derive(Deserialize)]
struct StoredFile {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    parts: Vec<StoredPart>,
}

#[derive(Deserialize)]
struct StoredPart {
    #[serde(default)]
    ic: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "type")]
    part_type: String,
    #[serde(default)]
    embeddings: Vec<Vec<f32>>,
}

/// Score pixel boxes from a detector. Coordinates are clamped to the frame.
pub(crate) fn match_pixel_boxes(
    entries: &[CatalogEntry],
    rgb: &[u8],
    width: u32,
    height: u32,
    boxes: &[[i32; 4]],
) -> Vec<CatalogMatch> {
    let mut warned = false;
    boxes
        .iter()
        .map(|bbox| {
            let norm = pixels_to_normalized(*bbox, width, height);
            match_normalized_warned(entries, rgb, width, height, norm, &mut warned)
        })
        .collect()
}

/// `box_norm` is `x1, y1, x2, y2` as fractions of the frame.
pub(crate) fn match_normalized(
    entries: &[CatalogEntry],
    rgb: &[u8],
    width: u32,
    height: u32,
    box_norm: [f32; 4],
) -> CatalogMatch {
    let mut warned = false;
    match_normalized_warned(entries, rgb, width, height, box_norm, &mut warned)
}

fn match_normalized_warned(
    entries: &[CatalogEntry],
    rgb: &[u8],
    width: u32,
    height: u32,
    box_norm: [f32; 4],
    warned: &mut bool,
) -> CatalogMatch {
    if entries.is_empty() {
        return unrecognized();
    }
    let pixels = normalized_to_pixels(box_norm, width, height);
    let (crop, crop_w, crop_h) = match crop_clamped(rgb, width, height, pixels) {
        Ok(crop) => crop,
        Err(err) => {
            warn_once(warned, &err);
            return unrecognized();
        }
    };
    let query = match dino::embed_rgb(&crop, crop_w, crop_h) {
        Ok(query) => query,
        Err(err) => {
            warn_once(warned, &err);
            return unrecognized();
        }
    };
    score_query(entries, &query)
}

fn warn_once(warned: &mut bool, err: &str) {
    if *warned {
        return;
    }
    *warned = true;
    eprintln!("warning: catalogue match skipped: {err}");
}

/// Rank a unit-length query. Used by tests and by the crop matcher.
pub(crate) fn score_query(entries: &[CatalogEntry], query: &[f32]) -> CatalogMatch {
    if entries.is_empty() || query.is_empty() {
        return unrecognized();
    }
    let mut best: HashMap<String, CatalogCandidate> = HashMap::new();
    for entry in entries {
        let Some(score) = dot(query, &entry.embedding) else {
            continue;
        };
        let replace = best
            .get(&entry.ic)
            .is_none_or(|current| score > current.score);
        if !replace {
            continue;
        }
        best.insert(
            entry.ic.clone(),
            CatalogCandidate {
                ic: entry.ic.clone(),
                description: entry.description.clone(),
                part_type: entry.part_type.clone(),
                score,
            },
        );
    }
    let mut ranked: Vec<CatalogCandidate> = best.into_values().collect();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.ic.cmp(&right.ic))
    });
    let top = ranked.first().cloned();
    let recognized = top
        .as_ref()
        .is_some_and(|candidate| candidate.score >= MATCH_THRESHOLD);
    let runners_up = ranked.into_iter().skip(1).take(RUNNERS_UP).collect();
    CatalogMatch {
        label: label_text(recognized, top.as_ref()),
        recognized,
        top,
        runners_up,
    }
}

fn unrecognized() -> CatalogMatch {
    CatalogMatch {
        recognized: false,
        label: "No Match".to_string(),
        top: None,
        runners_up: Vec::new(),
    }
}

fn label_text(recognized: bool, top: Option<&CatalogCandidate>) -> String {
    if !recognized {
        return "No Match".to_string();
    }
    let Some(top) = top else {
        return "No Match".to_string();
    };
    format!("{} ({}%)", top.ic, percent(top.score))
}

fn percent(score: f32) -> i32 {
    let value = f64::from(score) * 100.0;
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(0.0, 100.0) as i32
}

fn dot(query: &[f32], target: &[f32]) -> Option<f32> {
    if query.len() != target.len() || query.is_empty() {
        return None;
    }
    let mut score = 0.0f32;
    for (left, right) in query.iter().zip(target) {
        score += left * right;
    }
    if score.is_finite() { Some(score) } else { None }
}

fn pixels_to_normalized(bbox: [i32; 4], width: u32, height: u32) -> [f32; 4] {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    [
        bbox[0] as f32 / width,
        bbox[1] as f32 / height,
        bbox[2] as f32 / width,
        bbox[3] as f32 / height,
    ]
}

/// Inclusive pixel box. Each edge is inside `0..=extent-1`.
pub(crate) fn normalized_to_pixels(box_norm: [f32; 4], width: u32, height: u32) -> [i32; 4] {
    [
        axis_pixel(box_norm[0], width),
        axis_pixel(box_norm[1], height),
        axis_pixel(box_norm[2], width),
        axis_pixel(box_norm[3], height),
    ]
}

fn axis_pixel(value: f32, extent: u32) -> i32 {
    let limit = extent.saturating_sub(1) as i32;
    if extent == 0 || !value.is_finite() {
        return 0;
    }
    let scaled = f64::from(value) * f64::from(extent);
    let rounded = scaled.round();
    let pixel = if rounded.is_finite() {
        rounded.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    } else if scaled.is_sign_negative() {
        i32::MIN
    } else {
        i32::MAX
    };
    pixel.clamp(0, limit)
}

/// Copy the inclusive rectangle. Out-of-range edges are clamped, not panicked.
pub(crate) fn crop_clamped(
    rgb: &[u8],
    width: u32,
    height: u32,
    bbox: [i32; 4],
) -> Result<(Vec<u8>, u32, u32), String> {
    if width == 0 || height == 0 {
        return Err("empty crop".to_string());
    }
    let x_limit = width as i32 - 1;
    let y_limit = height as i32 - 1;
    let x1 = bbox[0].min(bbox[2]).clamp(0, x_limit);
    let x2 = bbox[0].max(bbox[2]).clamp(0, x_limit);
    let y1 = bbox[1].min(bbox[3]).clamp(0, y_limit);
    let y2 = bbox[1].max(bbox[3]).clamp(0, y_limit);
    let crop_w = (x2 - x1 + 1) as usize;
    let crop_h = (y2 - y1 + 1) as usize;
    let frame_w = width as usize;
    let expected = frame_w.saturating_mul(height as usize).saturating_mul(3);
    if expected == 0 || rgb.len() < expected || crop_w == 0 || crop_h == 0 {
        return Err("empty crop".to_string());
    }
    let mut pixels = Vec::with_capacity(crop_w * crop_h * 3);
    for row in 0..crop_h {
        let y = y1 as usize + row;
        let start = (y * frame_w + x1 as usize) * 3;
        let end = start + crop_w * 3;
        if end > expected {
            return Err("empty crop".to_string());
        }
        pixels.extend_from_slice(&rgb[start..end]);
    }
    Ok((pixels, crop_w as u32, crop_h as u32))
}

/// Match one normalised box on the latest full-resolution frame.
#[tauri::command]
pub async fn match_box_dino(
    frames: State<'_, FrameState>,
    catalog: State<'_, CatalogStore>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
) -> Result<CatalogMatch, String> {
    let entries = catalog.snapshot();
    let frame = {
        let guard = frames.latest.lock().unwrap_or_else(|err| err.into_inner());
        guard.clone()
    };
    let matched = tauri::async_runtime::spawn_blocking(move || {
        let Some(frame) = frame else {
            eprintln!("warning: catalogue match has no frame.");
            return unrecognized();
        };
        if frame.channels != 3 || frame.width == 0 || frame.height == 0 {
            eprintln!("warning: catalogue match has no frame.");
            return unrecognized();
        }
        match_normalized(
            entries.as_ref(),
            &frame.data,
            frame.width,
            frame.height,
            [x1 as f32, y1 as f32, x2 as f32, y2 as f32],
        )
    })
    .await
    .map_err(|_| "Catalogue match failed".to_string())?;
    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ic: &str, description: &str, part_type: &str, embedding: Vec<f32>) -> CatalogEntry {
        CatalogEntry {
            id: format!("{ic}#0"),
            ic: ic.to_string(),
            description: description.to_string(),
            part_type: part_type.to_string(),
            embedding,
        }
    }

    fn unit(values: &[f32]) -> Vec<f32> {
        dino::l2_normalize(values).unwrap()
    }

    #[test]
    fn missing_and_empty_catalogues_load_as_an_empty_list() {
        let missing = std::env::temp_dir().join(format!(
            "ls-vision-catalogue-missing-{}-{}",
            std::process::id(),
            "a"
        ));
        let _ = fs::remove_file(&missing);
        assert!(read_catalogue(&missing).is_empty());

        let empty = std::env::temp_dir().join(format!(
            "ls-vision-catalogue-empty-{}-{}",
            std::process::id(),
            "b"
        ));
        fs::write(&empty, " \n").unwrap();
        assert!(read_catalogue(&empty).is_empty());
        fs::write(&empty, "{\"version\":1,\"parts\":[]}").unwrap();
        assert!(read_catalogue(&empty).is_empty());
        fs::write(&empty, "not json").unwrap();
        assert!(read_catalogue(&empty).is_empty());
        let _ = fs::remove_file(&empty);
    }

    #[test]
    fn stored_embeddings_are_scaled_to_unit_length() {
        let path = std::env::temp_dir().join(format!(
            "ls-vision-catalogue-unit-{}.json",
            std::process::id()
        ));
        fs::write(
            &path,
            r#"{
                "version": 1,
                "parts": [{
                    "ic": "FAST-00124",
                    "description": "Bolt",
                    "type": "fastener",
                    "crops": ["catalogue-crops/FAST-00124/crop.jpg"],
                    "embeddings": [[3.0, 4.0], [0.0, 0.0]]
                }]
            }"#,
        )
        .unwrap();
        let entries = read_catalogue(&path);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "FAST-00124#0");
        assert_eq!(entries[0].ic, "FAST-00124");
        assert_eq!(entries[0].description, "Bolt");
        assert_eq!(entries[0].part_type, "fastener");
        let norm = entries[0]
            .embedding
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1.0e-6, "{norm}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn an_empty_catalogue_is_no_match_without_inference() {
        let rgb = [255u8, 0, 0];
        let matched = match_pixel_boxes(&[], &rgb, 1, 1, &[[-5, -5, 9, 9]]);
        assert_eq!(matched.len(), 1);
        assert!(!matched[0].recognized);
        assert_eq!(matched[0].label, "No Match");
        assert!(matched[0].top.is_none());
        let direct = score_query(&[], &[1.0, 0.0]);
        assert_eq!(direct.label, "No Match");
    }

    #[test]
    fn a_known_vector_returns_the_ic_and_three_runners_up() {
        let entries = vec![
            entry("FAST-00124", "Bolt", "fastener", unit(&[1.0, 0.0, 0.0])),
            entry("FAST-00002", "Nut", "fastener", unit(&[0.9, 0.2, 0.0])),
            entry("FAST-00003", "Washer", "fastener", unit(&[0.7, 0.4, 0.1])),
            entry("FAST-00004", "Screw", "fastener", unit(&[0.4, 0.5, 0.2])),
            entry("FAST-00005", "Pin", "fastener", unit(&[0.1, 0.2, 0.9])),
            entry(
                "FAST-00124",
                "Bolt again",
                "fastener",
                unit(&[0.2, 0.9, 0.0]),
            ),
        ];
        let matched = score_query(&entries, &unit(&[1.0, 0.0, 0.0]));
        assert!(matched.recognized);
        let top = matched.top.expect("top");
        assert_eq!(top.ic, "FAST-00124");
        assert_eq!(top.description, "Bolt");
        assert_eq!(top.part_type, "fastener");
        assert!(top.score > 0.80, "{}", top.score);
        assert_eq!(matched.label, "FAST-00124 (100%)");
        assert_eq!(matched.runners_up.len(), 3);
        assert_eq!(matched.runners_up[0].ic, "FAST-00002");
        assert!(matched.runners_up[0].score < top.score);
        assert!(
            matched
                .runners_up
                .iter()
                .all(|item| item.ic != "FAST-00124")
        );
    }

    #[test]
    fn a_score_below_the_threshold_is_unrecognized() {
        let query = unit(&[1.0, 0.0, 0.0]);
        let low = score_query(
            &[entry(
                "FAST-00124",
                "Bolt",
                "fastener",
                unit(&[1.0, 2.0, 0.0]),
            )],
            &query,
        );
        assert!(!low.recognized, "{:?}", low.top);
        assert_eq!(low.label, "No Match");
        let top = low.top.expect("top");
        assert_eq!(top.ic, "FAST-00124");
        assert!(top.score < MATCH_THRESHOLD, "{}", top.score);

        let edge = [0.5, 0.75_f32.sqrt(), 0.0];
        let exact = score_query(
            &[entry("FAST-00124", "Bolt", "fastener", edge.to_vec())],
            &query,
        );
        assert!(exact.recognized, "{:?}", exact.top);
        assert_eq!(exact.label, "FAST-00124 (50%)");
    }

    #[test]
    fn out_of_range_boxes_stay_inside_the_frame() {
        let width = 4u32;
        let height = 3u32;
        let mut rgb = vec![0u8; (width * height * 3) as usize];
        for index in 0..rgb.len() {
            rgb[index] = index as u8;
        }
        let full = crop_clamped(&rgb, width, height, [-20, -20, 500, 500]).unwrap();
        assert_eq!((full.1, full.2), (width, height));
        assert_eq!(full.0, rgb);

        let last = crop_clamped(
            &rgb,
            width,
            height,
            [i32::MAX, i32::MAX, i32::MAX, i32::MAX],
        )
        .unwrap();
        assert_eq!((last.1, last.2), (1, 1));
        assert_eq!(last.0, rgb[rgb.len() - 3..].to_vec());

        let pixels = normalized_to_pixels([-0.4, 1.8, 0.25, f32::NAN], width, height);
        assert!(pixels[0] >= 0 && pixels[0] < width as i32);
        assert!(pixels[2] >= 0 && pixels[2] < width as i32);
        assert!(pixels[1] >= 0 && pixels[1] < height as i32);
        assert_eq!(pixels[3], 0);
        let cropped = crop_clamped(&rgb, width, height, pixels).unwrap();
        assert!(cropped.1 >= 1 && cropped.1 <= width);
        assert!(cropped.2 >= 1 && cropped.2 <= height);
    }

    #[test]
    fn a_known_part_snapshot_scores_above_0_80() {
        let _hold = crate::dinov2::lock_for_test();
        let model = crate::dinov2::model_path();
        if !model.is_file() {
            eprintln!("skip: dinov2 model is missing");
            return;
        }
        crate::dinov2::load_at(&model);
        let width = 48u32;
        let height = 32u32;
        let mut rgb = vec![0u8; (width * height * 3) as usize];
        for y in 0..height {
            for x in 0..width {
                let index = ((y * width + x) * 3) as usize;
                rgb[index] = (x.wrapping_mul(5)) as u8;
                rgb[index + 1] = (y.wrapping_mul(7)) as u8;
                rgb[index + 2] = x.wrapping_add(y).wrapping_mul(3) as u8;
            }
        }
        let vector = dino::embed_rgb(&rgb, width, height).expect("embed");
        let path = std::env::temp_dir().join(format!(
            "ls-vision-catalogue-known-{}.json",
            std::process::id()
        ));
        let body = serde_json::json!({
            "version": 1,
            "parts": [{
                "ic": "FAST-00124",
                "description": "Known fastener",
                "type": "fastener",
                "dimensions": { "lengthMm": 40 },
                "crops": ["catalogue-crops/FAST-00124/crop.jpg"],
                "embeddings": [vector]
            }]
        });
        fs::write(&path, format!("{body}\n")).unwrap();
        let entries = read_catalogue(&path);
        assert_eq!(entries.len(), 1);
        let matched = match_normalized(&entries, &rgb, width, height, [0.0, 0.0, 1.0, 1.0]);
        assert!(matched.recognized, "{:?}", matched.top);
        let top = matched.top.expect("top");
        assert_eq!(top.ic, "FAST-00124");
        assert_eq!(top.description, "Known fastener");
        assert_eq!(top.part_type, "fastener");
        assert!(top.score > 0.80, "{}", top.score);
        assert!(
            matched.label.starts_with("FAST-00124 ("),
            "{}",
            matched.label
        );
        let _ = fs::remove_file(&path);
    }
}
