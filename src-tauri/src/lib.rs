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
#[tauri::command]
fn start_vision_sidecar() -> Result<String, String> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    // Project root is one level above src-tauri. The path is compiled in, so
    // the packaged exe still starts the sidecar from the repo checkout rather
    // than from the folder that contains the exe.
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

    let path = sidecar_dll_path(&project_root);
    let log_path = project_root.join("vision").join("sidecar.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(
            file,
            "launch python={} script={} cwd={}",
            python.display(),
            script.display(),
            project_root.display()
        );
    }

    let mut command = Command::new(&python);
    command
        .arg(&script)
        .current_dir(&project_root)
        .env("PATH", &path)
        .stdin(Stdio::null());
    if std::env::var_os("PYTHONUNBUFFERED").is_none() {
        command.env("PYTHONUNBUFFERED", "1");
    }
    // python.exe is a console program. Spawned from this GUI app it opens a
    // console window unless the window is suppressed and stdio is redirected.
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        if let Ok(err) = file.try_clone() {
            command.stdout(Stdio::from(file)).stderr(Stdio::from(err));
        }
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command
        .spawn()
        .map_err(|e| format!("Failed to start sidecar: {}", e))?;

    Ok(format!("Sidecar started (pid {})", child.id()))
}

fn sidecar_dll_path(project_root: &std::path::Path) -> std::ffi::OsString {
    use std::ffi::OsString;

    let venv = project_root.join("vision").join(".venv");
    let site = venv.join("Lib").join("site-packages");
    // pyzbar's first load is LoadLibrary("libiconv.dll"). Search the bundled
    // DLLs before the shorter PATH a GUI exe inherits from Explorer.
    let dirs = [site.join("pyzbar"), site.join("cv2"), venv.join("Scripts")];
    let mut combined = OsString::new();
    for dir in dirs {
        if !combined.is_empty() {
            combined.push(";");
        }
        combined.push(dir.as_os_str());
    }
    if let Some(path) = std::env::var_os("PATH") {
        if !path.is_empty() {
            combined.push(";");
            combined.push(path);
        }
    }
    combined
}
