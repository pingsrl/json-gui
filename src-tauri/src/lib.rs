mod commands;
pub mod json_index;
mod schema;

use commands::AppState;
use serde::{Deserialize, Serialize};
use sonic_rs;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;
#[cfg(target_os = "macos")]
use tauri::RunEvent;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedWindowState {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

fn window_state_path<R: tauri::Runtime>(window: &WebviewWindow<R>) -> Option<PathBuf> {
    let mut path = window.app_handle().path().app_config_dir().ok()?;
    if fs::create_dir_all(&path).is_err() {
        return None;
    }
    let label = window.label();
    let filename = if label == "main" {
        "window-state.json".to_string()
    } else {
        format!("window-state-{label}.json")
    };
    path.push(filename);
    Some(path)
}

fn load_window_state<R: tauri::Runtime>(window: &WebviewWindow<R>) -> Option<PersistedWindowState> {
    let path = window_state_path(window)?;
    let bytes = fs::read(path).ok()?;
    sonic_rs::from_slice(&bytes).ok()
}

fn capture_window_state<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
    persisted: Option<PersistedWindowState>,
) -> Option<PersistedWindowState> {
    let maximized = window.is_maximized().ok()?;
    let mut state = persisted.unwrap_or(PersistedWindowState {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        maximized,
    });

    if !maximized || state.width == 0 || state.height == 0 {
        let position = window.outer_position().ok()?;
        let size = window.outer_size().ok()?;
        if size.width == 0 || size.height == 0 {
            return None;
        }

        state.x = position.x;
        state.y = position.y;
        state.width = size.width;
        state.height = size.height;
    }

    state.maximized = maximized;
    Some(state)
}

fn save_window_state<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    let Some(path) = window_state_path(window) else {
        return;
    };
    let state = capture_window_state(window, load_window_state(window));
    let Some(state) = state else {
        return;
    };
    let Ok(bytes) = sonic_rs::to_vec(&state) else {
        return;
    };
    let _ = fs::write(path, bytes);
}

fn restore_window_state<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    let Some(state) = load_window_state(window) else {
        return;
    };
    if state.width == 0 || state.height == 0 {
        return;
    }

    let _ = window.set_size(PhysicalSize::new(state.width, state.height));
    let _ = window.set_position(PhysicalPosition::new(state.x, state.y));

    if state.maximized {
        let _ = window.maximize();
    }
}

