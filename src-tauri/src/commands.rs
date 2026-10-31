//! Tauri commands: the only surface the frontend is allowed to talk to.
//!
//! Every command returns `Result<_, String>` so the UI can show a message
//! instead of the process dying on a bad path or a corrupted row.

use std::path::PathBuf;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::attachments;
use crate::autostart;
use crate::backup;
use crate::crypto::{self, NONCE_LEN, SALT_LEN};
use crate::db::{self, Content, Item, Note, COLORS, MODE_LIST, MODE_TEXT};
use crate::shortcuts::ShortcutInfo;
use crate::state::AppState;
use crate::sync;
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
    pub plain_text: String,
    pub items: Vec<Item>,
    pub tags: Vec<String>,
    pub visible: bool,
    pub locked: bool,
    pub unlocked: bool,
    pub always_on_top: bool,
    pub reminder_at: Option<i64>,
    pub attachment: Option<String>,
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

fn note_changed(app: &AppHandle, id: &str) {
    let _ = app.emit("notes-changed", id);
    sync::schedule(app);
}

fn title_of(text: &str, items: &[Item], mode: &str) -> String {
    let first = if mode == MODE_LIST {
        items
            .iter()
            .find(|item| !item.text.trim().is_empty())
            .map(|item| item.text.clone())
            .unwrap_or_default()
    } else {
        db::html_to_text(text)
            .lines()
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

/// Upgrade legacy plaintext inside an encrypted v1 payload before rendering it.
fn upgrade_text_content(content: &mut Content, mode: &str) {
    if mode == MODE_TEXT && !content.rich_text {
        content.text = db::plain_to_html(&content.text);
        content.rich_text = true;
    }
    db::normalize_content(content, mode);
}

/// Persist content, sanitizing text-mode HTML before plaintext or encrypted storage.
fn store_content(
    conn: &Connection,
    state: &AppState,
    note: &Note,
    mode: &str,
    content: &Content,
) -> Result<(), String> {
    let mut content = content.clone();
    upgrade_text_content(&mut content, mode);
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
        let payload = serde_json::to_vec(&content).map_err(err)?;
        let sealed = crypto::seal(&key, salt, &payload).map_err(err)?;
        db::save_cipher(conn, &note.id, &sealed.cipher, &sealed.salt, &sealed.nonce).map_err(err)?;
        db::set_mode(conn, &note.id, mode).map_err(err)?;
    } else {
        db::save_plain_content(conn, &note.id, mode, &content).map_err(err)?;
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
    let plain_text = if note.mode == MODE_LIST {
        items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        db::html_to_text(&text)
    };
    Ok(NoteView {
        id: note.id.clone(),
        color: note.color.clone(),
        mode: note.mode.clone(),
        title,
        text,
        plain_text,
        items,
        tags,
        visible: note.visible,
        locked: note.locked,
        unlocked,
        always_on_top: note.always_on_top,
        reminder_at: note.reminder_at,
        attachment: attachments::rule_label(note),
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
    note_changed(app, &created.id);
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

/// Creating a window needs the event loop, so this must not run on the main
/// thread: `(async)` moves the handler to the thread pool, like every other
/// command that creates or shows a window.
#[tauri::command(async)]
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
    note_changed(&app, &id);
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
    note_changed(&app, &id);
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
    note_changed(&app, &id);
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
    note_changed(&app, &id);
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
    note_changed(&app, &id);
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
    note_changed(&app, &id);
    Ok(())
}

#[tauri::command(async)]
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
    note_changed(&app, &id);
    Ok(())
}

#[tauri::command]
pub fn delete_note(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    {
        let conn = state.conn()?;
        find(&conn, &id)?;
        sync::record_local_delete(&conn, &id)?;
        db::delete_note(&conn, &id).map_err(err)?;
    }
    state.forget_key(Some(&id));
    if let Some(window) = app.get_webview_window(&windows::note_label(&id)) {
        window.destroy().map_err(err)?;
    }
    note_changed(&app, &id);
    Ok(())
}

#[tauri::command]
pub fn list_attachable_windows() -> Result<Vec<attachments::Candidate>, String> {
    attachments::list_candidates()
}

#[tauri::command(async)]
pub fn attach_note(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    kind: String,
    value: String,
) -> Result<(), String> {
    if kind != attachments::KIND_EXE && kind != attachments::KIND_TITLE {
        return Err(bad("Tipo de regla desconocido"));
    }
    let value = value.trim();
    if value.is_empty() || value.len() > 500 {
        return Err(bad("La regla de ventana no es válida"));
    }
    {
        let conn = state.conn()?;
        find(&conn, &id)?;
        db::set_attachment(&conn, &id, Some(&kind), Some(value)).map_err(err)?;
    }
    attachments::refresh(&app);
    note_changed(&app, &id);
    Ok(())
}

#[tauri::command(async)]
pub fn detach_note(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let note = {
        let conn = state.conn()?;
        let note = find(&conn, &id)?;
        db::set_attachment(&conn, &id, None, None).map_err(err)?;
        note
    };
    windows::restore_note_z_order(&app, &id, note.always_on_top);
    windows::show_note(&app, &id)?;
    note_changed(&app, &id);
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
    note_changed(&app, &id);
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
    note_changed(&app, &id);
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
        let mut content = note.content();
        db::normalize_content(&mut content, &note.mode);
        let payload = serde_json::to_vec(&content).map_err(err)?;
        let (key, sealed) = crypto::seal_with_password(&password, &payload).map_err(err)?;
        db::save_cipher(&conn, &id, &sealed.cipher, &sealed.salt, &sealed.nonce).map_err(err)?;
        state.remember_key(&id, key);
    }
    note_changed(&app, &id);
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
    let mut content: Content =
        serde_json::from_slice(&plain).map_err(|_| bad("El contenido de la nota está dañado"))?;
    upgrade_text_content(&mut content, &note.mode);
    state.remember_key(&id, key);
    // Re-encrypt legacy text immediately, so it cannot be rendered as HTML later.
    store_content(&conn, &state, &note, &note.mode, &content)?;
    let stored = find(&conn, &id)?;
    let mut note_view = view(&state, &stored, tags)?;
    note_view.title = title_of(&content.text, &content.items, &note.mode);
    note_view.plain_text = content.plain(&note.mode);
    note_view.text = content.text;
    note_view.items = content.items;
    Ok(note_view)
}

/// Forget the key of a note: it goes back to the locked screen.
#[tauri::command]
pub fn relock_note(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.forget_key(Some(&id));
    note_changed(&app, &id);
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
        let mut content: Content = serde_json::from_slice(&plain)
            .map_err(|_| bad("El contenido de la nota está dañado"))?;
        upgrade_text_content(&mut content, &note.mode);
        db::clear_lock(&conn, &id, &note.mode, &content).map_err(err)?;
        state.forget_key(Some(&id));
    }
    note_changed(&app, &id);
    Ok(())
}

#[tauri::command(async)]
pub fn open_list(app: AppHandle) {
    windows::spawn_list(&app);
}

#[tauri::command(async)]
pub fn show_all(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    windows::show_all(&app, &state)
}

#[tauri::command(async)]
pub fn hide_all(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    windows::hide_all(&app, &state)
}

#[tauri::command]
pub fn quit_app(app: AppHandle, state: State<'_, AppState>) {
    quit_impl(&app, &state);
}

/// Real OS state of "Iniciar con Windows".
#[tauri::command(async)]
pub fn autostart_enabled(app: AppHandle) -> Result<bool, String> {
    autostart::is_enabled(&app)
}

/// Enable or disable "Iniciar con Windows" and return the state Windows reports.
#[tauri::command(async)]
pub fn set_autostart(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<bool, String> {
    let actual = autostart::set_enabled(&app, enabled)?;
    state.sync_autostart_item(actual);
    let _ = app.emit("autostart-changed", actual);
    Ok(actual)
}

// -------------------------------------------------------------------- ajustes

/// Everything the settings window needs on open.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SettingsState {
    pub autostart: bool,
    pub last_backup_at: Option<i64>,
    pub backups: Vec<backup::BackupEntry>,
    pub shortcuts: Vec<ShortcutInfo>,
    pub sync: sync::SyncStatus,
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(err)
}

/// Run a job that may block (native dialogs, file copies) on a thread of its
/// own, so the event loop never waits for the user to pick a file.
fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("notas-file-job".to_string())
        .spawn(move || {
            let _ = sender.send(job());
        })
        .map_err(err)?;
    match receiver.recv() {
        Ok(result) => result,
        Err(_) => Err(bad("La operación se interrumpió")),
    }
}

