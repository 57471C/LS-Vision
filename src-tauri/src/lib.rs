mod barcode;
mod bom;
mod bom_sql;
mod camera;
mod detect;
mod inference_ep;
mod local_config;
mod results;
mod training;

use tauri::Manager;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Ok(dir) = app.path().app_data_dir() {
                local_config::set_app_data_dir(dir);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            local_config::load_local_config,
            local_config::save_local_config,
            local_config::list_vision_models,
            local_config::set_active_model,
            local_config::update_model,
            bom::load_bom_json,
            bom::post_bom_api,
            bom::inspect_bom,
            barcode::decode_barcode,
            camera::list_cameras,
            camera::list_camera_modes,
            camera::capture_snapshot,
            camera::capture_preview,
            camera::set_camera_focus,
            camera::set_camera_autofocus,
            training::default_training_folder,
            training::check_training_folder,
            training::pick_training_folder,
            training::suggest_training_filename,
            training::save_training_image,
            detect::detect_image,
            detect::benchmark_detect,
            detect::model_class_names,
            inference_ep::inference_status,
            inference_ep::set_inference_ep,
            results::save_result,
            results::list_results,
            results::delete_result,
            results::result_image
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
