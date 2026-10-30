//! Window creation and visibility for note windows and the notes list.
//!
//! Every note is its own frameless window labelled `note-<id>`; the manager
//! window is labelled `list`. Both load the same frontend.
//!
//! Creating a WebView from inside an IPC handler deadlocks on Windows (the
//! WebView2 controller creation waits on the very message loop that is busy
//! running the handler), so every creation goes through `spawn_note` /
//! `spawn_list`: they hop to a worker thread and only then queue the creation
//! on the event loop.
//!
//! The worker hop is not decoration. `AppHandle::run_on_main_thread` calls
//! `tauri_runtime_wry::Context::send_user_message`, which runs the closure
//! inline when the caller already is the main thread; called from a regular
//! command it would build the window inside the handler and deadlock again.
//! From a worker thread the same call goes through the event loop proxy, so the
//! creation becomes a normal queued event.

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::db::{self, Note};
use crate::state::AppState;

pub const LIST_LABEL: &str = "list";
pub const SETTINGS_LABEL: &str = "settings";
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

/// Create the settings window (raises it when it already exists). It is a
/// normal window, with a frame and a taskbar button, unlike the notes.
pub fn open_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        SETTINGS_LABEL,
        WebviewUrl::App("index.html?view=settings".into()),
    )
    .title("Ajustes")
    .inner_size(620.0, 720.0)
    .min_inner_size(480.0, 520.0)
    .center()
    .build()?;
    Ok(())
}

/// Run window work on a worker thread, where `run_on_main_thread` really defers.
fn worker<F: FnOnce() + Send + 'static>(name: &str, job: F) {
    if let Err(error) = std::thread::Builder::new().name(name.to_string()).spawn(job) {
        eprintln!("[notas] no se pudo lanzar el hilo de ventanas: {error}");
    }
}

/// Open a note window on the event loop (safe from any thread and any handler).
pub fn spawn_note(app: &AppHandle, note_id: &str) {
    let handle = app.clone();
    let id = note_id.to_string();
    worker("notas-note-window", move || {
        // Read the row off the main thread: the event loop only builds windows.
        let note = {
            let state = handle.state::<AppState>();
            let found = match state.conn() {
                Ok(conn) => db::get(&conn, &id).ok().flatten(),
                Err(_) => None,
            };
            found
        };
        let Some(note) = note else {
            return;
        };
        let app = handle.clone();
        if let Err(error) = handle.run_on_main_thread(move || {
            if let Err(error) = open_note(&app, &note) {
                eprintln!(
                    "[notas] no se pudo abrir la ventana de la nota {}: {error}",
                    note.id
                );
            }
        }) {
            eprintln!("[notas] no se pudo encolar la ventana de la nota {id}: {error}");
        }
    });
}

/// Open the manager window on the event loop.
pub fn spawn_list(app: &AppHandle) {
    let handle = app.clone();
    worker("notas-list-window", move || {
        let app = handle.clone();
        if let Err(error) = handle.run_on_main_thread(move || {
            if let Err(error) = open_list(&app) {
                eprintln!("[notas] no se pudo abrir la lista de notas: {error}");
            }
        }) {
            eprintln!("[notas] no se pudo encolar la lista de notas: {error}");
        }
    });
}

/// Open the settings window on the event loop.
pub fn spawn_settings(app: &AppHandle) {
    let handle = app.clone();
    worker("notas-settings-window", move || {
        let app = handle.clone();
        if let Err(error) = handle.run_on_main_thread(move || {
            if let Err(error) = open_settings(&app) {
                eprintln!("[notas] no se pudo abrir los ajustes: {error}");
            }
        }) {
            eprintln!("[notas] no se pudo encolar la ventana de ajustes: {error}");
        }
    });
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

/// Show an attachment-controlled note without stealing focus from the matched
/// application. Creating a missing webview still uses the safe worker hop.
pub fn show_note_quietly(app: &AppHandle, id: &str) {
    if let Some(window) = app.get_webview_window(&note_label(id)) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_skip_taskbar(true);
    } else {
        spawn_note(app, id);
    }
}

/// Show every note (the manager window is left alone). Attachment-controlled
/// notes keep following the foreground rule instead of being forced visible.
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
        if note.attach_kind.is_none() {
            show_note(app, &note.id)?;
        }
    }
    crate::attachments::refresh(app);
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

/// Rebuild every note window from what the database says right now.
///
/// Used after restoring a backup: the rows the windows were built from are
/// gone, so the old windows are destroyed and the visible notes opened again
/// through `spawn_note`, which is the only safe way to create a webview from
/// outside the setup phase.
pub fn reload_all(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let labels: Vec<String> = app.webview_windows().into_keys().collect();
    for label in labels {
        if note_id_from_label(&label).is_some() {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.destroy();
            }
        }
    }
    let notes = {
        let conn = state.conn()?;
        db::list(&conn).map_err(|error| error.to_string())?
    };
    for note in notes
        .iter()
        .filter(|note| note.visible && note.attach_kind.is_none())
    {
        spawn_note(app, &note.id);
    }
    crate::attachments::refresh(app);
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
    for note in notes
        .iter()
        .filter(|note| note.visible && note.attach_kind.is_none())
    {
        open_note(app, note).map_err(|e| e.to_string())?;
    }
    Ok(())
}
