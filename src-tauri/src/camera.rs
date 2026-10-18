//! One Windows camera, held open on a single thread, returning a JPEG.
//!
//! Media Foundation delivers frames on the thread that owns the source
//! reader. List and capture therefore share one worker, and that worker
//! keeps at most one session. Switching cameras shuts the media source
//! down before the next device is activated, so the hardware encoder is
//! free for the new open. Frames arrive through an async source-reader
//! callback, which keeps a single latest frame and replaces it on every
//! sample. JPEG encoding runs only when `capture_snapshot` or
//! `capture_preview` asks for that frame, and it does not run on the
//! callback. `capture_snapshot` keeps the selected mode.
//! `capture_preview` scales the same frame to fit in 1280×720 for the
//! settings canvas. Focus, brightness, and contrast are stored by
//! Settings and are not applied here.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::local_config;

const JPEG_QUALITY: i32 = 80;
/// Preview JPEGs fit in this box. The reader stays on the selected mode.
const PREVIEW_MAX_WIDTH: u32 = 1280;
const PREVIEW_MAX_HEIGHT: u32 = 720;
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(4);
/// Give the driver time to drop the hardware MFT after `Shutdown`.
const RELEASE_WAIT: Duration = Duration::from_millis(250);
const RESOURCE_CODE: u32 = 0xC00D_3704;

struct Frame {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    format: PixelFormat,
    fps: Option<f32>,
}

enum Job {
    List(Sender<Result<Vec<u32>, String>>),
    Modes {
        index: u32,
        reply: Sender<Result<Vec<Mode>, String>>,
    },
    Grab {
        index: u32,
        reply: Sender<Result<Frame, String>>,
    },
}

static JOBS: OnceLock<Sender<Job>> = OnceLock::new();

fn jobs() -> &'static Sender<Job> {
    JOBS.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let _ = thread::Builder::new()
            .name("ls-vision-camera".to_string())
            .spawn(move || camera_loop(rx));
        tx
    })
}

fn camera_loop(incoming: mpsc::Receiver<Job>) {
    let ready = session::startup();
    let mut open: Option<session::Device> = None;
    while let Ok(job) = incoming.recv() {
        if let Err(ref message) = ready {
            let message = message.clone();
            match job {
                Job::List(reply) => {
                    let _ = reply.send(Err(message));
                }
                Job::Modes { reply, .. } => {
                    let _ = reply.send(Err(message));
                }
                Job::Grab { reply, .. } => {
                    let _ = reply.send(Err(message));
                }
            }
            continue;
        }
        match job {
            Job::List(reply) => {
                let _ = reply.send(session::list_indexes());
            }
            Job::Modes { index, reply } => {
                let _ = reply.send(modes_for(&mut open, index));
            }
            Job::Grab { index, reply } => {
                let _ = reply.send(grab(&mut open, index));
            }
        }
    }
    if let Some(mut device) = open.take() {
        device.shutdown();
    }
    session::shutdown_platform();
}

#[tauri::command]
pub fn list_cameras() -> Result<Vec<u32>, String> {
    let (tx, rx) = mpsc::channel();
    jobs()
        .send(Job::List(tx))
        .map_err(|_| "Camera thread stopped.".to_string())?;
    rx.recv()
        .map_err(|_| "Camera thread stopped.".to_string())?
}

#[tauri::command]
pub fn list_camera_modes(index: Option<u32>) -> Result<Value, String> {
    let index = index.unwrap_or_else(saved_index);
    if index > 32 {
        return Ok(fail("Camera index must be from 0 to 32."));
    }
    let (tx, rx) = mpsc::channel();
    jobs()
        .send(Job::Modes { index, reply: tx })
        .map_err(|_| "Camera thread stopped.".to_string())?;
    let modes = rx
        .recv()
        .map_err(|_| "Camera thread stopped.".to_string())??;
    let choices = mode_choices(&modes);
    let default_mode = choose_mode(&modes, None);
    let default_id = default_mode.map(mode_id).unwrap_or_default();
    Ok(json!({
        "status": "pass",
        "default": default_id,
        "modes": choices
            .iter()
            .map(|mode| {
                json!({
                    "id": mode_id(*mode),
                    "width": mode.width,
                    "height": mode.height,
                    "fps": (fps_milli(mode) as f64) / 1000.0,
                    "format": mode.format.label(),
                })
            })
            .collect::<Vec<_>>(),
    }))
}

#[tauri::command]
pub fn capture_snapshot(index: Option<u32>) -> Result<Value, String> {
    let frame = match command_frame(index) {
        Ok(frame) => frame,
        Err(response) => return response,
    };
    let mode = frame_mode(&frame);
    // Encode on this thread. The camera thread only opens the device
    // and copies samples, so a JPEG cannot stall the next frame.
    match encode_frame(&frame.bytes, mode) {
        Ok(jpeg) => Ok(snapshot_body(&frame, &jpeg, frame.width, frame.height)),
        Err(message) => Ok(fail(&message)),
    }
}

/// Latest frame, scaled to fit in 1280×720. The open mode is unchanged,
/// so a later `capture_snapshot` is still the full selected resolution.
#[tauri::command]
pub fn capture_preview(index: Option<u32>) -> Result<Value, String> {
    let frame = match command_frame(index) {
        Ok(frame) => frame,
        Err(response) => return response,
    };
    let mode = frame_mode(&frame);
    match encode_preview(&frame.bytes, mode) {
        Ok((jpeg, preview_width, preview_height)) => {
            let mut body = snapshot_body(&frame, &jpeg, frame.width, frame.height);
            if let Some(object) = body.as_object_mut() {
                object.insert("previewWidth".to_string(), json!(preview_width));
                object.insert("previewHeight".to_string(), json!(preview_height));
            }
            Ok(body)
        }
        Err(message) => Ok(fail(&message)),
    }
}

fn command_frame(index: Option<u32>) -> Result<Frame, Result<Value, String>> {
    let index = index.unwrap_or_else(saved_index);
    if index > 32 {
        return Err(Ok(fail("Camera index must be from 0 to 32.")));
    }
    match fetch_frame(index) {
        Ok(frame) => Ok(frame),
        Err(message) if message == "Camera thread stopped." => Err(Err(message)),
        Err(message) => Err(Ok(fail(&message))),
    }
}

fn fetch_frame(index: u32) -> Result<Frame, String> {
    let (tx, rx) = mpsc::channel();
    jobs()
        .send(Job::Grab { index, reply: tx })
        .map_err(|_| "Camera thread stopped.".to_string())?;
    match rx.recv() {
        Ok(frame) => frame,
        Err(_) => Err("Camera thread stopped.".to_string()),
    }
}

fn frame_mode(frame: &Frame) -> Mode {
    Mode {
        index: 0,
        width: frame.width,
        height: frame.height,
        fps_num: 0,
        fps_den: 1,
        format: frame.format,
    }
}

fn snapshot_body(frame: &Frame, jpeg: &[u8], width: u32, height: u32) -> Value {
    json!({
        "status": "pass",
        "snapshot": false,
        "image": format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)),
        "width": width,
        "height": height,
        "format": frame.format.label(),
        "fps": frame.fps,
    })
}

fn modes_for(open: &mut Option<session::Device>, index: u32) -> Result<Vec<Mode>, String> {
    if let Some(device) = open.as_ref()
        && device.index() == index
    {
        let modes = device.modes();
        if !modes.is_empty() {
            return Ok(modes);
        }
    }
    if let Some(mut previous) = open.take() {
        previous.shutdown();
    }
    session::probe_modes(index)
}

fn grab(open: &mut Option<session::Device>, index: u32) -> Result<Frame, String> {
    let wanted = saved_resolution();
    let keep = open
        .as_ref()
        .is_some_and(|device| device.index() == index && device.serves(wanted));
    if !keep {
        if let Some(mut previous) = open.take() {
            previous.shutdown();
        }
        *open = Some(session::open_device(index, wanted)?);
    }
    match take_frame(open.as_ref().expect("camera was opened")) {
        Ok(frame) => Ok(frame),
        Err(message) if is_resource(&message) => {
            if let Some(mut previous) = open.take() {
                previous.shutdown();
            }
            *open = Some(session::open_device(index, wanted)?);
            take_frame(open.as_ref().expect("camera was reopened"))
        }
        Err(message) => {
            if let Some(mut previous) = open.take() {
                previous.shutdown();
            }
            Err(message)
        }
    }
}

fn take_frame(device: &session::Device) -> Result<Frame, String> {
    let bytes = device.wait_frame(FIRST_FRAME_WAIT)?;
    let mode = device.mode();
    Ok(Frame {
        bytes,
        width: mode.width,
        height: mode.height,
        format: mode.format,
        fps: device.fps().map(round_fps),
    })
}

fn fail(message: &str) -> Value {
    json!({
        "status": "fail",
        "snapshot": false,
        "message": message,
    })
}

