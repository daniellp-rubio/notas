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

/// Apply a reminder that just came due to its row: mark it as fired and leave
/// the note visible.
///
/// A reminder is the app showing the note to the user, so the row must say so:
/// without this, the window is shown but `visible` stays 0 and the note never
/// comes back after a restart. Setting it before the window is raised also
/// fixes the case where the note had no window yet, since the new window is
/// built from the stored `visible` flag.
pub fn apply_fired(conn: &rusqlite::Connection, id: &str) -> Result<(), db::DbError> {
    db::mark_reminder_fired(conn, id)?;
    db::set_visible(conn, id, true)
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
            if let Err(error) = apply_fired(&conn, &id) {
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
    // `Default` asks Windows for the normal notification sound. Without a
    // sound the toast is built with `<audio silent="true"/>`, i.e. silent.
    let result = app
        .notification()
        .builder()
        .title("Recordatorio de Notas")
        .body(title)
        .sound("Default")
        .show();
    if let Err(error) = result {
        eprintln!("[notas] no se pudo mostrar la notificación: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firing_a_reminder_leaves_the_note_visible() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conn = db::open(&dir.path().join("notas.db")).expect("open");
        let mut note = db::Note::fresh("oculta".to_string(), 10.0, 10.0);
        note.visible = false;
        db::insert_note(&conn, &note).expect("insert");
        db::set_reminder(&conn, "oculta", Some(db::now_ms() - 1_000)).expect("reminder");
        assert!(!db::require(&conn, "oculta").expect("load").visible);

        let due = db::due_reminders(&conn, db::now_ms()).expect("due");
        assert_eq!(due, vec!["oculta".to_string()]);

        apply_fired(&conn, "oculta").expect("fire");
        let fired = db::require(&conn, "oculta").expect("load");
        assert!(fired.reminder_fired);
        assert!(
            fired.visible,
            "a note shown by its reminder must still be visible after a restart"
        );
        assert!(db::due_reminders(&conn, db::now_ms()).expect("due").is_empty());

        // The stored flag is what a restart reads: simulate it.
        drop(conn);
        let conn = db::open(&dir.path().join("notas.db")).expect("reopen");
        assert!(db::require(&conn, "oculta").expect("load").visible);
    }
}
