//! One Windows camera, held open on a single thread.
//!
//! Media Foundation delivers frames on the thread that owns the source
//! reader. List and capture therefore share one worker, and that worker
//! keeps at most one session. Switching cameras shuts the media source
//! down before the next device is activated, so the hardware encoder is
//! free for the new open. Frames arrive through an async source-reader
//! callback. The callback keeps the sample's contiguous media buffer and
//! hands that same buffer to a preview handle and a still handle. It does
//! not copy the pixels. `capture_preview` takes the preview handle, so the
//! next preview waits for a new sample, and it is the only path that
//! encodes a JPEG. `capture_snapshot` clones the still handle, converts
//! those pixels to an RGB `RawFrame`, and stores that frame. It does not
//! empty the preview slot and it does not encode a JPEG. Conversion runs
//! on the command thread, not on the callback or the camera thread.
//! `capture_snapshot` keeps the selected mode. `capture_preview` scales
//! that frame to fit in 1280×720. Those two commands stay the public
//! stream. `InspectionCamera` sits under them: `UvcBackend` applies the
//! saved UVC exposure, focus, white balance, and gain on the open device
//! after the mode is selected and before the first sample. A rejected
//! control is logged and does not stop the stream. Later changes write
//! that same handle and do not pause frame delivery. `PylonBackend`
//! returns "Pylon backend not linked yet" and does not open this reader.
//! This crate does not link the Pylon SDK. Brightness and contrast are
//! stored by Settings and are not applied here.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
use image::RgbImage;
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

/// Exposure for [`InspectionCamera::set_exposure`].
/// `absolute` is the UVC exposure-time absolute control, in units of 100 µs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExposureConfig {
    pub auto: bool,
    pub absolute: i32,
}

const PYLON_NOT_LINKED: &str = "Pylon backend not linked yet";

/// One capture device. The Media Foundation reader still owns the live
/// preview and the snapshot. A backend uses this trait for a still frame
/// and for exposure, gain, focus, and white balance, so a Pylon device can
/// replace the UVC body without a new operator path. Focus and white
/// balance default to no-ops so a backend that does not own those controls
/// can leave them unimplemented.
pub trait InspectionCamera: Send + Sync {
    fn capture_frame(&mut self) -> Result<image::RgbImage, String>;
    fn set_exposure(&mut self, config: &ExposureConfig) -> Result<(), String>;
    fn set_gain(&mut self, gain: f32) -> Result<(), String>;
    fn set_focus(&mut self, _auto: bool, _focus_absolute: Option<i32>) -> Result<(), String> {
        Ok(())
    }
    fn set_white_balance(&mut self, _auto: bool, _kelvin: Option<i32>) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraDriverKind {
    Uvc,
    Pylon,
}

/// Driver block for the current UVC camera. Focus is the 0–100 slider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvcSettings {
    pub auto_exposure: bool,
    pub exposure_absolute: i32,
    pub auto_focus: bool,
    pub focus_absolute: u32,
    pub auto_white_balance: bool,
    pub white_balance_kelvin: i32,
    pub gain: f32,
}

impl Default for UvcSettings {
    fn default() -> Self {
        Self {
            auto_exposure: false,
            exposure_absolute: 156,
            auto_focus: false,
            focus_absolute: 25,
            auto_white_balance: false,
            white_balance_kelvin: 4500,
            gain: 0.0,
        }
    }
}

/// Loaded for `camera.type = "pylon"` and not applied yet.
#[derive(Debug, Clone, PartialEq)]
pub struct PylonSettings {
    pub exposure_time_us: f32,
    pub gain_db: f32,
    pub pixel_format: String,
}

impl Default for PylonSettings {
    fn default() -> Self {
        Self {
            exposure_time_us: 15000.0,
            gain_db: 0.0,
            pixel_format: "RGB8".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedCameraDriver {
    pub kind: CameraDriverKind,
    pub device_index: u32,
    pub uvc: UvcSettings,
    pub pylon: PylonSettings,
}

/// Active UVC camera. Control changes go to the open Media Foundation device.
/// `capture_frame` reads the same still the snapshot stores. It is not the
/// paced preview.
pub struct UvcBackend {
    index: u32,
    settings: UvcSettings,
    applied: Option<i32>,
}

impl UvcBackend {
    pub fn new(index: u32, settings: UvcSettings) -> Self {
        Self {
            index,
            settings,
            applied: None,
        }
    }

    pub fn applied(&self) -> Option<i32> {
        self.applied
    }
}

impl InspectionCamera for UvcBackend {
    fn capture_frame(&mut self) -> Result<image::RgbImage, String> {
        let frame = fetch_frame(self.index, true)?;
        let (rgb, width, height) = use_frame(&frame, rgb_frame)?;
        RgbImage::from_raw(width, height, rgb)
            .ok_or_else(|| "Could not build the camera image.".to_string())
    }

    fn set_exposure(&mut self, config: &ExposureConfig) -> Result<(), String> {
        self.settings.auto_exposure = config.auto;
        self.settings.exposure_absolute = config.absolute;
        let applied = apply_exposure(self.index, config.auto, config.absolute)?;
        self.applied = Some(applied);
        Ok(())
    }

    fn set_gain(&mut self, gain: f32) -> Result<(), String> {
        self.settings.gain = gain;
        let applied = apply_gain(self.index, gain)?;
        self.applied = Some(applied);
        Ok(())
    }

    fn set_focus(&mut self, auto: bool, focus_absolute: Option<i32>) -> Result<(), String> {
        self.settings.auto_focus = auto;
        if let Some(value) = focus_absolute {
            self.settings.focus_absolute = value.clamp(0, 100) as u32;
        }
        let request = if auto {
            FocusRequest::Auto
        } else {
            FocusRequest::Manual(self.settings.focus_absolute)
        };
        submit_focus(self.index, request)
    }

    fn set_white_balance(&mut self, auto: bool, kelvin: Option<i32>) -> Result<(), String> {
        self.settings.auto_white_balance = auto;
        if let Some(value) = kelvin {
            self.settings.white_balance_kelvin = value;
        }
        let applied = apply_white_balance(self.index, auto, self.settings.white_balance_kelvin)?;
        self.applied = Some(applied);
        Ok(())
    }
}

/// Basler cameras will implement [`InspectionCamera`] here.
/// No Pylon dependency is linked yet. `pylonSettings` is loaded and unused.
pub struct PylonBackend {
    #[allow(dead_code)]
    settings: PylonSettings,
}

impl PylonBackend {
    pub fn new(settings: PylonSettings) -> Self {
        Self { settings }
    }
}

impl InspectionCamera for PylonBackend {
    fn capture_frame(&mut self) -> Result<image::RgbImage, String> {
        Err(PYLON_NOT_LINKED.to_string())
    }

    fn set_exposure(&mut self, _config: &ExposureConfig) -> Result<(), String> {
        Err(PYLON_NOT_LINKED.to_string())
    }

    fn set_gain(&mut self, _gain: f32) -> Result<(), String> {
        Err(PYLON_NOT_LINKED.to_string())
    }
}

/// The backend named by `camera.type`. Pylon is constructed so the trait
/// stays the only capture surface a later SDK has to fill in. Selecting it
/// logs once that the backend is not linked. Capture then returns
/// [`PYLON_NOT_LINKED`] and does not open the Media Foundation stream.
pub fn open_backend(driver: &LoadedCameraDriver) -> Box<dyn InspectionCamera> {
    match driver.kind {
        CameraDriverKind::Uvc => Box::new(UvcBackend::new(driver.device_index, driver.uvc)),
        CameraDriverKind::Pylon => {
            log_pylon_unlinked();
            Box::new(PylonBackend::new(driver.pylon.clone()))
        }
    }
}

fn log_pylon_unlinked() {
    static LOGGED: OnceLock<()> = OnceLock::new();
    LOGGED.get_or_init(|| {
        eprintln!("Pylon backend is selected but not linked.");
    });
}

struct Frame {
    pixels: FrameHandle,
    width: u32,
    height: u32,
    format: PixelFormat,
    fps: Option<f32>,
}

/// One sample. Owned bytes are the test path. On Windows the camera path
/// holds the Media Foundation buffer and locks it only while encoding.
enum FrameKind {
    #[cfg(test)]
    Owned(Vec<u8>),
    #[cfg(windows)]
    Media(session::MediaFrame),
}

#[derive(Clone)]
struct FrameHandle {
    kind: Arc<FrameKind>,
}

impl FrameHandle {
    #[cfg(test)]
    fn owned(bytes: Vec<u8>) -> Self {
        Self {
            kind: Arc::new(FrameKind::Owned(bytes)),
        }
    }

    #[cfg(windows)]
    fn media(frame: session::MediaFrame) -> Self {
        Self {
            kind: Arc::new(FrameKind::Media(frame)),
        }
    }

    #[cfg(test)]
    fn same_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.kind, &other.kind)
    }

    /// `use_pixels` must finish with the slice. The media buffer is unlocked
    /// when this returns, and a second caller waits until then.
    fn with_pixels<R>(&self, use_pixels: impl FnOnce(&[u8]) -> R) -> Result<R, String> {
        match self.kind.as_ref() {
            #[cfg(test)]
            FrameKind::Owned(bytes) => Ok(use_pixels(bytes)),
            #[cfg(windows)]
            FrameKind::Media(frame) => frame.with_pixels(use_pixels),
        }
    }
}

/// Latest sample, as two handles on one buffer. Taking the preview handle
/// does not clear the still, and cloning the still does not clear the preview.
struct FrameSlots {
    preview: Option<FrameHandle>,
    still: Option<FrameHandle>,
}

impl FrameSlots {
    fn publish(&mut self, frame: FrameHandle) {
        self.still = Some(frame.clone());
        self.preview = Some(frame);
    }