fn saved_index() -> u32 {
    let Ok(root) = local_config::project_root() else {
        return 0;
    };
    let Ok(config) = local_config::load_document(&root) else {
        return 0;
    };
    config
        .get("camera")
        .and_then(|camera| camera.get("index"))
        .and_then(|value| value.as_u64())
        .and_then(|index| u32::try_from(index).ok())
        .filter(|index| *index <= 32)
        .unwrap_or(0)
}

fn saved_resolution() -> Option<Requested> {
    let root = local_config::project_root().ok()?;
    let config = local_config::load_document(&root).ok()?;
    let text = config
        .get("camera")
        .and_then(|camera| camera.get("resolution"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    parse_resolution(text)
}

fn parse_resolution(text: &str) -> Option<Requested> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (size, rate) = match text.split_once('@') {
        Some((size, rate)) => (size, Some(rate)),
        None => (text, None),
    };
    let (width, height) = size.split_once('x')?;
    let width = width.parse::<u32>().ok()?;
    let height = height.parse::<u32>().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let fps_milli = match rate {
        Some(rate) => Some(parse_fps_milli(rate)?),
        None => None,
    };
    Some(Requested {
        width,
        height,
        fps_milli,
    })
}

fn parse_fps_milli(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (whole, frac) = match text.split_once('.') {
        Some((whole, frac)) => (whole, Some(frac)),
        None => (text, None),
    };
    let whole = whole.parse::<u32>().ok()?;
    let Some(frac) = frac else {
        return whole.checked_mul(1000);
    };
    if frac.is_empty() || frac.len() > 3 || !frac.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let digits = frac.parse::<u32>().ok()?;
    let scale = 10_u32.pow(3 - frac.len() as u32);
    whole
        .checked_mul(1000)?
        .checked_add(digits.checked_mul(scale)?)
}

fn is_resource(message: &str) -> bool {
    message.contains("0xC00D3704")
        || message
            .to_ascii_lowercase()
            .contains("lack of hardware resources")
}

fn short_camera(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return "Camera did not open.".to_string();
    }
    let mut end = text.len();
    if let Some(index) = text.find('\n') {
        end = end.min(index);
    }
    let text = text[..end].trim();
    if text.chars().count() > 180 {
        let cut = text
            .char_indices()
            .nth(180)
            .map(|(index, _)| index)
            .unwrap_or(text.len());
        format!("Camera did not open. {}…", text[..cut].trim_end())
    } else {
        format!("Camera did not open. {text}")
    }
}

