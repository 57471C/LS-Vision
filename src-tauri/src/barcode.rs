//! Decode one Code 39 or QR symbol from the latest raw camera frame.
//! `capture_snapshot` stores that frame. This command crops the saved
//! barcode ROI and reads those RGB pixels. It does not take an image argument.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;

use rxing::helpers::detect_in_image_with_hints;
use rxing::{BarcodeFormat, DecodeHints, Exceptions};

#[cfg(test)]
use rxing::{MultiFormatWriter, Writer};
use serde_json::{Value, json};

use crate::frame::{FrameState, RawFrame};
use crate::local_config;

const MAX_PIXELS: usize = 40_000_000;
const READ_ERROR: &str = "Could not read the image.";
const NO_CODE: &str = "No code detected";
// Setup/debug only. Set this to true to append barcode-debug.log under
// LocalAppData (or Documents). Leave it false during normal operation.
// That file is never written under Program Files.
const BARCODE_SETUP_DEBUG: bool = false;

#[tauri::command]
pub fn decode_barcode(state: tauri::State<FrameState>) -> Result<Value, String> {
    let frame = {
        let guard = state.latest.lock().unwrap_or_else(|err| err.into_inner());
        guard.clone()
    };
    decode_latest(frame.as_ref())
}

fn decode_latest(frame: Option<&RawFrame>) -> Result<Value, String> {
    let Some(frame) = frame else {
        return Ok(idle());
    };
    let roi = configured_barcode_box(frame.width, frame.height);
    decode_frame(frame, roi)
}

fn idle() -> Value {
    json!({
        "status": "idle",
        "message": NO_CODE,
    })
}

fn pass(barcode: &str, kind: &str) -> Value {
    json!({
        "status": "pass",
        "barcode": barcode,
        "type": kind,
    })
}

/// One trailing "/" was a temporary Code 39 suffix. Leave any further slashes.
fn clean_code(text: &str) -> &str {
    text.strip_suffix('/').unwrap_or(text)
}

fn format_name(format: &BarcodeFormat) -> Option<&'static str> {
    match format {
        BarcodeFormat::CODE_39 => Some("CODE39"),
        BarcodeFormat::QR_CODE => Some("QRCODE"),
        _ => None,
    }
}

/// Values from 0 to 1 are a portion of the frame. Anything larger is pixels.
fn pixel_barcode_roi(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    image_width: u32,
    image_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    if image_width < 1 || image_height < 1 || width <= 0.0 || height <= 0.0 {
        return None;
    }
    if !x.is_finite() || !y.is_finite() || !width.is_finite() || !height.is_finite() {
        return None;
    }
    let portion = (0.0..=1.0).contains(&x)
        && (0.0..=1.0).contains(&y)
        && (0.0..=1.0).contains(&width)
        && (0.0..=1.0).contains(&height);
    let image_width_f = f64::from(image_width);
    let image_height_f = f64::from(image_height);
    let left = if portion { x * image_width_f } else { x };
    let top = if portion { y * image_height_f } else { y };
    let box_w = if portion {
        width * image_width_f
    } else {
        width
    };
    let box_h = if portion {
        height * image_height_f
    } else {
        height
    };
    if box_w <= 0.0 || box_h <= 0.0 {
        return None;
    }
    let x0 = left.floor().clamp(0.0, f64::from(image_width - 1)) as u32;
    let y0 = top.floor().clamp(0.0, f64::from(image_height - 1)) as u32;
    let x1 = (left + box_w)
        .ceil()
        .clamp(f64::from(x0 + 1), image_width_f) as u32;
    let y1 = (top + box_h)
        .ceil()
        .clamp(f64::from(y0 + 1), image_height_f) as u32;
    let cropped_w = x1.saturating_sub(x0);
    let cropped_h = y1.saturating_sub(y0);
    if cropped_w == 0 || cropped_h == 0 {
        return None;
    }
    Some((x0, y0, cropped_w, cropped_h))
}

fn configured_barcode_box(width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let root = local_config::project_root().ok()?;
    let document = local_config::load_document(&root).ok()?;
    let roi = document.get("barcodeRoi")?;
    if roi.is_null() {
        return None;
    }
    let object = roi.as_object()?;
    let x = object.get("x")?.as_f64()?;
    let y = object.get("y")?.as_f64()?;
    let w = object.get("width")?.as_f64()?;
    let h = object.get("height")?.as_f64()?;
    pixel_barcode_roi(x, y, w, h, width, height)
}

