//! Notas: sticky notes for the Windows desktop.
//!
//! The library owns every piece of state (SQLite + derived keys) and exposes it
//! through commands; the frontend is a thin view that never touches the disk.
mod attachments;
mod autostart;
mod backup;
mod commands;
mod crypto;
mod db;
mod reminders;
mod shortcuts;
mod state;
mod sync;
mod tray;
mod windows;

use tauri::{Emitter, Manager};

use state::AppState;

/// Name of the app in Windows: the `Run` value written for autostart and what
/// Task Manager shows at startup. Fixed so the registry entry never changes
/// with the package name.
pub const APP_NAME: &str = "Notas";

/// First run: leave one note on screen instead of an empty desktop.
fn seed_first_run(conn: &rusqlite::Connection) -> Result<(), db::DbError> {
    if db::meta_get(conn, "seeded")?.is_some() {
        return Ok(());
    }
    db::meta_set(conn, "seeded", "1")?;
    if db::count(conn)? == 0 {
        let mut note = db::Note::fresh(uuid::Uuid::new_v4().to_string(), 260.0, 190.0);
        note.width = 320.0;
        note.height = 280.0;
        let content = db::Content {
            text: db::plain_to_html(
                "Bienvenido a Notas\n\nArrastra la nota desde la barra de arriba.\nFija, colorea, etiqueta y pon recordatorios.\n\nLa primera línea es el título en la lista de notas.",
            ),
            items: Vec::new(),
            rich_text: true,
        };
        db::insert_note(conn, &note)?;
        db::save_plain_content(conn, &note.id, db::MODE_TEXT, &content)?;
    }
    Ok(())
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Registered here (not on the builder) so the plugin state exists before
    // the first-run default reads it.
    app.handle().plugin(
        tauri_plugin_autostart::Builder::new()
            .app_name(APP_NAME)
            .build(),
    )?;

    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let conn = db::open(&data_dir.join("notas.db"))?;
    seed_first_run(&conn)?;
    autostart::ensure_initial_default(app.handle(), &conn);
    app.manage(AppState::new(conn));

    tray::create(app.handle())?;
    {
        let state = app.state::<AppState>();
        windows::restore(app.handle(), &state).map_err(Box::<dyn std::error::Error>::from)?;
    }
    let status = shortcuts::register(app.handle());
    app.state::<AppState>().set_shortcuts(status);
    reminders::start(app.handle().clone());
    backup::start_scheduler(app.handle().clone());
    attachments::start(app.handle().clone());
    sync::start(app.handle().clone());
    Ok(())
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let state = app.state::<AppState>();
            if let Err(error) = windows::show_all(app, &state) {
                eprintln!("[notas] no se pudieron mostrar las notas: {error}");
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(setup)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let Some(id) = windows::note_id_from_label(window.label()).map(str::to_string)
                else {
                    return;
                };
                let state = window.state::<AppState>();
                if state.is_exiting() {
                    return;
                }
                // Closing a note only hides it: the app keeps living in the tray.
                api.prevent_close();
                let _ = window.hide();
                if let Ok(conn) = state.conn() {
                    let _ = db::set_visible(&conn, &id, false);
                }
                let _ = window.app_handle().emit("notes-changed", id);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_notes,
            commands::get_note,
            commands::create_note,
            commands::save_text,
            commands::save_items,
            commands::set_mode,
            commands::set_color,
            commands::set_pin,
            commands::save_geometry,
            commands::set_reminder,
            commands::set_visible,
            commands::delete_note,
            commands::add_tag,
            commands::remove_tag,
            commands::lock_note,
            commands::unlock_note,
            commands::relock_note,
            commands::remove_password,
            commands::open_list,
            commands::show_all,
            commands::hide_all,
            commands::autostart_enabled,
            commands::set_autostart,
            commands::open_settings,
            commands::settings_state,
            commands::choose_sync_folder,
            commands::disable_sync,
            commands::sync_now,
            commands::list_attachable_windows,
            commands::attach_note,
            commands::detach_note,
            commands::backup_now,
            commands::restore_backup,
            commands::open_backups_folder,
            commands::export_markdown,
            commands::export_json,
            commands::import_json,
            commands::quit_app,
        ]);

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("[notas] no se pudo iniciar la aplicación: {error}");
            std::process::exit(1);
        }
    };

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
            // `code == None` means "the last window closed"; keep running in the tray.
            if code.is_none() {
                api.prevent_exit();
            } else {
                app_handle.state::<AppState>().forget_key(None);
            }
        }
    });
}
