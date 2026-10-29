//! Backups, exports and imports of the notes database.
//!
//! The daily backup is a byte copy of `notas.db` made through SQLite's online
//! backup API, so it is consistent even while the app writes to the original.
//! A backup is a copy of the database file, which already never stores the
//! plaintext of a locked note: locked content stays encrypted in it.
//!
//! Exports are for the user, never for the app's own restore path: the JSON
//! copy keeps the encrypted payload of locked notes (base64 of cipher, salt and
//! nonce) and the Markdown copy only prints a placeholder for them. No export
//! ever contains the plaintext of a locked note.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use chrono::Local;
use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::db::{self, Content, Item, Note, COLORS, MODE_LIST, MODE_TEXT};

/// File name prefix of the automatic backups: `notas-YYYY-MM-DD.db`.
const DAILY_PREFIX: &str = "notas-";
/// How many daily backups are kept.
pub const KEEP: usize = 7;
/// Marker of the JSON format written by "Exportar copia completa".
pub const FORMAT: &str = "notas-backup";
/// Version of that format.
pub const VERSION: i64 = 1;
/// How often the background loop looks at the clock.
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 30);
/// One backup per day.
const DAILY_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

/// What the settings window shows about one automatic backup.
#[derive(Debug, Clone, Serialize)]
pub struct BackupEntry {
    pub name: String,
    pub size: i64,
    pub modified_at: i64,
}

/// Counters of an import, shown to the user as a summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ImportSummary {
    pub imported: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// One note inside the JSON copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportedNote {
    id: String,
    color: String,
    mode: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    locked: bool,
    /// Base64 of the AES-GCM ciphertext (locked notes only).
    #[serde(default)]
    cipher: Option<String>,
    /// Base64 of the Argon2id salt (locked notes only).
    #[serde(default)]
    salt: Option<String>,
    /// Base64 of the AES-GCM nonce (locked notes only).
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    always_on_top: bool,
    #[serde(default = "default_true")]
    visible: bool,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    #[serde(default)]
    reminder_at: Option<i64>,
    #[serde(default)]
    reminder_fired: bool,
    created_at: i64,
    updated_at: i64,
    #[serde(default)]
    tags: Vec<String>,
}

/// The whole JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupFile {
    format: String,
    version: i64,
    exported_at: i64,
    notes: Vec<ExportedNote>,
}

fn default_true() -> bool {
    true
}

/// Folder that holds the automatic backups.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default()
}

/// Default name offered by "Exportar a Markdown".
pub fn suggested_markdown_name() -> String {
    format!("notas-{}.md", Local::now().format("%Y-%m-%d"))
}

/// Default name offered by "Exportar copia completa".
pub fn suggested_json_name() -> String {
    format!("notas-copia-{}.json", Local::now().format("%Y-%m-%d"))
}

/// `notas-YYYY-MM-DD.db` for today, without touching the disk.
pub fn daily_name() -> String {
    format!("{DAILY_PREFIX}{}.db", Local::now().format("%Y-%m-%d"))
}

/// True for a name the rotation owns: `notas-YYYY-MM-DD.db`, nothing else.
fn is_daily_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(DAILY_PREFIX) else {
        return false;
    };
    let Some(date) = rest.strip_suffix(".db") else {
        return false;
    };
    let bytes = date.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

/// Names of the daily backups, newest first.
fn daily_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .filter(|entry| entry.file_type().map(|kind| kind.is_file()).unwrap_or(false))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| is_daily_name(name))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
}

/// Copy the live database into `dest` through the online backup API.
fn copy_into(conn: &Connection, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("No se pudo crear la carpeta: {error}"))?;
    }
    let mut target =
        Connection::open(dest).map_err(|error| format!("No se pudo crear la copia: {error}"))?;
    {
        let backup = Backup::new(conn, &mut target)
            .map_err(|error| format!("No se pudo iniciar la copia: {error}"))?;
        backup
            .run_to_completion(256, Duration::from_millis(0), None)
            .map_err(|error| format!("No se pudo copiar la base de datos: {error}"))?;
    }
    Ok(())
}

/// Write a consistent copy to `dir/name`. The copy is built under a temporary
/// name first so a half-written file never looks like a backup.
fn write_named(conn: &Connection, dir: &Path, name: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|error| format!("No se pudo crear la carpeta: {error}"))?;
    let final_path = dir.join(name);
    let temp_path = dir.join(format!("{name}.tmp"));
    let _ = fs::remove_file(&temp_path);
    copy_into(conn, &temp_path)?;
    let _ = fs::remove_file(&final_path);
    fs::rename(&temp_path, &final_path)
        .map_err(|error| format!("No se pudo guardar la copia: {error}"))?;
    Ok(final_path)
}