#[cfg(test)]
fn edge_hex(bytes: &[u8], tail: bool) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let sample = if tail {
        &bytes[bytes.len().saturating_sub(8)..]
    } else {
        &bytes[..bytes.len().min(8)]
    };
    sample
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
fn image_debug_line(chars: usize, bytes: &[u8]) -> String {
    format!(
        "barcode debug chars={chars} bytes={} head={} tail={}",
        bytes.len(),
        edge_hex(bytes, false),
        edge_hex(bytes, true),
    )
}

fn record_debug(line: &str) {
    if cfg!(test) {
        return;
    }
    eprintln!("{line}");
    let Ok(dir) = local_config::user_save_dir() else {
        return;
    };
    let path = dir.join("barcode-debug.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

/// NotFoundException is a miss. Any other crate error is returned as-is.
fn from_crate_error(err: Exceptions) -> Value {
    if matches!(err, Exceptions::NotFoundException(_)) {
        return idle();
    }
    json!({
        "status": "fail",
        "message": err.to_string(),
    })
}

fn decode_frame(frame: &RawFrame, roi: Option<(u32, u32, u32, u32)>) -> Result<Value, String> {
    if frame.channels != 3 || frame.width == 0 || frame.height == 0 {
        return Err(READ_ERROR.to_string());
    }
    let pixels = (frame.width as usize).saturating_mul(frame.height as usize);
    if pixels == 0 || pixels > MAX_PIXELS {
        return Err(READ_ERROR.to_string());
    }
    let expected = pixels.saturating_mul(3);
    if frame.data.len() < expected {
        return Err(READ_ERROR.to_string());
    }
    let (bytes, width, height) = if let Some((x, y, w, h)) = roi {
        let fits = x.saturating_add(w) <= frame.width && y.saturating_add(h) <= frame.height;
        if !fits || w == 0 || h == 0 {
            (frame.data[..expected].to_vec(), frame.width, frame.height)
        } else {
            (frame.crop(x, y, w, h), w, h)
        }
    } else {
        (frame.data[..expected].to_vec(), frame.width, frame.height)
    };
    decode_rgb(bytes, width, height)
}

/// A 4K photo of this bench's label is missed. The same pixels at 1920 decode.
/// Halve until the long edge fits, and try that copy before the original.
const MAX_READ_EDGE: u32 = 1920;

fn decode_rgb(bytes: Vec<u8>, width: u32, height: u32) -> Result<Value, String> {
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3);
    if width == 0 || height == 0 || bytes.len() != expected {
        return Err(READ_ERROR.to_string());
    }
    if let Some((smaller, small_w, small_h)) = fit_read_size(&bytes, width, height) {
        let fitted = decode_rgb_once(smaller, small_w, small_h)?;
        if fitted["status"] == "pass" {
            return Ok(fitted);
        }
    }
    decode_rgb_once(bytes, width, height)
}

/// Halve while the long edge is over [`MAX_READ_EDGE`]. `None` when it already fits.
fn fit_read_size(bytes: &[u8], width: u32, height: u32) -> Option<(Vec<u8>, u32, u32)> {
    if width.max(height) <= MAX_READ_EDGE || width < 2 || height < 2 {
        return None;
    }
    let mut current = image::RgbImage::from_raw(width, height, bytes.to_vec())?;
    while current.width().max(current.height()) > MAX_READ_EDGE {
        let long_edge = current.width().max(current.height());
        let dest_long = long_edge / 2;
        if dest_long < 1 {
            break;
        }
        let scale = f64::from(dest_long) / f64::from(long_edge);
        let dest_w = ((f64::from(current.width()) * scale).round() as u32).max(1);
        let dest_h = ((f64::from(current.height()) * scale).round() as u32).max(1);
        if dest_w == current.width() && dest_h == current.height() {
            break;
        }
        current = image::imageops::resize(
            &current,
            dest_w,
            dest_h,
            image::imageops::FilterType::Triangle,
        );
    }
    if current.width() == width && current.height() == height {
        return None;
    }
    let (fitted_w, fitted_h) = current.dimensions();
    Some((current.into_raw(), fitted_w, fitted_h))
}

fn decode_rgb_once(bytes: Vec<u8>, width: u32, height: u32) -> Result<Value, String> {
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3);
    if width == 0 || height == 0 || bytes.len() != expected {
        return Err(READ_ERROR.to_string());
    }
    if BARCODE_SETUP_DEBUG {
        record_debug(&format!(
            "barcode debug width={width} height={height} bytes={expected}"
        ));
    }
    let image =
        image::RgbImage::from_raw(width, height, bytes).ok_or_else(|| READ_ERROR.to_string())?;
    let mut hints = DecodeHints {
        PossibleFormats: Some(HashSet::from([
            BarcodeFormat::CODE_39,
            BarcodeFormat::QR_CODE,
        ])),
        TryHarder: Some(true),
        ..DecodeHints::default()
    };
    let value =
        match detect_in_image_with_hints(image::DynamicImage::ImageRgb8(image), None, &mut hints) {
            Ok(found) => match format_name(found.getBarcodeFormat()) {
                Some(kind) => {
                    let code = clean_code(found.getText());
                    if code.is_empty() {
                        idle()
                    } else {
                        pass(code, kind)
                    }
                }
                None => idle(),
            },
            Err(err) => from_crate_error(err),
        };
    if BARCODE_SETUP_DEBUG {
        let status = value["status"].as_str().unwrap_or("").to_string();
        let detail = value["message"]
            .as_str()
            .or_else(|| value["barcode"].as_str())
            .unwrap_or("")
            .to_string();
        record_debug(&format!("barcode debug result={status} {detail}"));
    }
    Ok(value)
}

