mod commands;
mod json_index;

use commands::AppState;
use std::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(AppState {
            index: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_file,
            commands::get_children,
            commands::get_path,
            commands::search,
            commands::get_raw,
            commands::expand_to,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