/// Take today's automatic backup and keep only the newest `KEEP` files.
pub fn create(conn: &Connection, data_dir: &Path) -> Result<PathBuf, String> {
    let folder = dir(data_dir);
    let path = write_named(conn, &folder, &daily_name())?;
    let _ = db::meta_set(conn, "last_backup_at", &now_ms().to_string());
    prune(&folder, KEEP);
    Ok(path)
}

/// Delete the oldest daily backups until only `keep` remain.
pub fn prune(folder: &Path, keep: usize) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for name in daily_names(folder).into_iter().skip(keep) {
        let path = folder.join(name);
        if fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// The daily backups that exist right now, newest first.
pub fn list(data_dir: &Path) -> Vec<BackupEntry> {
    let folder = dir(data_dir);
    daily_names(&folder)
        .into_iter()
        .map(|name| {
            let path = folder.join(&name);
            let meta = fs::metadata(&path).ok();
            BackupEntry {
                size: meta.as_ref().map(|meta| meta.len() as i64).unwrap_or(0),
                modified_at: meta
                    .and_then(|meta| meta.modified().ok())
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_millis() as i64)
                    .unwrap_or(0),
                name,
            }
        })
        .collect()
}

/// When the last automatic backup was written, if there is one.
pub fn last_at(conn: &Connection, data_dir: &Path) -> Option<i64> {
    if let Ok(Some(value)) = db::meta_get(conn, "last_backup_at") {
        if let Ok(ms) = value.parse::<i64>() {
            return Some(ms);
        }
    }
    list(data_dir).first().map(|entry| entry.modified_at)
}

/// True when there is no backup yet or the newest one is older than a day.
pub fn is_due(conn: &Connection, data_dir: &Path) -> bool {
    match last_at(conn, data_dir) {
        Some(ms) => now_ms() - ms >= DAILY_INTERVAL_MS,
        None => true,
    }
}

/// A safety copy of the live database, kept next to the daily ones and never
/// rotated away (its name does not match `notas-YYYY-MM-DD.db`).
pub fn safety_copy(conn: &Connection, data_dir: &Path) -> Result<PathBuf, String> {
    let name = format!(
        "notas-pre-restauracion-{}.db",
        Local::now().format("%Y-%m-%d-%H%M%S")
    );
    write_named(conn, &dir(data_dir), &name)
}

/// Replace the live database with a backup file, keeping the connection open.
pub fn restore(conn: &mut Connection, path: &Path) -> Result<(), String> {
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("No se pudo abrir la copia: {error}"))?;
    source
        .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get::<_, i64>(0))
        .map_err(|_| "El archivo no es una copia válida de Notas".to_string())?;
    {
        let backup = Backup::new(&source, conn)
            .map_err(|error| format!("No se pudo iniciar la restauración: {error}"))?;
        backup
            .run_to_completion(256, Duration::from_millis(0), None)
            .map_err(|error| format!("No se pudo restaurar la copia: {error}"))?;
    }
    db::migrate(conn).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn exists(data_dir: &Path, name: &str) -> bool {
    is_daily_name(name) && dir(data_dir).join(name).is_file()
}

// ------------------------------------------------------------------ markdown

fn markdown_note(note: &Note, tags: &[String]) -> String {
    const HIDDEN: &str = "🔒 Nota bloqueada (no se exporta su contenido)";
    let mut out = String::new();
    if note.locked {
        out.push_str("## ");
        out.push_str(HIDDEN);
        out.push('\n');
        return out;
    }
    let title = note.title();
    let title = if title.is_empty() {
        "Nota sin título".to_string()
    } else {
        title
    };
    out.push_str("## ");
    out.push_str(&title);
    out.push_str("\n\n");
    if note.mode == MODE_LIST {
        for item in &note.content().items {
            out.push_str(if item.done { "- [x] " } else { "- [ ] " });
            out.push_str(&item.text);
            out.push('\n');
        }
    } else {
        let text = note.content().text;
        if !text.trim().is_empty() {
            out.push_str(&text);
            out.push('\n');
        }
    }
    if !tags.is_empty() {
        out.push('\n');
        out.push_str("Etiquetas: ");
        out.push_str(&tags.join(", "));
        out.push('\n');
    }
    out
}

