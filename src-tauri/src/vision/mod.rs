//! Vision geometry shared by the YOLO detect path.
//!
//! DINOv2 keeps its own 224 letterbox in `dinov2` so ImageNet padding stays
//! separate from this gray YOLO canvas.

pub(crate) mod letterbox;
