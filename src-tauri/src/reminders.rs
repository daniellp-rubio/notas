//! Background reminder loop.
//!
//! One thread checks the database every 15 seconds. A due reminder fires once:
//! the row is marked, a Windows notification is shown, the note window is
//! raised and the window is told to refresh its bell. Reminders that came due
//! while the app was closed are found by the first check after startup.

use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::db;
use crate::state::AppState;
use crate::windows;

pub const INTERVAL: Duration = Duration::from_secs(15);

/// Start the loop. It runs for the lifetime of the process.
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        tick(&app);
        loop {
            thread::sleep(INTERVAL);
            tick(&app);
        }
    });
}

fn tick(app: &AppHandle) {
    let state = app.state::<AppState>();
    let conn = match state.conn() {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("[notas] {error}");
            return;
        }
    };
    let due = match db::due_reminders(&conn, db::now_ms()) {
        Ok(ids) => ids,
        Err(error) => {
            eprintln!("[notas] no se pudieron leer los recordatorios: {error}");
            return;
        }
    };
    drop(conn);

    for id in due {
        {
            let state = app.state::<AppState>();
            let conn = match state.conn() {
                Ok(conn) => conn,
                Err(error) => {
                    eprintln!("[notas] {error}");
                    continue;
                }
            };
            if let Err(error) = db::mark_reminder_fired(&conn, &id) {
                eprintln!("[notas] no se pudo marcar el recordatorio {id}: {error}");
            }
        }

        let title = title_of(app, &id);
        notify(app, &title);

        // Window APIs dispatch to the event loop on their own; creation is
        // deferred inside `show_note`, so this is safe from this thread.
        if let Err(error) = windows::show_note(app, &id) {
            eprintln!("[notas] no se pudo mostrar la nota {id}: {error}");
        }
        let _ = app.emit_to(windows::note_label(&id), "reminder-fired", &id);
        let _ = app.emit("notes-changed", &id);
    }
}

/// Title shown in the notification. Locked notes never reveal their content.
fn title_of(app: &AppHandle, id: &str) -> String {
    let state = app.state::<AppState>();
    let note = match state.conn() {
        Ok(conn) => db::get(&conn, id).ok().flatten(),
        Err(_) => None,
    };
    match note {
        Some(note) if note.locked => "Nota bloqueada".to_string(),
        Some(note) => {
            let title = note.title();
            if title.is_empty() {
                "Nota sin título".to_string()
            } else {
                title
            }
        }
        None => "Nota sin título".to_string(),
    }
}

fn notify(app: &AppHandle, title: &str) {
    let result = app
        .notification()
        .builder()
        .title("Recordatorio de Notas")
        .body(title)
        .show();
    if let Err(error) = result {
        eprintln!("[notas] no se pudo mostrar la notificación: {error}");
    }
}
