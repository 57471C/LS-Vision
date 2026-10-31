//! Vision geometry shared by the ONNX detect path.
//!
//! DINOv2 keeps its own 224 letterbox in `dinov2` so ImageNet padding stays
//! separate from this gray YOLO canvas. RF-DETR stretches its own input and
//! does not use the YOLO letterbox.

pub(crate) mod letterbox;
pub(crate) mod rfdetr;
