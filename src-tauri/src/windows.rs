//! Window creation and visibility for note windows and the notes list.
//!
//! Every note is its own frameless window labelled `note-<id>`; the manager
//! window is labelled `list`. Both load the same frontend.
//!
//! Creating a WebView from inside an IPC handler deadlocks on Windows (the
//! controller creation needs the event loop, which is blocked by the command),
//! so every creation goes through `spawn_note` / `spawn_list`: they queue the
//! work on the event loop and return at once.

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::db::{self, Note};
use crate::state::AppState;

pub const LIST_LABEL: &str = "list";
pub const MIN_WIDTH: f64 = 200.0;
pub const MIN_HEIGHT: f64 = 160.0;

pub fn note_label(id: &str) -> String {
    format!("note-{id}")
}

pub fn note_id_from_label(label: &str) -> Option<&str> {
    label.strip_prefix("note-")
}

/// Turn stored geometry into something a window can actually use.
fn geometry(note: &Note) -> (f64, f64, f64, f64) {
    (
        note.x,
        note.y,
        note.width.max(MIN_WIDTH),
        note.height.max(MIN_HEIGHT),
    )
}

/// Create the window of a note (no-op when it already exists).
pub fn open_note(app: &AppHandle, note: &Note) -> tauri::Result<()> {
    let label = note_label(&note.id);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let (x, y, width, height) = geometry(note);
    let url = format!("index.html?note={}", note.id);
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
        .title("Notas")
        .decorations(false)
        .skip_taskbar(true)
        .resizable(true)
        // No shadow: the window rectangle is exactly the paper, so the saved
        // position and size are exactly what the user sees.
        .shadow(false)
        .always_on_top(note.always_on_top)
        .position(x, y)
        .inner_size(width, height)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .visible(note.visible)
        .build()?;
    // `ITaskbarList::DeleteTab` is undone when Explorer adds the button back.
    let _ = window.set_skip_taskbar(true);
    Ok(())
}

/// Create the manager window (raises it when it already exists).
pub fn open_list(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LIST_LABEL) {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        LIST_LABEL,
        WebviewUrl::App("index.html?view=list".into()),
    )
    .title("Lista de notas")
    .inner_size(820.0, 580.0)
    .min_inner_size(560.0, 360.0)
    .center()
    .build()?;
    Ok(())
}

/// Open a note window on the event loop (safe from any thread and any handler).
pub fn spawn_note(app: &AppHandle, note_id: &str) {
    let handle = app.clone();
    let id = note_id.to_string();
    if let Err(error) = app.run_on_main_thread(move || {
        let state = handle.state::<AppState>();
        let note = match state.conn() {
            Ok(conn) => db::get(&conn, &id).ok().flatten(),
            Err(_) => None,
        };
        let Some(note) = note else {
            return;
        };
        if let Err(error) = open_note(&handle, &note) {
            eprintln!("[notas] no se pudo abrir la ventana de la nota {id}: {error}");
        }
    }) {
        eprintln!("[notas] no se pudo encolar la ventana de la nota {note_id}: {error}");
    }
}

/// Open the manager window on the event loop.
pub fn spawn_list(app: &AppHandle) {
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if let Err(error) = open_list(&handle) {
            eprintln!("[notas] no se pudo abrir la lista de notas: {error}");
        }
    }) {
        eprintln!("[notas] no se pudo encolar la lista de notas: {error}");
    }
}

/// Show a note window, creating it later when it does not exist yet.
pub fn show_note(app: &AppHandle, id: &str) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(&note_label(id)) {
        window.show().map_err(|e| e.to_string())?;
        let _ = window.unminimize();
        let _ = window.set_skip_taskbar(true);
        window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    spawn_note(app, id);
    Ok(())
}

/// Show every note (the manager window is left alone).
pub fn show_all(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let notes = {
        let conn = state.conn()?;
        db::list(&conn).map_err(|e| e.to_string())?
    };
    for note in notes {
        {
            let conn = state.conn()?;
            db::set_visible(&conn, &note.id, true).map_err(|e| e.to_string())?;
        }
        show_note(app, &note.id)?;
    }
    let _ = app.emit("notes-changed", "");
    Ok(())
}

/// Hide every note window and remember that they are hidden.
pub fn hide_all(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let notes = {
        let conn = state.conn()?;
        db::list(&conn).map_err(|e| e.to_string())?
    };
    for note in notes {
        {
            let conn = state.conn()?;
            db::set_visible(&conn, &note.id, false).map_err(|e| e.to_string())?;
        }
        if let Some(window) = app.get_webview_window(&note_label(&note.id)) {
            let _ = window.hide();
        }
    }
    let _ = app.emit("notes-changed", "");
    Ok(())
}

/// Reopen every note that was visible when the app was last running.
///
/// Runs from `setup`, before the event loop starts, where creating a webview
/// inline is safe.
pub fn restore(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let notes = {
        let conn = state.conn()?;
        db::list(&conn).map_err(|e| e.to_string())?
    };
    for note in notes.iter().filter(|note| note.visible) {
        open_note(app, note).map_err(|e| e.to_string())?;
    }
    Ok(())
}
