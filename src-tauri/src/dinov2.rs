//! Optional DINOv2 ONNX session, stored beside the YOLO session.
//!
//! Startup opens `src-tauri/models/dinov2_vits14.onnx` when that file is present.
//! A missing or unloadable file is a warning. Detection keeps using the YOLO session.
//! This module does not crop, embed, or compare a BoM.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use crate::detect;
use crate::inference_ep::{self, Provider};

const FILE_NAME: &str = "dinov2_vits14.onnx";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dinov2Port {
    pub name: String,
    pub shape: Vec<i64>,
    pub dtype: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dinov2Status {
    pub loaded: bool,
    pub path: String,
    pub inputs: Vec<Dinov2Port>,
    pub outputs: Vec<Dinov2Port>,
    pub message: String,
}

struct State {
    session: Option<ort::session::Session>,
    status: Dinov2Status,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// `src-tauri/models/dinov2_vits14.onnx`, from the crate directory baked in at compile time.
pub(crate) fn model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join(FILE_NAME)
}

/// Open the DINOv2 session once. A missing file does not touch the YOLO cache.
pub(crate) fn load_on_startup() {
    load_at(&model_path());
}

pub(crate) fn load_at(path: &Path) -> Dinov2Status {
    let status = match open_held(path) {
        Ok((session, status)) => {
            *lock(&STATE) = Some(State {
                session: Some(session),
                status: status.clone(),
            });
            status
        }
        Err(message) => {
            let status = Dinov2Status {
                loaded: false,
                path: path.display().to_string(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                message,
            };
            *lock(&STATE) = Some(State {
                session: None,
                status: status.clone(),
            });
            status
        }
    };
    log_status(&status);
    status
}

/// Whether the DINOv2 session loaded, plus its input and output names and shapes.
#[tauri::command]
pub fn dinov2_status() -> Dinov2Status {
    let guard = lock(&STATE);
    let Some(state) = guard.as_ref() else {
        return idle_status();
    };
    let Some(session) = state.session.as_ref() else {
        return state.status.clone();
    };
    let mut status = state.status.clone();
    status.inputs = ports(session.inputs());
    status.outputs = ports(session.outputs());
    status
}

fn idle_status() -> Dinov2Status {
    Dinov2Status {
        loaded: false,
        path: model_path().display().to_string(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        message: "DINOv2 session has not been loaded.".to_string(),
    }
}

fn open_held(path: &Path) -> Result<(ort::session::Session, Dinov2Status), String> {
    if !path.is_file() {
        return Err(format!(
            "DINOv2 model is missing at {}. YOLO detection is unchanged.",
            path.display()
        ));
    }
    let (session, fallback) = open_dinov2(path)?;
    let mut message = format!("DINOv2 session is ready. Loaded from {}.", path.display());
    if let Some(note) = fallback {
        message.push(' ');
        message.push_str(&note);
    }
    let status = Dinov2Status {
        loaded: true,
        path: path.display().to_string(),
        inputs: ports(session.inputs()),
        outputs: ports(session.outputs()),
        message,
    };
    Ok((session, status))
}

/// Same execution provider as YOLO, with a CPU fallback that does not change the saved provider.
fn open_dinov2(path: &Path) -> Result<(ort::session::Session, Option<String>), String> {
    let env = detect::runtime().map_err(|err| {
        format!("DINOv2 model failed to load ({err}). YOLO detection is unchanged.")
    })?;
    let requested = inference_ep::selected_provider();
    match detect::open_session(&env, path, requested) {
        Ok(session) => Ok((session, None)),
        Err(err) if requested != Provider::Cpu => {
            match detect::open_session(&env, path, Provider::Cpu) {
                Ok(session) => Ok((
                    session,
                    Some(format!(
                        "{} did not start for DINOv2. Using CPU.",
                        requested.label()
                    )),
                )),
                Err(cpu_err) => Err(format!(
                    "DINOv2 model failed to load ({err} {cpu_err}). YOLO detection is unchanged."
                )),
            }
        }
        Err(err) => Err(format!(
            "DINOv2 model failed to load ({err}). YOLO detection is unchanged."
        )),
    }
}

fn ports(outlets: &[ort::value::Outlet]) -> Vec<Dinov2Port> {
    outlets
        .iter()
        .map(|outlet| Dinov2Port {
            name: outlet.name().to_string(),
            shape: outlet
                .dtype()
                .tensor_shape()
                .map(|shape| shape.to_vec())
                .unwrap_or_default(),
            dtype: outlet.dtype().to_string(),
        })
        .collect()
}

fn log_status(status: &Dinov2Status) {
    if !status.loaded {
        eprintln!("warning: {}", status.message);
        return;
    }
    eprintln!("{}", status.message);
    for port in &status.inputs {
        eprintln!("DINOv2 input {} {}", port.name, port.dtype);
    }
    for port in &status.outputs {
        eprintln!("DINOv2 output {} {}", port.name, port.dtype);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_warns_and_a_present_model_reports_shapes() {
        let missing = std::env::temp_dir().join("ls-vision-dinov2-missing.onnx");
        let _ = std::fs::remove_file(&missing);
        let status = load_at(&missing);
        assert!(!status.loaded);
        assert!(status.message.contains("missing"), "{}", status.message);
        assert!(
            status.message.contains("YOLO detection is unchanged."),
            "{}",
            status.message
        );
        assert!(status.inputs.is_empty());
        assert!(status.outputs.is_empty());
        assert_eq!(dinov2_status(), status);

        let path = model_path();
        if !path.is_file() {
            eprintln!("skip: {FILE_NAME} is not in src-tauri/models");
            return;
        }
        let status = load_at(&path);
        assert!(status.loaded, "{}", status.message);
        assert!(
            status.message.contains("DINOv2 session is ready"),
            "{}",
            status.message
        );
        let input = status
            .inputs
            .iter()
            .find(|port| port.name == "input")
            .expect("input");
        assert_eq!(input.shape, vec![1, 3, 224, 224]);
        let output = status
            .outputs
            .iter()
            .find(|port| port.name == "output")
            .expect("output");
        assert_eq!(output.shape, vec![1, 384]);
        assert_eq!(dinov2_status().inputs, status.inputs);
        assert_eq!(dinov2_status().outputs, status.outputs);
        assert!(dinov2_status().loaded);
    }
}
