// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![greet, start_vision_sidecar])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
use tauri_plugin_shell::ShellExt;
#[tauri::command]
fn start_vision_sidecar() -> Result<String, String> {
    use std::path::PathBuf;
    use std::process::Command;

    // Project root is one level above src-tauri
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("Could not find project root")?
        .to_path_buf();

    let python = project_root
        .join("vision")
        .join(".venv")
        .join("Scripts")
        .join("python.exe");
    let script = project_root.join("vision").join("main.py");

    if !python.exists() {
        return Err(format!("Python not found at: {}", python.display()));
    }
    if !script.exists() {
        return Err(format!("Script not found at: {}", script.display()));
    }

    let mut child = Command::new(&python)
        .arg(&script)
        .current_dir(&project_root)
        .spawn()
        .map_err(|e| format!("Failed to start sidecar: {}", e))?;

    Ok(format!("Sidecar started (pid {})", child.id()))
}
