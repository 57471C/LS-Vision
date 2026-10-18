mod barcode;
mod bom;
mod bom_sql;
mod camera;
mod detect;
mod local_config;

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
        .invoke_handler(tauri::generate_handler![
            greet,
            kill_vision_sidecar,
            local_config::load_local_config,
            local_config::save_local_config,
            local_config::list_vision_models,
            local_config::set_active_model,
            bom::load_bom_json,
            bom::post_bom_api,
            bom::inspect_bom,
            barcode::decode_barcode,
            camera::list_cameras,
            camera::list_camera_modes,
            camera::capture_snapshot,
            camera::capture_preview,
            detect::detect_image
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
#[tauri::command]
fn kill_vision_sidecar() -> Result<String, String> {
    let pids = vision_sidecar_pids()?;
    if pids.is_empty() {
        return Ok("Vision sidecar is not running.".to_string());
    }
    for pid in &pids {
        stop_process(*pid)?;
    }
    let listed = pids
        .iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!("Stopped vision sidecar (pid {listed})."))
}

const SIDECAR_PORT: u16 = 8008;

fn listening_pids_on(text: &str, port: u16) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        if !parts[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if !parts[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        let Some((_, port_text)) = parts[1].rsplit_once(':') else {
            continue;
        };
        let Ok(found) = port_text.parse::<u16>() else {
            continue;
        };
        if found != port {
            continue;
        }
        let Ok(pid) = parts[4].parse::<u32>() else {
            continue;
        };
        if pid > 0 && !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

fn is_sidecar_command(command_line: &str, listening: bool) -> bool {
    let line = command_line.to_ascii_lowercase();
    if !line.contains("main.py") {
        return false;
    }
    listening || line.contains("vision")
}

fn output_text(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes);
    if body.contains(&0) {
        let words = body
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16_lossy(&words);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(windows)]
fn hidden(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(windows)]
fn command_output(command: &mut std::process::Command) -> Result<std::process::Output, String> {
    hidden(command);
    command
        .output()
        .map_err(|err| format!("Could not inspect the vision sidecar: {err}"))
}

#[cfg(windows)]
fn listening_pids(port: u16) -> Result<Vec<u32>, String> {
    let output = command_output(std::process::Command::new("netstat").args(["-ano", "-p", "tcp"]))?;
    if !output.status.success() {
        let err = output_text(&output.stderr);
        return Err(format!("Could not inspect port {port}: {err}"));
    }
    Ok(listening_pids_on(&output_text(&output.stdout), port))
}

#[cfg(windows)]
fn process_command_line(pid: u32) -> Result<String, String> {
    let script = format!(
        "$OutputEncoding = [System.Text.Encoding]::UTF8; \
         [Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
         (Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\").CommandLine"
    );
    let output = command_output(std::process::Command::new("powershell.exe").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &script,
    ]))?;
    if !output.status.success() {
        let err = output_text(&output.stderr);
        return Err(format!("Could not read pid {pid}: {err}"));
    }
    Ok(output_text(&output.stdout))
}

#[cfg(windows)]
fn tcp_listener_pids(port: u16) -> Result<Vec<u32>, String> {
    const AF_INET: u32 = 2;
    const TCP_TABLE_OWNER_PID_LISTENER: i32 = 3;
    const NO_ERROR: u32 = 0;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    unsafe extern "system" {
        fn GetExtendedTcpTable(
            table: *mut u8,
            size: *mut u32,
            order: i32,
            family: u32,
            class: i32,
            reserved: u32,
        ) -> u32;
    }

    unsafe {
        let mut size = 0u32;
        let first = GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            AF_INET,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if first != NO_ERROR && first != ERROR_INSUFFICIENT_BUFFER {
            return Err(format!("Could not list TCP listeners ({first})"));
        }
        let mut buffer = vec![0u8; size as usize];
        let code = GetExtendedTcpTable(
            buffer.as_mut_ptr(),
            &mut size,
            0,
            AF_INET,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if code != NO_ERROR {
            return Err(format!("Could not list TCP listeners ({code})"));
        }
        if buffer.len() < 4 {
            return Ok(Vec::new());
        }
        let count = u32::from_le_bytes(buffer[0..4].try_into().unwrap()) as usize;
        let mut pids = Vec::new();
        for index in 0..count {
            let start = 4 + index * 24;
            if start + 24 > buffer.len() {
                break;
            }
            let local_port = u32::from_le_bytes(buffer[start + 8..start + 12].try_into().unwrap());
            let found = u16::from_be((local_port & 0xFFFF) as u16);
            if found != port {
                continue;
            }
            let pid = u32::from_le_bytes(buffer[start + 20..start + 24].try_into().unwrap());
            if pid > 0 && !pids.contains(&pid) {
                pids.push(pid);
            }
        }
        Ok(pids)
    }
}

#[cfg(windows)]
fn main_py_processes() -> Result<Vec<(u32, String)>, String> {
    let script = "\
$OutputEncoding = [System.Text.Encoding]::UTF8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-CimInstance Win32_Process |
  Where-Object { $_.CommandLine -and $_.CommandLine -match 'main\\.py' } |
  ForEach-Object { '{0}|{1}' -f $_.ProcessId, ($_.CommandLine -replace '\\|', '/') }
";
    let output = command_output(std::process::Command::new("powershell.exe").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]))?;
    if !output.status.success() {
        let err = output_text(&output.stderr);
        return Err(format!("Could not list Python processes: {err}"));
    }
    let mut found = Vec::new();
    for line in output_text(&output.stdout).lines() {
        let Some((pid_text, command)) = line.trim().split_once('|') else {
            continue;
        };
        let Ok(pid) = pid_text.trim().parse::<u32>() else {
            continue;
        };
        if pid > 0 {
            found.push((pid, command.to_string()));
        }
    }
    Ok(found)
}

#[cfg(windows)]
fn vision_sidecar_pids() -> Result<Vec<u32>, String> {
    let processes = main_py_processes()?;
    let listeners = match tcp_listener_pids(SIDECAR_PORT) {
        Ok(pids) => pids,
        Err(_) => listening_pids(SIDECAR_PORT).unwrap_or_default(),
    };
    let mut matched = Vec::new();
    let mut other = false;
    for pid in listeners {
        let known = processes
            .iter()
            .find(|(id, _)| *id == pid)
            .map(|(_, line)| line.clone());
        let line = match known {
            Some(line) => line,
            None => process_command_line(pid).unwrap_or_default(),
        };
        if is_sidecar_command(&line, true) {
            matched.push(pid);
        } else if !line.trim().is_empty() {
            other = true;
        }
    }
    for (pid, line) in &processes {
        if is_sidecar_command(line, false) && !matched.contains(pid) {
            matched.push(*pid);
        }
    }
    if matched.is_empty() && other {
        return Err("Port 8008 is open, but that process is not the vision sidecar.".to_string());
    }
    Ok(matched)
}

#[cfg(windows)]
fn stop_process(pid: u32) -> Result<(), String> {
    let output = command_output(std::process::Command::new("taskkill").args([
        "/F",
        "/T",
        "/PID",
        &pid.to_string(),
    ]))?;
    if output.status.success() {
        return Ok(());
    }
    let text = format!(
        "{}{}",
        output_text(&output.stderr),
        output_text(&output.stdout)
    );
    if text.to_ascii_lowercase().contains("not found") {
        return Ok(());
    }
    Err(format!("Could not stop pid {pid}: {text}"))
}

#[cfg(not(windows))]
fn vision_sidecar_pids() -> Result<Vec<u32>, String> {
    Err("Kill Vision Sidecar is only available on Windows.".to_string())
}

#[cfg(not(windows))]
fn stop_process(_pid: u32) -> Result<(), String> {
    Err("Kill Vision Sidecar is only available on Windows.".to_string())
}

#[cfg(test)]
mod tests {
    use super::{is_sidecar_command, listening_pids_on, output_text};

    #[test]
    fn netstat_keeps_only_the_sidecar_port() {
        let sample = "\
  TCP    127.0.0.1:8008         0.0.0.0:0              LISTENING       18432
  TCP    127.0.0.1:18008        0.0.0.0:0              LISTENING       99
  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       4
  TCP    [::1]:8008             [::]:0                 LISTENING       18432
  TCP    127.0.0.1:8008         127.0.0.1:54000        ESTABLISHED     18432
";
        assert_eq!(listening_pids_on(sample, 8008), vec![18432]);
    }

    #[test]
    fn command_line_must_be_the_vision_script() {
        assert!(is_sidecar_command(r"C:\Python\python.exe main.py", true));
        assert!(!is_sidecar_command(r"C:\Python\python.exe main.py", false));
        assert!(is_sidecar_command(
            r"C:\Scripts\LS-Vision\vision\main.py",
            false
        ));
    }

    #[test]
    fn powershell_utf16_command_line_decodes() {
        let bytes: Vec<u8> = "vision\\main.py"
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        assert_eq!(output_text(&bytes), "vision\\main.py");
    }
}