fn round_fps(fps: f32) -> f32 {
    (fps * 10.0).round() / 10.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PixelFormat {
    Mjpeg,
    Nv12,
    Yuy2,
    Rgb24,
}

impl PixelFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Mjpeg => "MJPEG",
            Self::Nv12 => "NV12",
            Self::Yuy2 => "YUY2",
            Self::Rgb24 => "RGB24",
        }
    }

    /// NV12 is the layout these cameras stream without a second transform.
    fn preference(self) -> u8 {
        match self {
            Self::Nv12 => 3,
            Self::Mjpeg => 2,
            Self::Yuy2 => 1,
            Self::Rgb24 => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mode {
    index: u32,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    format: PixelFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Requested {
    width: u32,
    height: u32,
    /// Thousandths of a frame. Absent means the rate closest to 30 fps.
    fps_milli: Option<u32>,
}

/// Pick a mode the camera actually listed.
///
/// A saved size and rate win when that size is listed. Otherwise the highest
/// 16:9 mode at 30 fps wins, or the closest listed rate when 30 is absent.
/// NV12 wins ties. 640×480 is used only when it is the saved mode, or when
/// the camera lists nothing else.
fn choose_mode(modes: &[Mode], wanted: Option<Requested>) -> Option<Mode> {
    let usable: Vec<&Mode> = modes
        .iter()
        .filter(|mode| mode.width > 0 && mode.height > 0 && mode.fps_num > 0 && mode.fps_den > 0)
        .collect();
    if usable.is_empty() {
        return None;
    }
    if let Some(wanted) = wanted {
        let same_size: Vec<&Mode> = usable
            .iter()
            .copied()
            .filter(|mode| mode.width == wanted.width && mode.height == wanted.height)
            .collect();
        if !same_size.is_empty() {
            return Some(pick_sized(&same_size, wanted.fps_milli.unwrap_or(30_000)));
        }
    }
    Some(default_mode(&usable))
}

fn pick_sized(modes: &[&Mode], target_milli: u32) -> Mode {
    modes
        .iter()
        .copied()
        .max_by_key(|mode| {
            let distance = fps_milli(mode).abs_diff(u64::from(target_milli));
            (
                u64::MAX - distance,
                mode.format.preference(),
                u32::MAX - mode.index,
            )
        })
        .copied()
        .expect("sized mode list is not empty")
}

fn default_mode(modes: &[&Mode]) -> Mode {
    let sixteen: Vec<&Mode> = modes
        .iter()
        .copied()
        .filter(|mode| is_sixteen_by_nine(mode))
        .collect();
    let pool: &[&Mode] = if sixteen.is_empty() { modes } else { &sixteen };
    let nearest = pool
        .iter()
        .map(|mode| fps_milli(mode).abs_diff(30_000))
        .min()
        .unwrap_or(0);
    pool.iter()
        .copied()
        .filter(|mode| fps_milli(mode).abs_diff(30_000) == nearest)
        .max_by_key(|mode| {
            (
                mode.width as u64 * mode.height as u64,
                mode.format.preference(),
                u32::MAX - mode.index,
            )
        })
        .copied()
        .expect("default mode list is not empty")
}

fn is_sixteen_by_nine(mode: &Mode) -> bool {
    mode.width.saturating_mul(9) == mode.height.saturating_mul(16)
}

fn mode_id(mode: Mode) -> String {
    format!(
        "{}x{}@{}",
        mode.width,
        mode.height,
        fps_token(fps_milli(&mode))
    )
}

fn fps_token(milli: u64) -> String {
    let whole = milli / 1000;
    let frac = milli % 1000;
    if frac == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{frac:03}")
            .trim_end_matches('0')
            .to_string()
    }
}

/// One entry per size and rate. NV12 represents that entry when it is listed.
fn mode_choices(modes: &[Mode]) -> Vec<Mode> {
    let mut choices: Vec<Mode> = Vec::new();
    for mode in modes {
        if mode.width == 0 || mode.height == 0 || mode.fps_num == 0 || mode.fps_den == 0 {
            continue;
        }
        let rate = fps_milli(mode);
        if let Some(existing) = choices.iter_mut().find(|item| {
            item.width == mode.width && item.height == mode.height && fps_milli(item) == rate
        }) {
            if mode.format.preference() > existing.format.preference() {
                *existing = *mode;
            }
        } else {
            choices.push(*mode);
        }
    }
    choices.sort_by(|left, right| {
        let area = (right.width as u64 * right.height as u64)
            .cmp(&(left.width as u64 * left.height as u64));
        area.then(fps_milli(right).cmp(&fps_milli(left)))
    });
    choices
}

fn fps_milli(mode: &Mode) -> u64 {
    (mode.fps_num as u64).saturating_mul(1000) / mode.fps_den as u64
}

struct FpsMeter {
    stamps: VecDeque<Instant>,
}

impl FpsMeter {
    fn new() -> Self {
        Self {
            stamps: VecDeque::new(),
        }
    }

    fn note(&mut self, now: Instant) {
        self.stamps.push_back(now);
        let window = Duration::from_secs(1);
        while self
            .stamps
            .front()
            .is_some_and(|stamp| now.saturating_duration_since(*stamp) > window)
        {
            self.stamps.pop_front();
        }
    }

    fn measured(&self, now: Instant) -> Option<f32> {
        let window = Duration::from_secs(1);
        let recent: Vec<Instant> = self
            .stamps
            .iter()
            .copied()
            .filter(|stamp| now.saturating_duration_since(*stamp) <= window)
            .collect();
        if recent.len() < 2 {
            return None;
        }
        let span = recent
            .last()
            .unwrap()
            .saturating_duration_since(*recent.first().unwrap());
        if span < Duration::from_millis(200) {
            return None;
        }
        Some((recent.len() - 1) as f32 / span.as_secs_f32())
    }
}

fn encode_frame(bytes: &[u8], mode: Mode) -> Result<Vec<u8>, String> {
    let (rgb, width, height) = match mode.format {
        PixelFormat::Mjpeg => mjpeg_to_rgb(bytes)?,
        PixelFormat::Yuy2 => (
            yuy2_to_rgb(bytes, mode.width, mode.height)?,
            mode.width,
            mode.height,
        ),
        PixelFormat::Nv12 => return nv12_to_jpeg(bytes, mode.width, mode.height),
        PixelFormat::Rgb24 => (
            bgr_to_rgb(bytes, mode.width, mode.height)?,
            mode.width,
            mode.height,
        ),
    };
    compress_rgb(&rgb, width, height)
}

/// Fit `width`×`height` inside 1280×720. Sizes that already fit are unchanged
/// so a 720p (or smaller) mode is not scaled up. The open reader is not moved
/// to another listed mode.
fn preview_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 || (width <= PREVIEW_MAX_WIDTH && height <= PREVIEW_MAX_HEIGHT) {
        return (width, height);
    }
    let by_width = u64::from(width) * u64::from(PREVIEW_MAX_HEIGHT)
        >= u64::from(height) * u64::from(PREVIEW_MAX_WIDTH);
    let (mut scaled_width, mut scaled_height) = if by_width {
        let scaled_width = PREVIEW_MAX_WIDTH & !1;
        let scaled_height =
            (u64::from(height) * u64::from(scaled_width) / u64::from(width)) as u32 & !1;
        (scaled_width, scaled_height)
    } else {
        let scaled_height = PREVIEW_MAX_HEIGHT & !1;
        let scaled_width =
            (u64::from(width) * u64::from(scaled_height) / u64::from(height)) as u32 & !1;
        (scaled_width, scaled_height)
    };
    if scaled_width < 2 {
        scaled_width = 2;
    }
    if scaled_height < 2 {
        scaled_height = 2;
    }
    (scaled_width, scaled_height)
}

fn encode_preview(bytes: &[u8], mode: Mode) -> Result<(Vec<u8>, u32, u32), String> {
    if mode.format == PixelFormat::Nv12 {
        let (width, height) = preview_size(mode.width, mode.height);
        if width == mode.width && height == mode.height {
            return Ok((nv12_to_jpeg(bytes, mode.width, mode.height)?, width, height));
        }
        let scaled = scale_nv12(bytes, mode.width, mode.height, width, height)?;
        return Ok((nv12_to_jpeg(&scaled, width, height)?, width, height));
    }
    let (rgb, width, height) = match mode.format {
        PixelFormat::Mjpeg => mjpeg_to_rgb(bytes)?,
        PixelFormat::Yuy2 => (
            yuy2_to_rgb(bytes, mode.width, mode.height)?,
            mode.width,
            mode.height,
        ),
        PixelFormat::Rgb24 => (
            bgr_to_rgb(bytes, mode.width, mode.height)?,
            mode.width,
            mode.height,
        ),
        PixelFormat::Nv12 => unreachable!("NV12 is encoded above"),
    };
    let (preview_width, preview_height) = preview_size(width, height);
    if preview_width == width && preview_height == height {
        return Ok((
            compress_rgb(&rgb, width, height)?,
            preview_width,
            preview_height,
        ));
    }
    let scaled = scale_rgb(&rgb, width, height, preview_width, preview_height)?;
    Ok((
        compress_rgb(&scaled, preview_width, preview_height)?,
        preview_width,
        preview_height,
    ))
}

fn scale_nv12(
    bytes: &[u8],
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Result<Vec<u8>, String> {
    if src_width < 2
        || src_height < 2
        || dst_width < 2
        || dst_height < 2
        || src_width % 2 != 0
        || src_height % 2 != 0
        || dst_width % 2 != 0
        || dst_height % 2 != 0
    {
        return Err("Could not encode the frame.".to_string());
    }
    let src_width = src_width as usize;
    let src_height = src_height as usize;
    let dst_width = dst_width as usize;
    let dst_height = dst_height as usize;
    let y_size = src_width.saturating_mul(src_height);
    let need = y_size.saturating_add(y_size / 2);
    if bytes.len() < need {
        return Err("Could not encode the frame.".to_string());
    }
    let mut out = vec![0_u8; dst_width * dst_height + dst_width * dst_height / 2];
    for y in 0..dst_height {
        let src_y = y * src_height / dst_height;
        let src_row = src_y * src_width;
        let dst_row = y * dst_width;
        for x in 0..dst_width {
            let src_x = x * src_width / dst_width;
            out[dst_row + x] = bytes[src_row + src_x];
        }
    }
    let src_chroma_width = src_width / 2;
    let src_chroma_height = src_height / 2;
    let dst_chroma_width = dst_width / 2;
    let dst_chroma_height = dst_height / 2;
    let src_uv = y_size;
    let dst_uv = dst_width * dst_height;
    for y in 0..dst_chroma_height {
        let src_y = y * src_chroma_height / dst_chroma_height;
        let src_row = src_uv + src_y * src_chroma_width * 2;
        let dst_row = dst_uv + y * dst_chroma_width * 2;
        for x in 0..dst_chroma_width {
            let src_x = x * src_chroma_width / dst_chroma_width;
            let src_at = src_row + src_x * 2;
            let dst_at = dst_row + x * 2;
            out[dst_at] = bytes[src_at];
            out[dst_at + 1] = bytes[src_at + 1];
        }
    }
    Ok(out)
}

fn scale_rgb(
    bytes: &[u8],
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Result<Vec<u8>, String> {
    if src_width == 0 || src_height == 0 || dst_width == 0 || dst_height == 0 {
        return Err("Could not encode the frame.".to_string());
    }
    let src_width = src_width as usize;
    let src_height = src_height as usize;
    let dst_width = dst_width as usize;
    let dst_height = dst_height as usize;
    let need = src_width.saturating_mul(src_height).saturating_mul(3);
    if bytes.len() < need {
        return Err("Could not encode the frame.".to_string());
    }
    let mut out = vec![0_u8; dst_width * dst_height * 3];
    for y in 0..dst_height {
        let src_y = y * src_height / dst_height;
        for x in 0..dst_width {
            let src_x = x * src_width / dst_width;
            let src_at = (src_y * src_width + src_x) * 3;
            let dst_at = (y * dst_width + x) * 3;
            out[dst_at] = bytes[src_at];
            out[dst_at + 1] = bytes[src_at + 1];
            out[dst_at + 2] = bytes[src_at + 2];
        }
    }
    Ok(out)
}

fn mjpeg_to_rgb(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let image = turbojpeg::decompress(bytes, turbojpeg::PixelFormat::RGB)
        .map_err(|_| "Could not decode the camera frame.".to_string())?;
    let width =
        u32::try_from(image.width).map_err(|_| "Could not decode the camera frame.".to_string())?;
    let height = u32::try_from(image.height)
        .map_err(|_| "Could not decode the camera frame.".to_string())?;
    if width == 0 || height == 0 {
        return Err("Could not decode the camera frame.".to_string());
    }
    let row = image.width.saturating_mul(3);
    if image.pitch < row {
        return Err("Could not decode the camera frame.".to_string());
    }
    if image.pitch == row {
        return Ok((image.pixels, width, height));
    }
    let mut packed = Vec::with_capacity(row.saturating_mul(image.height));
    for y in 0..image.height {
        let start = y.saturating_mul(image.pitch);
        let end = start + row;
        if end > image.pixels.len() {
            return Err("Could not decode the camera frame.".to_string());
        }
        packed.extend_from_slice(&image.pixels[start..end]);
    }
    Ok((packed, width, height))
}

fn compress_rgb(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("Could not encode the frame.".to_string());
    }
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3);
    if rgb.len() < expected {
        return Err("Could not encode the frame.".to_string());
    }
    let image = turbojpeg::Image {
        pixels: &rgb[..expected],
        width: width as usize,
        pitch: (width as usize).saturating_mul(3),
        height: height as usize,
        format: turbojpeg::PixelFormat::RGB,
    };
    let jpeg = turbojpeg::compress(image, JPEG_QUALITY, turbojpeg::Subsamp::Sub2x2)
        .map_err(|_| "Could not encode the frame.".to_string())?;
    Ok(jpeg.as_ref().to_vec())
}

fn yuy2_to_rgb(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let row = (width as usize).saturating_mul(2);
    let need = row.saturating_mul(height as usize);
    if width < 2 || height == 0 || width % 2 != 0 || bytes.len() < need {
        return Err("Could not decode the camera frame.".to_string());
    }
    let mut rgb = vec![0_u8; (width as usize) * (height as usize) * 3];
    let mut src = 0;
    let mut dst = 0;
    for _ in 0..height {
        let row_end = src + row;
        while src + 4 <= row_end {
            let y0 = bytes[src] as i32;
            let u = bytes[src + 1] as i32;
            let y1 = bytes[src + 2] as i32;
            let v = bytes[src + 3] as i32;
            write_rgb(&mut rgb, dst, y0, u, v);
            write_rgb(&mut rgb, dst + 3, y1, u, v);
            src += 4;
            dst += 6;
        }
    }
    Ok(rgb)
}

fn nv12_to_jpeg(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width < 2 || height < 2 || width % 2 != 0 || height % 2 != 0 {
        return Err("Could not encode the frame.".to_string());
    }
    let width = width as usize;
    let height = height as usize;
    let y_size = width.saturating_mul(height);
    let chroma = y_size / 4;
    let need = y_size.saturating_add(chroma.saturating_mul(2));
    if bytes.len() < need || chroma == 0 {
        return Err("Could not encode the frame.".to_string());
    }
    let mut planar = vec![0_u8; need];
    planar[..y_size].copy_from_slice(&bytes[..y_size]);
    let (u, v) = planar[y_size..].split_at_mut(chroma);
    for (index, pair) in bytes[y_size..need].chunks_exact(2).enumerate() {
        if index >= chroma {
            break;
        }
        u[index] = pair[0];
        v[index] = pair[1];
    }
    let image = turbojpeg::YuvImage {
        pixels: planar.as_slice(),
        width,
        align: 1,
        height,
        subsamp: turbojpeg::Subsamp::Sub2x2,
    };
    let jpeg = turbojpeg::compress_yuv(image, JPEG_QUALITY)
        .map_err(|_| "Could not encode the frame.".to_string())?;
    Ok(jpeg.as_ref().to_vec())
}

fn bgr_to_rgb(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let need = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3);
    if width == 0 || height == 0 || bytes.len() < need {
        return Err("Could not decode the camera frame.".to_string());
    }
    let mut rgb = bytes[..need].to_vec();
    for pixel in rgb.chunks_exact_mut(3) {
        pixel.swap(0, 2);
    }
    Ok(rgb)
}

