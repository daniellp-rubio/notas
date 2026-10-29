//! System tray icon: the app lives here when every note is hidden.

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;
use crate::{autostart, commands, windows};

pub const TRAY_ID: &str = "notas-tray";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let new_note = MenuItem::with_id(app, "new-note", "Nueva nota", true, None::<&str>)?;
    let show_all = MenuItem::with_id(app, "show-all", "Mostrar todas", true, None::<&str>)?;
    let hide_all = MenuItem::with_id(app, "hide-all", "Ocultar todas", true, None::<&str>)?;
    let list = MenuItem::with_id(app, "list", "Lista de notas", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Ajustes…", true, None::<&str>)?;
    // The check mirrors the OS state; a failure to read it shows as unchecked
    // instead of aborting the tray.
    let autostart_checked = autostart::is_enabled(app).unwrap_or_else(|error| {
        eprintln!("[notas] no se pudo leer el inicio con Windows: {error}");
        false
    });
    let autostart_item = CheckMenuItem::with_id(
        app,
        "autostart",
        "Iniciar con Windows",
        true,
        autostart_checked,
        None::<&str>,
    )?;
    app.state::<AppState>().set_autostart_item(autostart_item.clone());
    let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &new_note,
            &show_all,
            &hide_all,
            &list,
            &settings,
            &autostart_item,
            &separator,
            &quit,
        ],
    )?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Notas")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            let state = app.state::<AppState>();
            match id.as_str() {
                "new-note" => {
                    if let Err(error) = commands::create_note_impl(app, &state) {
                        eprintln!("[notas] no se pudo crear la nota: {error}");
                    }
                }
                "show-all" => {
                    if let Err(error) = windows::show_all(app, &state) {
                        eprintln!("[notas] no se pudieron mostrar las notas: {error}");
                    }
                }
                "hide-all" => {
                    if let Err(error) = windows::hide_all(app, &state) {
                        eprintln!("[notas] no se pudieron ocultar las notas: {error}");
                    }
                }
                "list" => windows::spawn_list(app),
                "settings" => windows::spawn_settings(app),
                "autostart" => {
                    let current = autostart::is_enabled(app).unwrap_or(false);
                    match autostart::set_enabled(app, !current) {
                        Ok(actual) => {
                            state.sync_autostart_item(actual);
                            let _ = app.emit("autostart-changed", actual);
                        }
                        Err(error) => {
                            eprintln!("[notas] no se pudo cambiar «Iniciar con Windows»: {error}")
                        }
                    }
                }
                "quit" => commands::quit_impl(app, &state),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                let state = app.state::<AppState>();
                if let Err(error) = windows::show_all(app, &state) {
                    eprintln!("[notas] no se pudieron mostrar las notas: {error}");
                }
            }
        })
        .build(app)?;

    Ok(())
}
