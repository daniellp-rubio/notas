//! Tauri commands: the only surface the frontend is allowed to talk to.
//!
//! Every command returns `Result<_, String>` so the UI can show a message
//! instead of the process dying on a bad path or a corrupted row.

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::crypto::{self, NONCE_LEN, SALT_LEN};
use crate::db::{self, Content, Item, Note, COLORS, MODE_LIST, MODE_TEXT};
use crate::state::AppState;
use crate::windows;

const NOT_FOUND: &str = "La nota no existe";

/// What the frontend receives for one note.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NoteView {
    pub id: String,
    pub color: String,
    pub mode: String,
    pub title: String,
    pub text: String,
    pub items: Vec<Item>,
    pub tags: Vec<String>,
    pub visible: bool,
    pub locked: bool,
    pub unlocked: bool,
    pub always_on_top: bool,
    pub reminder_at: Option<i64>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

fn err<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
}

fn bad(message: &str) -> String {
    message.to_string()
}

fn title_of(text: &str, items: &[Item], mode: &str) -> String {
    let first = if mode == MODE_LIST {
        items
            .iter()
            .find(|item| !item.text.trim().is_empty())
            .map(|item| item.text.clone())
            .unwrap_or_default()
    } else {
        text.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .to_string()
    };
    first.trim().chars().take(80).collect()
}

fn find(conn: &Connection, id: &str) -> Result<Note, String> {
    db::get(conn, id).map_err(err)?.ok_or_else(|| bad(NOT_FOUND))
}

/// Decrypt the payload of a note that has a key in memory.
fn decrypt(state: &AppState, note: &Note) -> Result<Content, String> {
    let key = state
        .key(&note.id)
        .ok_or_else(|| bad("La nota está bloqueada"))?;
    let cipher = note
        .body_cipher
        .as_deref()
        .ok_or_else(|| bad("La nota no tiene contenido cifrado"))?;
    let salt = note.salt.as_deref().ok_or_else(|| bad("Falta la sal"))?;
    let nonce = note.nonce.as_deref().ok_or_else(|| bad("Falta el nonce"))?;
    if salt.len() != SALT_LEN {
        return Err(bad("Los datos cifrados están dañados"));
    }
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| bad("Los datos cifrados están dañados"))?;
    let plain = crypto::open(&key, nonce, cipher).map_err(|_| bad("No se pudo descifrar la nota"))?;
    serde_json::from_slice(&plain).map_err(|_| bad("El contenido de la nota está dañado"))
}

fn load_content(state: &AppState, note: &Note) -> Result<Content, String> {
    if note.locked {
        decrypt(state, note)
    } else {
        Ok(note.content())
    }
}

/// Persist content, encrypting it when the note has a password.
fn store_content(
    conn: &Connection,
    state: &AppState,
    note: &Note,
    mode: &str,
    content: &Content,
) -> Result<(), String> {
    if note.locked {
        let key = state
            .key(&note.id)
            .ok_or_else(|| bad("La nota está bloqueada"))?;
        let salt: [u8; SALT_LEN] = note
            .salt
            .as_deref()
            .ok_or_else(|| bad("Falta la sal"))?
            .try_into()
            .map_err(|_| bad("Los datos cifrados están dañados"))?;
        let payload = serde_json::to_vec(content).map_err(err)?;
        let sealed = crypto::seal(&key, salt, &payload).map_err(err)?;
        db::save_cipher(conn, &note.id, &sealed.cipher, &sealed.salt, &sealed.nonce).map_err(err)?;
        db::set_mode(conn, &note.id, mode).map_err(err)?;
    } else {
        db::save_plain_content(conn, &note.id, mode, content).map_err(err)?;
    }
    Ok(())
}

fn view(state: &AppState, note: &Note, tags: Vec<String>) -> Result<NoteView, String> {
    let unlocked = note.locked && state.has_key(&note.id);
    let (text, items) = if note.locked {
        if unlocked {
            let content = decrypt(state, note)?;
            (content.text, content.items)
        } else {
            (String::new(), Vec::new())
        }
    } else {
        let content = note.content();
        (content.text, content.items)
    };
    let title = if note.locked && !unlocked {
        String::new()
    } else {
        title_of(&text, &items, &note.mode)
    };
    Ok(NoteView {
        id: note.id.clone(),
        color: note.color.clone(),
        mode: note.mode.clone(),
        title,
        text,
        items,
        tags,
        visible: note.visible,
        locked: note.locked,
        unlocked,
        always_on_top: note.always_on_top,
        reminder_at: note.reminder_at,
        x: note.x,
        y: note.y,
        width: note.width,
        height: note.height,
    })
}

