//! File-based synchronization through a user-selected cloud folder.
//!
//! Each device keeps its own SQLite database and exchanges one JSON document per
//! note. The note document carries only portable content state; window geometry
//! and the user's visible/hidden choice always stay on the current device.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::backup;
use crate::db::{self, Note};
use crate::state::AppState;

const FOLDER: &str = "Notas-sync";
const NOTES: &str = "notes";
const SYNC_FOLDER_KEY: &str = "sync_folder";
const DEVICE_ID_KEY: &str = "device_id";
const LAST_SYNC_KEY: &str = "sync_last_at";
const LAST_ERROR_KEY: &str = "sync_last_error";
const TOMBSTONE_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
static PENDING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
pub struct SyncStatus {
    pub folder: Option<String>,
    pub suggested_folder: Option<String>,
    pub last_sync_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SyncResult {
    pub synced: usize,
    pub conflicts: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteRecord {
    #[serde(flatten)]
    note: backup::ExportedNote,
    device_id: String,
    #[serde(default)]
    deleted: bool,
}

impl RemoteRecord {
    fn id(&self) -> &str {
        &self.note.id
    }
}

#[derive(Clone)]
struct LocalRecord {
    note: Option<Note>,
    tags: Vec<String>,
    updated_at: i64,
    device_id: String,
    deleted: bool,
}

impl LocalRecord {
    fn from_note(note: Note, tags: Vec<String>, device_id: &str) -> Self {
        Self {
            updated_at: note.updated_at,
            note: Some(note),
            tags,
            device_id: device_id.to_string(),
            deleted: false,
        }
    }

    fn tombstone(updated_at: i64, device_id: String) -> Self {
        Self { note: None, tags: Vec::new(), updated_at, device_id, deleted: true }
    }
}

pub fn device_id(conn: &rusqlite::Connection) -> Result<String, String> {
    if let Some(id) = db::meta_get(conn, DEVICE_ID_KEY).map_err(|error| error.to_string())? {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    db::meta_set(conn, DEVICE_ID_KEY, &id).map_err(|error| error.to_string())?;
    Ok(id)
}

pub fn status(conn: &rusqlite::Connection) -> Result<SyncStatus, String> {
    let folder = db::meta_get(conn, SYNC_FOLDER_KEY).map_err(|error| error.to_string())?;
    let last_sync_at = db::meta_get(conn, LAST_SYNC_KEY)
        .map_err(|error| error.to_string())?
        .and_then(|value| value.parse().ok());
    let error = db::meta_get(conn, LAST_ERROR_KEY).map_err(|error| error.to_string())?;
    let suggested_folder = std::env::var_os("OneDrive").map(|value| value.to_string_lossy().to_string());
    Ok(SyncStatus { folder, suggested_folder, last_sync_at, error })
}

pub fn configure(conn: &rusqlite::Connection, folder: &Path) -> Result<(), String> {
    if !folder.is_dir() {
        return Err("La carpeta elegida no existe".to_string());
    }
    db::meta_set(conn, SYNC_FOLDER_KEY, &folder.display().to_string()).map_err(|error| error.to_string())
}

pub fn disable(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute("DELETE FROM meta WHERE key IN (?1, ?2, ?3)", [SYNC_FOLDER_KEY, LAST_SYNC_KEY, LAST_ERROR_KEY])
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn record_local_delete(conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
    let device = device_id(conn)?;
    db::sync_set_tombstone(conn, id, db::now_ms(), &device).map_err(|error| error.to_string())
}

fn root(folder: &Path) -> PathBuf {
    folder.join(FOLDER)
}

fn notes_dir(folder: &Path) -> PathBuf {
    root(folder).join(NOTES)
}

#[cfg(windows)]
fn replace_atomic(temporary: &Path, path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| format!("No se pudo terminar la sincronización: {error}"))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_atomic(temporary: &Path, path: &Path) -> Result<(), String> {
    fs::rename(temporary, path).map_err(|error| format!("No se pudo terminar la sincronización: {error}"))
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "Ruta de sincronización inválida".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("No se pudo crear la carpeta de sincronización: {error}"))?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, contents).map_err(|error| format!("No se pudo escribir la sincronización: {error}"))?;
    if let Err(error) = replace_atomic(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn read_remote(folder: &Path) -> (HashMap<String, RemoteRecord>, usize) {
    let mut records = HashMap::new();
    let mut skipped = 0;
    let Ok(entries) = fs::read_dir(notes_dir(folder)) else { return (records, skipped) };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let parsed = fs::read_to_string(entry.path())
            .ok()
            .and_then(|raw| serde_json::from_str::<RemoteRecord>(&raw).ok());
        let Some(record) = parsed else {
            skipped += 1;
            continue;
        };
        let id = record.id().to_string();
        if !valid_id(&id) {
            skipped += 1;
            continue;
        }
        records.insert(id, record);
    }
    (records, skipped)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn remote_record(id: &str, local: &LocalRecord) -> Result<RemoteRecord, String> {
    let note = match local.note.as_ref() {
        Some(note) => backup::export_one(note, local.tags.clone())?,
        None => backup::ExportedNote {
            id: id.to_string(),
            color: "amarillo".to_string(),
            mode: db::MODE_TEXT.to_string(),
            text: String::new(),
            items: Vec::new(),
            locked: false,
            cipher: None,
            salt: None,
            nonce: None,
            always_on_top: false,
            visible: true,
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            reminder_at: None,
            reminder_fired: false,
            attach_kind: None,
            attach_value: None,
            created_at: local.updated_at,
            updated_at: local.updated_at,
            tags: Vec::new(),
        },
    };
    Ok(RemoteRecord {
        note,
        device_id: local.device_id.clone(),
        deleted: local.deleted,
    })
}

fn local_records(conn: &rusqlite::Connection, device: &str) -> Result<HashMap<String, LocalRecord>, String> {
    let mut records = HashMap::new();
    let mut tags = db::all_tags(conn).map_err(|error| error.to_string())?;
    for note in db::list(conn).map_err(|error| error.to_string())? {
        records.insert(note.id.clone(), LocalRecord::from_note(note.clone(), tags.remove(&note.id).unwrap_or_default(), device));
    }
    for (id, deleted_at, tombstone_device) in db::sync_tombstones(conn).map_err(|error| error.to_string())? {
        let replace = records.get(&id).is_none_or(|current| current.updated_at < deleted_at);
        if replace {
            records.insert(id, LocalRecord::tombstone(deleted_at, tombstone_device));
        }
    }
    Ok(records)
}

fn remote_to_local(remote: &RemoteRecord) -> Result<LocalRecord, String> {
    if remote.deleted {
        return Ok(LocalRecord::tombstone(
            remote.note.updated_at,
            remote.device_id.clone(),
        ));
    }
    let tags = remote.note.tags.clone();
    Ok(LocalRecord {
        note: Some(backup::to_note(remote.note.clone(), 2)?),
        tags,
        updated_at: remote.note.updated_at,
        device_id: remote.device_id.clone(),
        deleted: false,
    })
}

fn winner_is_remote(local: &LocalRecord, remote: &RemoteRecord) -> bool {
    remote.note.updated_at > local.updated_at
        || (remote.note.updated_at == local.updated_at && remote.device_id > local.device_id)
}

fn changed_since(conn: &rusqlite::Connection, id: &str, updated_at: i64) -> Result<bool, String> {
    Ok(db::sync_last_synced(conn, id)
        .map_err(|error| error.to_string())?
        .is_some_and(|last| updated_at > last))
}

fn preserve_local_device_fields(incoming: &mut Note, local: Option<&Note>) {
    if let Some(local) = local {
        incoming.visible = local.visible;
        incoming.x = local.x;
        incoming.y = local.y;
        incoming.width = local.width;
        incoming.height = local.height;
    }
}

fn write_local(conn: &rusqlite::Connection, id: &str, incoming: LocalRecord) -> Result<(), String> {
    if incoming.deleted {
        db::delete_note(conn, id).map_err(|error| error.to_string())?;
        db::sync_set_tombstone(conn, id, incoming.updated_at, &incoming.device_id).map_err(|error| error.to_string())?;
        return Ok(());
    }
    let previous = db::get(conn, id).map_err(|error| error.to_string())?;
    let mut note = incoming.note.ok_or_else(|| "Registro de sincronización inválido".to_string())?;
    preserve_local_device_fields(&mut note, previous.as_ref());
    if previous.is_some() {
        db::delete_note(conn, id).map_err(|error| error.to_string())?;
    }
    db::insert_note(conn, &note).map_err(|error| error.to_string())?;
    for tag in incoming.tags {
        db::add_tag(conn, &note.id, &tag).map_err(|error| error.to_string())?;
    }
    db::sync_remove_tombstone(conn, id).map_err(|error| error.to_string())?;
    Ok(())
}

fn conflict_copy(conn: &rusqlite::Connection, loser: LocalRecord) -> Result<(), String> {
    let Some(mut note) = loser.note else { return Ok(()) };
    note.id = uuid::Uuid::new_v4().to_string();
    note.created_at = db::now_ms();
    note.updated_at = note.created_at;
    db::insert_note(conn, &note).map_err(|error| error.to_string())?;
    if !note.locked {
        let mut content = note.content();
        if note.mode == db::MODE_TEXT {
            content.text = format!(
                "<div>(conflicto) {}</div>{}",
                db::html_to_text(&content.text),
                content.text
            );
        } else if let Some(item) = content.items.first_mut() {
            item.text = format!("(conflicto) {}", item.text);
        }
        db::save_plain_content(conn, &note.id, &note.mode, &content)
            .map_err(|error| error.to_string())?;
    }
    for tag in loser.tags {
        db::add_tag(conn, &note.id, &tag).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn markdown(conn: &rusqlite::Connection, folder: &Path) -> Result<String, String> {
    let temporary = folder.join(format!(".Notas-{}.md.tmp", uuid::Uuid::new_v4()));
    backup::export_markdown(conn, &temporary)?;
    let markdown = fs::read_to_string(&temporary).map_err(|error| error.to_string())?;
    let _ = fs::remove_file(temporary);
    Ok(markdown.replace("🔒 Nota bloqueada (no se exporta su contenido)", "🔒 Nota bloqueada"))
}

pub fn sync_now(conn: &rusqlite::Connection, folder: &Path, device: &str) -> Result<SyncResult, String> {
    let sync_root = root(folder);
    fs::create_dir_all(notes_dir(folder)).map_err(|error| format!("No se pudo crear la carpeta de sincronización: {error}"))?;
    db::sync_prune_tombstones(conn, db::now_ms() - TOMBSTONE_RETENTION_MS).map_err(|error| error.to_string())?;
    let (remote, skipped) = read_remote(folder);
    let mut local = local_records(conn, device)?;
    let ids: BTreeSet<String> = local.keys().chain(remote.keys()).cloned().collect();
    let mut result = SyncResult { skipped, ..SyncResult::default() };

    for id in ids {
        let local_entry = local.get(&id).cloned();
        let remote_entry = remote.get(&id).cloned();
        match (local_entry, remote_entry) {
            (Some(local_entry), Some(remote_entry)) => {
                let remote_local = remote_to_local(&remote_entry)?;
                let conflict = changed_since(conn, &id, local_entry.updated_at)?
                    && changed_since(conn, &id, remote_local.updated_at)?;
                if conflict && !local_entry.deleted && !remote_local.deleted {
                    let loser = if winner_is_remote(&local_entry, &remote_entry) { local_entry.clone() } else { remote_local.clone() };
                    conflict_copy(conn, loser)?;
                    result.conflicts += 1;
                }
                if winner_is_remote(&local_entry, &remote_entry) {
                    write_local(conn, &id, remote_local.clone())?;
                    local.insert(id, remote_local);
                }
            }
            (None, Some(remote_entry)) => {
                let incoming = remote_to_local(&remote_entry)?;
                write_local(conn, &id, incoming.clone())?;
                local.insert(id, incoming);
            }
            _ => {}
        }
    }

    // Re-read after incoming writes and conflict copies, then make the folder a
    // complete atomic snapshot of this device's resolved view.
    local = local_records(conn, device)?;
    for (id, entry) in &local {
        let record = remote_record(id, entry)?;
        let bytes = serde_json::to_vec_pretty(&record).map_err(|error| error.to_string())?;
        atomic_write(&notes_dir(folder).join(format!("{id}.json")), &bytes)?;
        db::sync_mark_synced(conn, id, entry.updated_at).map_err(|error| error.to_string())?;
        result.synced += 1;
    }
    atomic_write(&sync_root.join("LEEME.md"), b"# Sincronizacion de Notas\n\nEsta carpeta la gestiona Notas. No edites los archivos JSON mientras la aplicacion sincroniza. Las notas bloqueadas permanecen cifradas.\n")?;
    atomic_write(&sync_root.join("Notas.md"), markdown(conn, &sync_root)?.as_bytes())?;
    db::meta_set(conn, LAST_SYNC_KEY, &db::now_ms().to_string()).map_err(|error| error.to_string())?;
    conn.execute("DELETE FROM meta WHERE key = ?1", [LAST_ERROR_KEY]).map_err(|error| error.to_string())?;
    Ok(result)
}

fn configured_sync(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let Some(folder) = db::meta_get(&conn, SYNC_FOLDER_KEY).map_err(|error| error.to_string())? else { return Ok(()) };
    let device = device_id(&conn)?;
    match sync_now(&conn, Path::new(&folder), &device) {
        Ok(_) => Ok(()),
        Err(error) => {
            db::meta_set(&conn, LAST_ERROR_KEY, &error).map_err(|db_error| db_error.to_string())?;
            Err(error)
        }
    }
}

pub fn schedule(app: &AppHandle) {
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(5));
        if let Err(error) = configured_sync(&handle) {
            eprintln!("[notas] no se pudo sincronizar: {error}");
        }
        PENDING.store(false, Ordering::Release);
    });
}

pub fn start(app: AppHandle) {
    thread::spawn(move || loop {
        if let Err(error) = configured_sync(&app) {
            eprintln!("[notas] no se pudo sincronizar: {error}");
        }
        thread::sleep(Duration::from_secs(60));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Content;

    fn setup(root: &Path, id: &str, device: &str) -> rusqlite::Connection {
        let conn = db::open(&root.join(format!("{device}.db"))).expect("db");
        db::meta_set(&conn, DEVICE_ID_KEY, device).expect("device");
        let mut note = Note::fresh(id.to_string(), 10.0, 20.0);
        note.updated_at = 100;
        db::insert_note(&conn, &note).expect("note");
        db::save_plain_content(&conn, id, db::MODE_TEXT, &Content { text: "<div>A</div>".to_string(), items: vec![], rich_text: true }).expect("text");
        conn
    }

    #[test]
    fn two_devices_merge_conflict_and_tombstone() {
        let temp = tempfile::tempdir().expect("temp");
        let folder = temp.path().join("cloud");
        fs::create_dir_all(&folder).expect("cloud");
        let a = setup(temp.path(), "shared", "a-device");
        let b = db::open(&temp.path().join("b.db")).expect("b");
        db::meta_set(&b, DEVICE_ID_KEY, "b-device").expect("device");
        sync_now(&a, &folder, "a-device").expect("A pushes");
        sync_now(&b, &folder, "b-device").expect("B pulls");
        assert!(db::get(&b, "shared").expect("get").is_some());

        db::save_plain_content(&a, "shared", db::MODE_TEXT, &Content { text: "<div>desde A</div>".to_string(), items: vec![], rich_text: true }).expect("A edit");
        db::save_plain_content(&b, "shared", db::MODE_TEXT, &Content { text: "<div>desde B</div>".to_string(), items: vec![], rich_text: true }).expect("B edit");
        sync_now(&a, &folder, "a-device").expect("A sync");
        let result = sync_now(&b, &folder, "b-device").expect("B conflict");
        assert!(result.conflicts <= 1);
        assert!(db::count(&b).expect("count") >= 1);

        record_local_delete(&a, "shared").expect("tombstone");
        db::delete_note(&a, "shared").expect("delete");
        sync_now(&a, &folder, "a-device").expect("A delete sync");
        sync_now(&b, &folder, "b-device").expect("B delete sync");
        assert!(db::get(&b, "shared").expect("get").is_none());
    }

    #[test]
    fn atomic_write_and_markdown_do_not_leak_locked_text() {
        let temp = tempfile::tempdir().expect("temp");
        let conn = setup(temp.path(), "locked", "device");
        let note = db::require(&conn, "locked").expect("note");
        let content = Content { text: "<div>SECRETO_V2C</div>".to_string(), items: vec![], rich_text: true };
        let (_, sealed) = crate::crypto::seal_with_password("clave", &serde_json::to_vec(&content).expect("payload")).expect("seal");
        db::save_cipher(&conn, &note.id, &sealed.cipher, &sealed.salt, &sealed.nonce).expect("lock");
        sync_now(&conn, temp.path(), "device").expect("sync");
        let markdown = fs::read_to_string(root(temp.path()).join("Notas.md")).expect("markdown");
        assert!(!markdown.contains("SECRETO_V2C"));
        assert!(root(temp.path()).join("notes").join("locked.json").is_file());
        assert!(!root(temp.path()).join("notes").join("locked.json.tmp").exists());
    }
}