fn save_dialog(
    app: &AppHandle,
    title: &str,
    file_name: &str,
    filter: &str,
    extension: &str,
) -> Option<PathBuf> {
    app.dialog()
        .file()
        .set_title(title)
        .set_file_name(file_name)
        .add_filter(filter, &[extension])
        .blocking_save_file()
        .and_then(|path| path.into_path().ok())
}

fn open_dialog(
    app: &AppHandle,
    title: &str,
    filter: &str,
    extension: &str,
) -> Option<PathBuf> {
    app.dialog()
        .file()
        .set_title(title)
        .add_filter(filter, &[extension])
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
}

fn folder_dialog(app: &AppHandle) -> Option<PathBuf> {
    app.dialog()
        .file()
        .set_title("Elegir carpeta de sincronización")
        .blocking_pick_folder()
        .and_then(|path| path.into_path().ok())
}

/// Open the settings window (never on the main thread: see `windows`).
#[tauri::command(async)]
pub fn open_settings(app: AppHandle) {
    windows::spawn_settings(&app);
}

#[tauri::command]
pub fn settings_state(app: AppHandle, state: State<'_, AppState>) -> Result<SettingsState, String> {
    let folder = data_dir(&app)?;
    let autostart = autostart::is_enabled(&app).unwrap_or(false);
    let (last_backup_at, backups, sync_status) = {
        let conn = state.conn()?;
        (
            backup::last_at(&conn, &folder),
            backup::list(&folder),
            sync::status(&conn)?,
        )
    };
    Ok(SettingsState {
        autostart,
        last_backup_at,
        backups,
        shortcuts: state.shortcuts(),
        sync: sync_status,
    })
}


