mod barcode;
mod bom;
mod bom_sql;
mod camera;
mod detect;
mod inference_ep;
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
        .invoke_handler(tauri::generate_handler![
            greet,
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
            detect::detect_image,
            detect::benchmark_detect,
            inference_ep::inference_status,
            inference_ep::set_inference_ep
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
