//! Which ONNX Runtime execution providers this PC can use.
//!
//! CPU is always listed. DirectML is listed when the DirectML runtime loads and
//! a hardware Direct3D 12 adapter is present. CUDA is listed when an NVIDIA
//! adapter is present and the CUDA execution provider library loads.
//! `GetAvailableProviders` only says the DLL was built with an EP, so the GPU
//! and the library are checked as well.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;

use crate::local_config;

const NVIDIA_VENDOR: u32 = 0x10DE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Provider {
    Cpu,
    DirectMl,
    Cuda,
}

impl Provider {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::DirectMl => "directml",
            Self::Cuda => "cuda",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::DirectMl => "DirectML",
            Self::Cuda => "CUDA",
        }
    }
}

pub(crate) fn parse_provider(value: &str) -> Option<Provider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cpu" => Some(Provider::Cpu),
        "directml" | "dml" => Some(Provider::DirectMl),
        "cuda" => Some(Provider::Cuda),
        _ => None,
    }
}

/// DirectML, then CUDA, then CPU. A saved provider wins when it is still listed.
pub(crate) fn choose(available: &[Provider], saved: Option<Provider>) -> Provider {
    if let Some(saved) = saved
        && available.contains(&saved)
    {
        return saved;
    }
    for candidate in [Provider::DirectMl, Provider::Cuda, Provider::Cpu] {
        if available.contains(&candidate) {
            return candidate;
        }
    }
    Provider::Cpu
}

#[derive(Clone)]
struct Observed {
    requested: Provider,
    active: Provider,
    message: String,
}

static AVAILABLE: Mutex<Option<Vec<Provider>>> = Mutex::new(None);
static OBSERVED: Mutex<Option<Observed>> = Mutex::new(None);

const OPENVINO_READY: &str = "Using OpenVINO.";
const OPENVINO_UNAVAILABLE: &str = "OpenVINO is not available. Using ONNX Runtime.";
const OPENVINO_FAILED: &str = "OpenVINO did not start. Using ONNX Runtime.";
/// Name passed to `Environment::register_ep_library`. Devices then report this EP.
const OPENVINO_PLUGIN_NAME: &str = "OpenVINOExecutionProvider";
/// Separate plugin package name. The pyke `openvino` prebuilt does not ship this file.
const OPENVINO_PLUGIN_DLL: &str = "onnxruntime_providers_openvino_plugin.dll";
/// Provider DLL from the pyke `openvino` prebuilt. It exports `CreateEpFactories`.
const OPENVINO_PROVIDER_DLL: &str = "onnxruntime_providers_openvino.dll";

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenVinoRun {
    Off,
    Pending,
    Active,
    Fallback,
}

#[derive(Clone)]
struct OpenVinoObserved {
    run: OpenVinoRun,
    message: String,
}

static OPENVINO: Mutex<OpenVinoObserved> = Mutex::new(OpenVinoObserved {
    run: OpenVinoRun::Off,
    message: String::new(),
});
/// Kept for the process. Dropping the handle does not unregister the plugin.
static OPENVINO_PLUGIN: Mutex<Option<ort::ep::ExecutionProviderLibrary>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

pub(crate) fn remember(requested: Provider, active: Provider, message: impl Into<String>) {
    *lock(&OBSERVED) = Some(Observed {
        requested,
        active,
        message: message.into(),
    });
}

pub(crate) fn available_providers(refresh: bool) -> Vec<Provider> {
    let mut guard = lock(&AVAILABLE);
    if refresh || guard.is_none() {
        *guard = Some(probe());
    }
    (*guard).clone().unwrap_or_else(|| vec![Provider::Cpu])
}

pub(crate) fn selected_provider() -> Provider {
    choose(&available_providers(false), read_saved())
}

fn read_saved() -> Option<Provider> {
    let root = local_config::project_root().ok()?;
    let document = local_config::load_document(&root).ok()?;
    document
        .get("inferenceEp")
        .and_then(|value| value.as_str())
        .and_then(parse_provider)
}

/// Missing, or any value that is not a JSON boolean, stays off.
pub(crate) fn use_openvino_value(value: Option<&serde_json::Value>) -> bool {
    value.and_then(serde_json::Value::as_bool).unwrap_or(false)
}

