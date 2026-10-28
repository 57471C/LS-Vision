//! DINOv2 embedding for catalogue matching.
//!
//! A crop is stretched to 224×224 with bilinear interpolation, packed as
//! float32 NCHW, and ImageNet-normalised. The forward pass uses the session
//! opened at startup from `dinov2_vits14.onnx`. The returned vector is L2
//! normalised so a later cosine similarity is a dot product.
//!
//! The BoM reference file still letterboxes its crops. This path is only for
//! the imported catalogue.

use crate::detect;
use crate::dinov2;

pub(crate) const SIZE: usize = 224;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Stretch `rgb` to 224×224 and return a planar `[1, 3, 224, 224]` buffer.
///
/// Channel 0 is `((r / 255) - 0.485) / 0.229`, channel 1 is
/// `((g / 255) - 0.456) / 0.224`, and channel 2 is `((b / 255) - 0.406) / 0.225`.
pub(crate) fn preprocess(rgb: &[u8], width: u32, height: u32) -> Result<Vec<f32>, String> {
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err("empty crop".to_string());
    }
    let width = width as usize;
    let height = height as usize;
    let expected = width.saturating_mul(height).saturating_mul(3);
    if expected == 0 || rgb.len() < expected {
        return Err("empty crop".to_string());
    }
    let resized = detect::resize_linear(&rgb[..expected], width, height, SIZE, SIZE);
    let plane = SIZE * SIZE;
    if resized.len() < plane * 3 {
        return Err("empty crop".to_string());
    }
    let mut data = vec![0.0f32; 3 * plane];
    for index in 0..plane {
        let src = index * 3;
        for channel in 0..3 {
            let unit = f32::from(resized[src + channel]) / 255.0;
            data[channel * plane + index] = (unit - MEAN[channel]) / STD[channel];
        }
    }
    Ok(data)
}

/// Embed one RGB crop. The vector length is 1.
pub(crate) fn embed_rgb(rgb: &[u8], width: u32, height: u32) -> Result<Vec<f32>, String> {
    let data = preprocess(rgb, width, height)?;
    let values = dinov2::forward_nchw(data)?;
    l2_normalize(&values)
}

/// Scale `values` so its L2 norm is 1.
pub(crate) fn l2_normalize(values: &[f32]) -> Result<Vec<f32>, String> {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return Err("DINOv2 returned an empty embedding.".to_string());
    }
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return Err("DINOv2 returned an empty embedding.".to_string());
    }
    Ok(values.iter().map(|value| value / norm).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_red_uses_imagenet_normalization() {
        let rgb = vec![255u8, 0, 0];
        let data = preprocess(&rgb, 1, 1).unwrap();
        assert_eq!(data.len(), 3 * SIZE * SIZE);
        let plane = SIZE * SIZE;
        let red = (1.0 - MEAN[0]) / STD[0];
        let green = (0.0 - MEAN[1]) / STD[1];
        let blue = (0.0 - MEAN[2]) / STD[2];
        assert!((data[0] - red).abs() < 1.0e-5, "{}", data[0]);
        assert!((data[plane] - green).abs() < 1.0e-5, "{}", data[plane]);
        assert!(
            (data[plane * 2] - blue).abs() < 1.0e-5,
            "{}",
            data[plane * 2]
        );
        assert!(
            data[..plane]
                .iter()
                .all(|value| (*value - red).abs() < 1.0e-4)
        );
        assert!(
            data[plane..plane * 2]
                .iter()
                .all(|value| (*value - green).abs() < 1.0e-4)
        );
        assert!(
            data[plane * 2..]
                .iter()
                .all(|value| (*value - blue).abs() < 1.0e-4)
        );
    }

    #[test]
    fn an_empty_crop_is_rejected() {
        assert!(preprocess(&[], 0, 0).is_err());
        assert!(preprocess(&[255, 0, 0], 2, 2).is_err());
    }

    #[test]
    fn l2_normalize_makes_a_unit_vector() {
        let unit = l2_normalize(&[3.0, 4.0]).unwrap();
        let norm = unit.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1.0e-6, "{norm}");
        assert!((unit[0] - 0.6).abs() < 1.0e-6);
        assert!((unit[1] - 0.8).abs() < 1.0e-6);
        assert!(l2_normalize(&[]).is_err());
        assert!(l2_normalize(&[0.0, 0.0]).is_err());
    }
}