/// Write every note as Markdown. Locked notes leave only a placeholder behind.
pub fn export_markdown(conn: &Connection, out: &Path) -> Result<usize, String> {
    let notes = db::list(conn).map_err(|error| error.to_string())?;
    let mut tags = db::all_tags(conn).map_err(|error| error.to_string())?;
    let mut body = String::new();
    body.push_str("# Notas\n\n");
    body.push_str(&format!(
        "Exportado el {}\n\n",
        Local::now().format("%d/%m/%Y %H:%M")
    ));
    for note in &notes {
        body.push_str("---\n\n");
        let note_tags = tags.remove(&note.id).unwrap_or_default();
        body.push_str(&markdown_note(note, &note_tags));
        body.push('\n');
    }
    fs::write(out, body).map_err(|error| format!("No se pudo escribir el archivo: {error}"))?;
    Ok(notes.len())
}

// ---------------------------------------------------------------------- json

fn to_note(exported: ExportedNote) -> Result<Note, String> {
    let mode = match exported.mode.as_str() {
        MODE_TEXT => MODE_TEXT.to_string(),
        MODE_LIST => MODE_LIST.to_string(),
        _ => return Err(format!("Modo desconocido en la nota {}", exported.id)),
    };
    let color = if COLORS.contains(&exported.color.as_str()) {
        exported.color
    } else {
        "amarillo".to_string()
    };
    let content = Content {
        text: exported.text,
        items: exported.items,
    };
    let (body_plain, content_json) = if exported.locked {
        (String::new(), String::new())
    } else {
        let items_json = serde_json::to_string(&content.items)
            .map_err(|error| format!("Contenido inválido: {error}"))?;
        (
            content.plain(&mode),
            format!(r#"{{"items":{items_json}}}"#),
        )
    };
    let clamp = |value: f64, low: f64, high: f64, fallback: f64| {
        if value.is_finite() {
            value.clamp(low, high)
        } else {
            fallback
        }
    };
    Ok(Note {
        id: exported.id,
        color,
        mode,
        body_plain,
        content_json,
        body_cipher: exported
            .cipher
            .map(|value| BASE64.decode(value).map_err(|_| "Cifrado inválido".to_string()))
            .transpose()?,
        salt: exported
            .salt
            .map(|value| BASE64.decode(value).map_err(|_| "Sal inválida".to_string()))
            .transpose()?,
        nonce: exported
            .nonce
            .map(|value| BASE64.decode(value).map_err(|_| "Nonce inválido".to_string()))
            .transpose()?,
        locked: exported.locked,
        always_on_top: exported.always_on_top,
        visible: exported.visible,
        x: clamp(exported.x, -20_000.0, 20_000.0, 160.0),
        y: clamp(exported.y, -20_000.0, 20_000.0, 140.0),
        width: clamp(exported.width, 200.0, 4000.0, 300.0),
        height: clamp(exported.height, 160.0, 4000.0, 260.0),
        reminder_at: exported.reminder_at,
        reminder_fired: exported.reminder_fired,
        created_at: exported.created_at,
        updated_at: exported.updated_at,
    })
}

/// Locked notes are written as cipher + salt + nonce, never as plaintext.
fn export_one(note: &Note, tags: Vec<String>) -> Result<ExportedNote, String> {
    let (text, items) = if note.locked {
        (String::new(), Vec::new())
    } else {
        let content = note.content();
        (content.text, content.items)
    };
    let encode = |bytes: &[u8]| BASE64.encode(bytes);
    Ok(ExportedNote {
        id: note.id.clone(),
        color: note.color.clone(),
        mode: note.mode.clone(),
        text,
        items,
        locked: note.locked,
        cipher: note.body_cipher.as_deref().map(encode),
        salt: note.salt.as_deref().map(encode),
        nonce: note.nonce.as_deref().map(encode),
        always_on_top: note.always_on_top,
        visible: note.visible,
        x: note.x,
        y: note.y,
        width: note.width,
        height: note.height,
        reminder_at: note.reminder_at,
        reminder_fired: note.reminder_fired,
        created_at: note.created_at,
        updated_at: note.updated_at,
        tags,
    })
}

/// Write the versioned JSON copy with every note, tag and window property.
pub fn export_json(conn: &Connection, out: &Path) -> Result<usize, String> {
    let notes = db::list(conn).map_err(|error| error.to_string())?;
    let mut tags = db::all_tags(conn).map_err(|error| error.to_string())?;
    let mut exported = Vec::with_capacity(notes.len());
    for note in &notes {
        exported.push(export_one(note, tags.remove(&note.id).unwrap_or_default())?);
    }
    let file = BackupFile {
        format: FORMAT.to_string(),
        version: VERSION,
        exported_at: now_ms(),
        notes: exported,
    };
    let json = serde_json::to_string_pretty(&file)
        .map_err(|error| format!("No se pudo preparar la copia: {error}"))?;
    fs::write(out, json).map_err(|error| format!("No se pudo escribir el archivo: {error}"))?;
    Ok(file.notes.len())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '-' || value == '_')
}