pub(crate) fn read_use_openvino() -> bool {
    let Ok(root) = local_config::project_root() else {
        return false;
    };
    let Ok(document) = local_config::load_document(&root) else {
        return false;
    };
    use_openvino_value(document.get("useOpenVino"))
}

fn store_use_openvino(enabled: bool) -> Result<(), String> {
    let root = local_config::project_root()?;
    let mut document = local_config::load_document(&root)?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| "Config must be a JSON object.".to_string())?;
    object.insert("useOpenVino".to_string(), serde_json::Value::Bool(enabled));
    local_config::save_document(&root, &document)
}

/// Saved switch is on, and the OpenVINO plugin registered.
pub(crate) fn wants_openvino() -> bool {
    let run = lock(&OPENVINO).run;
    matches!(run, OpenVinoRun::Pending | OpenVinoRun::Active)
}

pub(crate) fn openvino_is_active() -> bool {
    lock(&OPENVINO).run == OpenVinoRun::Active
}

/// `openvino/` next to the exe (the installer resource folder), then beside the
/// exe, then `resources/openvino`. The `*_plugin.dll` name wins inside a folder.
/// Otherwise the pyke provider DLL is used. The bundled folder wins over a lone
/// provider DLL beside the exe, because `openvino.dll` and TBB sit in that folder.
/// ORT 1.31 does not read `ORT_OPENVINO_DLL`.
pub(crate) fn find_openvino_plugin(exe_dir: &std::path::Path) -> Option<PathBuf> {
    let names = [OPENVINO_PLUGIN_DLL, OPENVINO_PROVIDER_DLL];
    let bundled = exe_dir.join("openvino");
    for name in names {
        let path = bundled.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    for name in names {
        let beside = exe_dir.join(name);
        if beside.is_file() {
            return Some(beside);
        }
    }
    let nested_dir = exe_dir.join("resources").join("openvino");
    for name in names {
        let nested = nested_dir.join(name);
        if nested.is_file() {
            return Some(nested);
        }
    }
    None
}

fn register_openvino_plugin() -> Result<(), String> {
    let mut registered = lock(&OPENVINO_PLUGIN);
    if registered.is_some() {
        return Ok(());
    }
    let exe =
        std::env::current_exe().map_err(|_| "Could not find the app directory.".to_string())?;
    let Some(dir) = exe.parent() else {
        return Err("Could not find the app directory.".to_string());
    };
    let Some(path) = find_openvino_plugin(dir) else {
        return Err(format!(
            "{OPENVINO_PLUGIN_DLL} or {OPENVINO_PROVIDER_DLL} was not found next to the app."
        ));
    };
    let env = crate::detect::runtime()?;
    let library = with_plugin_dll_directory(path.parent(), || {
        env.register_ep_library(OPENVINO_PLUGIN_NAME, &path)
    })
    .map_err(|err| {
        format!(
            "could not register {}: {}",
            path.display(),
            one_line(&err.to_string())
        )
    })?;
    eprintln!("OpenVINO plugin {}", path.display());
    *registered = Some(library);
    Ok(())
}

fn one_line(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return "unknown error".to_string();
    }
    if text.chars().count() <= 240 {
        return text;
    }
    let trimmed: String = text.chars().take(237).collect();
    format!("{trimmed}...")
}

/// Read the switch and register the OpenVINO plugin when it is on.
/// Off leaves the ONNX Runtime path alone. A missing DLL or a failed
/// registration records the fallback line and stays on ONNX Runtime.
pub(crate) fn prepare_openvino() {
    if !read_use_openvino() {
        *lock(&OPENVINO) = OpenVinoObserved {
            run: OpenVinoRun::Off,
            message: String::new(),
        };
        return;
    }
    if !crate::detect::ensure_runtime() {
        note_openvino_unavailable("ONNX Runtime did not start.");
        return;
    }
    if !read_use_openvino() {
        *lock(&OPENVINO) = OpenVinoObserved {
            run: OpenVinoRun::Off,
            message: String::new(),
        };
        return;
    }
    if let Err(err) = register_openvino_plugin() {
        note_openvino_unavailable(&err);
        return;
    }
    *lock(&OPENVINO) = OpenVinoObserved {
        run: OpenVinoRun::Pending,
        message: String::new(),
    };
}