/// Select and enable a cloud parent folder. Synchronization itself is explicit
/// and runs on the file worker so the native picker never blocks WebView IPC.
#[tauri::command(async)]
pub fn choose_sync_folder(
    app: AppHandle,
    path: Option<String>,
) -> Result<Option<sync::SyncStatus>, String> {
    let handle = app.clone();
    blocking(move || {
        let picked = match path {
            Some(path) => Some(PathBuf::from(path)),
            None => folder_dialog(&handle),
        };
        let Some(folder) = picked else { return Ok(None) };
        let state = handle.state::<AppState>();
        let conn = state.conn()?;
        sync::configure(&conn, &folder)?;
        let device = sync::device_id(&conn)?;
        sync::sync_now(&conn, &folder, &device)?;
        let status = sync::status(&conn)?;
        drop(conn);
        let _ = handle.emit("notes-changed", "");
        Ok(Some(status))
    })
}

#[tauri::command]
pub fn disable_sync(state: State<'_, AppState>) -> Result<(), String> {
    let conn = state.conn()?;
    sync::disable(&conn)
}

#[tauri::command(async)]
pub fn sync_now(app: AppHandle) -> Result<sync::SyncResult, String> {
    let handle = app.clone();
    blocking(move || {
        let state = handle.state::<AppState>();
        let conn = state.conn()?;
        let status = sync::status(&conn)?;
        let folder = status
            .folder
            .ok_or_else(|| bad("Elige primero una carpeta de sincronización"))?;
        let device = sync::device_id(&conn)?;
        let result = sync::sync_now(&conn, PathBuf::from(folder).as_path(), &device)?;
        drop(conn);
        let _ = handle.emit("notes-changed", "");
        Ok(result)
    })
}

