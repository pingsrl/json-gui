mod commands;
mod json_index;

use commands::AppState;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
#[cfg(target_os = "macos")]
use tauri::RunEvent;

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

            // ── Menu nativo ──────────────────────────────────────────────────
            let open_i         = MenuItem::with_id(app, "open",         "Apri...",                   true, Some("CmdOrCtrl+O"))?;
            let recent_i       = MenuItem::with_id(app, "recent",       "Recenti…",                  true, None::<&str>)?;
            let reload_i       = MenuItem::with_id(app, "reload",       "Ricarica",                  true, Some("CmdOrCtrl+R"))?;
            let check_update_i = MenuItem::with_id(app, "check-update", "Controlla aggiornamenti…",  true, None::<&str>)?;

            let file_menu = Submenu::with_items(app, "File", true, &[
                &open_i,
                &recent_i,
                &PredefinedMenuItem::separator(app)?,
                &reload_i,
            ])?;

            let edit_menu = Submenu::with_items(app, "Edit", true, &[
                &PredefinedMenuItem::undo(app, None)?,
                &PredefinedMenuItem::redo(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::cut(app, None)?,
                &PredefinedMenuItem::copy(app, None)?,
                &PredefinedMenuItem::paste(app, None)?,
                &PredefinedMenuItem::select_all(app, None)?,
            ])?;

            #[cfg(target_os = "macos")]
            let app_menu = Submenu::with_items(app, "JsonGUI", true, &[
                &PredefinedMenuItem::about(app, None, None)?,
                &check_update_i,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ])?;

            #[cfg(target_os = "macos")]
            let menu = Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu])?;
            #[cfg(not(target_os = "macos"))]
            let menu = Menu::with_items(app, &[&file_menu, &edit_menu])?;

            app.set_menu(menu)?;

            app.on_menu_event(|app, event| {
                match event.id().as_ref() {
                    "open"         => { app.emit("menu-open",         ()).ok(); }
                    "reload"       => { app.emit("menu-reload",       ()).ok(); }
                    "recent"       => { app.emit("menu-recent",       ()).ok(); }
                    "check-update" => { app.emit("menu-check-update", ()).ok(); }
                    _ => {}
                }
            });

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
            commands::open_from_string,
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