fn write_rgb(rgb: &mut [u8], at: usize, y: i32, u: i32, v: i32) {
    let c = y - 16;
    let d = u - 128;
    let e = v - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    rgb[at] = r.clamp(0, 255) as u8;
    rgb[at + 1] = g.clamp(0, 255) as u8;
    rgb[at + 2] = b.clamp(0, 255) as u8;
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(windows)]
mod session {
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use windows::Win32::Media::MediaFoundation::{
        IMFActivate, IMFAttributes, IMFMediaSource, IMFMediaType, IMFSample, IMFSourceReader,
        IMFSourceReaderCallback, IMFSourceReaderCallback_Impl, MF_API_VERSION,
        MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        MF_E_HW_MFT_FAILED_START_STREAMING, MF_E_NO_MORE_TYPES, MF_MT_FRAME_RATE,
        MF_MT_FRAME_RATE_RANGE_MAX, MF_MT_FRAME_RATE_RANGE_MIN, MF_MT_FRAME_SIZE, MF_MT_SUBTYPE,
        MF_READWRITE_DISABLE_CONVERTERS, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
        MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ASYNC_CALLBACK,
        MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
        MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MFCreateAttributes,
        MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFSTARTUP_NOSOCKET, MFShutdown,
        MFStartup, MFVideoFormat_MJPG, MFVideoFormat_NV12, MFVideoFormat_RGB24, MFVideoFormat_YUY2,
    };
    #[cfg(test)]
    use windows::Win32::Media::MediaFoundation::{
        MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_E_INVALIDSTREAMNUMBER,
    };
    use windows::Win32::System::Com::{
        COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows::core::{Error, HRESULT, Ref, implement};

    use super::{
        FpsMeter, Mode, PixelFormat, RELEASE_WAIT, RESOURCE_CODE, Requested, choose_mode,
        fps_milli, lock, short_camera,
    };

    const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
    const FLAG_ERROR: u32 = MF_SOURCE_READERF_ERROR.0 as u32;
    const FLAG_EOS: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
    const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

    struct Shared {
        stop: AtomicBool,
        flush_done: AtomicBool,
        reader: Mutex<Option<IMFSourceReader>>,
        frame: Mutex<Option<Vec<u8>>>,
        frame_cv: Condvar,
        fps: Mutex<FpsMeter>,
        error: Mutex<Option<String>>,
    }

    impl Shared {
        fn new() -> Self {
            Self {
                stop: AtomicBool::new(false),
                flush_done: AtomicBool::new(false),
                reader: Mutex::new(None),
                frame: Mutex::new(None),
                frame_cv: Condvar::new(),
                fps: Mutex::new(FpsMeter::new()),
                error: Mutex::new(None),
            }
        }

        /// One slot. The next sample replaces it, so a slow preview cannot
        /// build a queue of frames behind the reader.
        fn publish(&self, bytes: Vec<u8>) {
            *lock(&self.frame) = Some(bytes);
            lock(&self.fps).note(Instant::now());
            *lock(&self.error) = None;
            self.frame_cv.notify_all();
        }

        fn fail(&self, message: String) {
            *lock(&self.error) = Some(message);
            self.frame_cv.notify_all();
        }

        fn rearm(&self) {
            if self.stop.load(Ordering::Acquire) {
                return;
            }
            let reader = lock(&self.reader).clone();
            if let Some(reader) = reader {
                unsafe {
                    let _ = reader.ReadSample(STREAM, 0, None, None, None, None);
                }
            }
        }
    }

    #[implement(IMFSourceReaderCallback)]
    struct ReaderCallback {
        shared: Arc<Shared>,
    }

    impl IMFSourceReaderCallback_Impl for ReaderCallback_Impl {
        fn OnReadSample(
            &self,
            status: HRESULT,
            _stream: u32,
            flags: u32,
            _timestamp: i64,
            sample: Ref<IMFSample>,
        ) -> windows::core::Result<()> {
            if self.shared.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            if status.is_err() {
                self.shared.fail(short_camera(&hresult_text(status)));
                return Ok(());
            }
            if flags & (FLAG_ERROR | FLAG_EOS) != 0 {
                self.shared
                    .fail(short_camera("The camera stopped streaming."));
                return Ok(());
            }
            // Request the next sample before copying this one so the
            // driver is not stalled on the snapshot encode.
            self.shared.rearm();
            if let Some(sample) = sample.as_ref()
                && let Some(bytes) = copy_sample(sample)
            {
                self.shared.publish(bytes);
            }
            Ok(())
        }

        fn OnFlush(&self, _stream: u32) -> windows::core::Result<()> {
            self.shared.flush_done.store(true, Ordering::Release);
            self.shared.frame_cv.notify_all();
            Ok(())
        }

        fn OnEvent(
            &self,
            _stream: u32,
            _event: Ref<windows::Win32::Media::MediaFoundation::IMFMediaEvent>,
        ) -> windows::core::Result<()> {
            Ok(())
        }
    }

    pub(super) struct Device {
        index: u32,
        modes: Vec<Mode>,
        mode: Mode,
        shared: Arc<Shared>,
        reader: Option<IMFSourceReader>,
        source: Option<IMFMediaSource>,
        activate: Option<IMFActivate>,
        alive: bool,
    }

    impl Device {
        pub(super) fn index(&self) -> u32 {
            self.index
        }

        pub(super) fn modes(&self) -> Vec<Mode> {
            self.modes.clone()
        }

        /// The open reader already matches this request, so a settings change
        /// does not reopen the camera when the chosen mode is unchanged.
        pub(super) fn serves(&self, wanted: Option<Requested>) -> bool {
            self.alive && choose_mode(&self.modes, wanted).is_some_and(|mode| mode == self.mode)
        }

        pub(super) fn mode(&self) -> Mode {
            self.mode
        }

        pub(super) fn fps(&self) -> Option<f32> {
            lock(&self.shared.fps).measured(Instant::now())
        }

        pub(super) fn wait_frame(&self, timeout: Duration) -> Result<Vec<u8>, String> {
            let deadline = Instant::now() + timeout;
            let mut frame = lock(&self.shared.frame);
            loop {
                if frame.is_some() {
                    return Ok(frame.take().unwrap_or_default());
                }
                if let Some(message) = lock(&self.shared.error).clone() {
                    return Err(message);
                }
                let now = Instant::now();
                if now >= deadline {
                    return Err("Camera did not deliver a frame.".to_string());
                }
                let (next, wait) = self
                    .shared
                    .frame_cv
                    .wait_timeout(frame, deadline.saturating_duration_since(now))
                    .unwrap_or_else(|err| err.into_inner());
                frame = next;
                if wait.timed_out() && frame.is_none() {
                    if let Some(message) = lock(&self.shared.error).clone() {
                        return Err(message);
                    }
                    return Err("Camera did not deliver a frame.".to_string());
                }
            }
        }

        /// Flush, then shut the media source down, then drop the reader.
        /// The hardware MFT stays reserved until `Shutdown` returns, so the
        /// next open waits before it activates another camera.
        pub(super) fn shutdown(&mut self) {
            if !self.alive {
                return;
            }
            self.alive = false;
            self.shared.stop.store(true, Ordering::SeqCst);
            {
                *lock(&self.shared.reader) = None;
            }
            // Flush while the reader still exists, wait until that flush
            // finishes, stop the source, then drop the reader. Shutting the
            // source down after the reader is gone leaves the hardware MFT
            // reserved, and the next open fails with 0xC00D3704.
            self.shared.flush_done.store(false, Ordering::Release);
            if let Some(reader) = self.reader.as_ref() {
                unsafe {
                    let _ = reader.Flush(ALL_STREAMS);
                }
            }
            let started = Instant::now();
            while !self.shared.flush_done.load(Ordering::Acquire)
                && started.elapsed() < Duration::from_millis(500)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            if let Some(source) = self.source.as_ref() {
                unsafe {
                    let _ = source.Stop();
                }
            }
            self.reader.take();
            if let Some(source) = self.source.take() {
                unsafe {
                    let _ = source.Shutdown();
                }
            }
            if let Some(activate) = self.activate.take() {
                unsafe {
                    let _ = activate.ShutdownObject();
                }
            }
            std::thread::sleep(RELEASE_WAIT);
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    pub(super) fn startup() -> Result<(), String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|err| short_camera(&mf_error(&err)))?;
            MFStartup(MF_API_VERSION, MFSTARTUP_NOSOCKET)
                .map_err(|err| short_camera(&mf_error(&err)))?;
        }
        Ok(())
    }