#[cfg(test)]
fn symbol_rgb(format: BarcodeFormat, text: &str, width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let matrix = MultiFormatWriter
        .encode(text, &format, width as i32, height as i32)
        .unwrap_or_else(|err| panic!("encode {text}: {err}"));
    let symbol: image::DynamicImage = (&matrix).into();
    let scaled = symbol.resize(width, height, image::imageops::FilterType::Nearest);
    let border = 40u32;
    let mut canvas = image::RgbImage::from_pixel(
        scaled.width() + border * 2,
        scaled.height() + border * 2,
        image::Rgb([255, 255, 255]),
    );
    image::imageops::overlay(&mut canvas, &scaled.to_rgb8(), border.into(), border.into());
    let (canvas_width, canvas_height) = canvas.dimensions();
    (canvas.into_raw(), canvas_width, canvas_height)
}

#[cfg(test)]
fn frame_of(data: Vec<u8>, width: u32, height: u32) -> RawFrame {
    RawFrame {
        width,
        height,
        channels: 3,
        data,
        timestamp_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_trailing_slash_is_removed() {
        assert_eq!(clean_code("6PBBK001/"), "6PBBK001");
        assert_eq!(clean_code("6PBBK001//"), "6PBBK001/");
        assert_eq!(clean_code("6PBBK001"), "6PBBK001");
        assert_eq!(clean_code("/"), "");
    }

    #[test]
    fn a_portion_roi_maps_onto_pixels() {
        let (x, y, w, h) = pixel_barcode_roi(0.25, 0.5, 0.5, 0.25, 200, 100).unwrap();
        assert_eq!((x, y, w, h), (50, 50, 100, 25));
    }

    #[test]
    fn code39_rgb_decodes() {
        let (bytes, width, height) = symbol_rgb(BarcodeFormat::CODE_39, "6PBBK001", 480, 140);
        let value = decode_rgb(bytes, width, height).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "CODE39");
    }

    #[test]
    fn a_trailing_slash_on_the_symbol_is_stripped() {
        let (bytes, width, height) = symbol_rgb(BarcodeFormat::CODE_39, "6PBBK001/", 520, 140);
        let value = decode_rgb(bytes, width, height).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "CODE39");
    }

    #[test]
    fn qr_rgb_decodes() {
        let (bytes, width, height) = symbol_rgb(BarcodeFormat::QR_CODE, "6PBBK001", 240, 240);
        let value = decode_rgb(bytes, width, height).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "QRCODE");
    }

    #[test]
    fn a_saved_roi_crops_before_the_read() {
        let (symbol, symbol_w, symbol_h) = symbol_rgb(BarcodeFormat::QR_CODE, "6PBBK001", 120, 120);
        let width = symbol_w + 30;
        let height = symbol_h + 20;
        let mut data = vec![255_u8; (width * height * 3) as usize];
        for row in 0..symbol_h {
            let src = (row * symbol_w * 3) as usize;
            let dst = ((row + 8) * width + 10) as usize * 3;
            let len = (symbol_w * 3) as usize;
            data[dst..dst + len].copy_from_slice(&symbol[src..src + len]);
        }
        let frame = frame_of(data, width, height);
        let value = decode_frame(&frame, Some((10, 8, symbol_w, symbol_h))).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "QRCODE");
    }

    #[test]
    fn a_blank_image_finds_no_code() {
        let bytes = vec![255_u8; 80 * 80 * 3];
        let value = decode_rgb(bytes, 80, 80).unwrap();
        assert_eq!(value["status"], "idle");
        assert_eq!(value["message"], NO_CODE);
        assert!(value.get("barcode").is_none());
    }

    #[test]
    fn code128_is_not_returned() {
        let (bytes, width, height) = symbol_rgb(BarcodeFormat::CODE_128, "6PBBK001", 480, 140);
        let value = decode_rgb(bytes, width, height).unwrap();
        assert_eq!(value["status"], "idle");
        assert!(value.get("barcode").is_none());
    }

    #[test]
    fn a_missing_frame_is_idle() {
        let value = decode_latest(None).unwrap();
        assert_eq!(value["status"], "idle");
        assert_eq!(value["message"], NO_CODE);
    }

    #[test]
    fn a_short_buffer_is_rejected() {
        let frame = frame_of(vec![0_u8; 10], 10, 10);
        assert_eq!(decode_frame(&frame, None).unwrap_err(), READ_ERROR);
    }

    #[test]
    fn debug_line_reports_size_and_edge_bytes() {
        let bytes: Vec<u8> = (0..20).collect();
        let line = image_debug_line(240_000, &bytes);
        assert!(line.contains("chars=240000"));
        assert!(line.contains("bytes=20"));
        assert!(line.contains("head=00 01 02 03 04 05 06 07"));
        assert!(line.contains("tail=0c 0d 0e 0f 10 11 12 13"));
    }

    #[test]
    fn a_frame_wider_than_1920_is_halved_until_it_fits() {
        let bytes = vec![255_u8; 3840 * 2160 * 3];
        let (fitted, width, height) = fit_read_size(&bytes, 3840, 2160).unwrap();
        assert_eq!((width, height), (1920, 1080));
        assert_eq!(fitted.len(), 1920 * 1080 * 3);

        let bytes = vec![255_u8; 1999 * 876 * 3];
        let (_, width, height) = fit_read_size(&bytes, 1999, 876).unwrap();
        assert!(width <= MAX_READ_EDGE && height <= MAX_READ_EDGE);
        assert!(width < 1999 && height < 876);

        assert!(fit_read_size(&vec![255_u8; 1920 * 1080 * 3], 1920, 1080).is_none());
    }

    #[test]
    fn an_enlarged_qr_still_decodes() {
        let (bytes, width, height) = symbol_rgb(BarcodeFormat::QR_CODE, "6PBBK001", 240, 240);
        let image = image::RgbImage::from_raw(width, height, bytes).unwrap();
        let big = image::imageops::resize(
            &image,
            width * 8,
            height * 8,
            image::imageops::FilterType::Nearest,
        );
        let (big_w, big_h) = big.dimensions();
        assert!(big_w.max(big_h) > MAX_READ_EDGE);
        let value = decode_rgb(big.into_raw(), big_w, big_h).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "QRCODE");
    }

    #[test]
    fn a_four_k_copy_of_the_saved_label_still_decodes() {
        let path = local_config::project_root()
            .unwrap()
            .join("vision")
            .join("debug-barcode.jpg");
        if !path.exists() {
            return;
        }
        let image = image::open(&path)
            .unwrap_or_else(|err| panic!("open {}: {err}", path.display()))
            .to_rgb8();
        let (width, height) = image.dimensions();
        let up = image::imageops::resize(
            &image,
            width * 2,
            height * 2,
            image::imageops::FilterType::Nearest,
        );
        let (up_w, up_h) = up.dimensions();
        let frame = frame_of(up.into_raw(), up_w, up_h);
        let roi = configured_barcode_box(up_w, up_h);
        let value = decode_frame(&frame, roi).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "17820737");
        assert_eq!(value["type"], "QRCODE");
    }

    #[test]
    fn a_not_found_error_stays_a_miss_and_other_crate_errors_are_returned() {
        let miss = from_crate_error(Exceptions::NOT_FOUND);
        assert_eq!(miss["status"], "idle");
        assert_eq!(miss["message"], NO_CODE);

        let fail = from_crate_error(Exceptions::checksum_with("bad digit"));
        assert_eq!(fail["status"], "fail");
        assert_eq!(fail["message"], "ChecksumException - bad digit");
        assert!(fail.get("barcode").is_none());
    }
}
