//! Which ONNX Runtime execution providers this PC can use.
//!
//! CPU is always listed. DirectML is listed when the DirectML runtime loads and
//! a hardware Direct3D 12 adapter is present. CUDA is listed when an NVIDIA
//! adapter is present and the CUDA execution provider library loads.
//! `GetAvailableProviders` only says the DLL was built with an EP, so the GPU
//! and the library are checked as well.

use std::path::PathBuf;
use std::sync::Mutex;

use ort::ep::ExecutionProvider;
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
}

pub(crate) fn view(refresh: bool) -> InferenceView {
    let providers = available_providers(refresh);
    let selected = choose(&providers, read_saved());
    let observed = {
        let guard = lock(&OBSERVED);
        (*guard).clone()
    };
    let (active, message) = match observed {
        Some(item) if item.requested == selected => (item.active, item.message),
        _ => (selected, String::new()),
    };
    InferenceView {
        providers: providers.iter().copied().map(Provider::id).collect(),
        selected: selected.id(),
        active: active.id(),
        message,
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
    crate::detect::reload_session()?;
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
    ort::ep::directml::api().is_some()
}

fn cuda_usable(facts: &GpuFacts) -> bool {
    facts.nvidia && cuda_libraries_load()
}

fn cuda_libraries_load() -> bool {
    if let Some(path) = find_cuda_provider_dll() {
        return library_loads(path.as_os_str());
    }
    ort::ep::CUDA::is_available().unwrap_or(false) && library_name_loads("nvcuda.dll")
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