/// Create a note and open its window. Shared with the tray menu.
pub fn create_note_impl(app: &AppHandle, state: &AppState) -> Result<NoteView, String> {
    let created = {
        let conn = state.conn()?;
        let total = db::count(&conn).map_err(err)? as f64;
        let step = (total.rem_euclid(10.0)) * 26.0;
        let note = Note::fresh(uuid::Uuid::new_v4().to_string(), 180.0 + step, 150.0 + step);
        db::insert_note(&conn, &note).map_err(err)?;
        note
    };
    windows::spawn_note(app, &created.id);
    let _ = app.emit("notes-changed", &created.id);
    view(state, &created, Vec::new())
}

/// Quit the app on purpose (the tray "Salir" item).
pub fn quit_impl(app: &AppHandle, state: &AppState) {
    state.set_exiting();
    app.exit(0);
}

#[tauri::command]
pub fn list_notes(state: State<'_, AppState>) -> Result<Vec<NoteView>, String> {
    let conn = state.conn()?;
    let notes = db::list(&conn).map_err(err)?;
    let mut tags = db::all_tags(&conn).map_err(err)?;
    let mut out = Vec::with_capacity(notes.len());
    for note in &notes {
        out.push(view(&state, note, tags.remove(&note.id).unwrap_or_default())?);
    }
    Ok(out)
}

#[tauri::command]
pub fn get_note(state: State<'_, AppState>, id: String) -> Result<NoteView, String> {
    let conn = state.conn()?;
    let note = find(&conn, &id)?;
    let tags = db::note_tags(&conn, &id).map_err(err)?;
    view(&state, &note, tags)
}

#[tauri::command]
pub fn create_note(app: AppHandle, state: State<'_, AppState>) -> Result<NoteView, String> {
    create_note_impl(&app, &state)
}