/// Read a JSON copy and merge it into the database.
///
/// A note whose id already exists is skipped unless the imported row is
/// newer, in which case it replaces the local one.
pub fn import_json(conn: &Connection, path: &Path) -> Result<ImportSummary, String> {
    let raw =
        fs::read_to_string(path).map_err(|error| format!("No se pudo leer el archivo: {error}"))?;
    let file: BackupFile = serde_json::from_str(&raw)
        .map_err(|_| "El archivo no es una copia válida de Notas".to_string())?;
    if file.format != FORMAT {
        return Err("El archivo no es una copia válida de Notas".to_string());
    }
    if file.version > VERSION {
        return Err("La copia es de una versión más nueva de Notas".to_string());
    }
    if file.notes.iter().any(|note| !valid_id(&note.id)) {
        return Err("La copia contiene una nota con un identificador inválido".to_string());
    }

    let mut summary = ImportSummary {
        imported: 0,
        updated: 0,
        unchanged: 0,
    };
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("No se pudo iniciar la importación: {error}"))?;
    for exported in file.notes {
        let tags = exported.tags.clone();
        let note = to_note(exported)?;
        let existing = db::get(&transaction, &note.id).map_err(|error| error.to_string())?;
        match existing {
            Some(current) if note.updated_at <= current.updated_at => summary.unchanged += 1,
            Some(_) => {
                db::delete_note(&transaction, &note.id).map_err(|error| error.to_string())?;
                db::insert_note(&transaction, &note).map_err(|error| error.to_string())?;
                for tag in &tags {
                    db::add_tag(&transaction, &note.id, tag).map_err(|error| error.to_string())?;
                }
                summary.updated += 1;
            }
            None => {
                db::insert_note(&transaction, &note).map_err(|error| error.to_string())?;
                for tag in &tags {
                    db::add_tag(&transaction, &note.id, tag).map_err(|error| error.to_string())?;
                }
                summary.imported += 1;
            }
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("No se pudo guardar la importación: {error}"))?;
    Ok(summary)
}

// ------------------------------------------------------------------ schedule

/// Once a day, from a thread of its own: the first check runs at startup, so a
/// backup missed while the app was closed is written as soon as it opens.
pub fn start_scheduler(app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("notas-backups".to_string())
        .spawn(move || loop {
            run_once(&app);
            std::thread::sleep(CHECK_INTERVAL);
        })
        .ok();
}