fn note_openvino_unavailable(err: &str) {
    eprintln!("OpenVINO plugin: {err}");
    eprintln!("{OPENVINO_UNAVAILABLE}");
    *lock(&OPENVINO) = OpenVinoObserved {
        run: OpenVinoRun::Fallback,
        message: OPENVINO_UNAVAILABLE.to_string(),
    };
}

/// Windows looks for the plugin's OpenVINO and TBB imports in the exe directory.
/// A plugin under `resources/openvino` also needs that folder on the search path.
fn with_plugin_dll_directory<T>(dir: Option<&std::path::Path>, run: impl FnOnce() -> T) -> T {
    let _guard = dir.and_then(push_dll_directory);
    run()
}

fn push_dll_directory(dir: &std::path::Path) -> Option<DllDirectoryGuard> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::System::LibraryLoader::SetDllDirectoryW;
        use windows::core::PCWSTR;

        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        if unsafe { SetDllDirectoryW(PCWSTR(wide.as_ptr())) }.is_err() {
            None
        } else {
            Some(DllDirectoryGuard)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        None
    }
}

struct DllDirectoryGuard;

impl Drop for DllDirectoryGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::System::LibraryLoader::SetDllDirectoryW;
            use windows::core::PCWSTR;

            unsafe {
                let _ = SetDllDirectoryW(PCWSTR(std::ptr::null()));
            }
        }
    }
}

pub(crate) fn mark_openvino_started() {
    *lock(&OPENVINO) = OpenVinoObserved {
        run: OpenVinoRun::Active,
        message: OPENVINO_READY.to_string(),
    };
}

pub(crate) fn mark_openvino_failed(err: &str) {
    eprintln!("OpenVINO did not start: {err}");
    *lock(&OPENVINO) = OpenVinoObserved {
        run: OpenVinoRun::Fallback,
        message: OPENVINO_FAILED.to_string(),
    };
}

fn compose_message(provider_message: String, openvino: &OpenVinoObserved) -> String {
    match openvino.run {
        OpenVinoRun::Off | OpenVinoRun::Pending => provider_message,
        OpenVinoRun::Active | OpenVinoRun::Fallback => {
            if provider_message.is_empty() {
                openvino.message.clone()
            } else if openvino.message.is_empty() {
                provider_message
            } else {
                format!("{} {provider_message}", openvino.message)
            }
        }
    }
}

fn store(provider: Provider) -> Result<(), String> {
    let root = local_config::project_root()?;
    let mut document = local_config::load_document(&root)?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| "Config must be a JSON object.".to_string())?;
    object.insert(
        "inferenceEp".to_string(),
        serde_json::Value::String(provider.id().to_string()),
    );
    local_config::save_document(&root, &document)
}

#[derive(Serialize)]
pub(crate) struct InferenceView {
    pub providers: Vec<&'static str>,
    pub selected: &'static str,
    pub active: &'static str,
    pub message: String,
    #[serde(rename = "useOpenVino")]
    pub use_open_vino: bool,
    #[serde(rename = "openVinoActive")]
    pub open_vino_active: bool,
}

pub(crate) fn view(refresh: bool) -> InferenceView {
    let providers = available_providers(refresh);
    let selected = choose(&providers, read_saved());
    let observed = {
        let guard = lock(&OBSERVED);
        (*guard).clone()
    };
    let (active, provider_message) = match observed {
        Some(item) if item.requested == selected => (item.active, item.message),
        _ => (selected, String::new()),
    };
    let openvino = lock(&OPENVINO).clone();
    InferenceView {
        providers: providers.iter().copied().map(Provider::id).collect(),
        selected: selected.id(),
        active: active.id(),
        message: compose_message(provider_message, &openvino),
        use_open_vino: read_use_openvino(),
        open_vino_active: openvino.run == OpenVinoRun::Active,
    }
}

#[tauri::command]
pub fn inference_status() -> Result<InferenceView, String> {
    Ok(view(true))
}

#[tauri::command]
pub fn set_inference_ep(provider: String) -> Result<InferenceView, String> {
    let available = available_providers(true);
    let Some(choice) = parse_provider(&provider) else {
        return Err("Choose CPU, DirectML, or CUDA.".to_string());
    };
    if !available.contains(&choice) {
        return Err(format!("{} is not available on this PC.", choice.label()));
    }
    store(choice)?;
    *lock(&OBSERVED) = None;
    let openvino = read_use_openvino();
    if openvino {
        prepare_openvino();
    }
    let reloaded = crate::detect::reload_session();
    if openvino {
        crate::dinov2::reload_session();
    }
    reloaded?;
    Ok(view(false))
}

