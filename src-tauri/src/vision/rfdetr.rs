//! RF-DETR ONNX layout and decode.
//!
//! LS-Trainer exports `model.export()` from rfdetr. The graph takes a float32
//! NCHW tensor that has been stretched to the model size and ImageNet-normalised.
//! `dets` is normalised `cxcywh`. `labels` are class logits, with the background
//! slot last on a fine-tuned export. Scores are independent sigmoids. The export
//! does not run NMS.

use serde_json::Value;

pub const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectorKind {
    Yolo,
    RfDetr,
}

impl DetectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yolo => "yolo",
            Self::RfDetr => "rf-detr",
        }
    }
}

/// Settings > Training override. `auto` uses the file name and the output layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectorChoice {
    Auto,
    Yolo,
    RfDetr,
}

impl DetectorChoice {
    pub fn parse(text: Option<&str>) -> Self {
        let key = text
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-");
        match key.as_str() {
            "yolo" => Self::Yolo,
            "rf-detr" | "rfdetr" => Self::RfDetr,
            _ => Self::Auto,
        }
    }

    pub fn resolve(self, auto: DetectorKind) -> DetectorKind {
        match self {
            Self::Auto => auto,
            Self::Yolo => DetectorKind::Yolo,
            Self::RfDetr => DetectorKind::RfDetr,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputPort {
    pub name: String,
    pub shape: Vec<i64>,
}

/// One runtime tensor. `values` is the first batch, already sliced by the caller
/// only when the caller wants to; decode reads the leading batch itself.
pub struct TensorOut {
    pub name: String,
    pub shape: Vec<i64>,
    pub values: Vec<f32>,
}

/// One box in the stretched view, in pixels, before NMS and before the crop origin.
#[derive(Clone, Debug)]
pub struct RfBox {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub score: f32,
    pub class_id: usize,
}

/// `*yolo*` is YOLO. `*rfdetr*`, `*rf-detr*`, and `*incomplete*` are RF-DETR.
pub fn filename_kind(name: &str) -> Option<DetectorKind> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("rf-detr") || lower.contains("rfdetr") || lower.contains("incomplete") {
        return Some(DetectorKind::RfDetr);
    }
    if lower.contains("yolo") {
        return Some(DetectorKind::Yolo);
    }
    None
}

/// Output names first, then shapes. A single YOLO `output0` is not RF-DETR.
pub fn output_kind(outputs: &[OutputPort]) -> Option<DetectorKind> {
    if named_rfdetr(outputs) || shaped_rfdetr(outputs) {
        return Some(DetectorKind::RfDetr);
    }
    if shaped_yolo(outputs) {
        return Some(DetectorKind::Yolo);
    }
    None
}

/// File name when the graph is ambiguous. A clear output layout wins a disagreement,
/// so a YOLO graph whose file name contains `incomplete` stays on the YOLO decoder.
pub fn resolve_auto(file_name: &str, outputs: &[OutputPort]) -> DetectorKind {
    match (filename_kind(file_name), output_kind(outputs)) {
        (_, Some(from_graph)) => from_graph,
        (Some(from_name), None) => from_name,
        (None, None) => DetectorKind::Yolo,
    }
}

pub fn graph_has_nms(outputs: &[OutputPort]) -> bool {
    outputs
        .iter()
        .any(|port| port.name.to_ascii_lowercase().contains("nms"))
}

pub fn describe_ports(outputs: &[OutputPort]) -> String {
    if outputs.is_empty() {
        return "no outputs".to_string();
    }
    outputs
        .iter()
        .map(|port| {
            let shape = port
                .shape
                .iter()
                .map(|dim| dim.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} [{shape}]", port.name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Saved `detector`. Missing, blank, or unknown stays Auto.
pub fn choice_from_document(document: &Value) -> DetectorChoice {
    DetectorChoice::parse(document.get("detector").and_then(Value::as_str))
}

/// Float32 NCHW ImageNet tensor from tightly packed RGB bytes.
pub fn imagenet_tensor(rgb: &[u8], width: usize, height: usize) -> Result<Vec<f32>, String> {
    if width < 1 || height < 1 || width > 4096 || height > 4096 {
        return Err("Detect failed: RF-DETR input size is invalid".to_string());
    }
    let plane = width.saturating_mul(height);
    if rgb.len() < plane.saturating_mul(3) {
        return Err("Detect failed: RF-DETR input is short".to_string());
    }
    let mut data = vec![0f32; plane.saturating_mul(3)];
    for index in 0..plane {
        let pixel = index * 3;
        for channel in 0..3 {
            let unit = f32::from(rgb[pixel + channel]) / 255.0;
            let value = (unit - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
            if !value.is_finite() {
                return Err("Detect failed: RF-DETR input is not finite".to_string());
            }
            data[channel * plane + index] = value;
        }
    }
    Ok(data)
}

/// Decode `dets` and `labels` into view-pixel boxes. NMS is the caller's job.
pub fn decode_tensors(
    outputs: &[TensorOut],
    names: &[String],
    view_w: f32,
    view_h: f32,
    conf_threshold: f32,
) -> Result<Vec<RfBox>, String> {
    if !view_w.is_finite() || !view_h.is_finite() || view_w <= 0.0 || view_h <= 0.0 {
        return Err("Detect failed: RF-DETR view size is invalid".to_string());
    }
    let (boxes, logits) = pair_outputs(outputs)?;
    let (box_values, queries, box_cols) = first_matrix(&boxes.values, &boxes.shape)?;
    let (logit_values, logit_rows, class_count) = first_matrix(&logits.values, &logits.shape)?;
    if queries == 0 || logit_rows == 0 || class_count == 0 {
        return Ok(Vec::new());
    }
    if queries != logit_rows || box_cols != 4 {
        return Err("Detect failed: unexpected RF-DETR output shape".to_string());
    }
    let foreground = foreground_classes(class_count, names);
    if foreground == 0 {
        return Ok(Vec::new());
    }
    let mut scores = vec![0f32; queries * foreground];
    for query in 0..queries {
        for class_id in 0..foreground {
            scores[query * foreground + class_id] =
                sigmoid(logit_values[query * class_count + class_id]);
        }
    }
    let mut found = Vec::new();
    for (score, query, class_id) in top_pairs(&scores, queries, foreground, conf_threshold) {
        let base = query * 4;
        let cx = box_values[base];
        let cy = box_values[base + 1];
        let bw = box_values[base + 2];
        let bh = box_values[base + 3];
        if ![cx, cy, bw, bh].iter().all(|value| value.is_finite()) {
            continue;
        }
        let (x1, x2) = axis_span(cx, bw, view_w);
        let (y1, y2) = axis_span(cy, bh, view_h);
        if x2 <= x1 || y2 <= y1 {
            continue;
        }
        found.push(RfBox {
            x1,
            y1,
            x2,
            y2,
            score,
            class_id,
        });
    }
    Ok(found)
}

fn named_rfdetr(outputs: &[OutputPort]) -> bool {
    let boxes = outputs.iter().any(|port| box_name(&port.name));
    let labels = outputs.iter().any(|port| label_name(&port.name));
    boxes && labels
}

fn box_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("pred_boxes") || name.contains("dets") || name == "boxes"
}

fn label_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("pred_logits") || name.contains("labels") || name == "logits"
}

fn shaped_rfdetr(outputs: &[OutputPort]) -> bool {
    let boxes: Vec<_> = outputs
        .iter()
        .filter(|port| port.shape.last() == Some(&4i64) && rank3(&port.shape))
        .collect();
    let labels: Vec<_> = outputs
        .iter()
        .filter(|port| port.shape.last().is_some_and(|dim| *dim > 4i64) && rank3(&port.shape))
        .collect();
    if boxes.len() != 1 || labels.len() != 1 {
        return false;
    }
    queries(&boxes[0].shape) == queries(&labels[0].shape)
}

fn shaped_yolo(outputs: &[OutputPort]) -> bool {
    if outputs.len() != 1 || !rank3(&outputs[0].shape) {
        return false;
    }
    let Some((rows, cols)) = tail(&outputs[0].shape) else {
        return false;
    };
    let small = rows.min(cols);
    let large = rows.max(cols);
    // (1, 4 + classes, boxes) or an end-to-end (1, boxes, 6). A last dimension
    // of 4 with no class score is the RF-DETR box tensor, not a YOLO head.
    small >= 5i64 && large >= small
}

fn rank3(shape: &[i64]) -> bool {
    shape.len() == 3 && shape.iter().all(|dim| *dim != 0i64)
}

fn queries(shape: &[i64]) -> Option<i64> {
    match shape {
        [_, rows, _] => Some(*rows),
        _ => None,
    }
}

fn tail(shape: &[i64]) -> Option<(i64, i64)> {
    let rows = *shape.get(shape.len().wrapping_sub(2))?;
    let cols = *shape.last()?;
    if rows > 0i64 && cols > 0i64 {
        Some((rows, cols))
    } else {
        None
    }
}

fn pair_outputs(outputs: &[TensorOut]) -> Result<(&TensorOut, &TensorOut), String> {
    let boxes = outputs.iter().find(|tensor| box_name(&tensor.name));
    let labels = outputs.iter().find(|tensor| label_name(&tensor.name));
    if let (Some(boxes), Some(labels)) = (boxes, labels) {
        return Ok((boxes, labels));
    }
    let by_shape_boxes: Vec<_> = outputs
        .iter()
        .filter(|tensor| tensor.shape.last() == Some(&4i64))
        .collect();
    let by_shape_labels: Vec<_> = outputs
        .iter()
        .filter(|tensor| tensor.shape.last().is_some_and(|dim| *dim != 4i64))
        .collect();
    if by_shape_boxes.len() == 1 && by_shape_labels.len() == 1 {
        return Ok((by_shape_boxes[0], by_shape_labels[0]));
    }
    if outputs.len() == 2 {
        return Ok((&outputs[0], &outputs[1]));
    }
    Err("Detect failed: RF-DETR outputs were not found".to_string())
}

fn first_matrix<'a>(values: &'a [f32], shape: &[i64]) -> Result<(&'a [f32], usize, usize), String> {
    let (rows, cols) = match shape {
        [rows, cols] => (*rows, *cols),
        [batch, rows, cols] if *batch >= 1i64 => (*rows, *cols),
        _ => return Err("Detect failed: unexpected RF-DETR output shape".to_string()),
    };
    if rows < 0i64 || cols < 0i64 {
        return Err("Detect failed: unexpected RF-DETR output shape".to_string());
    }
    let rows = rows as usize;
    let cols = cols as usize;
    let need = rows.saturating_mul(cols);
    if values.len() < need {
        return Err("Detect failed: unexpected RF-DETR output shape".to_string());
    }
    Ok((&values[..need], rows, cols))
}

/// The last logit is background when the name list is one shorter than the
/// logit width, and when the file has no names at all. A name list that already
/// covers every slot is left intact.
fn foreground_classes(class_count: usize, names: &[String]) -> usize {
    if names.len() + 1 == class_count || (names.is_empty() && class_count >= 2) {
        class_count - 1
    } else {
        class_count
    }
}

fn sigmoid(value: f32) -> f32 {
    if !value.is_finite() {
        return if value.is_sign_negative() { 0.0 } else { 1.0 };
    }
    let clipped = value.clamp(-88.0, 88.0);
    1.0 / (1.0 + (-clipped).exp())
}

/// Top `queries` query/class pairs, then drop scores at or below the threshold.
fn top_pairs(
    scores: &[f32],
    queries: usize,
    classes: usize,
    conf_threshold: f32,
) -> Vec<(f32, usize, usize)> {
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|&left, &right| {
        rank_score(scores[right])
            .total_cmp(&rank_score(scores[left]))
            .then(left.cmp(&right))
    });
    let cap = queries.min(order.len());
    let mut found = Vec::new();
    for index in order.into_iter().take(cap) {
        let score = scores[index];
        if !score.is_finite() || score <= conf_threshold {
            continue;
        }
        found.push((score, index / classes, index % classes));
    }
    found
}

fn rank_score(score: f32) -> f32 {
    if score.is_nan() { f32::INFINITY } else { score }
}

fn axis_span(center: f32, size: f32, limit: f32) -> (f32, f32) {
    let mut start = (center - size * 0.5) * limit;
    let mut end = (center + size * 0.5) * limit;
    if end < start {
        std::mem::swap(&mut start, &mut end);
    }
    (start.clamp(0.0, limit), end.clamp(0.0, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, shape: &[i64]) -> OutputPort {
        OutputPort {
            name: name.to_string(),
            shape: shape.to_vec(),
        }
    }

    fn tensor(name: &str, shape: &[i64], values: Vec<f32>) -> TensorOut {
        TensorOut {
            name: name.to_string(),
            shape: shape.to_vec(),
            values,
        }
    }

    #[test]
    fn filename_heuristics_match_the_export_names() {
        assert_eq!(filename_kind("yolo11n.onnx"), Some(DetectorKind::Yolo));
        assert_eq!(filename_kind("YOLO26s.onnx"), Some(DetectorKind::Yolo));
        assert_eq!(
            filename_kind("rf-detr-medium.onnx"),
            Some(DetectorKind::RfDetr)
        );
        assert_eq!(filename_kind("rfdetr.onnx"), Some(DetectorKind::RfDetr));
        assert_eq!(
            filename_kind("incomplete-21-10-26.onnx"),
            Some(DetectorKind::RfDetr)
        );
        assert_eq!(filename_kind("best.onnx"), None);
        assert_eq!(filename_kind("RF-D-m-2e.onnx"), None);
    }

    #[test]
    fn output_layout_wins_when_the_file_name_disagrees() {
        let yolo = vec![port("output0", &[1, 13, 33600])];
        let rfdetr = vec![port("dets", &[1, 300, 4]), port("labels", &[1, 300, 8])];
        assert_eq!(
            resolve_auto("incomplete-21-10-26.onnx", &yolo),
            DetectorKind::Yolo
        );
        assert_eq!(resolve_auto("best.onnx", &rfdetr), DetectorKind::RfDetr);
        assert_eq!(
            resolve_auto("RF-D-m-2e.onnx", &rfdetr),
            DetectorKind::RfDetr
        );
        assert_eq!(
            resolve_auto("incomplete-21-10-26.onnx", &rfdetr),
            DetectorKind::RfDetr
        );
        assert_eq!(output_kind(&yolo), Some(DetectorKind::Yolo));
        assert_eq!(output_kind(&rfdetr), Some(DetectorKind::RfDetr));
        assert!(!graph_has_nms(&rfdetr));
    }

    #[test]
    fn an_unnamed_pair_still_reads_as_rfdetr() {
        let ports = vec![port("output0", &[1, 300, 4]), port("output1", &[1, 300, 8])];
        assert_eq!(output_kind(&ports), Some(DetectorKind::RfDetr));
    }

    #[test]
    fn detector_override_forces_the_decoder() {
        assert_eq!(
            DetectorChoice::parse(Some("YOLO")).resolve(DetectorKind::RfDetr),
            DetectorKind::Yolo
        );
        assert_eq!(
            DetectorChoice::parse(Some("rf-detr")).resolve(DetectorKind::Yolo),
            DetectorKind::RfDetr
        );
        assert_eq!(
            DetectorChoice::parse(Some("rfdetr")).resolve(DetectorKind::Yolo),
            DetectorKind::RfDetr
        );
        assert_eq!(
            DetectorChoice::parse(None).resolve(DetectorKind::RfDetr),
            DetectorKind::RfDetr
        );
        assert_eq!(
            DetectorChoice::parse(Some("auto")).resolve(DetectorKind::Yolo),
            DetectorKind::Yolo
        );
        assert_eq!(
            choice_from_document(&serde_json::json!({})).resolve(DetectorKind::RfDetr),
            DetectorKind::RfDetr
        );
    }

    #[test]
    fn imagenet_tensor_centres_a_white_pixel() {
        let data = imagenet_tensor(&[255, 255, 255], 1, 1).unwrap();
        let expected = [
            (1.0 - 0.485) / 0.229,
            (1.0 - 0.456) / 0.224,
            (1.0 - 0.406) / 0.225,
        ];
        for (got, want) in data.iter().zip(expected) {
            assert!((got - want).abs() < 1e-5);
        }
    }

    #[test]
    fn decode_scales_cxcywh_and_drops_the_background_slot() {
        let boxes = decode_tensors(
            &[
                tensor("dets", &[1, 1, 4], vec![0.5, 0.5, 0.2, 0.4]),
                tensor("labels", &[1, 1, 2], vec![8.0, 8.0]),
            ],
            &[],
            100.0,
            50.0,
            0.45,
        )
        .unwrap();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].class_id, 0);
        assert!(boxes[0].score > 0.99);
        assert!((boxes[0].x1 - 40.0).abs() < 1e-3);
        assert!((boxes[0].y1 - 15.0).abs() < 1e-3);
        assert!((boxes[0].x2 - 60.0).abs() < 1e-3);
        assert!((boxes[0].y2 - 35.0).abs() < 1e-3);
    }

    #[test]
    fn decode_drops_a_score_at_the_threshold_and_a_background_hit() {
        let at_half = decode_tensors(
            &[
                tensor("dets", &[1, 1, 4], vec![0.5, 0.5, 0.2, 0.2]),
                tensor("labels", &[1, 1, 2], vec![0.0, -8.0]),
            ],
            &[],
            10.0,
            10.0,
            0.5,
        )
        .unwrap();
        assert!(at_half.is_empty());
        let background = decode_tensors(
            &[
                tensor("dets", &[1, 1, 4], vec![0.5, 0.5, 0.2, 0.2]),
                tensor("labels", &[1, 1, 2], vec![-8.0, 8.0]),
            ],
            &[],
            10.0,
            10.0,
            0.45,
        )
        .unwrap();
        assert!(background.is_empty());
    }

    #[test]
    fn a_full_name_list_keeps_the_last_slot() {
        let boxes = decode_tensors(
            &[
                tensor("pred_boxes", &[1, 1, 4], vec![0.5, 0.5, 1.0, 1.0]),
                tensor("pred_logits", &[1, 1, 2], vec![-8.0, 8.0]),
            ],
            &["bolt".to_string(), "nut".to_string()],
            20.0,
            20.0,
            0.45,
        )
        .unwrap();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].class_id, 1);
    }
}
