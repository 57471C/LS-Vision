//! Decode one Code 39 or QR symbol from an image.
//! The camera frame comes from `capture_snapshot`. This command only reads
//! pixels it is given: a data URL, raw base64, or a snapshot body with an `image` field.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rxing::helpers::detect_in_image_with_hints;
use rxing::{BarcodeFormat, DecodeHints, Exceptions};

#[cfg(test)]
use image::ImageFormat;
#[cfg(test)]
use rxing::{MultiFormatWriter, Writer};
use serde_json::{Value, json};
#[cfg(test)]
use std::io::Cursor;

use crate::local_config;

const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;
const READ_ERROR: &str = "Could not read the image.";
const NO_CODE: &str = "No code detected";
// Setup/debug only. Set this to true to save debug-barcode.jpg and append
// barcode-debug.log under LocalAppData (or Documents). Leave it false during
// normal operation. Those files are never written under Program Files.
const BARCODE_SETUP_DEBUG: bool = false;

#[tauri::command]
pub fn decode_barcode(image: String) -> Result<Value, String> {
    let chars = image.len();
    match prepare(&image)? {
        None => {
            // A snapshot body with no image is a miss, not a crate error.
            if BARCODE_SETUP_DEBUG {
                record_debug(&format!(
                    "barcode debug chars={chars} bytes=0 result=no-image"
                ));
            }
            Ok(idle())
        }
        Some(bytes) => decode_bytes(chars, &bytes),
    }
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

fn prepare(input: &str) -> Result<Option<Vec<u8>>, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err(READ_ERROR.to_string());
    }
    if text.starts_with('{') {
        let value: Value = serde_json::from_str(text).map_err(|_| READ_ERROR.to_string())?;
        let Some(object) = value.as_object() else {
            return Err(READ_ERROR.to_string());
        };
        match object.get("image") {
            None => return Ok(None),
            Some(Value::String(image)) if image.trim().is_empty() => return Ok(None),
            Some(Value::String(image)) => return payload_bytes(image).map(Some),
            Some(_) => return Err(READ_ERROR.to_string()),
        }
    }
    payload_bytes(text).map(Some)
}

fn payload_bytes(text: &str) -> Result<Vec<u8>, String> {
    let text = text.trim();
    let encoded = if let Some(rest) = text.strip_prefix("data:") {
        let (_, data) = rest.split_once(',').ok_or(READ_ERROR)?;
        data
    } else {
        text
    };
    let compact: String = encoded.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.is_empty() {
        return Err(READ_ERROR.to_string());
    }
    let bytes = STANDARD
        .decode(compact.as_bytes())
        .map_err(|_| READ_ERROR.to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err(READ_ERROR.to_string());
    }
    Ok(bytes)
}

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

fn image_debug_line(chars: usize, bytes: &[u8]) -> String {
    format!(
        "barcode debug chars={chars} bytes={} head={} tail={}",
        bytes.len(),
        edge_hex(bytes, false),
        edge_hex(bytes, true),
    )
}

