mod barcode;
mod bom;
mod bom_sql;
mod camera;
mod catalog;
mod catalogue;
mod detect;
mod dinov2;
mod frame;
mod inference;
mod inference_ep;
mod local_config;
mod metrology;
mod results;
mod training;
mod vision;

use std::sync::{Arc, Mutex};

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(frame::FrameState {
                latest: Arc::new(Mutex::new(None)),
            });
            if let Ok(dir) = app.path().app_data_dir() {
                local_config::set_app_data_dir(dir);
            }
            // Optional. A missing file logs a warning and leaves YOLO detection alone.
            dinov2::load_on_startup();
            catalogue::load_on_startup();
            // A missing catalogue.json warns and leaves the match list empty.
            app.manage(catalog::CatalogStore::open());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            local_config::load_local_config,
            local_config::save_local_config,
            local_config::list_vision_models,
            local_config::set_active_model,
            local_config::update_model,
            bom::load_bom_json,
            bom::post_bom_api,
            bom::post_inventory_descriptions,
            bom::inspect_bom,
            barcode::decode_barcode,
            camera::list_cameras,
            camera::list_camera_modes,
            camera::capture_snapshot,
            camera::capture_preview,
            camera::set_camera_focus,
            camera::set_camera_autofocus,
            camera::set_camera_exposure,
            camera::set_camera_gain,
            training::default_training_folder,
            training::check_training_folder,
            training::pick_training_folder,
            training::suggest_training_filename,
            training::save_training_image,
            training::save_detect_labels,
            detect::detect_image,
            detect::benchmark_detect,
            detect::model_class_names,
            dinov2::dinov2_status,
            catalogue::import_catalogue,
            catalogue::list_catalogue,
            catalogue::delete_catalogue_part,
            catalogue::preview_catalogue_crop,
            catalogue::save_catalogue_crop,
            catalog::match_box_dino,
            dinov2::update_references,
            dinov2::list_references,
            dinov2::set_reference_threshold,
            dinov2::delete_reference,
            dinov2::save_detection_references,
            dinov2::embed_marquee,
            dinov2::save_marquee_reference,
            dinov2::clear_marquee_reference,
            dinov2::release_preview_frame,
            inference_ep::inference_status,
            inference_ep::set_inference_ep,
            inference_ep::set_use_openvino,
            results::save_result,
            results::post_inspection_result,
            results::list_results,
            results::delete_result,
            results::result_image
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