/// Write today's automatic backup now and return its path.
#[tauri::command(async)]
pub fn backup_now(app: AppHandle) -> Result<String, String> {
    let handle = app.clone();
    blocking(move || {
        let folder = data_dir(&handle)?;
        let state = handle.state::<AppState>();
        let conn = state.conn()?;
        let path = backup::create(&conn, &folder)?;
        Ok(path.display().to_string())
    })
}

/// Replace every note with the contents of one automatic backup.
#[tauri::command(async)]
pub fn restore_backup(app: AppHandle, name: String) -> Result<(), String> {
    let handle = app.clone();
    blocking(move || {
        let folder = data_dir(&handle)?;
        if !backup::exists(&folder, &name) {
            return Err(bad("Esa copia ya no existe"));
        }
        let state = handle.state::<AppState>();
        let path = backup::dir(&folder).join(&name);
        {
            let mut conn = state.conn()?;
            // A copy of what is about to be replaced, before touching anything.
            backup::safety_copy(&conn, &folder)?;
            backup::restore(&mut conn, &path)?;
        }
        state.forget_key(None);
        windows::reload_all(&handle, &state)?;
        Ok(())
    })
}

#[tauri::command(async)]
pub fn open_backups_folder(app: AppHandle) -> Result<String, String> {
    let folder = backup::dir(&data_dir(&app)?);
    std::fs::create_dir_all(&folder).map_err(err)?;
    std::process::Command::new("explorer")
        .arg(&folder)
        .spawn()
        .map_err(|error| format!("No se pudo abrir la carpeta: {error}"))?;
    Ok(folder.display().to_string())
}

/// Destination chosen by the caller, or the native save dialog when there is
/// none. The dialog path is what the settings window uses; an explicit path
/// keeps the command usable without a window in front.
fn target_path(
    app: &AppHandle,
    chosen: Option<String>,
    title: &str,
    file_name: &str,
    filter: &str,
    extension: &str,
) -> Option<PathBuf> {
    if let Some(path) = chosen {
        return Some(PathBuf::from(path));
    }
    save_dialog(app, title, file_name, filter, extension)
}

/// One Markdown file with every note; locked ones only as a placeholder.
#[tauri::command(async)]
pub fn export_markdown(app: AppHandle, path: Option<String>) -> Result<Option<String>, String> {
    let handle = app.clone();
    blocking(move || {
        let Some(path) = target_path(
            &handle,
            path,
            "Exportar a Markdown",
            &backup::suggested_markdown_name(),
            "Markdown",
            "md",
        ) else {
            return Ok(None);
        };
        let state = handle.state::<AppState>();
        let conn = state.conn()?;
        backup::export_markdown(&conn, &path)?;
        Ok(Some(path.display().to_string()))
    })
}

/// The versioned JSON copy, with the encrypted payload of locked notes.
#[tauri::command(async)]
pub fn export_json(app: AppHandle, path: Option<String>) -> Result<Option<String>, String> {
    let handle = app.clone();
    blocking(move || {
        let Some(path) = target_path(
            &handle,
            path,
            "Exportar copia completa",
            &backup::suggested_json_name(),
            "Copia de Notas",
            "json",
        ) else {
            return Ok(None);
        };
        let state = handle.state::<AppState>();
        let conn = state.conn()?;
        backup::export_json(&conn, &path)?;
        Ok(Some(path.display().to_string()))
    })
}

/// Merge a JSON copy into the current notes.
#[tauri::command(async)]
pub fn import_json(
    app: AppHandle,
    path: Option<String>,
) -> Result<Option<backup::ImportSummary>, String> {
    let handle = app.clone();
    blocking(move || {
        let picked = match path {
            Some(path) => Some(PathBuf::from(path)),
            None => open_dialog(&handle, "Importar copia", "Copia de Notas", "json"),
        };
        let Some(path) = picked else {
            return Ok(None);
        };
        let state = handle.state::<AppState>();
        let summary = {
            let conn = state.conn()?;
            backup::import_json(&conn, &path)?
        };
        // Replaced rows may keep an id whose key belonged to the old payload.
        state.forget_key(None);
        // The rows changed under the open windows: rebuild them from the database.
        windows::reload_all(&handle, &state)?;
        Ok(Some(summary))
    })
}
