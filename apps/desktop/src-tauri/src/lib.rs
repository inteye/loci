mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::get_default_project_path,
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
