mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_default_project_path,
            commands::pick_project_directory,
            commands::get_model_settings,
            commands::save_model_settings,
            commands::test_model_connection,
            commands::get_graph,
            commands::get_trace,
            commands::get_doc,
            commands::get_eval,
            commands::ask,
            commands::index_project,
            commands::get_memories,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