/// Latest frame for inspection outside the app. Tests skip the write.
fn store_debug_jpeg(bytes: &[u8]) -> Result<Option<PathBuf>, String> {
    if cfg!(test) {
        return Ok(None);
    }
    let path = local_config::user_save_dir()?.join("debug-barcode.jpg");
    std::fs::write(&path, bytes).map_err(|err| err.to_string())?;
    Ok(Some(path))
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

fn decode_bytes(chars: usize, bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(READ_ERROR.to_string());
    }
    let loaded = image::load_from_memory(bytes);
    if BARCODE_SETUP_DEBUG {
        let saved = store_debug_jpeg(bytes);
        let mut line = image_debug_line(chars, bytes);
        match &loaded {
            Ok(image) => line.push_str(&format!(
                " width={} height={}",
                image.width(),
                image.height()
            )),
            Err(err) => line.push_str(&format!(" load_error={err}")),
        }
        match &saved {
            Ok(Some(path)) => line.push_str(&format!(" saved={}", path.display())),
            Ok(None) => {}
            Err(err) => line.push_str(&format!(" save_error={err}")),
        }
        record_debug(&line);
    }

    let image = loaded.map_err(|_| READ_ERROR.to_string())?;
    let mut hints = DecodeHints::default();
    hints.PossibleFormats = Some(HashSet::from([
        BarcodeFormat::CODE_39,
        BarcodeFormat::QR_CODE,
    ]));
    hints.TryHarder = Some(true);
    let value = match detect_in_image_with_hints(image, None, &mut hints) {
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
fn symbol_bytes(
    format: BarcodeFormat,
    text: &str,
    width: u32,
    height: u32,
    codec: ImageFormat,
) -> Vec<u8> {
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
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(canvas)
        .write_to(&mut Cursor::new(&mut bytes), codec)
        .expect("encode image");
    bytes
}

#[cfg(test)]
fn data_url(bytes: &[u8], mime: &str) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
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
    fn code39_png_data_url_decodes() {
        let bytes = symbol_bytes(
            BarcodeFormat::CODE_39,
            "6PBBK001",
            480,
            140,
            ImageFormat::Png,
        );
        let value = decode_barcode(data_url(&bytes, "image/png")).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "CODE39");
    }

    #[test]
    fn code39_jpeg_and_raw_base64_decode() {
        let bytes = symbol_bytes(
            BarcodeFormat::CODE_39,
            "6PBBGR01",
            480,
            140,
            ImageFormat::Jpeg,
        );
        let raw = decode_barcode(STANDARD.encode(&bytes)).unwrap();
        assert_eq!(raw["barcode"], "6PBBGR01");
        assert_eq!(raw["type"], "CODE39");

        let spaced = data_url(&bytes, "image/jpeg").replace(",", ",\n");
        let url = decode_barcode(spaced).unwrap();
        assert_eq!(url["barcode"], "6PBBGR01");
    }

    #[test]
    fn a_snapshot_body_uses_its_image() {
        let bytes = symbol_bytes(
            BarcodeFormat::CODE_39,
            "6PBBK001",
            480,
            140,
            ImageFormat::Png,
        );
        let body = json!({
            "status": "pass",
            "snapshot": true,
            "image": data_url(&bytes, "image/png"),
        });
        let value = decode_barcode(body.to_string()).unwrap();
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "CODE39");
    }

    #[test]
    fn a_trailing_slash_on_the_symbol_is_stripped() {
        let bytes = symbol_bytes(
            BarcodeFormat::CODE_39,
            "6PBBK001/",
            520,
            140,
            ImageFormat::Png,
        );
        let value = decode_barcode(data_url(&bytes, "image/png")).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "CODE39");
    }

    #[test]
    fn qr_png_decodes() {
        let bytes = symbol_bytes(
            BarcodeFormat::QR_CODE,
            "6PBBK001",
            240,
            240,
            ImageFormat::Png,
        );
        let value = decode_barcode(data_url(&bytes, "image/png")).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["barcode"], "6PBBK001");
        assert_eq!(value["type"], "QRCODE");
    }

    #[test]
    fn a_blank_image_finds_no_code() {
        let image = image::RgbImage::from_pixel(80, 80, image::Rgb([255, 255, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let value = decode_barcode(data_url(&bytes, "image/png")).unwrap();
        assert_eq!(value["status"], "idle");
        assert_eq!(value["message"], NO_CODE);
        assert!(value.get("barcode").is_none());
    }

    #[test]
    fn code128_is_not_returned() {
        let bytes = symbol_bytes(
            BarcodeFormat::CODE_128,
            "6PBBK001",
            480,
            140,
            ImageFormat::Png,
        );
        let value = decode_barcode(data_url(&bytes, "image/png")).unwrap();
        assert_eq!(value["status"], "idle");
        assert!(value.get("barcode").is_none());
    }

    #[test]
    fn a_snapshot_without_an_image_is_idle() {
        let value = decode_barcode(r#"{"status":"idle","snapshot":false}"#.to_string()).unwrap();
        assert_eq!(value["status"], "idle");
        assert_eq!(value["message"], NO_CODE);
    }

    #[test]
    fn bad_payloads_are_errors() {
        assert_eq!(decode_barcode(String::new()).unwrap_err(), READ_ERROR);
        assert_eq!(decode_barcode("   ".to_string()).unwrap_err(), READ_ERROR);
        assert_eq!(decode_barcode("hello".to_string()).unwrap_err(), READ_ERROR);
        assert_eq!(
            decode_barcode("data:image/png;base64,!!!!".to_string()).unwrap_err(),
            READ_ERROR
        );
        assert_eq!(decode_barcode("{".to_string()).unwrap_err(), READ_ERROR);
        assert_eq!(
            decode_barcode(r#"{"image":1}"#.to_string()).unwrap_err(),
            READ_ERROR
        );
    }

    #[test]
    fn an_oversized_buffer_is_rejected() {
        let bytes = vec![0u8; MAX_IMAGE_BYTES + 1];
        assert_eq!(decode_bytes(0, &bytes).unwrap_err(), READ_ERROR);
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
