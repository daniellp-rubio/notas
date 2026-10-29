//! "Iniciar con Windows": launch the app when the user logs in.
//!
//! The state lives in the operating system (the `Run` value that
//! `tauri-plugin-autostart` manages under `HKCU\Software\Microsoft\Windows\
//! CurrentVersion\Run`), never in our own settings: every decision reads the
//! real state from the plugin, so the tray check and the UI can never drift
//! from what Windows will actually do.
//!
//! The first run enables it once and records that it did, so a later opt-out is
//! never overridden. Failures are logged and the app keeps running: a registry
//! that cannot be written must not take the notes down with it.

use rusqlite::Connection;
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

use crate::db;

/// `meta` key: the first-run default was already applied. Written only after a
/// successful enable, so a failed attempt is retried on the next start.
const INITIALIZED_KEY: &str = "autostart_initialized";

/// What Windows reports right now.
pub fn is_enabled(app: &AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|error| error.to_string())
}

/// Enable or disable autostart and return the state the OS reports afterwards,
/// which is what the caller must show: an error only means "nothing changed".
pub fn set_enabled(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    let changed = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(error) = changed {
        eprintln!("[notas] no se pudo cambiar el inicio con Windows: {error}");
    }
    manager.is_enabled().map_err(|error| error.to_string())
}

/// Apply the first-run default once. It never overrides a later user choice.
pub fn ensure_initial_default(app: &AppHandle, conn: &Connection) {
    match db::meta_get(conn, INITIALIZED_KEY) {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(error) => {
            eprintln!("[notas] no se pudo leer el estado del inicio con Windows: {error}");
            return;
        }
    }

    let enabled = match app.autolaunch().enable() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("[notas] no se pudo activar el inicio con Windows: {error}");
            // Already registered (for example after a data wipe) is success too.
            is_enabled(app).unwrap_or(false)
        }
    };

    if !enabled {
        return;
    }
    if let Err(error) = db::meta_set(conn, INITIALIZED_KEY, "1") {
        eprintln!("[notas] no se pudo guardar el estado del inicio con Windows: {error}");
    }
}