    pub(super) fn shutdown_platform() {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }

    pub(super) fn list_indexes() -> Result<Vec<u32>, String> {
        let devices = enum_devices()?;
        Ok((0..devices.count as u32).collect())
    }

    #[cfg(test)]
    pub(super) fn explain(index: u32) -> Result<String, String> {
        let devices = enum_devices()?;
        let activate = devices
            .get(index)
            .ok_or_else(|| format!("Camera {index} was not found."))?;
        let name = friendly_name(&activate);
        drop(devices);
        let source = match unsafe { activate.ActivateObject::<IMFMediaSource>() } {
            Ok(source) => source,
            Err(err) => {
                release_failed(None, None, Some(activate));
                return Err(short_camera(&mf_error(&err)));
            }
        };
        let mut attributes = None;
        if let Err(err) = unsafe { MFCreateAttributes(&mut attributes, 2) } {
            release_failed(None, Some(source), Some(activate));
            return Err(short_camera(&mf_error(&err)));
        }
        let Some(attributes) = attributes else {
            release_failed(None, Some(source), Some(activate));
            return Err(short_camera("Could not list modes."));
        };
        if let Err(err) = unsafe {
            attributes
                .SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)
                .and_then(|_| {
                    attributes.SetUINT32(&MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN, 1)
                })
        } {
            release_failed(None, Some(source), Some(activate));
            return Err(short_camera(&mf_error(&err)));
        }
        let reader = match unsafe { MFCreateSourceReaderFromMediaSource(&source, &attributes) } {
            Ok(reader) => reader,
            Err(err) => {
                release_failed(None, Some(source), Some(activate));
                return Err(short_camera(&mf_error(&err)));
            }
        };
        let report = match stream_report(&reader) {
            Ok(report) => report,
            Err(message) => {
                release_failed(Some(reader), Some(source), Some(activate));
                return Err(message);
            }
        };
        release_failed(Some(reader), Some(source), Some(activate));
        Ok(format!("camera {index}: {name}\n{report}"))
    }

    #[cfg(test)]
    fn friendly_name(activate: &IMFActivate) -> String {
        let mut value = windows::core::PWSTR::null();
        let mut length = 0_u32;
        let read = unsafe {
            activate.GetAllocatedString(
                &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                &mut value,
                &mut length,
            )
        };
        if read.is_err() || value.0.is_null() {
            return String::new();
        }
        let name = unsafe {
            String::from_utf16_lossy(std::slice::from_raw_parts(value.0, length as usize))
        };
        unsafe { CoTaskMemFree(Some(value.0.cast())) };
        name
    }

    #[cfg(test)]
    fn stream_report(reader: &IMFSourceReader) -> Result<String, String> {
        let mut lines = Vec::new();
        for stream in 0..8_u32 {
            match unsafe { reader.GetNativeMediaType(stream, 0) } {
                Ok(media_type) => {
                    lines.push(format!("  stream {stream}: {}", mode_line(&media_type, 0)));
                    let mut index = 1_u32;
                    loop {
                        match unsafe { reader.GetNativeMediaType(stream, index) } {
                            Ok(media_type) => {
                                lines.push(format!("    {}", mode_line(&media_type, index)));
                                index = index.saturating_add(1);
                            }
                            Err(err) if err.code() == MF_E_NO_MORE_TYPES => break,
                            Err(_) => break,
                        }
                    }
                }
                Err(err) if err.code() == MF_E_INVALIDSTREAMNUMBER => break,
                Err(err) if stream == 0 => return Err(short_camera(&mf_error(&err))),
                Err(_) => break,
            }
        }
        if lines.is_empty() {
            return Err(short_camera("Camera has no supported video mode."));
        }
        Ok(lines.join("\n"))
    }

    #[cfg(test)]
    fn mode_line(media_type: &IMFMediaType, index: u32) -> String {
        let name = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }
            .map(subtype_name)
            .unwrap_or_else(|_| "?".to_string());
        let (width, height) = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }
            .ok()
            .map(|size| ((size >> 32) as u32, size as u32))
            .unwrap_or((0, 0));
        let rate = best_rate(media_type)
            .map(|(numerator, denominator)| format!("{numerator}/{denominator}"))
            .unwrap_or_else(|| "?".to_string());
        format!("{index}: {width}x{height} {name} {rate}")
    }

    #[cfg(test)]
    fn subtype_name(guid: windows::core::GUID) -> String {
        if guid == MFVideoFormat_MJPG {
            "MJPEG".to_string()
        } else if guid == MFVideoFormat_NV12 {
            "NV12".to_string()
        } else if guid == MFVideoFormat_YUY2 {
            "YUY2".to_string()
        } else if guid == MFVideoFormat_RGB24 {
            "RGB24".to_string()
        } else {
            format!("{guid:?}")
        }
    }

    pub(super) fn probe_modes(index: u32) -> Result<Vec<Mode>, String> {
        let devices = enum_devices()?;
        let activate = devices
            .get(index)
            .ok_or_else(|| format!("Camera {index} was not found."))?;
        drop(devices);
        let source = match unsafe { activate.ActivateObject::<IMFMediaSource>() } {
            Ok(source) => source,
            Err(err) => {
                release_failed(None, None, Some(activate));
                return Err(short_camera(&mf_error(&err)));
            }
        };
        let mut attributes = None;
        if let Err(err) = unsafe { MFCreateAttributes(&mut attributes, 2) } {
            release_failed(None, Some(source), Some(activate));
            return Err(short_camera(&mf_error(&err)));
        }
        let Some(attributes) = attributes else {
            release_failed(None, Some(source), Some(activate));
            return Err(short_camera("Could not list modes."));
        };
        if let Err(err) = unsafe {
            attributes
                .SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)
                .and_then(|_| {
                    attributes.SetUINT32(&MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN, 1)
                })
        } {
            release_failed(None, Some(source), Some(activate));
            return Err(short_camera(&mf_error(&err)));
        }
        let reader = match unsafe { MFCreateSourceReaderFromMediaSource(&source, &attributes) } {
            Ok(reader) => reader,
            Err(err) => {
                release_failed(None, Some(source), Some(activate));
                return Err(short_camera(&mf_error(&err)));
            }
        };
        let modes = match native_modes(&reader) {
            Ok(modes) => modes,
            Err(message) => {
                release_failed(Some(reader), Some(source), Some(activate));
                return Err(message);
            }
        };
        release_failed(Some(reader), Some(source), Some(activate));
        Ok(modes)
    }

    pub(super) fn open_device(index: u32, wanted: Option<Requested>) -> Result<Device, String> {
        // Leave extra hardware decoders off so they do not take the camera's
        // MFT slot. A failure with 0xC00D3704 shuts that source down and
        // retries once with hardware transforms on. The integrated camera
        // sometimes needs that second attempt. The async error arrives with
        // the first sample, so each attempt waits for a frame or that error.
        let mut last = "Camera did not open.".to_string();
        for hardware in [false, true] {
            match try_open(index, wanted, hardware) {
                Ok(mut device) => match confirm_started(&mut device) {
                    Ok(()) => return Ok(device),
                    Err(message) if super::is_resource(&message) => last = message,
                    Err(message) => return Err(message),
                },
                Err(message) if super::is_resource(&message) => last = message,
                Err(message) => return Err(message),
            }
        }
        Err(last)
    }

    fn confirm_started(device: &mut Device) -> Result<(), String> {
        match device.wait_frame(Duration::from_secs(2)) {
            Ok(_) => Ok(()),
            Err(message) if super::is_resource(&message) => {
                device.shutdown();
                Err(message)
            }
            Err(message) if message.contains("did not deliver a frame") => Ok(()),
            Err(message) => {
                device.shutdown();
                Err(message)
            }
        }
    }

    fn try_open(index: u32, wanted: Option<Requested>, hardware: bool) -> Result<Device, String> {
        let devices = enum_devices()?;
        let activate = devices
            .get(index)
            .ok_or_else(|| format!("Camera {index} was not found."))?;
        drop(devices);
        let source = unsafe { activate.ActivateObject::<IMFMediaSource>() }
            .map_err(|err| short_camera(&mf_error(&err)))?;
        let shared = Arc::new(Shared::new());
        let callback: IMFSourceReaderCallback = ReaderCallback {
            shared: Arc::clone(&shared),
        }
        .into();
        let attributes = match reader_attributes(&callback, hardware) {
            Ok(attributes) => attributes,
            Err(message) => {
                release_failed(None, Some(source), Some(activate));
                return Err(message);
            }
        };
        let reader = match unsafe { MFCreateSourceReaderFromMediaSource(&source, &attributes) } {
            Ok(reader) => reader,
            Err(err) => {
                release_failed(None, Some(source), Some(activate));
                return Err(short_camera(&mf_error(&err)));
            }
        };
        let mut device = Device {
            index,
            modes: Vec::new(),
            mode: Mode {
                index: 0,
                width: 0,
                height: 0,
                fps_num: 0,
                fps_den: 1,
                format: PixelFormat::Mjpeg,
            },
            shared: Arc::clone(&shared),
            reader: Some(reader),
            source: Some(source),
            activate: Some(activate),
            alive: true,
        };
        let reader = device.reader.as_ref().expect("reader");
        let modes = match native_modes(reader) {
            Ok(modes) => modes,
            Err(message) => {
                device.shutdown();
                return Err(message);
            }
        };
        device.modes = modes.clone();
        if let Err(message) = select_mode(reader, &modes, wanted, &mut device.mode) {
            device.shutdown();
            return Err(message);
        }
        *lock(&shared.reader) = device.reader.clone();
        let started = unsafe {
            device
                .reader
                .as_ref()
                .expect("reader")
                .ReadSample(STREAM, 0, None, None, None, None)
        };
        if let Err(err) = started {
            let message = short_camera(&mf_error(&err));
            device.shutdown();
            return Err(message);
        }
        Ok(device)
    }

    fn release_failed(
        reader: Option<IMFSourceReader>,
        source: Option<IMFMediaSource>,
        activate: Option<IMFActivate>,
    ) {
        if let Some(reader) = reader.as_ref() {
            unsafe {
                let _ = reader.Flush(ALL_STREAMS);
            }
        }
        if let Some(source) = source.as_ref() {
            unsafe {
                let _ = source.Stop();
            }
        }
        drop(reader);
        if let Some(source) = source {
            unsafe {
                let _ = source.Shutdown();
            }
        }
        if let Some(activate) = activate {
            unsafe {
                let _ = activate.ShutdownObject();
            }
        }
        std::thread::sleep(RELEASE_WAIT);
    }

    fn select_mode(
        reader: &IMFSourceReader,
        modes: &[Mode],
        wanted: Option<Requested>,
        chosen: &mut Mode,
    ) -> Result<(), String> {
        let mut remaining = modes.to_vec();
        let mut last = "Camera has no supported video mode.".to_string();
        while let Some(mode) = choose_mode(&remaining, wanted) {
            if let Some(wanted) = wanted {
                if mode.width != wanted.width || mode.height != wanted.height {
                    break;
                }
                if let Some(target) = wanted.fps_milli
                    && fps_milli(&mode).abs_diff(u64::from(target)) > 1_000
                {
                    break;
                }
            }
            match set_native_mode(reader, mode) {
                Ok(()) => {
                    *chosen = mode;
                    return Ok(());
                }
                Err(err) if is_hresult_resource(&err) => {
                    return Err(short_camera(&mf_error(&err)));
                }
                Err(err) => {
                    last = mf_error(&err);
                    remaining.retain(|item| item.index != mode.index);
                }
            }
        }
        Err(short_camera(&last))
    }

    fn set_native_mode(reader: &IMFSourceReader, mode: Mode) -> Result<(), Error> {
        let media_type = unsafe { reader.GetNativeMediaType(STREAM, mode.index) }?;
        unsafe {
            media_type.SetUINT64(&MF_MT_FRAME_RATE, pack_ratio(mode.fps_num, mode.fps_den))?;
            // Drop every pin first. A front camera that also exposes IR will
            // start a second hardware transform and fail with 0xC00D3704.
            let _ = reader.SetStreamSelection(ALL_STREAMS, false);
            for stream in 0..8_u32 {
                let _ = reader.SetStreamSelection(stream, false);
            }
            reader.SetStreamSelection(STREAM, true)?;
            reader.SetCurrentMediaType(STREAM, None, &media_type)?;
        }
        Ok(())
    }

    fn native_modes(reader: &IMFSourceReader) -> Result<Vec<Mode>, String> {
        let mut modes = Vec::new();
        let mut index = 0_u32;
        loop {
            match unsafe { reader.GetNativeMediaType(STREAM, index) } {
                Ok(media_type) => {
                    if let Some(mode) = describe_mode(&media_type, index) {
                        modes.push(mode);
                    }
                    index = index.saturating_add(1);
                }
                Err(err) if err.code() == MF_E_NO_MORE_TYPES => break,
                Err(err) if index == 0 => return Err(short_camera(&mf_error(&err))),
                Err(_) => break,
            }
        }
        if modes.is_empty() {
            return Err(short_camera("Camera has no supported video mode."));
        }
        Ok(modes)
    }

    fn describe_mode(media_type: &IMFMediaType, index: u32) -> Option<Mode> {
        let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }.ok()?;
        let format = if subtype == MFVideoFormat_MJPG {
            PixelFormat::Mjpeg
        } else if subtype == MFVideoFormat_NV12 {
            PixelFormat::Nv12
        } else if subtype == MFVideoFormat_YUY2 {
            PixelFormat::Yuy2
        } else if subtype == MFVideoFormat_RGB24 {
            PixelFormat::Rgb24
        } else {
            return None;
        };
        let size = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.ok()?;
        let width = (size >> 32) as u32;
        let height = size as u32;
        let (fps_num, fps_den) = best_rate(media_type)?;
        Some(Mode {
            index,
            width,
            height,
            fps_num,
            fps_den,
            format,
        })
    }

    fn best_rate(media_type: &IMFMediaType) -> Option<(u32, u32)> {
        // Use the rate the native type advertises. Writing the range maximum
        // asks some integrated cameras for a mode they never listed, and the
        // hardware MFT then refuses to start.
        if let Some(rate) = read_rate(media_type, &MF_MT_FRAME_RATE) {
            return Some(rate);
        }
        let mut best: Option<(u32, u32)> = None;
        for key in [MF_MT_FRAME_RATE_RANGE_MAX, MF_MT_FRAME_RATE_RANGE_MIN] {
            let Some(rate) = read_rate(media_type, &key) else {
                continue;
            };
            best = Some(match best {
                Some(current) if rate_milli(current) >= rate_milli(rate) => current,
                _ => rate,
            });
        }
        best
    }

    fn read_rate(media_type: &IMFMediaType, key: &windows::core::GUID) -> Option<(u32, u32)> {
        let packed = unsafe { media_type.GetUINT64(key) }.ok()?;
        unpack_ratio(packed)
    }

    fn rate_milli(rate: (u32, u32)) -> u64 {
        (rate.0 as u64).saturating_mul(1000) / rate.1 as u64
    }

    fn unpack_ratio(packed: u64) -> Option<(u32, u32)> {
        let numerator = (packed >> 32) as u32;
        let denominator = packed as u32;
        if numerator == 0 || denominator == 0 {
            None
        } else {
            Some((numerator, denominator))
        }
    }

    fn pack_ratio(numerator: u32, denominator: u32) -> u64 {
        ((numerator as u64) << 32) | denominator as u64
    }

    fn copy_sample(sample: &IMFSample) -> Option<Vec<u8>> {
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }.ok()?;
        let mut ptr = ptr::null_mut();
        let mut length = 0_u32;
        unsafe { buffer.Lock(&mut ptr, None, Some(&mut length)) }.ok()?;
        let bytes = if ptr.is_null() || length == 0 || length as usize > MAX_FRAME_BYTES {
            None
        } else {
            Some(unsafe { std::slice::from_raw_parts(ptr, length as usize) }.to_vec())
        };
        unsafe {
            let _ = buffer.Unlock();
        }
        bytes
    }

    struct Activates {
        ptr: *mut Option<IMFActivate>,
        count: usize,
    }

    impl Activates {
        fn get(&self, index: u32) -> Option<IMFActivate> {
            let index = index as usize;
            if self.ptr.is_null() || index >= self.count {
                return None;
            }
            unsafe { (*self.ptr.add(index)).clone() }
        }
    }

    impl Drop for Activates {
        fn drop(&mut self) {
            if self.ptr.is_null() {
                return;
            }
            unsafe {
                for index in 0..self.count {
                    drop(self.ptr.add(index).read());
                }
                CoTaskMemFree(Some(self.ptr.cast()));
            }
            self.ptr = ptr::null_mut();
        }
    }

    fn enum_devices() -> Result<Activates, String> {
        let attributes = source_attributes()?;
        let mut ptr: *mut Option<IMFActivate> = ptr::null_mut();
        let mut count = 0_u32;
        unsafe { MFEnumDeviceSources(&attributes, &mut ptr, &mut count) }
            .map_err(|err| short_camera(&mf_error(&err)))?;
        Ok(Activates {
            ptr,
            count: count as usize,
        })
    }

    fn source_attributes() -> Result<IMFAttributes, String> {
        let mut attributes = None;
        unsafe { MFCreateAttributes(&mut attributes, 1) }
            .map_err(|err| short_camera(&mf_error(&err)))?;
        let attributes = attributes.ok_or_else(|| short_camera("Could not list cameras."))?;
        unsafe {
            attributes.SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
        }
        .map_err(|err| short_camera(&mf_error(&err)))?;
        Ok(attributes)
    }

    fn reader_attributes(
        callback: &IMFSourceReaderCallback,
        hardware: bool,
    ) -> Result<IMFAttributes, String> {
        let mut attributes = None;
        unsafe { MFCreateAttributes(&mut attributes, 4) }
            .map_err(|err| short_camera(&mf_error(&err)))?;
        let attributes = attributes.ok_or_else(|| short_camera("Could not open the camera."))?;
        unsafe {
            // Keep the native subtype. A color converter would be a second
            // transform on top of the camera's own hardware MFT.
            attributes
                .SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)
                .map_err(|err| short_camera(&mf_error(&err)))?;
            attributes
                .SetUINT32(
                    &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
                    u32::from(hardware),
                )
                .map_err(|err| short_camera(&mf_error(&err)))?;
            attributes
                .SetUINT32(&MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN, 1)
                .map_err(|err| short_camera(&mf_error(&err)))?;
            attributes
                .SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, callback)
                .map_err(|err| short_camera(&mf_error(&err)))?;
        }
        Ok(attributes)
    }

    fn is_hresult_resource(err: &Error) -> bool {
        err.code() == MF_E_HW_MFT_FAILED_START_STREAMING || err.code().0 as u32 == RESOURCE_CODE
    }

    fn mf_error(err: &Error) -> String {
        hresult_text(err.code())
    }

    fn hresult_text(code: HRESULT) -> String {
        let hex = format!("0x{:08X}", code.0 as u32);
        let message = code.message();
        let message = message.trim();
        if message.is_empty() || message.contains(&hex) {
            if message.is_empty() {
                hex
            } else {
                message.to_string()
            }
        } else {
            format!("{message} ({hex})")
        }
    }
}

