//! Global keyboard shortcuts registered with the operating system.
//!
//! The shortcuts work even when no note has the focus, which is why they are
//! registered through `tauri-plugin-global-shortcut` rather than in the
//! frontend. Registration can fail when another application already owns the
//! same combination: that is not fatal. The app keeps running, the failure is
//! logged and the settings window shows the shortcut as unavailable.

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::commands;
use crate::db;
use crate::state::AppState;
use crate::windows;

/// Open a new note, the same as the tray item "Nueva nota".
pub const NEW_NOTE: &str = "Ctrl+Alt+N";
/// Hide every note, or show them all again when none is on screen.
pub const TOGGLE_VISIBILITY: &str = "Ctrl+Alt+J";

/// What the settings window shows for one shortcut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShortcutInfo {
    pub id: String,
    pub label: String,
    pub accelerator: String,
    pub active: bool,
}

impl ShortcutInfo {
    fn pending(id: &str, label: &str, accelerator: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            accelerator: accelerator.to_string(),
            active: false,
        }
    }
}

/// Register both shortcuts and return their real status.
pub fn register(app: &AppHandle) -> Vec<ShortcutInfo> {
    let mut status = Vec::with_capacity(2);

    let new_note = ShortcutInfo::pending("new-note", "Nueva nota", NEW_NOTE);
    let toggle = ShortcutInfo::pending("toggle-notes", "Mostrar u ocultar las notas", TOGGLE_VISIBILITY);

    let new_note_result = app.global_shortcut().on_shortcut(
        NEW_NOTE,
        move |app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let state = app.state::<AppState>();
            if let Err(error) = commands::create_note_impl(app, &state) {
                eprintln!("[notas] no se pudo crear la nota con el atajo: {error}");
            }
        },
    );
    status.push(match new_note_result {
        Ok(()) => ShortcutInfo {
            active: true,
            ..new_note
        },
        Err(error) => {
            eprintln!("[notas] atajo {NEW_NOTE} no disponible: {error}");
            new_note
        }
    });

    let toggle_result = app.global_shortcut().on_shortcut(
        TOGGLE_VISIBILITY,
        move |app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let state = app.state::<AppState>();
            let any_visible = match state.conn() {
                Ok(conn) => db::list(&conn)
                    .map(|notes| notes.iter().any(|note| note.visible))
                    .unwrap_or(false),
                Err(_) => false,
            };
            let result = if any_visible {
                windows::hide_all(app, &state)
            } else {
                windows::show_all(app, &state)
            };
            if let Err(error) = result {
                eprintln!("[notas] no se pudieron alternar las notas: {error}");
            }
        },
    );
    status.push(match toggle_result {
        Ok(()) => ShortcutInfo {
            active: true,
            ..toggle
        },
        Err(error) => {
            eprintln!("[notas] atajo {TOGGLE_VISIBILITY} no disponible: {error}");
            toggle
        }
    });

    status
}
