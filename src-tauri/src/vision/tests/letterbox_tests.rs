use super::*;

#[test]
fn test_letterbox_roundtrip_1920x1080() {
    let orig_w = 1920;
    let orig_h = 1080;
    let target = 1280.0;
    let t = LetterboxTransform::new(orig_w, orig_h, target);

    // Verification of Ultralytics parameters for 1920x1080 -> 1280
    // gain = 1280 / 1920 = 0.6666667
    // scaled_w = 1280, scaled_h = 720
    // dw = 0.0, dh = (1280 - 720) / 2 = 280.0
    assert_eq!(t.pad_x, 0.0);
    assert_eq!(t.pad_y, 280.0);

    // Define key native landmarks
    let test_points = [
        (0.0, 0.0),                                 // Top-Left corner
        (orig_w as f32, 0.0),                       // Top-Right corner
        (0.0, orig_h as f32),                       // Bottom-Left corner
        (orig_w as f32, orig_h as f32),             // Bottom-Right corner
        (orig_w as f32 / 2.0, orig_h as f32 / 2.0), // Center
    ];

    for (orig_x, orig_y) in test_points {
        // Forward map to tensor space
        let tensor_x = (orig_x * t.gain) + t.pad_x;
        let tensor_y = (orig_y * t.gain) + t.pad_y;

        // Reverse map back to native
        let (rec_x, rec_y) = t.unpad_point(tensor_x, tensor_y);

        assert!(
            (orig_x - rec_x).abs() < 1e-3,
            "X drift: expected {orig_x}, got {rec_x}"
        );
        assert!(
            (orig_y - rec_y).abs() < 1e-3,
            "Y drift: expected {orig_y}, got {rec_y}"
        );
    }
}

#[test]
fn test_letterbox_square_3536x3536() {
    let t = LetterboxTransform::new(3536, 3536, 1280.0);
    // Native square aspect ratio should yield 0 padding
    assert_eq!(t.pad_x, 0.0);
    assert_eq!(t.pad_y, 0.0);

    let (rec_x, rec_y) = t.unpad_point(640.0, 640.0);
    assert!((rec_x - 1768.0).abs() < 1e-2);
    assert!((rec_y - 1768.0).abs() < 1e-2);
}

#[test]
fn test_unpad_clamping() {
    let t = LetterboxTransform::new(1920, 1080, 1280.0);
    // Point in top letterbox band (above valid content)
    let (x, y) = t.unpad_point(640.0, 100.0);
    assert_eq!(y, 0.0); // Clamped to top edge
    assert!(x >= 0.0 && x <= 1920.0);
}

#[test]
fn odd_remainder_assigns_the_extra_pixel_with_the_ultralytics_offset() {
    // 30×20 into 32. The vertical remainder is 11 pixels: top 5, bottom 6.
    let transform = LetterboxTransform::new(30, 20, 32.0);
    assert_eq!(transform.scaled_w, 32.0);
    assert_eq!(transform.scaled_h, 21.0);
    assert_eq!(transform.pad_x, 0.0);
    assert_eq!(transform.pad_y, 5.0);
}

#[test]
fn example3_frame_pads_eighty_pixels_on_the_top() {
    let transform = LetterboxTransform::new(2016, 1512, 640.0);
    assert_eq!(transform.scaled_w, 640.0);
    assert_eq!(transform.scaled_h, 480.0);
    assert_eq!(transform.pad_x, 0.0);
    assert_eq!(transform.pad_y, 80.0);
}

#[test]
fn bbox_and_polygon_unpad_together() {
    let transform = LetterboxTransform::new(1920, 1080, 1280.0);
    let bbox = transform.unpad_bbox([0.0, 280.0, 1280.0, 1000.0]);
    assert!((bbox[0]).abs() < 1e-3);
    assert!((bbox[1]).abs() < 1e-3);
    assert!((bbox[2] - 1920.0).abs() < 1e-2);
    let polygon = transform.unpad_polygon(&[[0.0, 280.0], [1280.0, 1000.0]]);
    assert_eq!(polygon.len(), 2);
    assert!((polygon[0][0] - bbox[0]).abs() < 1e-5);
    assert!((polygon[0][1] - bbox[1]).abs() < 1e-5);
    assert!((polygon[1][0] - bbox[2]).abs() < 1e-5);
    assert!((polygon[1][1] - bbox[3]).abs() < 1e-5);
}

#[test]
fn millimetres_use_the_unpadded_sensor_delta() {
    let transform = LetterboxTransform::new(1920, 1080, 1280.0);
    let (x0, y0) = transform.unpad_point(0.0, transform.pad_y);
    let (x1, y1) = transform.unpad_point(0.0, transform.pad_y + 100.0);
    let native = millimetres_from_sensor_px(y1 - y0, 0.085).unwrap();
    let tensor_pad = millimetres_from_sensor_px(transform.pad_y, 0.085).unwrap();
    assert!((y0).abs() < 1e-3);
    assert!((x0).abs() < 1e-3);
    assert!((x1).abs() < 1e-3);
    assert!(native < tensor_pad);
    assert!((native - (100.0 / transform.gain) * 0.085).abs() < 1e-3);
    assert!(millimetres_from_sensor_px(10.0, 0.0).is_none());
    assert!(millimetres_from_sensor_px(f32::NAN, 0.085).is_none());
}