#[cfg(not(windows))]
mod session {
    use std::time::Duration;

    use super::{Mode, Requested};

    pub(super) struct Device {
        index: u32,
        mode: Mode,
    }

    impl Device {
        pub(super) fn index(&self) -> u32 {
            self.index
        }

        pub(super) fn modes(&self) -> Vec<Mode> {
            Vec::new()
        }

        pub(super) fn serves(&self, _wanted: Option<Requested>) -> bool {
            false
        }

        pub(super) fn mode(&self) -> Mode {
            self.mode
        }

        pub(super) fn fps(&self) -> Option<f32> {
            None
        }

        pub(super) fn wait_frame(&self, _timeout: Duration) -> Result<Vec<u8>, String> {
            Err("Camera capture is only available on Windows.".to_string())
        }

        pub(super) fn shutdown(&mut self) {}
    }

    pub(super) fn startup() -> Result<(), String> {
        Ok(())
    }

    pub(super) fn shutdown_platform() {}

    pub(super) fn list_indexes() -> Result<Vec<u32>, String> {
        Err("Camera capture is only available on Windows.".to_string())
    }

    pub(super) fn probe_modes(_index: u32) -> Result<Vec<Mode>, String> {
        Err("Camera capture is only available on Windows.".to_string())
    }

    pub(super) fn open_device(_index: u32, _wanted: Option<Requested>) -> Result<Device, String> {
        Err("Camera capture is only available on Windows.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(
        index: u32,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        format: PixelFormat,
    ) -> Mode {
        Mode {
            index,
            width,
            height,
            fps_num,
            fps_den,
            format,
        }
    }

    #[test]
    fn blank_request_skips_640x480_when_a_higher_mode_is_listed() {
        let modes = vec![
            mode(0, 640, 480, 60, 1, PixelFormat::Mjpeg),
            mode(1, 1280, 720, 30, 1, PixelFormat::Yuy2),
            mode(2, 1920, 1080, 30, 1, PixelFormat::Mjpeg),
        ];
        let chosen = choose_mode(&modes, None).unwrap();
        assert_eq!(
            (chosen.width, chosen.height, chosen.format),
            (1920, 1080, PixelFormat::Mjpeg)
        );
    }

    #[test]
    fn slow_high_resolution_loses_to_a_smooth_lower_one() {
        let modes = vec![
            mode(0, 3840, 2160, 5, 1, PixelFormat::Mjpeg),
            mode(1, 1920, 1080, 30000, 1001, PixelFormat::Nv12),
            mode(2, 640, 480, 120, 1, PixelFormat::Yuy2),
        ];
        let chosen = choose_mode(&modes, None).unwrap();
        assert_eq!((chosen.width, chosen.height), (1920, 1080));
        assert_eq!(chosen.fps_num, 30000);
    }

    #[test]
    fn saved_resolution_is_used_when_it_is_listed() {
        let modes = vec![
            mode(0, 640, 480, 30, 1, PixelFormat::Mjpeg),
            mode(1, 1920, 1080, 30, 1, PixelFormat::Mjpeg),
            mode(2, 1280, 720, 60, 1, PixelFormat::Yuy2),
            mode(3, 1280, 720, 30, 1, PixelFormat::Mjpeg),
        ];
        let chosen = choose_mode(
            &modes,
            Some(Requested {
                width: 1280,
                height: 720,
                fps_milli: None,
            }),
        )
        .unwrap();
        assert_eq!(
            (chosen.width, chosen.height, chosen.format),
            (1280, 720, PixelFormat::Mjpeg)
        );
        assert_eq!(chosen.fps_num, 30);
    }

    #[test]
    fn missing_saved_resolution_uses_the_highest_smooth_mode() {
        let modes = vec![
            mode(0, 640, 480, 30, 1, PixelFormat::Yuy2),
            mode(1, 1920, 1080, 30, 1, PixelFormat::Nv12),
        ];
        let chosen = choose_mode(
            &modes,
            Some(Requested {
                width: 3840,
                height: 2160,
                fps_milli: Some(30_000),
            }),
        )
        .unwrap();
        assert_eq!(
            (chosen.width, chosen.height, chosen.format),
            (1920, 1080, PixelFormat::Nv12)
        );
    }

    #[test]
    fn default_is_the_highest_16_by_9_at_30_and_prefers_nv12() {
        let modes = vec![
            mode(0, 640, 480, 30, 1, PixelFormat::Nv12),
            mode(1, 1920, 1440, 30, 1, PixelFormat::Nv12),
            mode(2, 2560, 1440, 15, 1, PixelFormat::Nv12),
            mode(3, 1920, 1080, 30, 1, PixelFormat::Mjpeg),
            mode(4, 1920, 1080, 30, 1, PixelFormat::Nv12),
            mode(5, 1280, 720, 30, 1, PixelFormat::Nv12),
        ];
        let chosen = choose_mode(&modes, None).unwrap();
        assert_eq!(
            (chosen.width, chosen.height, chosen.format),
            (1920, 1080, PixelFormat::Nv12)
        );
    }

    #[test]
    fn exact_saved_rate_is_not_replaced_by_a_higher_mode() {
        let modes = vec![
            mode(0, 2560, 1440, 30, 1, PixelFormat::Nv12),
            mode(1, 1280, 720, 30, 1, PixelFormat::Mjpeg),
            mode(2, 1280, 720, 30, 1, PixelFormat::Nv12),
        ];
        let chosen = choose_mode(
            &modes,
            Some(Requested {
                width: 1280,
                height: 720,
                fps_milli: Some(30_000),
            }),
        )
        .unwrap();
        assert_eq!(
            (chosen.width, chosen.height, chosen.format),
            (1280, 720, PixelFormat::Nv12)
        );
        assert_eq!(mode_id(chosen), "1280x720@30");
    }

    #[test]
    fn missing_saved_mode_does_not_fall_back_to_vga() {
        let modes = vec![
            mode(0, 640, 480, 120, 1, PixelFormat::Nv12),
            mode(1, 1280, 720, 30, 1, PixelFormat::Nv12),
        ];
        let chosen = choose_mode(
            &modes,
            Some(Requested {
                width: 3840,
                height: 2160,
                fps_milli: Some(30_000),
            }),
        )
        .unwrap();
        assert_eq!((chosen.width, chosen.height), (1280, 720));
    }

    #[test]
    fn nv12_block_becomes_a_jpeg() {
        let jpeg = encode_frame(
            &[16, 16, 16, 16, 128, 128],
            Mode {
                index: 0,
                width: 2,
                height: 2,
                fps_num: 30,
                fps_den: 1,
                format: PixelFormat::Nv12,
            },
        )
        .unwrap();
        assert!(jpeg.starts_with(&[0xFF, 0xD8]), "{jpeg:?}");
    }

    #[test]
    fn preview_fits_large_modes_inside_720p() {
        assert_eq!(preview_size(3840, 2160), (1280, 720));
        assert_eq!(preview_size(2560, 1440), (1280, 720));
        assert_eq!(preview_size(1920, 1080), (1280, 720));
        assert_eq!(preview_size(1920, 1440), (960, 720));
        assert_eq!(preview_size(1440, 1080), (960, 720));
        assert_eq!(preview_size(1280, 720), (1280, 720));
        assert_eq!(preview_size(640, 480), (640, 480));
        assert_eq!(preview_size(640, 360), (640, 360));
    }

    #[test]
    fn scaled_nv12_keeps_the_sampled_pixel() {
        let mut bytes = vec![1, 2, 3, 4, 5, 6, 7, 8];
        bytes.extend_from_slice(&[10, 20, 30, 40]);
        let scaled = scale_nv12(&bytes, 4, 2, 2, 2).unwrap();
        assert_eq!(scaled, vec![1, 3, 5, 7, 10, 20]);
    }

    #[test]
    fn preview_encode_shrinks_a_tall_nv12_frame() {
        let width = 8_u32;
        let height = 722_u32;
        let y_size = (width * height) as usize;
        let bytes = vec![16_u8; y_size + y_size / 2];
        let (jpeg, preview_width, preview_height) = encode_preview(
            &bytes,
            Mode {
                index: 0,
                width,
                height,
                fps_num: 30,
                fps_den: 1,
                format: PixelFormat::Nv12,
            },
        )
        .unwrap();
        assert_eq!((preview_width, preview_height), (6, 720));
        assert!(jpeg.starts_with(&[0xFF, 0xD8]), "{jpeg:?}");
        assert!(jpeg.len() < bytes.len());
    }

    #[test]
    fn only_listed_vga_stays_vga() {
        let modes = vec![mode(0, 640, 480, 30, 1, PixelFormat::Mjpeg)];
        let chosen = choose_mode(&modes, None).unwrap();
        assert_eq!((chosen.width, chosen.height), (640, 480));
    }

    #[test]
    fn measured_fps_uses_the_frames_inside_one_second() {
        let start = Instant::now();
        let mut meter = FpsMeter::new();
        for step in 0..10 {
            meter.note(start + Duration::from_millis(step * 100));
        }
        let fps = meter.measured(start + Duration::from_millis(900)).unwrap();
        assert!((fps - 10.0).abs() < 0.2, "{fps}");
        assert!(
            meter
                .measured(start + Duration::from_millis(2500))
                .is_none()
        );
    }

    #[test]
    fn yuy2_pair_becomes_two_pixels() {
        let rgb = yuy2_to_rgb(&[128, 128, 128, 128], 2, 1).unwrap();
        assert_eq!(rgb.len(), 6);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "opens a real camera"]
    fn live_preview_is_smaller_than_the_capture() {
        let listed = list_cameras().expect("list cameras");
        if listed.is_empty() {
            eprintln!("no cameras enumerated");
            return;
        }
        for index in listed {
            let _ = capture_snapshot(Some(index)).expect("open");
            thread::sleep(Duration::from_millis(400));
            let started = Instant::now();
            let body = capture_preview(Some(index)).expect("preview");
            eprintln!(
                "camera {index} preview encode {} ms capture {}x{} preview {}x{} fps={} jpeg_chars={}",
                started.elapsed().as_millis(),
                body["width"],
                body["height"],
                body["previewWidth"],
                body["previewHeight"],
                body["fps"],
                body["image"].as_str().map(|image| image.len()).unwrap_or(0)
            );
            let width = body["width"].as_u64().unwrap_or(0);
            let height = body["height"].as_u64().unwrap_or(0);
            let preview_width = body["previewWidth"].as_u64().unwrap_or(0);
            let preview_height = body["previewHeight"].as_u64().unwrap_or(0);
            assert!(preview_width > 0 && preview_width <= 1280, "{body}");
            assert!(preview_height > 0 && preview_height <= 720, "{body}");
            assert!(preview_width <= width && preview_height <= height, "{body}");
            let fps = body["fps"].as_f64().unwrap_or(0.0);
            assert!(fps > 20.0, "camera {index} capture fps {fps}: {body}");
            for step in 0..3 {
                thread::sleep(Duration::from_millis(66));
                let body = capture_preview(Some(index)).expect("preview");
                let fps = body["fps"].as_f64().unwrap_or(0.0);
                eprintln!(
                    "camera {index} preview step {step} fps={fps} {}x{}",
                    body["previewWidth"], body["previewHeight"]
                );
                assert!(fps > 20.0, "camera {index} capture fps {fps}");
                assert!(body["previewWidth"].as_u64().unwrap_or(9999) <= 1280);
                assert!(body["previewHeight"].as_u64().unwrap_or(9999) <= 720);
            }
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "opens a real camera"]
    fn live_sustained_rate() {
        let listed = list_cameras().expect("list cameras");
        if listed.is_empty() {
            eprintln!("no cameras enumerated");
            return;
        }
        for index in listed {
            let modes = list_camera_modes(Some(index)).expect("modes");
            eprintln!("camera {index} modes {}", modes["default"]);
            let _ = capture_snapshot(Some(index)).expect("open");
            for step in 0..4 {
                thread::sleep(Duration::from_millis(500));
                let started = Instant::now();
                let body = capture_snapshot(Some(index)).expect("frame");
                eprintln!(
                    "camera {index} step {step}: encode {} ms, {}x{} {} fps={}",
                    started.elapsed().as_millis(),
                    body["width"],
                    body["height"],
                    body["format"],
                    body["fps"]
                );
                let fps = body["fps"].as_f64().unwrap_or(0.0);
                assert!(
                    fps > 20.0,
                    "camera {index} measured {fps} fps while streaming: {body}"
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "opens a real camera"]
    fn live_cameras_open_one_at_a_time() {
        let listed = list_cameras().expect("list cameras");
        if listed.is_empty() {
            eprintln!("no cameras enumerated");
            return;
        }
        session::startup().expect("media foundation");
        for index in &listed {
            match session::explain(*index) {
                Ok(text) => eprintln!("{text}"),
                Err(err) => eprintln!("explain {index}: {err}"),
            }
        }
        // Open the later index first so a front camera is not only tried
        // after the other device has been streaming.
        let mut order = listed.clone();
        order.reverse();
        let mut opened = Vec::new();
        for index in &order {
            match capture_snapshot(Some(*index)) {
                Ok(body) if body["status"] == "pass" => {
                    thread::sleep(Duration::from_millis(800));
                    let again = capture_snapshot(Some(*index)).expect("second frame");
                    eprintln!(
                        "camera {index}: {}x{} {} fps={}",
                        again["width"], again["height"], again["format"], again["fps"]
                    );
                    assert_eq!(again["status"], "pass", "{again}");
                    let fps = again["fps"].as_f64().unwrap_or(0.0);
                    assert!(fps > 2.0, "camera {index} measured {fps} fps: {again}");
                    opened.push(*index);
                }
                Ok(body) => eprintln!("camera {index} failed: {body}"),
                Err(err) => eprintln!("camera {index} error: {err}"),
            }
        }
        assert_eq!(
            opened.len(),
            listed.len(),
            "every camera should stream: listed {listed:?}, opened {opened:?}"
        );
        if opened.len() >= 2 {
            let back = capture_snapshot(Some(opened[0])).expect("reopen");
            eprintln!(
                "reopen camera {}: {}x{} {} fps={}",
                opened[0], back["width"], back["height"], back["format"], back["fps"]
            );
            assert_eq!(back["status"], "pass", "reopen after switch: {back}");
            thread::sleep(Duration::from_millis(800));
            let again = capture_snapshot(Some(opened[0])).expect("reopen fps");
            let fps = again["fps"].as_f64().unwrap_or(0.0);
            assert!(
                fps > 2.0,
                "reopened camera {} measured {fps} fps: {again}",
                opened[0]
            );
        }
    }
}