#[tauri::command]
pub fn save_text(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    text: String,
) -> Result<(), String> {
    {
        let conn = state.conn()?;
        let note = find(&conn, &id)?;
        if note.locked && !state.has_key(&id) {
            return Err(bad("La nota está bloqueada"));
        }
        let mut content = load_content(&state, &note)?;
        content.text = text;
        store_content(&conn, &state, &note, &note.mode, &content)?;
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn save_items(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    items: Vec<Item>,
) -> Result<(), String> {
    {
        let conn = state.conn()?;
        let note = find(&conn, &id)?;
        if note.locked && !state.has_key(&id) {
            return Err(bad("La nota está bloqueada"));
        }
        let mut content = load_content(&state, &note)?;
        content.items = items;
        store_content(&conn, &state, &note, &note.mode, &content)?;
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn set_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    mode: String,
) -> Result<NoteView, String> {
    if mode != MODE_TEXT && mode != MODE_LIST {
        return Err(bad("Modo desconocido"));
    }
    let view = {
        let conn = state.conn()?;
        let note = find(&conn, &id)?;
        if note.mode == mode {
            let tags = db::note_tags(&conn, &id).map_err(err)?;
            view(&state, &note, tags)?
        } else {
            let mut content = load_content(&state, &note)?;
            if mode == MODE_LIST {
                content.convert_to_list();
            } else {
                content.convert_to_text();
            }
            store_content(&conn, &state, &note, &mode, &content)?;
            // Read the row back: the columns changed, the struct did not.
            let stored = find(&conn, &id)?;
            let tags = db::note_tags(&conn, &id).map_err(err)?;
            view(&state, &stored, tags)?
        }
    };
    let _ = app.emit("notes-changed", &id);
    Ok(view)
}

#[tauri::command]
pub fn set_color(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    color: String,
) -> Result<(), String> {
    if !COLORS.contains(&color.as_str()) {
        return Err(bad("Color desconocido"));
    }
    {
        let conn = state.conn()?;
        db::set_color(&conn, &id, &color).map_err(err)?;
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn set_pin(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    pin: bool,
) -> Result<(), String> {
    {
        let conn = state.conn()?;
        db::set_pin(&conn, &id, pin).map_err(err)?;
    }
    if let Some(window) = app.get_webview_window(&windows::note_label(&id)) {
        window
            .set_always_on_top(pin)
            .map_err(|error| bad(&error.to_string()))?;
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn save_geometry(
    state: State<'_, AppState>,
    id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let finite = [x, y, width, height].iter().all(|value| value.is_finite());
    if !finite {
        return Err(bad("Geometría inválida"));
    }
    let width = width.clamp(windows::MIN_WIDTH, 4000.0);
    let height = height.clamp(windows::MIN_HEIGHT, 4000.0);
    let x = x.clamp(-20_000.0, 20_000.0);
    let y = y.clamp(-20_000.0, 20_000.0);
    let conn = state.conn()?;
    db::set_geometry(&conn, &id, x, y, width, height).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn set_reminder(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    at: Option<i64>,
) -> Result<(), String> {
    if let Some(value) = at {
        if value <= 0 {
            return Err(bad("Fecha de recordatorio inválida"));
        }
    }
    {
        let conn = state.conn()?;
        db::set_reminder(&conn, &id, at).map_err(err)?;
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn set_visible(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    visible: bool,
) -> Result<(), String> {
    {
        let conn = state.conn()?;
        db::set_visible(&conn, &id, visible).map_err(err)?;
    }
    if visible {
        windows::show_note(&app, &id)?;
    } else if let Some(window) = app.get_webview_window(&windows::note_label(&id)) {
        window.hide().map_err(err)?;
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn delete_note(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    {
        let conn = state.conn()?;
        db::delete_note(&conn, &id).map_err(err)?;
    }
    state.forget_key(Some(&id));
    if let Some(window) = app.get_webview_window(&windows::note_label(&id)) {
        window.destroy().map_err(err)?;
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn add_tag(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    tag: String,
) -> Result<Vec<String>, String> {
    let tags = {
        let conn = state.conn()?;
        db::add_tag(&conn, &id, &tag).map_err(err)?
    };
    let _ = app.emit("notes-changed", &id);
    Ok(tags)
}

#[tauri::command]
pub fn remove_tag(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    tag: String,
) -> Result<Vec<String>, String> {
    let tags = {
        let conn = state.conn()?;
        db::remove_tag(&conn, &id, &tag).map_err(err)?
    };
    let _ = app.emit("notes-changed", &id);
    Ok(tags)
}

/// Protect a note with a password. The note stays editable in this session;
/// every later save is encrypted again with the in-memory key.
#[tauri::command]
pub fn lock_note(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    password: String,
) -> Result<(), String> {
    if password.trim().len() < 4 {
        return Err(bad("La contraseña debe tener al menos 4 caracteres"));
    }
    {
        let conn = state.conn()?;
        let note = find(&conn, &id)?;
        if note.locked {
            return Err(bad("La nota ya tiene contraseña"));
        }
        let payload = serde_json::to_vec(&note.content()).map_err(err)?;
        let (key, sealed) = crypto::seal_with_password(&password, &payload).map_err(err)?;
        db::save_cipher(&conn, &id, &sealed.cipher, &sealed.salt, &sealed.nonce).map_err(err)?;
        state.remember_key(&id, key);
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn unlock_note(
    state: State<'_, AppState>,
    id: String,
    password: String,
) -> Result<NoteView, String> {
    let conn = state.conn()?;
    let note = find(&conn, &id)?;
    let tags = db::note_tags(&conn, &id).map_err(err)?;
    if !note.locked {
        return view(&state, &note, tags);
    }
    let cipher = note
        .body_cipher
        .as_deref()
        .ok_or_else(|| bad("La nota no tiene contenido cifrado"))?;
    let salt = note.salt.as_deref().ok_or_else(|| bad("Falta la sal"))?;
    let nonce = note.nonce.as_deref().ok_or_else(|| bad("Falta el nonce"))?;
    let (key, plain) = crypto::open_with_password(&password, cipher, salt, nonce)
        .map_err(|_| bad("Contraseña incorrecta"))?;
    let content: Content =
        serde_json::from_slice(&plain).map_err(|_| bad("El contenido de la nota está dañado"))?;
    state.remember_key(&id, key);
    let mut note_view = view(&state, &note, tags)?;
    note_view.title = title_of(&content.text, &content.items, &note.mode);
    note_view.text = content.text;
    note_view.items = content.items;
    Ok(note_view)
}

/// Forget the key of a note: it goes back to the locked screen.
#[tauri::command]
pub fn relock_note(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.forget_key(Some(&id));
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

/// Remove the password of a note: the content goes back to plaintext on disk.
#[tauri::command]
pub fn remove_password(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    password: String,
) -> Result<(), String> {
    {
        let conn = state.conn()?;
        let note = find(&conn, &id)?;
        if !note.locked {
            return Err(bad("La nota no tiene contraseña"));
        }
        let cipher = note
            .body_cipher
            .as_deref()
            .ok_or_else(|| bad("La nota no tiene contenido cifrado"))?;
        let salt = note.salt.as_deref().ok_or_else(|| bad("Falta la sal"))?;
        let nonce = note.nonce.as_deref().ok_or_else(|| bad("Falta el nonce"))?;
        let (_key, plain) = crypto::open_with_password(&password, cipher, salt, nonce)
            .map_err(|_| bad("Contraseña incorrecta"))?;
        let content: Content = serde_json::from_slice(&plain)
            .map_err(|_| bad("El contenido de la nota está dañado"))?;
        db::clear_lock(&conn, &id, &note.mode, &content).map_err(err)?;
        state.forget_key(Some(&id));
    }
    let _ = app.emit("notes-changed", &id);
    Ok(())
}

#[tauri::command]
pub fn open_list(app: AppHandle) {
    windows::spawn_list(&app);
}

#[tauri::command]
pub fn show_all(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    windows::show_all(&app, &state)
}

#[tauri::command]
pub fn hide_all(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    windows::hide_all(&app, &state)
}

#[tauri::command]
pub fn quit_app(app: AppHandle, state: State<'_, AppState>) {
    quit_impl(&app, &state);
}