    fn take_preview(&mut self) -> Option<FrameHandle> {
        self.preview.take()
    }

    fn copy_still(&self) -> Option<FrameHandle> {
        self.still.clone()
    }

    fn clear(&mut self) {
        self.preview = None;
        self.still = None;
    }
}

enum FocusRequest {
    Manual(u32),
    Auto,
}

enum Job {
    List(Sender<Result<Vec<u32>, String>>),
    Modes {
        index: u32,
        reply: Sender<Result<Vec<Mode>, String>>,
    },
    Grab {
        index: u32,
        still: bool,
        reply: Sender<Result<Frame, String>>,
    },
    Focus {
        index: u32,
        request: FocusRequest,
        reply: Sender<Result<(), String>>,
    },
    Exposure {
        index: u32,
        auto: bool,
        absolute: i32,
        reply: Sender<Result<i32, String>>,
    },
    Gain {
        index: u32,
        gain: f32,
        reply: Sender<Result<i32, String>>,
    },
    WhiteBalance {
        index: u32,
        auto: bool,
        kelvin: i32,
        reply: Sender<Result<i32, String>>,
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
                Job::Focus { reply, .. } => {
                    let _ = reply.send(Err(message));
                }
                Job::Exposure { reply, .. } => {
                    let _ = reply.send(Err(message));
                }
                Job::Gain { reply, .. } => {
                    let _ = reply.send(Err(message));
                }
                Job::WhiteBalance { reply, .. } => {
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
            Job::Grab {
                index,
                still,
                reply,
            } => {
                let _ = reply.send(grab(&mut open, index, still));
            }
            Job::Focus {
                index,
                request,
                reply,
            } => {
                let _ = reply.send(apply_focus(&mut open, index, request));
            }
            Job::Exposure {
                index,
                auto,
                absolute,
                reply,
            } => {
                let _ = reply.send(apply_exposure_job(&mut open, index, auto, absolute));
            }
            Job::Gain {
                index,
                gain,
                reply,
            } => {
                let _ = reply.send(apply_gain_job(&mut open, index, gain));
            }
            Job::WhiteBalance {
                index,
                auto,
                kelvin,
                reply,
            } => {
                let _ = reply.send(apply_white_balance_job(&mut open, index, auto, kelvin));
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
pub fn capture_snapshot(
    index: Option<u32>,
    state: tauri::State<crate::frame::FrameState>,
) -> Result<Value, String> {
    store_still(index, &state.latest)
}

/// Copy the still handle, convert it to RGB, and store that frame.
/// The preview slot is left alone. This path does not encode a JPEG.
fn store_still(
    index: Option<u32>,
    latest: &Mutex<Option<crate::frame::RawFrame>>,
) -> Result<Value, String> {
    let frame = match command_frame(index, true) {
        Ok(frame) => frame,
        Err(response) => return response,
    };
    let raw = match use_frame(&frame, rgb_frame) {
        Ok((data, width, height)) => crate::frame::RawFrame {
            width,
            height,
            channels: 3,
            data,
            timestamp_ms: timestamp_ms(),
        },
        Err(message) => return Ok(fail(&message)),
    };
    let body = json!({
        "status": "pass",
        "width": raw.width,
        "height": raw.height,
        "timestamp_ms": raw.timestamp_ms,
    });
    *lock(latest) = Some(raw);
    Ok(body)
}

fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
fn capture_still(index: Option<u32>) -> Result<Value, String> {
    store_still(index, test_latest())
}

#[cfg(test)]
fn test_latest() -> &'static Mutex<Option<crate::frame::RawFrame>> {
    static LATEST: OnceLock<Mutex<Option<crate::frame::RawFrame>>> = OnceLock::new();
    LATEST.get_or_init(|| Mutex::new(None))
}

/// Latest frame, scaled to fit in 1280×720. The open mode is unchanged,
/// so a later `capture_snapshot` is still the full selected resolution.
#[tauri::command]
pub fn capture_preview(index: Option<u32>) -> Result<Value, String> {
    let frame = match command_frame(index, false) {
        Ok(frame) => frame,
        Err(response) => return response,
    };
    match use_frame(&frame, encode_preview) {
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

/// Set manual focus on the open camera. `focus` is 0–100. The value is
/// stored only after the camera accepts it. Null stays autofocus.
#[tauri::command]
pub fn set_camera_focus(index: Option<u32>, focus: u32) -> Result<Value, String> {
    let index = index.unwrap_or_else(saved_index);
    if index > 32 {
        return Ok(fail("Camera index must be from 0 to 32."));
    }
    if focus > 100 {
        return Ok(fail("Focus must be from 0 to 100."));
    }
    if let Err(message) = submit_focus(index, FocusRequest::Manual(focus)) {
        return Ok(fail(&message));
    }
    if let Err(message) = remember_focus(index, Some(focus)) {
        return Ok(fail(&message));
    }
    Ok(json!({
        "status": "pass",
        "index": index,
        "focus": focus,
    }))
}

/// Return the open camera to autofocus and clear the saved manual value.
#[tauri::command]
pub fn set_camera_autofocus(index: Option<u32>) -> Result<Value, String> {
    let index = index.unwrap_or_else(saved_index);
    if index > 32 {
        return Ok(fail("Camera index must be from 0 to 32."));
    }
    if let Err(message) = submit_focus(index, FocusRequest::Auto) {
        return Ok(fail(&message));
    }
    if let Err(message) = remember_focus(index, None) {
        return Ok(fail(&message));
    }
    Ok(json!({
        "status": "pass",
        "index": index,
        "focus": null,
    }))
}

/// Manual exposure on the open camera. The saved value stays the UVC
/// absolute number. `applied` is the value the device accepted.
#[tauri::command]
pub fn set_camera_exposure(index: Option<u32>, exposure_absolute: i32) -> Result<Value, String> {
    let index = index.unwrap_or_else(saved_index);
    if index > 32 {
        return Ok(fail("Camera index must be from 0 to 32."));
    }
    if !(-13..=20_000).contains(&exposure_absolute) {
        return Ok(fail("Exposure must be from -13 to 20000."));
    }
    let mut backend = UvcBackend::new(index, loaded_driver().uvc);
    if let Err(message) = backend.set_exposure(&ExposureConfig {
        auto: false,
        absolute: exposure_absolute,
    }) {
        return Ok(fail(&message));
    }
    if let Err(message) = remember_uvc_exposure(exposure_absolute) {
        return Ok(fail(&message));
    }
    Ok(json!({
        "status": "pass",
        "index": index,
        "exposureAbsolute": exposure_absolute,
        "autoExposure": false,
        "applied": backend.applied(),
    }))
}

/// Manual gain on the open camera. The number is the UVC gain control.
#[tauri::command]
pub fn set_camera_gain(index: Option<u32>, gain: f32) -> Result<Value, String> {
    let index = index.unwrap_or_else(saved_index);
    if index > 32 {
        return Ok(fail("Camera index must be from 0 to 32."));
    }
    if !gain.is_finite() || !(0.0..=255.0).contains(&gain) {
        return Ok(fail("Gain must be from 0 to 255."));
    }
    let mut backend = UvcBackend::new(index, loaded_driver().uvc);
    if let Err(message) = backend.set_gain(gain) {
        return Ok(fail(&message));
    }
    if let Err(message) = remember_uvc_gain(gain) {
        return Ok(fail(&message));
    }
    Ok(json!({
        "status": "pass",
        "index": index,
        "gain": json_gain(gain),
        "applied": backend.applied(),
    }))
}

fn command_frame(index: Option<u32>, still: bool) -> Result<Frame, Result<Value, String>> {
    let index = index.unwrap_or_else(saved_index);
    if index > 32 {
        return Err(Ok(fail("Camera index must be from 0 to 32.")));
    }
    match fetch_frame(index, still) {
        Ok(frame) => Ok(frame),
        Err(message) if message == "Camera thread stopped." => Err(Err(message)),
        Err(message) => Err(Ok(fail(&message))),
    }
}

fn fetch_frame(index: u32, still: bool) -> Result<Frame, String> {
    let (tx, rx) = mpsc::channel();
    jobs()
        .send(Job::Grab {
            index,
            still,
            reply: tx,
        })
        .map_err(|_| "Camera thread stopped.".to_string())?;
    match rx.recv() {
        Ok(frame) => frame,
        Err(_) => Err("Camera thread stopped.".to_string()),
    }
}

fn use_frame<T>(
    frame: &Frame,
    encode: impl FnOnce(&[u8], Mode) -> Result<T, String>,
) -> Result<T, String> {
    let mode = frame_mode(frame);
    frame.pixels.with_pixels(|bytes| encode(bytes, mode))?
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
    refuse_unlinked_pylon(open)?;
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

fn grab(open: &mut Option<session::Device>, index: u32, still: bool) -> Result<Frame, String> {
    refuse_unlinked_pylon(open)?;
    let opened = ensure_device(open, index)?;
    if opened {
        apply_open_controls(open);
    }
    match take_frame(open.as_ref().expect("camera was opened"), still) {
        Ok(frame) => Ok(frame),
        Err(message) if is_resource(&message) => {
            if let Some(mut previous) = open.take() {
                previous.shutdown();
            }
            *open = Some(session::open_device(index, saved_resolution())?);
            apply_open_controls(open);
            take_frame(open.as_ref().expect("camera was reopened"), still)
        }
        Err(message) => {
            if let Some(mut previous) = open.take() {
                previous.shutdown();
            }
            Err(message)
        }
    }
}

/// Open this camera when the current session is a different device or mode.
/// Returns true when a new session was opened. A focus change uses this
/// without applying the previously saved focus first.
fn ensure_device(open: &mut Option<session::Device>, index: u32) -> Result<bool, String> {
    let wanted = saved_resolution();
    let keep = open
        .as_ref()
        .is_some_and(|device| device.index() == index && device.serves(wanted));
    if keep {
        return Ok(false);
    }
    if let Some(mut previous) = open.take() {
        previous.shutdown();
    }
    *open = Some(session::open_device(index, wanted)?);
    Ok(true)
}

/// UVC settings on an open reader. Called again after `open_device` returns
/// so a driver that only accepts controls once streaming has started still
/// receives them. The first write happens inside `try_open`, before
/// `ReadSample`. A rejected control is logged and does not stop the stream.
/// Pylon never opens this reader.
fn apply_open_controls(open: &mut Option<session::Device>) {
    let Some(device) = open.as_ref() else {
        return;
    };
    apply_device_controls(device, "open");
}

fn apply_device_controls(device: &session::Device, phase: &str) {
    let driver = loaded_driver();
    if driver.kind != CameraDriverKind::Uvc {
        return;
    }
    let Some(settings) = uvc_controls_for_open(&driver, stored_focus_for(device.index())) else {
        return;
    };
    if let Err(message) = device.apply_uvc(&settings, phase) {
        eprintln!("UVC warning ({phase}): {message}");
    }
}

/// `camera.type` of `pylon` constructs the stub and returns its error.
/// The Media Foundation device is shut down and is not opened again.
fn refuse_unlinked_pylon(open: &mut Option<session::Device>) -> Result<(), String> {
    let driver = loaded_driver();
    if driver.kind != CameraDriverKind::Pylon {
        return Ok(());
    }
    if let Some(mut previous) = open.take() {
        previous.shutdown();
    }
    let mut backend = open_backend(&driver);
    backend.capture_frame().map(|_| ())
}

fn submit_focus(index: u32, request: FocusRequest) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    jobs()
        .send(Job::Focus {
            index,
            request,
            reply: tx,
        })
        .map_err(|_| "Camera thread stopped.".to_string())?;
    rx.recv()
        .map_err(|_| "Camera thread stopped.".to_string())?
}

fn apply_exposure(index: u32, auto: bool, absolute: i32) -> Result<i32, String> {
    let (tx, rx) = mpsc::channel();
    jobs()
        .send(Job::Exposure {
            index,
            auto,
            absolute,
            reply: tx,
        })
        .map_err(|_| "Camera thread stopped.".to_string())?;
    rx.recv()
        .map_err(|_| "Camera thread stopped.".to_string())?
}

fn apply_gain(index: u32, gain: f32) -> Result<i32, String> {
    let (tx, rx) = mpsc::channel();
    jobs()
        .send(Job::Gain {
            index,
            gain,
            reply: tx,
        })
        .map_err(|_| "Camera thread stopped.".to_string())?;
    rx.recv()
        .map_err(|_| "Camera thread stopped.".to_string())?
}

fn apply_white_balance(index: u32, auto: bool, kelvin: i32) -> Result<i32, String> {
    let (tx, rx) = mpsc::channel();
    jobs()
        .send(Job::WhiteBalance {
            index,
            auto,
            kelvin,
            reply: tx,
        })
        .map_err(|_| "Camera thread stopped.".to_string())?;
    rx.recv()
        .map_err(|_| "Camera thread stopped.".to_string())?
}

fn apply_exposure_job(
    open: &mut Option<session::Device>,
    index: u32,
    auto: bool,
    absolute: i32,
) -> Result<i32, String> {
    refuse_unlinked_pylon(open)?;
    let opened = ensure_device(open, index)?;
    if opened {
        apply_open_controls(open);
    }
    let device = open.as_ref().expect("camera was opened");
    let applied = device.set_exposure_control(auto, absolute)?;
    if !auto {
        log_if_clamped("exposure", i64::from(absolute), applied);
    }
    Ok(applied)
}

fn apply_gain_job(open: &mut Option<session::Device>, index: u32, gain: f32) -> Result<i32, String> {
    refuse_unlinked_pylon(open)?;
    let opened = ensure_device(open, index)?;
    if opened {
        apply_open_controls(open);
    }
    let device = open.as_ref().expect("camera was opened");
    let applied = device.set_gain_control(gain)?;
    if gain.is_finite() {
        log_if_clamped("gain", i64::from(gain.round() as i32), applied);
    }
    Ok(applied)
}

fn apply_white_balance_job(
    open: &mut Option<session::Device>,
    index: u32,
    auto: bool,
    kelvin: i32,
) -> Result<i32, String> {
    refuse_unlinked_pylon(open)?;
    let opened = ensure_device(open, index)?;
    if opened {
        apply_open_controls(open);
    }
    let device = open.as_ref().expect("camera was opened");
    let applied = device.set_white_balance_control(auto, kelvin)?;
    if !auto {
        log_if_clamped("white balance", i64::from(kelvin), applied);
    }
    Ok(applied)
}

fn apply_focus(
    open: &mut Option<session::Device>,
    index: u32,
    request: FocusRequest,
) -> Result<(), String> {
    refuse_unlinked_pylon(open)?;
    let opened = ensure_device(open, index)?;
    if opened {
        apply_open_controls(open);
    }
    let device = open.as_ref().expect("camera was opened");
    match request {
        FocusRequest::Manual(focus) => {
            let applied = device.set_focus(focus)?;
            log_if_clamped("focus", i64::from(focus), applied);
            Ok(())
        }
        FocusRequest::Auto => device.set_autofocus(),
    }
}

fn take_frame(device: &session::Device, still: bool) -> Result<Frame, String> {
    let pixels = if still {
        device.copy_still(FIRST_FRAME_WAIT)?
    } else {
        device.wait_frame(FIRST_FRAME_WAIT)?
    };
    let mode = device.mode();
    Ok(Frame {
        pixels,
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
        .map(camera_device_index)
        .unwrap_or(0)
}

/// `index` is the camera Settings selects. `deviceIndex` is the driver field
/// and is used when `index` is absent. The two stay equal when Settings saves.
pub fn camera_device_index(camera: &Value) -> u32 {
    json_index(camera.get("index"))
        .or_else(|| json_index(camera.get("deviceIndex")))
        .unwrap_or(0)
}

fn json_index(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(|value| value.as_u64())
        .and_then(|index| u32::try_from(index).ok())
        .filter(|index| *index <= 32)
}

fn loaded_driver() -> LoadedCameraDriver {
    let camera = local_config::project_root()
        .ok()
        .and_then(|root| local_config::load_document(&root).ok())
        .and_then(|config| config.get("camera").cloned())
        .unwrap_or_else(|| json!({}));
    load_camera_driver(&camera)
}

/// Read `camera.type` and the matching settings object.
/// A missing type is UVC. Unknown text is UVC so an old file still opens.
/// The other block is parsed and left unused by the open path.
pub fn load_camera_driver(camera: &Value) -> LoadedCameraDriver {
    let kind = match camera.get("type").and_then(Value::as_str) {
        Some("pylon") => CameraDriverKind::Pylon,
        _ => CameraDriverKind::Uvc,
    };
    LoadedCameraDriver {
        kind,
        device_index: camera_device_index(camera),
        uvc: read_uvc_settings(camera.get("uvcSettings")),
        pylon: read_pylon_settings(camera.get("pylonSettings")),
    }
}

/// Settings to push when a UVC reader opens. `None` when this camera is not UVC.
pub fn uvc_controls_for_open(
    driver: &LoadedCameraDriver,
    stored_focus: Option<u32>,
) -> Option<UvcSettings> {
    if driver.kind != CameraDriverKind::Uvc {
        return None;
    }
    let mut settings = driver.uvc;
    if let Some(focus) = stored_focus {
        settings.auto_focus = false;
        settings.focus_absolute = focus;
    }
    Some(settings)
}

fn read_uvc_settings(value: Option<&Value>) -> UvcSettings {
    let Some(value) = value.filter(|value| value.is_object()) else {
        return UvcSettings::default();
    };
    let defaults = UvcSettings::default();
    UvcSettings {
        auto_exposure: bool_field(value, "autoExposure", defaults.auto_exposure),
        exposure_absolute: i32_field(
            value,
            "exposureAbsolute",
            defaults.exposure_absolute,
            -13,
            20_000,
        ),
        auto_focus: bool_field(value, "autoFocus", defaults.auto_focus),
        focus_absolute: u32_field(value, "focusAbsolute", defaults.focus_absolute, 0, 100),
        auto_white_balance: bool_field(value, "autoWhiteBalance", defaults.auto_white_balance),
        white_balance_kelvin: i32_field(
            value,
            "whiteBalanceKelvin",
            defaults.white_balance_kelvin,
            1_000,
            15_000,
        ),
        gain: f32_field(value, "gain", defaults.gain, 0.0, 255.0),
    }
}

fn read_pylon_settings(value: Option<&Value>) -> PylonSettings {
    let Some(value) = value.filter(|value| value.is_object()) else {
        return PylonSettings::default();
    };
    let defaults = PylonSettings::default();
    let pixel_format = value
        .get("pixelFormat")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.len() <= 32)
        .unwrap_or(defaults.pixel_format.as_str())
        .to_string();
    PylonSettings {
        exposure_time_us: f32_field(
            value,
            "exposureTimeUs",
            defaults.exposure_time_us,
            1.0,
            1_000_000.0,
        ),
        gain_db: f32_field(value, "gainDb", defaults.gain_db, 0.0, 48.0),
        pixel_format,
    }
}

fn bool_field(value: &Value, name: &str, default: bool) -> bool {
    value.get(name).and_then(Value::as_bool).unwrap_or(default)
}

fn i32_field(value: &Value, name: &str, default: i32, min: i32, max: i32) -> i32 {
    let Some(number) = value.get(name).and_then(Value::as_i64) else {
        return default;
    };
    i32::try_from(number)
        .ok()
        .filter(|number| (min..=max).contains(number))
        .unwrap_or(default)
}

fn u32_field(value: &Value, name: &str, default: u32, min: u32, max: u32) -> u32 {
    let Some(number) = value.get(name).and_then(Value::as_u64) else {
        return default;
    };
    u32::try_from(number)
        .ok()
        .filter(|number| (min..=max).contains(number))
        .unwrap_or(default)
}

fn f32_field(value: &Value, name: &str, default: f32, min: f32, max: f32) -> f32 {
    let Some(number) = value.get(name).and_then(Value::as_f64) else {
        return default;
    };
    if !number.is_finite() {
        return default;
    }
    let number = number as f32;
    if (min..=max).contains(&number) {
        number
    } else {
        default
    }
}

fn default_uvc_value() -> Value {
    let settings = UvcSettings::default();
    json!({
        "autoExposure": settings.auto_exposure,
        "exposureAbsolute": settings.exposure_absolute,
        "autoFocus": settings.auto_focus,
        "focusAbsolute": settings.focus_absolute,
        "autoWhiteBalance": settings.auto_white_balance,
        "whiteBalanceKelvin": settings.white_balance_kelvin,
        "gain": json_gain(settings.gain),
    })
}

fn default_pylon_value() -> Value {
    let settings = PylonSettings::default();
    json!({
        "exposureTimeUs": settings.exposure_time_us,
        "gainDb": settings.gain_db,
        "pixelFormat": settings.pixel_format,
    })
}

fn json_gain(gain: f32) -> Value {
    if gain.is_finite() && gain.fract() == 0.0 {
        json!(gain as i32)
    } else {
        json!(gain)
    }
}

/// One control in the open log. A driver value that differs from the request
/// is the value that was written, including the exposure log2 conversion.
fn control_note(label: &str, requested: i64, applied: i32) -> String {
    if requested == i64::from(applied) {
        format!("{label} {applied}")
    } else {
        format!("{label} {requested} applied as {applied}")
    }
}

fn log_if_clamped(label: &str, requested: i64, applied: i32) {
    if requested != i64::from(applied) {
        eprintln!("UVC controls: {}", control_note(label, requested, applied));
    }
}

/// Align `value` to `step` inside `min..=max`. Steps use the distance from `min`.
pub(crate) fn snap_control_value(value: i32, min: i32, max: i32, step: i32) -> i32 {
    if max < min {
        return min;
    }
    let step = if step <= 0 { 1 } else { step };
    let value = value.clamp(min, max);
    let offset = value.saturating_sub(min);
    let aligned = offset - offset.rem_euclid(step);
    min.saturating_add(aligned)
}

/// UVC exposure absolute is 100 µs per step. DirectShow often stores the same
/// control as log2(seconds). A value already inside the device range is used.
/// A positive absolute against a negative log2 range is converted, then snapped.
pub(crate) fn exposure_device_value(absolute: i32, min: i32, max: i32, step: i32) -> i32 {
    if (min..=max).contains(&absolute) {
        return snap_control_value(absolute, min, max, step);
    }
    if max <= 0 && min < 0 && absolute > 0 {
        let seconds = f64::from(absolute) * 1.0e-4;
        if seconds > 0.0 {
            let log2 = seconds.log2().round() as i32;
            return snap_control_value(log2, min, max, step);
        }
    }
    snap_control_value(absolute, min, max, step)
}

pub(crate) fn gain_device_value(gain: f32, min: i32, max: i32, step: i32) -> i32 {
    if !gain.is_finite() {
        return snap_control_value(min, min, max, step);
    }
    let value = gain.round() as i32;
    snap_control_value(value, min, max, step)
}

pub(crate) fn white_balance_device_value(
    kelvin: i32,
    min: i32,
    max: i32,
    step: i32,
) -> Result<i32, String> {
    let kelvin_range = max >= 2_000 && min < max;
    if kelvin_range || (min..=max).contains(&kelvin) {
        return Ok(snap_control_value(kelvin, min, max, step));
    }
    Err(format!(
        "White balance range {min}..{max} does not accept {kelvin} K."
    ))
}

fn remember_uvc_exposure(absolute: i32) -> Result<(), String> {
    remember_uvc(|settings| {
        settings.insert("autoExposure".to_string(), json!(false));
        settings.insert("exposureAbsolute".to_string(), json!(absolute));
    })
}

fn remember_uvc_gain(gain: f32) -> Result<(), String> {
    remember_uvc(|settings| {
        settings.insert("gain".to_string(), json_gain(gain));
    })
}

fn remember_uvc(update: impl FnOnce(&mut serde_json::Map<String, Value>)) -> Result<(), String> {
    let root = local_config::project_root()?;
    let mut config = local_config::load_document(&root)?;
    {
        let Some(document) = config.as_object_mut() else {
            return Err("Config must be a JSON object.".to_string());
        };
        if document
            .get("camera")
            .is_some_and(|camera| !camera.is_object())
        {
            return Err("Camera settings must be a JSON object.".to_string());
        }
        let camera = document
            .entry("camera")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("camera object");
        let settings = uvc_object_mut(camera);
        update(settings);
    }
    local_config::save_document(&root, &config)
}

fn uvc_object_mut(
    camera: &mut serde_json::Map<String, Value>,
) -> &mut serde_json::Map<String, Value> {
    if camera.get("type").and_then(Value::as_str).is_none() {
        camera.insert("type".to_string(), json!("uvc"));
    }
    if !camera.get("pylonSettings").is_some_and(Value::is_object) {
        camera.insert("pylonSettings".to_string(), default_pylon_value());
    }
    let settings = camera
        .entry("uvcSettings")
        .or_insert_with(default_uvc_value);
    if !settings.is_object() {
        *settings = default_uvc_value();
    }
    settings.as_object_mut().expect("uvc settings")
}

fn touch_uvc_focus(camera: &mut serde_json::Map<String, Value>, focus: Option<u32>) {
    let settings = uvc_object_mut(camera);
    match focus {
        Some(value) => {
            settings.insert("autoFocus".to_string(), json!(false));
            settings.insert("focusAbsolute".to_string(), json!(value));
        }
        None => {
            settings.insert("autoFocus".to_string(), json!(true));
        }
    }
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

/// Map a 0–100 slider onto the camera range. 0 is `min`. 100 is the highest
/// step-aligned value that is still at most `max`. A step of 0 or less is 1.
fn focus_device_value(percent: u32, min: i32, max: i32, step: i32) -> i32 {
    if max <= min {
        return min;
    }
    let percent = i64::from(percent.min(100));
    let step = i64::from(if step <= 0 { 1 } else { step });
    let min = i64::from(min);
    let max = i64::from(max);
    let span = max - min;
    let target = min + span * percent / 100;
    let offset = target - min;
    let aligned = min + offset - offset.rem_euclid(step);
    let max_offset = max - min;
    let aligned_max = min + max_offset - max_offset.rem_euclid(step);
    aligned.min(aligned_max) as i32
}

/// A number in `byIndex` wins. Null, or no record, means leave autofocus.
fn stored_focus(camera: &Value, index: u32) -> Option<u32> {
    let camera = camera.as_object()?;
    if let Some(by_index) = camera.get("byIndex").and_then(Value::as_object)
        && let Some(record) = by_index.get(&index.to_string())
    {
        return record.get("focus").and_then(focus_number);
    }
    let active = camera
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())?;
    if active == index {
        return camera.get("focus").and_then(focus_number);
    }
    None
}

fn focus_number(value: &Value) -> Option<u32> {
    let number = value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))?;
    let number = u32::try_from(number).ok()?;
    (number <= 100).then_some(number)
}

fn stored_focus_for(index: u32) -> Option<u32> {
    let root = local_config::project_root().ok()?;
    let config = local_config::load_document(&root).ok()?;
    stored_focus(config.get("camera")?, index)
}

fn remember_focus(index: u32, focus: Option<u32>) -> Result<(), String> {
    let root = local_config::project_root()?;
    let mut config = local_config::load_document(&root)?;
    write_focus(&mut config, index, focus)?;
    local_config::save_document(&root, &config)
}

/// Write only `focus`, on this camera's record and on the active camera.
/// `None` clears a saved manual value so the camera stays on autofocus.
fn write_focus(config: &mut Value, index: u32, focus: Option<u32>) -> Result<(), String> {
    let Some(root) = config.as_object_mut() else {
        return Err("Config must be a JSON object.".to_string());
    };
    if root.get("camera").is_some_and(|camera| !camera.is_object()) {
        return Err("Camera settings must be a JSON object.".to_string());
    }
    let camera = root.entry("camera").or_insert_with(|| json!({}));
    let Some(camera) = camera.as_object_mut() else {
        return Err("Camera settings must be a JSON object.".to_string());
    };
    let stored = match focus {
        Some(value) => json!(value),
        None => Value::Null,
    };
    let active = camera
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value <= 32);
    let copied = if active == Some(index) {
        ["resolution", "displayFps", "brightness", "contrast"]
            .into_iter()
            .filter_map(|name| camera.get(name).map(|value| (name, value.clone())))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if camera
        .get("byIndex")
        .is_some_and(|value| !value.is_object())
    {
        camera.insert("byIndex".to_string(), json!({}));
    }
    {
        let by_index = camera
            .entry("byIndex")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("byIndex is an object");
        let key = index.to_string();
        match by_index.get_mut(&key) {
            Some(record) if record.is_object() => {
                record
                    .as_object_mut()
                    .expect("focus record")
                    .insert("focus".to_string(), stored.clone());
            }
            _ => {
                let mut record = serde_json::Map::new();
                for (name, value) in copied {
                    record.insert(name.to_string(), value);
                }
                record.insert("focus".to_string(), stored.clone());
                by_index.insert(key, Value::Object(record));
            }
        }
    }
    if active == Some(index) {
        camera.insert("focus".to_string(), stored);
    }
    touch_uvc_focus(camera, focus);
    Ok(())
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

fn rgb_frame(bytes: &[u8], mode: Mode) -> Result<(Vec<u8>, u32, u32), String> {
    match mode.format {
        PixelFormat::Mjpeg => mjpeg_to_rgb(bytes),
        PixelFormat::Yuy2 => Ok((
            yuy2_to_rgb(bytes, mode.width, mode.height)?,
            mode.width,
            mode.height,
        )),
        PixelFormat::Nv12 => Ok((
            nv12_to_rgb(bytes, mode.width, mode.height)?,
            mode.width,
            mode.height,
        )),
        PixelFormat::Rgb24 => Ok((
            bgr_to_rgb(bytes, mode.width, mode.height)?,
            mode.width,
            mode.height,
        )),
    }
}

#[cfg(test)]
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
        || !src_width.is_multiple_of(2)
        || !src_height.is_multiple_of(2)
        || !dst_width.is_multiple_of(2)
        || !dst_height.is_multiple_of(2)
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
    if width < 2 || height == 0 || !width.is_multiple_of(2) || bytes.len() < need {
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

fn nv12_to_rgb(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width < 2 || height < 2 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err("Could not decode the camera frame.".to_string());
    }
    let width = width as usize;
    let height = height as usize;
    let y_size = width.saturating_mul(height);
    let need = y_size.saturating_add(y_size / 2);
    if bytes.len() < need {
        return Err("Could not decode the camera frame.".to_string());
    }
    let mut rgb = vec![0_u8; width * height * 3];
    for row in 0..height {
        for col in 0..width {
            let y = i32::from(bytes[row * width + col]);
            let uv_at = y_size + (row / 2) * width + (col & !1);
            let u = i32::from(bytes[uv_at]);
            let v = i32::from(bytes[uv_at + 1]);
            write_rgb(&mut rgb, (row * width + col) * 3, y, u, v);
        }
    }
    Ok(rgb)
}

fn nv12_to_jpeg(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width < 2 || height < 2 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
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

    use windows::Win32::Media::DirectShow::{
        CameraControl_Exposure, CameraControl_Flags_Auto, CameraControl_Flags_Manual,
        CameraControl_Focus, IAMCameraControl, IAMVideoProcAmp, VideoProcAmp_Flags_Auto,
        VideoProcAmp_Flags_Manual, VideoProcAmp_Gain, VideoProcAmp_WhiteBalance,
    };
    use windows::Win32::Media::MediaFoundation::{
        IMFActivate, IMFAttributes, IMFMediaBuffer, IMFMediaSource, IMFMediaType, IMFSample,
        IMFSourceReader, IMFSourceReaderCallback, IMFSourceReaderCallback_Impl, MF_API_VERSION,
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
    use windows::core::{Error, HRESULT, Interface, Ref, implement};

    use super::{
        FpsMeter, Mode, PixelFormat, RELEASE_WAIT, RESOURCE_CODE, Requested, UvcSettings,
        choose_mode, exposure_device_value, fps_milli, gain_device_value, lock, short_camera,
        snap_control_value, white_balance_device_value,
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
        slots: Mutex<super::FrameSlots>,
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
                slots: Mutex::new(super::FrameSlots {
                    preview: None,
                    still: None,
                }),
                frame_cv: Condvar::new(),
                fps: Mutex::new(FpsMeter::new()),
                error: Mutex::new(None),
            }
        }

        /// Replace both handles. The next sample overwrites them, so a slow
        /// preview cannot build a queue behind the reader. The still handle
        /// stays when the preview handle is taken. A buffer handed off here
        /// is dropped when the next sample replaces it, unless a command
        /// still holds that handle for encode.
        fn publish(&self, frame: super::FrameHandle) {
            if self.stop.load(Ordering::Acquire) {
                return;
            }
            {
                let mut slots = lock(&self.slots);
                if self.stop.load(Ordering::Acquire) {
                    return;
                }
                slots.publish(frame);
            }
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
            // Request the next sample before keeping this buffer so the
            // driver is not stalled on the snapshot encode. The pixels stay
            // in the media buffer. Encoding locks it later, on the command
            // thread.
            self.shared.rearm();
            if self.shared.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            if let Some(sample) = sample.as_ref()
                && let Some(frame) = media_frame(sample)
            {
                self.shared.publish(super::FrameHandle::media(frame));
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

    struct FocusRange {
        control: IAMCameraControl,
        min: i32,
        max: i32,
        step: i32,
        default_value: i32,
        caps: i32,
    }

    struct ControlSpan {
        min: i32,
        max: i32,
        step: i32,
        default_value: i32,
        caps: i32,
    }

    fn camera_control_range(
        control: &IAMCameraControl,
        property: i32,
        label: &str,
    ) -> Result<ControlSpan, String> {
        read_control_range(
            label,
            |min, max, step, default_value, caps| unsafe {
                control.GetRange(property, min, max, step, default_value, caps)
            },
        )
    }

    fn proc_amp_range(
        control: &IAMVideoProcAmp,
        property: i32,
        label: &str,
    ) -> Result<ControlSpan, String> {
        read_control_range(
            label,
            |min, max, step, default_value, caps| unsafe {
                control.GetRange(property, min, max, step, default_value, caps)
            },
        )
    }

    fn read_control_range(
        label: &str,
        read: impl FnOnce(*mut i32, *mut i32, *mut i32, *mut i32, *mut i32) -> windows::core::Result<()>,
    ) -> Result<ControlSpan, String> {
        let mut min = 0_i32;
        let mut max = 0_i32;
        let mut step = 0_i32;
        let mut default_value = 0_i32;
        let mut caps = 0_i32;
        read(
            &mut min,
            &mut max,
            &mut step,
            &mut default_value,
            &mut caps,
        )
        .map_err(|err| short_camera(&mf_error(&err)))
        .map_err(|err| {
            if err == "Camera did not open." {
                format!("This camera does not support {label}.")
            } else {
                err
            }
        })?;
        Ok(ControlSpan {
            min,
            max,
            step,
            default_value,
            caps,
        })
    }

    fn write_camera_control(
        control: &IAMCameraControl,
        property: i32,
        value: i32,
        flags: i32,
        label: &str,
    ) -> Result<i32, String> {
        unsafe {
            control
                .Set(property, value, flags)
                .map_err(|err| format!("{label}: {}", short_camera(&mf_error(&err))))?;
            let mut got = value;
            let mut got_flags = flags;
            if control.Get(property, &mut got, &mut got_flags).is_ok() {
                return Ok(got);
            }
        }
        Ok(value)
    }

    fn write_proc_amp(
        control: &IAMVideoProcAmp,
        property: i32,
        value: i32,
        flags: i32,
        label: &str,
    ) -> Result<i32, String> {
        unsafe {
            control
                .Set(property, value, flags)
                .map_err(|err| format!("{label}: {}", short_camera(&mf_error(&err))))?;
            let mut got = value;
            let mut got_flags = flags;
            if control.Get(property, &mut got, &mut got_flags).is_ok() {
                return Ok(got);
            }
        }
        Ok(value)
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

        pub(super) fn wait_frame(&self, timeout: Duration) -> Result<super::FrameHandle, String> {
            self.wait_slot(timeout, true)
        }

        /// Clone the still handle. This does not take the preview handle, so
        /// a snapshot does not make the paced preview wait for another sample.
        pub(super) fn copy_still(&self, timeout: Duration) -> Result<super::FrameHandle, String> {
            self.wait_slot(timeout, false)
        }

        /// Manual focus. `percent` is the 0–100 slider stored as `focusAbsolute`.
        /// The driver min, max, and step are queried and the slider is mapped
        /// onto that range, so the written value is inside the range and on a
        /// step. The returned value is the one the driver kept. A camera that
        /// cannot be cast, or whose range or set call fails, returns an error
        /// and leaves the open session running.
        pub(super) fn set_focus(&self, percent: u32) -> Result<i32, String> {
            let range = self.focus_range()?;
            let manual = CameraControl_Flags_Manual.0;
            if range.caps != 0 && range.caps & manual == 0 {
                return Err("This camera does not support manual focus.".to_string());
            }
            let value = super::focus_device_value(percent, range.min, range.max, range.step);
            unsafe {
                range
                    .control
                    .Set(CameraControl_Focus.0, value, manual)
                    .map_err(|err| short_camera(&mf_error(&err)))?;
                let mut got = value;
                let mut got_flags = manual;
                if range
                    .control
                    .Get(CameraControl_Focus.0, &mut got, &mut got_flags)
                    .is_ok()
                {
                    return Ok(got);
                }
            }
            Ok(value)
        }

        /// Autofocus. The same failure leaves the open session running.
        pub(super) fn set_autofocus(&self) -> Result<(), String> {
            let range = self.focus_range()?;
            let auto = CameraControl_Flags_Auto.0;
            if range.caps != 0 && range.caps & auto == 0 {
                return Err("This camera does not support autofocus.".to_string());
            }
            unsafe {
                range
                    .control
                    .Set(CameraControl_Focus.0, range.default_value, auto)
                    .map_err(|err| short_camera(&mf_error(&err)))?;
            }
            Ok(())
        }

        /// Push the UVC block onto this open reader. One rejected control
        /// does not undo the others. The caller keeps the stream either way.
        /// `phase` is only used in the log (`before capture` or `open`).
        pub(super) fn apply_uvc(&self, settings: &UvcSettings, phase: &str) -> Result<(), String> {
            let mut notes = Vec::new();
            let mut errors = Vec::new();
            if settings.auto_exposure {
                match self.set_exposure_control(true, settings.exposure_absolute) {
                    Ok(_) => notes.push("exposure auto".to_string()),
                    Err(err) => errors.push(err),
                }
            } else {
                match self.set_exposure_control(false, settings.exposure_absolute) {
                    Ok(applied) => notes.push(super::control_note(
                        "exposure",
                        i64::from(settings.exposure_absolute),
                        applied,
                    )),
                    Err(err) => errors.push(err),
                }
            }
            if settings.auto_white_balance {
                match self.set_white_balance_control(true, settings.white_balance_kelvin) {
                    Ok(_) => notes.push("white balance auto".to_string()),
                    Err(err) => errors.push(err),
                }
            } else {
                match self.set_white_balance_control(false, settings.white_balance_kelvin) {
                    Ok(applied) => notes.push(super::control_note(
                        "white balance",
                        i64::from(settings.white_balance_kelvin),
                        applied,
                    )),
                    Err(err) => errors.push(err),
                }
            }
            match self.set_gain_control(settings.gain) {
                Ok(applied) => {
                    let requested = if settings.gain.is_finite() {
                        i64::from(settings.gain.round() as i32)
                    } else {
                        i64::from(applied)
                    };
                    notes.push(super::control_note("gain", requested, applied));
                }
                Err(err) => errors.push(err),
            }
            if settings.auto_focus {
                match self.set_autofocus() {
                    Ok(()) => notes.push("focus auto".to_string()),
                    Err(err) => errors.push(err),
                }
            } else {
                match self.set_focus(settings.focus_absolute) {
                    Ok(applied) => notes.push(super::control_note(
                        "focus",
                        i64::from(settings.focus_absolute),
                        applied,
                    )),
                    Err(err) => errors.push(err),
                }
            }
            if !notes.is_empty() {
                eprintln!("UVC controls ({phase}): {}", notes.join(", "));
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join(" "))
            }
        }

        pub(super) fn set_exposure_control(&self, auto: bool, absolute: i32) -> Result<i32, String> {
            self.set_named_camera_control(
                CameraControl_Exposure.0,
                "exposure",
                auto,
                absolute,
                true,
            )
        }

        pub(super) fn set_gain_control(&self, gain: f32) -> Result<i32, String> {
            let source = self
                .source
                .as_ref()
                .ok_or_else(|| "Camera did not open.".to_string())?;
            let control = source
                .cast::<IAMVideoProcAmp>()
                .map_err(|_| "This camera does not support gain.".to_string())?;
            let range = proc_amp_range(&control, VideoProcAmp_Gain.0, "gain")?;
            let manual = VideoProcAmp_Flags_Manual.0;
            if range.caps != 0 && range.caps & manual == 0 {
                return Err("This camera does not support manual gain.".to_string());
            }
            let value = gain_device_value(gain, range.min, range.max, range.step);
            write_proc_amp(&control, VideoProcAmp_Gain.0, value, manual, "gain")
        }

        pub(super) fn set_white_balance_control(&self, auto: bool, kelvin: i32) -> Result<i32, String> {
            let source = self
                .source
                .as_ref()
                .ok_or_else(|| "Camera did not open.".to_string())?;
            let control = source
                .cast::<IAMVideoProcAmp>()
                .map_err(|_| "This camera does not support white balance.".to_string())?;
            let range = proc_amp_range(&control, VideoProcAmp_WhiteBalance.0, "white balance")?;
            if auto {
                let flag = VideoProcAmp_Flags_Auto.0;
                if range.caps != 0 && range.caps & flag == 0 {
                    return Err("This camera does not support automatic white balance.".to_string());
                }
                return write_proc_amp(
                    &control,
                    VideoProcAmp_WhiteBalance.0,
                    range.default_value,
                    flag,
                    "white balance",
                );
            }
            let manual = VideoProcAmp_Flags_Manual.0;
            if range.caps != 0 && range.caps & manual == 0 {
                return Err("This camera does not support manual white balance.".to_string());
            }
            let value = white_balance_device_value(kelvin, range.min, range.max, range.step)?;
            write_proc_amp(
                &control,
                VideoProcAmp_WhiteBalance.0,
                value,
                manual,
                "white balance",
            )
        }

        fn set_named_camera_control(
            &self,
            property: i32,
            label: &str,
            auto: bool,
            absolute: i32,
            exposure: bool,
        ) -> Result<i32, String> {
            let source = self
                .source
                .as_ref()
                .ok_or_else(|| "Camera did not open.".to_string())?;
            let control = source
                .cast::<IAMCameraControl>()
                .map_err(|_| format!("This camera does not support {label}."))?;
            let range = camera_control_range(&control, property, label)?;
            let flag = if auto {
                CameraControl_Flags_Auto.0
            } else {
                CameraControl_Flags_Manual.0
            };
            if range.caps != 0 && range.caps & flag == 0 {
                let mode = if auto { "automatic" } else { "manual" };
                return Err(format!("This camera does not support {mode} {label}."));
            }
            let value = if auto {
                range.default_value
            } else if exposure {
                exposure_device_value(absolute, range.min, range.max, range.step)
            } else {
                snap_control_value(absolute, range.min, range.max, range.step)
            };
            write_camera_control(&control, property, value, flag, label)
        }

        fn focus_range(&self) -> Result<FocusRange, String> {
            let source = self
                .source
                .as_ref()
                .ok_or_else(|| "Camera did not open.".to_string())?;
            let control = source
                .cast::<IAMCameraControl>()
                .map_err(|_| "This camera does not support focus.".to_string())?;
            let mut min = 0_i32;
            let mut max = 0_i32;
            let mut step = 0_i32;
            let mut default_value = 0_i32;
            let mut caps = 0_i32;
            unsafe {
                control
                    .GetRange(
                        CameraControl_Focus.0,
                        &mut min,
                        &mut max,
                        &mut step,
                        &mut default_value,
                        &mut caps,
                    )
                    .map_err(|err| short_camera(&mf_error(&err)))?;
            }
            Ok(FocusRange {
                control,
                min,
                max,
                step,
                default_value,
                caps,
            })
        }

        fn wait_slot(
            &self,
            timeout: Duration,
            preview: bool,
        ) -> Result<super::FrameHandle, String> {
            let deadline = Instant::now() + timeout;
            let mut slots = lock(&self.shared.slots);
            loop {
                let ready = if preview {
                    slots.take_preview()
                } else {
                    slots.copy_still()
                };
                if let Some(bytes) = ready {
                    return Ok(bytes);
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
                    .wait_timeout(slots, deadline.saturating_duration_since(now))
                    .unwrap_or_else(|err| err.into_inner());
                slots = next;
                let empty = if preview {
                    slots.preview.is_none()
                } else {
                    slots.still.is_none()
                };
                if wait.timed_out() && empty {
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
            // Release the media buffers before the source shuts down.
            // A late callback sees `stop` and does not publish another one.
            lock(&self.shared.slots).clear();
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
        match device.copy_still(Duration::from_secs(2)) {
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
        // The source is open and the mode is selected, and ReadSample has not
        // started the callback. Write exposure, white balance, gain, and focus
        // on this handle now. A rejected control is a warning. Streaming still
        // starts, and this call does not run on the frame callback.
        super::apply_device_controls(&device, "before capture");
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

    /// Keep the sample's contiguous buffer. A sample that is already one
    /// buffer is not copied. `ConvertToContiguousBuffer` copies only when
    /// the sample is split across several buffers.
    fn media_frame(sample: &IMFSample) -> Option<MediaFrame> {
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }.ok()?;
        let length = unsafe { buffer.GetCurrentLength() }.ok()?;
        if length == 0 || length as usize > MAX_FRAME_BYTES {
            return None;
        }
        Some(MediaFrame {
            buffer,
            gate: Mutex::new(()),
        })
    }

    struct UnlockBuffer<'a>(&'a IMFMediaBuffer);

    impl Drop for UnlockBuffer<'_> {
        fn drop(&mut self) {
            unsafe {
                let _ = self.0.Unlock();
            }
        }
    }

    pub(super) struct MediaFrame {
        buffer: IMFMediaBuffer,
        gate: Mutex<()>,
    }

    // The source reader delivers this buffer on an MTA worker. The windows
    // crate does not mark the COM pointer Send. Callers initialise MTA on
    // the thread that locks it, and `gate` keeps Lock single-threaded.
    unsafe impl Send for MediaFrame {}
    unsafe impl Sync for MediaFrame {}

    impl MediaFrame {
        pub(super) fn with_pixels<R>(
            &self,
            use_pixels: impl FnOnce(&[u8]) -> R,
        ) -> Result<R, String> {
            let _gate = super::lock(&self.gate);
            ensure_com();
            let mut ptr = ptr::null_mut();
            let mut length = 0_u32;
            unsafe { self.buffer.Lock(&mut ptr, None, Some(&mut length)) }
                .map_err(|err| short_camera(&mf_error(&err)))?;
            let _unlock = UnlockBuffer(&self.buffer);
            if ptr.is_null() || length == 0 || length as usize > MAX_FRAME_BYTES {
                return Err("Could not read the camera frame.".to_string());
            }
            let bytes = unsafe { std::slice::from_raw_parts(ptr, length as usize) };
            Ok(use_pixels(bytes))
        }
    }

    fn ensure_com() {
        COM.with(|_| {});
    }

    thread_local! {
        static COM: () = {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
        };
    }

    #[cfg(test)]
    pub(super) fn memory_frame(bytes: &[u8]) -> Result<super::FrameHandle, String> {
        use windows::Win32::Media::MediaFoundation::MFCreateMemoryBuffer;

        if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
            return Err("Could not read the camera frame.".to_string());
        }
        let buffer = unsafe { MFCreateMemoryBuffer(bytes.len() as u32) }
            .map_err(|err| short_camera(&mf_error(&err)))?;
        let mut ptr = ptr::null_mut();
        unsafe { buffer.Lock(&mut ptr, None, None) }
            .map_err(|err| short_camera(&mf_error(&err)))?;
        let _unlock = UnlockBuffer(&buffer);
        if ptr.is_null() {
            return Err("Could not read the camera frame.".to_string());
        }
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        }
        drop(_unlock);
        unsafe {
            buffer
                .SetCurrentLength(bytes.len() as u32)
                .map_err(|err| short_camera(&mf_error(&err)))?;
        }
        Ok(super::FrameHandle::media(MediaFrame {
            buffer,
            gate: Mutex::new(()),
        }))
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

    use super::{Mode, Requested, UvcSettings};

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

        pub(super) fn wait_frame(&self, _timeout: Duration) -> Result<super::FrameHandle, String> {
            Err("Camera capture is only available on Windows.".to_string())
        }

        pub(super) fn copy_still(&self, _timeout: Duration) -> Result<super::FrameHandle, String> {
            Err("Camera capture is only available on Windows.".to_string())
        }

        pub(super) fn set_focus(&self, _percent: u32) -> Result<i32, String> {
            Err("Camera capture is only available on Windows.".to_string())
        }

        pub(super) fn set_autofocus(&self) -> Result<(), String> {
            Err("Camera capture is only available on Windows.".to_string())
        }

        pub(super) fn apply_uvc(&self, _settings: &UvcSettings, _phase: &str) -> Result<(), String> {
            Err("Camera capture is only available on Windows.".to_string())
        }

        pub(super) fn set_exposure_control(&self, _auto: bool, _absolute: i32) -> Result<i32, String> {
            Err("Camera capture is only available on Windows.".to_string())
        }

        pub(super) fn set_gain_control(&self, _gain: f32) -> Result<i32, String> {
            Err("Camera capture is only available on Windows.".to_string())
        }

        pub(super) fn set_white_balance_control(
            &self,
            _auto: bool,
            _kelvin: i32,
        ) -> Result<i32, String> {
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
    fn snapshot_copies_the_still_and_leaves_the_preview_slot() {
        let mut slots = FrameSlots {
            preview: None,
            still: None,
        };
        assert!(slots.copy_still().is_none());
        assert!(slots.take_preview().is_none());
        slots.publish(FrameHandle::owned(vec![1, 2, 3]));
        let still = slots.copy_still().expect("still");
        let preview = slots.take_preview().expect("preview");
        assert!(still.same_as(&preview));
        still
            .with_pixels(|bytes| assert_eq!(bytes, &[1, 2, 3]))
            .unwrap();
        preview
            .with_pixels(|bytes| assert_eq!(bytes, &[1, 2, 3]))
            .unwrap();
        assert!(slots.take_preview().is_none());
        let again = slots.copy_still().expect("still remains");
        assert!(again.same_as(&still));
        slots.publish(FrameHandle::owned(vec![4]));
        let new_still = slots.copy_still().expect("new still");
        assert!(!new_still.same_as(&still));
        new_still
            .with_pixels(|bytes| assert_eq!(bytes, &[4]))
            .unwrap();
        slots
            .take_preview()
            .expect("new preview")
            .with_pixels(|bytes| assert_eq!(bytes, &[4]))
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn media_buffer_is_handed_off_without_a_second_copy() {
        session::startup().expect("media foundation");
        let mut slots = FrameSlots {
            preview: None,
            still: None,
        };
        slots.publish(session::memory_frame(&[9, 8, 7]).expect("buffer"));
        let still = slots.copy_still().expect("still");
        let preview = slots.take_preview().expect("preview");
        assert!(still.same_as(&preview));
        still
            .with_pixels(|bytes| assert_eq!(bytes, &[9, 8, 7]))
            .unwrap();
        slots.publish(session::memory_frame(&[4]).expect("next buffer"));
        still
            .with_pixels(|bytes| assert_eq!(bytes, &[9, 8, 7]))
            .unwrap();
        slots
            .copy_still()
            .expect("new still")
            .with_pixels(|bytes| assert_eq!(bytes, &[4]))
            .unwrap();
    }

    #[test]
    fn focus_maps_the_slider_onto_the_device_range() {
        assert_eq!(focus_device_value(0, 0, 255, 1), 0);
        assert_eq!(focus_device_value(100, 0, 255, 1), 255);
        assert_eq!(focus_device_value(50, 0, 255, 1), 127);
        assert_eq!(focus_device_value(100, 0, 10, 3), 9);
        assert_eq!(focus_device_value(0, 10, 20, 5), 10);
        assert_eq!(focus_device_value(100, 10, 20, 5), 20);
        assert_eq!(focus_device_value(50, 10, 20, 5), 15);
        assert_eq!(focus_device_value(1, 0, 100, 5), 0);
        assert_eq!(focus_device_value(5, 0, 100, 5), 5);
        assert_eq!(focus_device_value(100, 5, 5, 1), 5);
        assert_eq!(focus_device_value(40, 0, 100, 0), 40);
        assert_eq!(focus_device_value(40, 0, 100, -2), 40);
        assert_eq!(focus_device_value(80, 20, 10, 1), 20);
        assert_eq!(focus_device_value(200, 0, 100, 1), 100);
    }

    #[test]
    fn stored_focus_ignores_null_and_prefers_the_camera_record() {
        let camera = json!({
            "index": 0,
            "focus": 20,
            "byIndex": {
                "0": { "focus": null, "resolution": "640x480@30" },
                "1": { "focus": 80 }
            }
        });
        assert_eq!(stored_focus(&camera, 0), None);
        assert_eq!(stored_focus(&camera, 1), Some(80));
        assert_eq!(stored_focus(&camera, 2), None);
        let active = json!({ "index": 1, "focus": 40 });
        assert_eq!(stored_focus(&active, 1), Some(40));
        assert_eq!(stored_focus(&active, 0), None);
    }

    #[test]
    fn write_focus_changes_only_the_focus_fields() {
        let mut config = json!({
            "trainingDir": "D:\\shots",
            "camera": {
                "index": 1,
                "resolution": "1920x1080@30",
                "displayFps": 15,
                "focus": null,
                "brightness": 60,
                "contrast": 40,
                "byIndex": {
                    "1": {
                        "resolution": "1920x1080@30",
                        "displayFps": 15,
                        "focus": null,
                        "brightness": 60,
                        "contrast": 40,
                        "scale": { "mmPerPx": 0.25, "width": 1280, "height": 720 }
                    }
                }
            }
        });
        write_focus(&mut config, 1, Some(35)).unwrap();
        assert_eq!(config["trainingDir"], "D:\\shots");
        assert_eq!(config["camera"]["resolution"], "1920x1080@30");
        assert_eq!(config["camera"]["displayFps"], 15);
        assert_eq!(config["camera"]["brightness"], 60);
        assert_eq!(config["camera"]["contrast"], 40);
        assert_eq!(config["camera"]["focus"], 35);
        assert_eq!(config["camera"]["byIndex"]["1"]["focus"], 35);
        assert_eq!(config["camera"]["byIndex"]["1"]["brightness"], 60);
        assert_eq!(config["camera"]["byIndex"]["1"]["scale"]["mmPerPx"], 0.25);
        assert_eq!(config["camera"]["byIndex"]["1"]["scale"]["width"], 1280);
        assert_eq!(
            config["camera"]["byIndex"]["1"]["resolution"],
            "1920x1080@30"
        );

        let mut bare = json!({
            "camera": {
                "index": 0,
                "resolution": "1280x720@30",
                "displayFps": 20,
                "focus": null,
                "brightness": 50,
                "contrast": 50
            }
        });
        write_focus(&mut bare, 0, Some(70)).unwrap();
        assert_eq!(bare["camera"]["focus"], 70);
        assert_eq!(bare["camera"]["resolution"], "1280x720@30");
        assert_eq!(bare["camera"]["brightness"], 50);
        assert_eq!(bare["camera"]["byIndex"]["0"]["focus"], 70);
        assert_eq!(bare["camera"]["byIndex"]["0"]["resolution"], "1280x720@30");
        assert_eq!(bare["camera"]["byIndex"]["0"]["displayFps"], 20);
        assert_eq!(bare["camera"]["byIndex"]["0"]["contrast"], 50);

        write_focus(&mut bare, 2, Some(10)).unwrap();
        assert_eq!(bare["camera"]["focus"], 70);
        assert_eq!(bare["camera"]["byIndex"]["2"]["focus"], 10);
        assert!(bare["camera"]["byIndex"]["2"].get("resolution").is_none());

        write_focus(&mut config, 1, None).unwrap();
        assert!(config["camera"]["focus"].is_null());
        assert!(config["camera"]["byIndex"]["1"]["focus"].is_null());
        assert_eq!(config["camera"]["brightness"], 60);
        assert_eq!(config["camera"]["resolution"], "1920x1080@30");
        assert_eq!(config["trainingDir"], "D:\\shots");
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
    fn nv12_block_becomes_rgb() {
        let rgb = nv12_to_rgb(&[16, 16, 16, 16, 128, 128], 2, 2).unwrap();
        assert_eq!(rgb.len(), 12);
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
            let _ = capture_still(Some(index)).expect("open");
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
            let _ = capture_still(Some(index)).expect("open");
            for step in 0..4 {
                thread::sleep(Duration::from_millis(500));
                let started = Instant::now();
                let body = capture_still(Some(index)).expect("frame");
                eprintln!(
                    "camera {index} step {step}: convert {} ms, {}x{} timestamp={}",
                    started.elapsed().as_millis(),
                    body["width"],
                    body["height"],
                    body["timestamp_ms"]
                );
                assert_eq!(body["status"], "pass", "{body}");
                assert!(body["width"].as_u64().unwrap_or(0) > 0, "{body}");
                assert!(body["height"].as_u64().unwrap_or(0) > 0, "{body}");
                assert!(body["timestamp_ms"].as_u64().unwrap_or(0) > 0, "{body}");
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
            match capture_still(Some(*index)) {
                Ok(body) if body["status"] == "pass" => {
                    thread::sleep(Duration::from_millis(800));
                    let again = capture_still(Some(*index)).expect("second frame");
                    eprintln!(
                        "camera {index}: {}x{} timestamp={}",
                        again["width"], again["height"], again["timestamp_ms"]
                    );
                    assert_eq!(again["status"], "pass", "{again}");
                    assert!(again["width"].as_u64().unwrap_or(0) > 0, "{again}");
                    assert!(again["height"].as_u64().unwrap_or(0) > 0, "{again}");
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
            let back = capture_still(Some(opened[0])).expect("reopen");
            eprintln!(
                "reopen camera {}: {}x{} timestamp={}",
                opened[0], back["width"], back["height"], back["timestamp_ms"]
            );
            assert_eq!(back["status"], "pass", "reopen after switch: {back}");
            thread::sleep(Duration::from_millis(800));
            let again = capture_still(Some(opened[0])).expect("reopen frame");
            assert_eq!(again["status"], "pass", "{again}");
            assert!(again["width"].as_u64().unwrap_or(0) > 0, "{again}");
        }
    }

    #[test]
    fn driver_block_follows_camera_type() {
        let uvc = load_camera_driver(&json!({ "index": 1 }));
        assert_eq!(uvc.kind, CameraDriverKind::Uvc);
        assert_eq!(uvc.device_index, 1);
        assert!(!uvc.uvc.auto_exposure);
        assert_eq!(uvc.uvc.exposure_absolute, 156);
        assert!(!uvc.uvc.auto_focus);
        assert_eq!(uvc.uvc.focus_absolute, 25);
        assert!(!uvc.uvc.auto_white_balance);
        assert_eq!(uvc.uvc.white_balance_kelvin, 4500);
        assert_eq!(uvc.uvc.gain, 0.0);
        assert_eq!(uvc.pylon.pixel_format, "RGB8");
        assert!((uvc.pylon.exposure_time_us - 15000.0).abs() < f32::EPSILON);

        let open = uvc_controls_for_open(&uvc, Some(50)).unwrap();
        assert_eq!(open.focus_absolute, 50);
        assert!(!open.auto_focus);
        assert_eq!(open.exposure_absolute, 156);

        let partial = load_camera_driver(&json!({
            "deviceIndex": 3,
            "uvcSettings": { "gain": 2 }
        }));
        assert_eq!(partial.device_index, 3);
        assert_eq!(partial.uvc.gain, 2.0);
        assert_eq!(partial.uvc.exposure_absolute, 156);

        let indexed = load_camera_driver(&json!({ "index": 1, "deviceIndex": 0 }));
        assert_eq!(indexed.device_index, 1);

        let pylon = load_camera_driver(&json!({
            "type": "pylon",
            "deviceIndex": 4,
            "pylonSettings": {
                "exposureTimeUs": 15000.0,
                "gainDb": 0.0,
                "pixelFormat": "RGB8"
            }
        }));
        assert_eq!(pylon.kind, CameraDriverKind::Pylon);
        assert!(uvc_controls_for_open(&pylon, None).is_none());
        let mut backend = open_backend(&pylon);
        let missing = backend.capture_frame().unwrap_err();
        assert_eq!(missing, "Pylon backend not linked yet");
        let exposure = ExposureConfig {
            auto: false,
            absolute: 156,
        };
        assert_eq!(
            backend.set_exposure(&exposure).unwrap_err(),
            "Pylon backend not linked yet"
        );
        assert_eq!(backend.set_gain(0.0).unwrap_err(), "Pylon backend not linked yet");
        assert!(backend.set_focus(false, Some(25)).is_ok());
        assert!(backend.set_white_balance(false, Some(4500)).is_ok());
    }

    #[test]
    fn clamped_control_logs_the_applied_value() {
        assert_eq!(control_note("exposure", 156, 156), "exposure 156");
        assert_eq!(
            control_note("exposure", 156, -6),
            "exposure 156 applied as -6"
        );
        assert_eq!(control_note("focus", 25, 63), "focus 25 applied as 63");
    }

    #[test]
    fn exposure_uses_absolute_units_or_log2_seconds() {
        assert_eq!(exposure_device_value(156, 1, 5000, 1), 156);
        assert_eq!(exposure_device_value(156, -13, -1, 1), -6);
        assert_eq!(gain_device_value(0.0, 0, 255, 1), 0);
        assert_eq!(snap_control_value(12, 0, 100, 5), 10);
        assert_eq!(white_balance_device_value(4500, 2800, 6500, 10).unwrap(), 4500);
        assert!(white_balance_device_value(4500, 0, 255, 1).is_err());
    }

    #[test]
    fn manual_focus_writes_the_uvc_absolute_value() {
        let mut config = json!({
            "camera": {
                "index": 1,
                "focus": null,
                "brightness": 60,
                "uvcSettings": {
                    "exposureAbsolute": 156,
                    "autoFocus": true,
                    "focusAbsolute": 10,
                    "gain": 0
                }
            }
        });
        write_focus(&mut config, 1, Some(25)).unwrap();
        assert_eq!(config["camera"]["focus"], 25);
        assert_eq!(config["camera"]["byIndex"]["1"]["focus"], 25);
        assert_eq!(config["camera"]["uvcSettings"]["focusAbsolute"], 25);
        assert_eq!(config["camera"]["uvcSettings"]["autoFocus"], false);
        assert_eq!(config["camera"]["uvcSettings"]["exposureAbsolute"], 156);
        assert_eq!(config["camera"]["uvcSettings"]["gain"], 0);
        assert_eq!(config["camera"]["brightness"], 60);
        write_focus(&mut config, 1, None).unwrap();
        assert!(config["camera"]["focus"].is_null());
        assert_eq!(config["camera"]["uvcSettings"]["autoFocus"], true);
        assert_eq!(config["camera"]["uvcSettings"]["focusAbsolute"], 25);
        assert_eq!(config["camera"]["pylonSettings"]["pixelFormat"], "RGB8");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "opens a real camera"]
    fn live_uvc_controls_preview_and_snapshot() {
        let index = saved_index();
        let exposure = set_camera_exposure(Some(index), 156).expect("exposure command");
        eprintln!("exposure {exposure}");
        assert_eq!(exposure["status"], "pass", "{exposure}");
        let applied = exposure["applied"].as_i64().expect("applied exposure");
        assert!(
            applied == 156 || (-13..=-1).contains(&applied),
            "{exposure}"
        );

        let gain = set_camera_gain(Some(index), 0.0).expect("gain command");
        eprintln!("gain {gain}");
        assert_eq!(gain["status"], "pass", "{gain}");
        assert_eq!(gain["applied"].as_i64().unwrap_or(-1), 0, "{gain}");

        let focus = set_camera_focus(Some(index), 25).expect("focus command");
        eprintln!("focus {focus}");
        assert_eq!(focus["status"], "pass", "{focus}");
        assert_eq!(focus["focus"], 25);

        let preview = capture_preview(Some(index)).expect("preview");
        assert_eq!(preview["status"], "pass", "{preview}");
        let image = preview["image"].as_str().unwrap_or("");
        assert!(image.starts_with("data:image/jpeg"), "{preview}");

        let still = capture_still(Some(index)).expect("snapshot");
        assert_eq!(still["status"], "pass", "{still}");
        assert!(still["width"].as_u64().unwrap_or(0) > 0, "{still}");
        assert!(still["height"].as_u64().unwrap_or(0) > 0, "{still}");

        let again = capture_preview(Some(index)).expect("preview continues");
        assert_eq!(again["status"], "pass", "{again}");
        assert!(
            again["image"]
                .as_str()
                .unwrap_or("")
                .starts_with("data:image/jpeg"),
            "{again}"
        );
    }
}
