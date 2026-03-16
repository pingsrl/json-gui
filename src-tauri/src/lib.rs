mod commands;
mod json_index;

use commands::AppState;
use std::sync::Mutex;
use tauri::Emitter;
#[cfg(target_os = "macos")]
use tauri::{Manager, RunEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState {
            index: Mutex::new(None),
            initial_path: Mutex::new(None),
        })
        .setup(|app| {
            // Windows/Linux: il file viene passato come argomento CLI
            let args: Vec<String> = std::env::args().collect();
            if let Some(path) = args.get(1) {
                if path.ends_with(".json") {
                    *app.state::<AppState>().initial_path.lock().unwrap() = Some(path.clone());
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_file,
            commands::get_children,
            commands::get_path,
            commands::search,
            commands::get_raw,
            commands::expand_to,
            commands::get_initial_path,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // macOS: file aperto via Finder / "Apri con"
            #[cfg(target_os = "macos")]
            if let RunEvent::Opened { urls } = event {
                for url in urls {
                    if let Ok(path) = url.to_file_path() {
                        if let Some(path_str) = path.to_str() {
                            let _ = app.emit("open-with", path_str.to_string());
                        }
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