/// Save Use OpenVINO and reload YOLO and DINOv2.
/// The saved switch stays on when the provider is missing. The status line
/// says the session stayed on ONNX Runtime.
#[tauri::command]
pub fn set_use_openvino(enabled: bool) -> Result<InferenceView, String> {
    store_use_openvino(enabled)?;
    *lock(&OBSERVED) = None;
    prepare_openvino();
    let reloaded = crate::detect::reload_session();
    crate::dinov2::reload_session();
    reloaded?;
    Ok(view(false))
}

fn probe() -> Vec<Provider> {
    let facts = gpu_facts();
    let runtime_ready = crate::detect::ensure_runtime();
    let mut found = Vec::new();
    if runtime_ready && directml_usable(&facts) {
        found.push(Provider::DirectMl);
    }
    if runtime_ready && cuda_usable(&facts) {
        found.push(Provider::Cuda);
    }
    found.push(Provider::Cpu);
    found
}

struct GpuFacts {
    direct3d12: bool,
    nvidia: bool,
}

fn gpu_facts() -> GpuFacts {
    #[cfg(windows)]
    {
        windows_gpu_facts()
    }
    #[cfg(not(windows))]
    {
        GpuFacts {
            direct3d12: false,
            nvidia: false,
        }
    }
}

#[cfg(windows)]
fn windows_gpu_facts() -> GpuFacts {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12Device};
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };

    let mut facts = GpuFacts {
        direct3d12: false,
        nvidia: false,
    };
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return facts;
    };
    for index in 0..32u32 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
            continue;
        }
        if desc.VendorId == NVIDIA_VENDOR {
            facts.nvidia = true;
        }
        let mut device: Option<ID3D12Device> = None;
        if unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }.is_ok() {
            facts.direct3d12 = true;
        }
    }
    facts
}

fn directml_usable(facts: &GpuFacts) -> bool {
    facts.direct3d12 && library_name_loads("DirectML.dll") && directml_api_present()
}

fn directml_api_present() -> bool {
    // The OpenVINO prebuilt does not include the DirectML execution provider.
    false
}

fn cuda_usable(facts: &GpuFacts) -> bool {
    facts.nvidia && cuda_libraries_load()
}

fn cuda_libraries_load() -> bool {
    // The OpenVINO prebuilt does not include the CUDA execution provider.
    let Some(path) = find_cuda_provider_dll() else {
        return false;
    };
    library_loads(path.as_os_str())
}