fn setup_window_state_persistence<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    restore_window_state(window);

    let listener_window = window.clone();
    window.clone().on_window_event(move |event| match event {
        WindowEvent::Moved(_)
        | WindowEvent::Resized(_)
        | WindowEvent::CloseRequested { .. }
        | WindowEvent::Destroyed => save_window_state(&listener_window),
        _ => {}
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState {
            windows: RwLock::new(HashMap::new()),
            initial_path: std::sync::Mutex::new(None),
            pending_content: std::sync::Mutex::new(HashMap::new()),
            runtime_monitor: std::sync::Mutex::new(commands::RuntimeMonitor::new()),
        })
        .setup(|app| {
            // Windows/Linux: the file is passed as a CLI argument
            let args: Vec<String> = std::env::args().collect();
            if let Some(path) = args.get(1) {
                if path.ends_with(".json") {
                    *app.state::<AppState>().initial_path.lock().unwrap() = Some(path.clone());
                }
            }

            // ── Native menu ──────────────────────────────────────────────────
            let open_i = MenuItem::with_id(app, "open", "Apri...", true, Some("CmdOrCtrl+O"))?;
            let new_window_i = MenuItem::with_id(
                app,
                "new-window",
                "Nuova finestra",
                true,
                Some("CmdOrCtrl+N"),
            )?;
            let close_window_i = PredefinedMenuItem::close_window(app, None)?;
            let recent_i = MenuItem::with_id(app, "recent", "Recenti…", true, None::<&str>)?;
            let reload_i = MenuItem::with_id(app, "reload", "Ricarica", true, Some("CmdOrCtrl+R"))?;
            let check_update_i = MenuItem::with_id(
                app,
                "check-update",
                "Controlla aggiornamenti…",
                true,
                None::<&str>,
            )?;
            let export_i = MenuItem::with_id(
                app,
                "export",
                "Esporta tipo…",
                true,
                Some("CmdOrCtrl+Shift+E"),
            )?;

            let file_menu = Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &open_i,
                    &new_window_i,
                    &close_window_i,
                    &recent_i,
                    &PredefinedMenuItem::separator(app)?,
                    &reload_i,
                    &PredefinedMenuItem::separator(app)?,
                    &export_i,
                ],
            )?;

            let edit_menu = Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?;

            #[cfg(target_os = "macos")]
            let app_menu = Submenu::with_items(
                app,
                "JsonGUI",
                true,
                &[
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
                ],
            )?;

            #[cfg(target_os = "macos")]
            let menu = Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu])?;
            #[cfg(not(target_os = "macos"))]
            let menu = Menu::with_items(app, &[&file_menu, &edit_menu])?;

            app.set_menu(menu)?;

            app.on_menu_event(|app, event| match event.id().as_ref() {
                "open" => {
                    app.emit("menu-open", ()).ok();
                }
                "reload" => {
                    app.emit("menu-reload", ()).ok();
                }
                "recent" => {
                    app.emit("menu-recent", ()).ok();
                }
                "check-update" => {
                    app.emit("menu-check-update", ()).ok();
                }
                "export" => {
                    app.emit("menu-export", ()).ok();
                }
                "new-window" => {
                    let label = format!(
                        "w{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                    );
                    let _ = tauri::WebviewWindowBuilder::new(
                        app,
                        &label,
                        tauri::WebviewUrl::App("index.html".into()),
                    )
                    .title("JsonGUI")
                    .inner_size(1200.0, 800.0)
                    .min_inner_size(600.0, 400.0)
                    .build()
                    .map(|new_window| {
                        setup_window_state_persistence(&new_window);
                        let app_for_destroy = app.clone();
                        let lbl = label.clone();
                        new_window.on_window_event(move |event| {
                            if let WindowEvent::Destroyed = event {
                                app_for_destroy.state::<AppState>().remove_window(&lbl);
                            }
                        });
                    });
                }
                _ => {}
            });

            if let Some(main_window) = app
                .get_webview_window("main")
                .or_else(|| app.webview_windows().into_values().next())
            {
                setup_window_state_persistence(&main_window);
                let app_handle_for_destroy = app.handle().clone();
                let main_label = main_window.label().to_string();
                main_window.on_window_event(move |event| {
                    if let WindowEvent::Destroyed = event {
                        app_handle_for_destroy
                            .state::<AppState>()
                            .remove_window(&main_label);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_file,
            commands::get_children,
            commands::get_children_page,
            commands::get_runtime_stats,
            commands::expand_subtree,
            commands::expand_subtree_streaming,
            commands::get_path,
            commands::search,
            commands::search_objects,
            commands::suggest_property_paths,
            commands::get_raw,
            commands::open_in_new_window,
            commands::get_pending_content,
            commands::expand_to,
            commands::get_expanded_slice,
            commands::get_initial_path,
            commands::open_from_string,
            commands::export_types,
            commands::take_screenshot
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // macOS: file opened via Finder / "Open with"
            #[cfg(target_os = "macos")]
            if let RunEvent::Opened { urls } = event {
                for url in urls {
                    if let Ok(path) = url.to_file_path() {
                        if let Some(path_str) = path.to_str() {
                            // Emit for already-open windows (app already running)
                            let _ = app.emit("open-with", path_str.to_string());
                            // Also store in initial_path: if the webview was not yet
                            // ready for the emit, the frontend retrieves it via get_initial_path.
                            let state = app.state::<AppState>();
                            let mut guard = state.initial_path.lock().unwrap();
                            if guard.is_none() {
                                *guard = Some(path_str.to_string());
                            }
                        }
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