fn run_once(app: &tauri::AppHandle) {
    use tauri::Manager;

    let data_dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("[notas] no se pudo leer la carpeta de datos: {error}");
            return;
        }
    };
    let state = app.state::<crate::state::AppState>();
    let conn = match state.conn() {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("[notas] {error}");
            return;
        }
    };
    if !is_due(&conn, &data_dir) {
        return;
    }
    match create(&conn, &data_dir) {
        Ok(path) => eprintln!("[notas] copia de seguridad: {}", path.display()),
        Err(error) => eprintln!("[notas] no se pudo hacer la copia de seguridad: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto;

    struct Temp {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    fn temp() -> Temp {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().to_path_buf();
        Temp { _dir: dir, root }
    }

    fn open(root: &Path) -> Connection {
        db::open(&root.join("notas.db")).expect("open db")
    }

    fn plain_note(conn: &Connection, id: &str, text: &str) -> Note {
        let mut note = Note::fresh(id.to_string(), 20.0, 30.0);
        note.color = "verde".to_string();
        db::insert_note(conn, &note).expect("insert");
        let content = Content {
            text: text.to_string(),
            items: Vec::new(),
        };
        db::save_plain_content(conn, id, MODE_TEXT, &content).expect("save");
        note
    }

    fn locked_note(conn: &Connection, id: &str, secret: &str, password: &str) {
        plain_note(conn, id, secret);
        // Read the row back: the saved text lives in the database, not in the struct.
        let note = db::require(conn, id).expect("note");
        db::add_tag(conn, id, "secreto").expect("tag");
        let content = note.content();
        let payload = serde_json::to_vec(&content).expect("json");
        let (_key, sealed) = crypto::seal_with_password(password, &payload).expect("seal");
        db::save_cipher(conn, id, &sealed.cipher, &sealed.salt, &sealed.nonce).expect("cipher");
    }

    #[test]
    fn rotation_keeps_the_seven_newest() {
        let temp = temp();
        let folder = dir(&temp.root);
        fs::create_dir_all(&folder).expect("mkdir");
        for day in 1..=9 {
            fs::write(folder.join(format!("notas-2026-01-0{day}.db")), b"x").expect("write");
        }
        // Files that are not daily backups must survive the rotation.
        fs::write(folder.join("notas-pre-restauracion-2026-01-01-000000.db"), b"x").expect("write");
        fs::write(folder.join("notas-2026-01-09.db.tmp"), b"x").expect("write");

        let removed = prune(&folder, KEEP);
        assert_eq!(removed.len(), 2, "nine dailies leave two over");
        let left = daily_names(&folder);
        assert_eq!(left.len(), 7);
        assert_eq!(left.first().map(String::as_str), Some("notas-2026-01-09.db"));
        assert_eq!(left.last().map(String::as_str), Some("notas-2026-01-03.db"));
        assert!(folder.join("notas-pre-restauracion-2026-01-01-000000.db").is_file());
        assert!(folder.join("notas-2026-01-09.db.tmp").is_file());
    }

    #[test]
    fn daily_backup_is_a_consistent_copy() {
        let temp = temp();
        let conn = open(&temp.root);
        plain_note(&conn, "nota-copia", "contenido vivo");
        let path = create(&conn, &temp.root).expect("backup");
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some(daily_name().as_str()));
        assert!(!is_due(&conn, &temp.root), "a fresh backup is not due");
        assert!(last_at(&conn, &temp.root).is_some());

        let copy = Connection::open(&path).expect("open copy");
        assert_eq!(db::count(&copy).expect("count"), 1);
        let note = db::require(&copy, "nota-copia").expect("note");
        assert_eq!(note.body_plain, "contenido vivo");
    }

    #[test]
    fn markdown_export_hides_locked_content() {
        const SECRET: &str = "SECRETO_V2A";
        let temp = temp();
        let conn = open(&temp.root);
        plain_note(&conn, "abierta", "Hola mundo");
        db::add_tag(&conn, "abierta", "ideas").expect("tag");
        locked_note(&conn, "cerrada", SECRET, "clave buena");

        let out = temp.root.join("notas.md");
        export_markdown(&conn, &out).expect("export");
        let text = fs::read_to_string(&out).expect("read");
        assert!(!text.contains(SECRET), "a locked note leaked into the Markdown export");
        assert!(text.contains("🔒 Nota bloqueada (no se exporta su contenido)"));
        assert!(text.contains("Hola mundo"));
        assert!(text.contains("Etiquetas: ideas"));
    }

    #[test]
    fn json_roundtrip_keeps_locked_notes_encrypted() {
        const SECRET: &str = "SECRETO_V2A";
        let temp = temp();
        let source = open(&temp.root);
        plain_note(&source, "abierta", "Hola mundo");
        db::add_tag(&source, "abierta", "ideas").expect("tag");
        db::set_color(&source, "abierta", "rosa").expect("color");
        db::set_geometry(&source, "abierta", 10.0, 20.0, 340.0, 300.0).expect("geometry");
        db::set_reminder(&source, "abierta", Some(1_800_000_000_000)).expect("reminder");
        locked_note(&source, "cerrada", SECRET, "clave buena");

        let out = temp.root.join("copia.json");
        export_json(&source, &out).expect("export");
        let raw = fs::read_to_string(&out).expect("read");
        assert!(!raw.contains(SECRET), "a locked note leaked into the JSON export");
        assert!(raw.contains("\"format\": \"notas-backup\""));

        // Wipe everything, then import into the same file.
        db::delete_note(&source, "abierta").expect("delete");
        db::delete_note(&source, "cerrada").expect("delete");
        assert_eq!(db::count(&source).expect("count"), 0);
        let summary = import_json(&source, &out).expect("import");
        assert_eq!(summary.imported, 2);
        assert_eq!(summary.updated, 0);
        assert_eq!(summary.unchanged, 0);

        let open_note = db::require(&source, "abierta").expect("note");
        assert_eq!(open_note.content().text, "Hola mundo");
        assert_eq!(open_note.color, "rosa");
        assert_eq!(
            (open_note.x, open_note.y, open_note.width, open_note.height),
            (10.0, 20.0, 340.0, 300.0)
        );
        assert_eq!(open_note.reminder_at, Some(1_800_000_000_000));
        assert_eq!(db::note_tags(&source, "abierta").expect("tags"), vec!["ideas"]);

        let locked = db::require(&source, "cerrada").expect("note");
        assert!(locked.locked);
        assert_eq!(locked.body_plain, "");
        let (_, plain) = crypto::open_with_password(
            "clave buena",
            locked.body_cipher.as_deref().expect("cipher"),
            locked.salt.as_deref().expect("salt"),
            locked.nonce.as_deref().expect("nonce"),
        )
        .expect("the imported locked note still decrypts");
        let content: Content = serde_json::from_slice(&plain).expect("json");
        assert_eq!(content.text, SECRET);
    }

    #[test]
    fn import_skips_equal_and_replaces_newer() {
        let temp = temp();
        let conn = open(&temp.root);
        plain_note(&conn, "vieja", "estado local");
        let local = db::require(&conn, "vieja").expect("note");

        let out = temp.root.join("copia.json");
        export_json(&conn, &out).expect("export");

        // Same row, older: imported into a fresh database.
        let other = temp.root.join("otra.db");
        let other_conn = db::open(&other).expect("open other");
        let mut file: BackupFile =
            serde_json::from_str(&fs::read_to_string(&out).expect("read")).expect("parse");
        file.notes[0].updated_at = local.updated_at - 1_000;
        file.notes[0].text = "mas viejo".to_string();
        let older = temp.root.join("vieja.json");
        fs::write(&older, serde_json::to_string(&file).expect("json")).expect("write");
        db::insert_note(&other_conn, &local).expect("seed");
        let summary = import_json(&other_conn, &older).expect("import");
        assert_eq!(summary.unchanged, 1);
        assert_eq!(
            db::require(&other_conn, "vieja").expect("note").content().text,
            "estado local"
        );

        // Same row, newer: replaces the local one.
        let mut file: BackupFile =
            serde_json::from_str(&fs::read_to_string(&out).expect("read")).expect("parse");
        file.notes[0].updated_at = local.updated_at + 1_000;
        file.notes[0].text = "mas nuevo".to_string();
        file.notes[0].tags = vec!["importada".to_string()];
        let newer = temp.root.join("nueva.json");
        fs::write(&newer, serde_json::to_string(&file).expect("json")).expect("write");
        let summary = import_json(&other_conn, &newer).expect("import");
        assert_eq!(summary.updated, 1);
        assert_eq!(
            db::require(&other_conn, "vieja").expect("note").content().text,
            "mas nuevo"
        );
        assert_eq!(
            db::note_tags(&other_conn, "vieja").expect("tags"),
            vec!["importada"]
        );

        // A note that does not exist yet is imported.
        let summary = import_json(&other_conn, &out).expect("import");
        assert_eq!(summary.imported, 0);
        assert_eq!(summary.unchanged, 1);
    }

    #[test]
    fn restore_replaces_the_live_data() {
        let temp = temp();
        let mut conn = open(&temp.root);
        plain_note(&conn, "original", "primera versión");
        let copy = create(&conn, &temp.root).expect("backup");
        db::delete_note(&conn, "original").expect("delete");
        plain_note(&conn, "posterior", "escrita después");
        assert_eq!(db::count(&conn).expect("count"), 1);

        let safety = safety_copy(&conn, &temp.root).expect("safety");
        assert!(safety.is_file());
        restore(&mut conn, &copy).expect("restore");
        assert_eq!(db::count(&conn).expect("count"), 1);
        assert!(db::get(&conn, "original").expect("get").is_some());
        assert!(db::get(&conn, "posterior").expect("get").is_none());
    }

    #[test]
    fn invalid_files_are_rejected() {
        let temp = temp();
        let conn = open(&temp.root);
        let missing = temp.root.join("nada.json");
        assert!(import_json(&conn, &missing).is_err());

        let wrong = temp.root.join("otro.json");
        fs::write(&wrong, r#"{"format":"otra-cosa","version":1,"exported_at":0,"notes":[]}"#)
            .expect("write");
        assert!(import_json(&conn, &wrong).is_err());
    }
}