fn find_cuda_provider_dll() -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.to_path_buf());
        if let Some(parent) = dir.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    for dir in dirs {
        let path = dir.join("onnxruntime_providers_cuda.dll");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn library_name_loads(name: &str) -> bool {
    library_loads(std::ffi::OsStr::new(name))
}

fn library_loads(file: &std::ffi::OsStr) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::FreeLibrary;
        use windows::Win32::System::LibraryLoader::LoadLibraryW;
        use windows::core::PCWSTR;

        let wide: Vec<u16> = file.encode_wide().chain(std::iter::once(0)).collect();
        unsafe {
            let Ok(module) = LoadLibraryW(PCWSTR(wide.as_ptr())) else {
                return false;
            };
            let _ = FreeLibrary(module);
            true
        }
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fastest_provider_prefers_directml_then_cuda_then_cpu() {
        assert_eq!(
            choose(&[Provider::Cpu, Provider::DirectMl, Provider::Cuda], None),
            Provider::DirectMl
        );
        assert_eq!(
            choose(&[Provider::Cpu, Provider::Cuda], None),
            Provider::Cuda
        );
        assert_eq!(choose(&[Provider::Cpu], None), Provider::Cpu);
    }

    #[test]
    fn saved_choice_is_kept_only_when_it_is_still_listed() {
        assert_eq!(
            choose(
                &[Provider::Cpu, Provider::DirectMl, Provider::Cuda],
                Some(Provider::Cuda)
            ),
            Provider::Cuda
        );
        assert_eq!(
            choose(&[Provider::Cpu, Provider::DirectMl], Some(Provider::Cuda)),
            Provider::DirectMl
        );
        assert_eq!(
            choose(&[Provider::Cpu], Some(Provider::DirectMl)),
            Provider::Cpu
        );
    }

    #[test]
    fn use_openvino_is_off_unless_the_json_boolean_is_true() {
        assert!(!use_openvino_value(None));
        assert!(!use_openvino_value(Some(&serde_json::Value::Null)));
        assert!(!use_openvino_value(Some(&serde_json::json!(false))));
        assert!(!use_openvino_value(Some(&serde_json::json!("true"))));
        assert!(use_openvino_value(Some(&serde_json::json!(true))));
    }

    #[test]
    fn openvino_fallback_line_stays_with_the_provider_message() {
        let fallback = OpenVinoObserved {
            run: OpenVinoRun::Fallback,
            message: OPENVINO_UNAVAILABLE.to_string(),
        };
        assert_eq!(
            compose_message("DirectML did not start. Using CPU.".to_string(), &fallback),
            "OpenVINO is not available. Using ONNX Runtime. DirectML did not start. Using CPU."
        );
        assert_eq!(
            compose_message(String::new(), &fallback),
            OPENVINO_UNAVAILABLE
        );
        assert_eq!(
            compose_message(
                String::new(),
                &OpenVinoObserved {
                    run: OpenVinoRun::Off,
                    message: String::new(),
                }
            ),
            ""
        );
    }

    #[test]
    fn openvino_plugin_dll_is_beside_the_exe_or_under_resources() {
        let root = std::env::temp_dir().join(format!("ls-vision-ov-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("resources").join("openvino")).unwrap();
        assert!(find_openvino_plugin(&root).is_none());
        let nested_provider = root
            .join("resources")
            .join("openvino")
            .join("onnxruntime_providers_openvino.dll");
        std::fs::write(&nested_provider, b"provider").unwrap();
        assert_eq!(
            find_openvino_plugin(&root).as_deref(),
            Some(nested_provider.as_path())
        );
        let nested = root
            .join("resources")
            .join("openvino")
            .join("onnxruntime_providers_openvino_plugin.dll");
        std::fs::write(&nested, b"plugin").unwrap();
        assert_eq!(
            find_openvino_plugin(&root).as_deref(),
            Some(nested.as_path())
        );
        let beside_provider = root.join("onnxruntime_providers_openvino.dll");
        std::fs::write(&beside_provider, b"provider").unwrap();
        assert_eq!(
            find_openvino_plugin(&root).as_deref(),
            Some(beside_provider.as_path())
        );
        let beside = root.join("onnxruntime_providers_openvino_plugin.dll");
        std::fs::write(&beside, b"plugin").unwrap();
        assert_eq!(
            find_openvino_plugin(&root).as_deref(),
            Some(beside.as_path())
        );
        let bundled_dir = root.join("openvino");
        std::fs::create_dir_all(&bundled_dir).unwrap();
        let bundled = bundled_dir.join("onnxruntime_providers_openvino.dll");
        std::fs::write(&bundled, b"bundled").unwrap();
        assert_eq!(
            find_openvino_plugin(&root).as_deref(),
            Some(bundled.as_path())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn provider_names_round_trip() {
        assert_eq!(parse_provider(" DirectML "), Some(Provider::DirectMl));
        assert_eq!(parse_provider("dml"), Some(Provider::DirectMl));
        assert_eq!(parse_provider("cuda"), Some(Provider::Cuda));
        assert_eq!(parse_provider("cpu"), Some(Provider::Cpu));
        assert_eq!(parse_provider("tensorrt"), None);
        assert_eq!(Provider::DirectMl.id(), "directml");
        assert_eq!(Provider::Cuda.label(), "CUDA");
    }

    #[test]
    fn this_pc_lists_cpu_and_only_known_providers() {
        let list = available_providers(true);
        eprintln!(
            "inference providers: {}",
            list.iter()
                .map(|item| item.id())
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert!(list.contains(&Provider::Cpu));
        assert!(
            list.iter().all(|item| {
                matches!(item, Provider::Cpu | Provider::DirectMl | Provider::Cuda)
            })
        );
        let mut ids: Vec<_> = list.iter().map(|item| item.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), list.len());
        assert_eq!(choose(&list, None), list[0]);
    }
}