#[test]
fn gray_pad_is_114_and_the_tensor_stays_inside_zero_to_one() {
    let transform = LetterboxTransform::new(2, 1, 4.0);
    assert_eq!(transform.pad_x, 0.0);
    assert!(transform.pad_y >= 1.0);
    let scaled_w = transform.scaled_w.round() as u32;
    let scaled_h = transform.scaled_h.round() as u32;
    let mut resized = vec![0u8; (scaled_w * scaled_h) as usize * 3];
    resized[0] = 255;
    resized[1] = 0;
    resized[2] = 0;
    let canvas = paint_letterbox(&resized, scaled_w, scaled_h, &transform).unwrap();
    assert_eq!(&canvas[0..3], &[PAD_RGB, PAD_RGB, PAD_RGB]);
    let left = transform.pad_x as usize;
    let top = transform.pad_y as usize;
    let at = ((top * 4) + left) * 3;
    assert_eq!(&canvas[at..at + 3], &[255, 0, 0]);

    let tensor = planar_tensor(&canvas, 4, TensorNormalization::ZeroToOne).unwrap();
    assert_eq!(tensor.len(), 3 * 4 * 4);
    let pad = f32::from(PAD_RGB) / 255.0;
    assert!((tensor[0] - pad).abs() < 1e-5);
    assert!((tensor[top * 4 + left] - 1.0).abs() < 1e-5);
    verify_input_tensor(&tensor, 4, TensorNormalization::ZeroToOne).unwrap();

    let raw = planar_tensor(&canvas, 4, TensorNormalization::ZeroTo255).unwrap();
    assert_eq!(raw[0], f32::from(PAD_RGB));
    assert_eq!(raw[top * 4 + left], 255.0);
}

#[test]
fn tensor_guard_rejects_a_bad_shape_nan_and_a_value_past_one() {
    let mut data = vec![0.5f32; 3 * 4];
    verify_input_tensor(&data, 2, TensorNormalization::ZeroToOne).unwrap();
    data[0] = f32::NAN;
    assert!(verify_input_tensor(&data, 2, TensorNormalization::ZeroToOne).is_err());
    data[0] = f32::INFINITY;
    assert!(verify_input_tensor(&data, 2, TensorNormalization::ZeroToOne).is_err());
    data[0] = 1.2;
    let err = verify_input_tensor(&data, 2, TensorNormalization::ZeroToOne).unwrap_err();
    assert!(err.contains("outside the model range"), "{err}");
    assert!(verify_input_tensor(&data[..3], 2, TensorNormalization::ZeroToOne).is_err());
}

#[test]
fn normalization_metadata_and_config_text() {
    assert_eq!(
        TensorNormalization::default(),
        TensorNormalization::ZeroToOne
    );
    assert_eq!(
        TensorNormalization::parse("zeroToOne"),
        Some(TensorNormalization::ZeroToOne)
    );
    assert_eq!(
        TensorNormalization::parse("0-255"),
        Some(TensorNormalization::ZeroTo255)
    );
    assert!(TensorNormalization::parse("maybe").is_none());
    assert_eq!(
        normalization_from_metadata([("tensor_normalization", "zeroTo255")]),
        TensorNormalization::ZeroTo255
    );
    assert_eq!(
        normalization_from_metadata([("names", "{0: 'part'}")]),
        TensorNormalization::ZeroToOne
    );
}

#[test]
fn yolo_tensor_uses_the_resize_size_from_the_transform() {
    let rgb = [10u8, 20, 30, 40, 50, 60];
    let (data, transform) = yolo_tensor(
        rgb.as_slice(),
        2,
        1,
        4,
        TensorNormalization::ZeroToOne,
        |src, sw, sh, dw, dh| {
            assert_eq!((sw, sh), (2, 1));
            assert_eq!(dw, transform_scaled(transform_of(2, 1, 4.0).scaled_w));
            assert_eq!(dh, transform_scaled(transform_of(2, 1, 4.0).scaled_h));
            vec![src[0]; dw * dh * 3]
        },
    )
    .unwrap();
    assert_eq!(data.len(), 3 * 16);
    assert_eq!(transform.orig_w, 2.0);
    assert!(data.iter().all(|value| value.is_finite()));
    verify_input_tensor(&data, 4, TensorNormalization::ZeroToOne).unwrap();
}

fn transform_of(width: u32, height: u32, target: f32) -> LetterboxTransform {
    LetterboxTransform::new(width, height, target)
}

fn transform_scaled(value: f32) -> usize {
    value.round() as usize
}
