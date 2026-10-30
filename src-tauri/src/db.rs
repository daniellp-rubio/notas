//! SQLite persistence: notes, tags, reminders and the schema migration.
//!
//! A note row keeps either plaintext (`body_plain` + `content_json` when the
//! note has no password, or while it is unlocked in memory) or the encrypted
//! payload (`body_cipher` + `salt` + `nonce`). A locked note never has
//! plaintext columns set.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Bump when the schema changes and add a migration step below.
pub const SCHEMA_VERSION: i32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("No se pudo acceder a la base de datos: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("La nota no existe")]
    NotFound,
}

pub type Result<T, E = DbError> = std::result::Result<T, E>;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// One checklist row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Item {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

/// Content of a note. Text notes keep sanitized HTML; checklist items stay plain text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Content {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub items: Vec<Item>,
    /// Missing from v1 encrypted payloads, which held plain text.
    #[serde(default)]
    pub rich_text: bool,
}

impl Content {
    /// Plain-text projection used for titles, search, mode conversion and Markdown.
    pub fn plain(&self, mode: &str) -> String {
        if mode == MODE_LIST {
            self.items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            html_to_text(&self.text)
        }
    }

    /// Turn the body into a checklist: every non-empty text line becomes an item.
    pub fn convert_to_list(&mut self) {
        if !self.items.is_empty() {
            return;
        }
        self.items = html_to_text(&self.text)
            .lines()
            .map(|line| line.trim_end())
            .filter(|line| !line.trim().is_empty())
            .map(|line| Item {
                text: line.to_string(),
                done: false,
            })
            .collect();
    }

    /// Turn the checklist back into rich-text lines (checkbox state is dropped).
    pub fn convert_to_text(&mut self) {
        self.text = plain_to_html(&self.plain(MODE_LIST));
        self.items.clear();
        self.rich_text = true;
    }
}

/// The only inline styles a note may retain. They are dark enough for every paper colour.
pub const FONT_SIZES: [&str; 3] = ["12px", "14px", "18px"];
pub const TEXT_COLORS: [&str; 5] = ["#1f1d1a", "#8b0000", "#003c8f", "#155724", "#5b5b5b"];

/// Escape legacy plain text and preserve every original line as a block.
pub fn plain_to_html(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let escaped = line
                .trim_end_matches('\r')
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            format!("<div>{escaped}</div>")
        })
        .collect()
}

/// Decode the entity forms emitted by the HTML serializer.
fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "#39" | "#x27" | "#X27" => Some('\''),
        _ => entity
            .strip_prefix("#x")
            .or_else(|| entity.strip_prefix("#X"))
            .and_then(|value| u32::from_str_radix(value, 16).ok())
            .or_else(|| entity.strip_prefix('#').and_then(|value| value.parse().ok()))
            .and_then(char::from_u32),
    }
}

/// Project sanitized HTML into readable text without relying on the frontend.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'<' {
            let Some(end) = html[index..].find('>') else {
                break;
            };
            let tag = html[index + 1..index + end].trim_start().to_ascii_lowercase();
            let name = tag
                .trim_start_matches('/')
                .split(|character: char| character.is_whitespace() || character == '/')
                .next()
                .unwrap_or_default();
            if matches!(name, "br" | "div" | "p" | "li") && !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            index += end + 1;
        } else if bytes[index] == b'&' {
            if let Some(end) = html[index..].find(';') {
                let entity = &html[index + 1..index + end];
                if let Some(character) = decode_entity(entity) {
                    out.push(character);
                    index += end + 1;
                    continue;
                }
            }
            out.push('&');
            index += 1;
        } else {
            let Some(character) = html[index..].chars().next() else {
                break;
            };
            out.push(character);
            index += character.len_utf8();
        }
    }
    out.trim_matches('\n').to_string()
}

fn allowed_style(style: &str) -> String {
    let mut kept = Vec::with_capacity(2);
    for declaration in style.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        let allowed = match property.as_str() {
            "font-size" => FONT_SIZES.contains(&value.as_str()),
            "color" => TEXT_COLORS.contains(&value.as_str()),
            _ => false,
        };
        if allowed && !kept.iter().any(|existing: &(String, String)| existing.0 == property) {
            kept.push((property, value));
        }
    }
    kept.into_iter()
        .map(|(property, value)| format!("{property}:{value}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// Sanitize a rich-text fragment. Structural parsing and CSS property filtering
/// are done by Ammonia; exact values are constrained afterwards.
pub fn sanitize_html(input: &str) -> String {
    let tags = HashSet::from([
        "p", "br", "div", "b", "strong", "i", "em", "u", "s", "strike", "ul", "ol", "li", "span",
    ]);
    let attributes = HashMap::from([("span", HashSet::from(["style"]))]);
    let mut cleaner = ammonia::Builder::default();
    cleaner
        .tags(tags)
        .tag_attributes(attributes)
        .filter_style_properties(HashSet::from(["font-size", "color"]));
    let html = cleaner.clean(input).to_string();
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    while let Some(start) = rest.find("style=\"") {
        let value_start = start + "style=\"".len();
        let Some(end) = rest[value_start..].find('"') else {
            return html;
        };
        out.push_str(&rest[..start]);
        let style = allowed_style(&rest[value_start..value_start + end]);
        if !style.is_empty() {
            out.push_str("style=\"");
            out.push_str(&style);
            out.push('"');
        }
        rest = &rest[value_start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// Normalize user-provided text before any plaintext or encrypted save.
pub fn normalize_content(content: &mut Content, mode: &str) {
    if mode == MODE_TEXT {
        content.text = sanitize_html(&content.text);
        content.rich_text = true;
    }
}

pub const MODE_TEXT: &str = "texto";
pub const MODE_LIST: &str = "lista";
pub const COLORS: [&str; 6] = ["amarillo", "rosa", "verde", "azul", "morado", "naranja"];

#[derive(Debug, Clone)]
pub struct Note {
    pub id: String,
    pub color: String,
    pub mode: String,
    pub body_plain: String,
    pub content_json: String,
    pub body_cipher: Option<Vec<u8>>,
    pub salt: Option<Vec<u8>>,
    pub nonce: Option<Vec<u8>>,
    pub locked: bool,
    pub always_on_top: bool,
    /// User intent. Attachment rules may hide a window without changing this.
    pub visible: bool,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub reminder_at: Option<i64>,
    pub reminder_fired: bool,
    pub attach_kind: Option<String>,
    pub attach_value: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Note {
    /// A brand-new plaintext note at the given position.
    pub fn fresh(id: String, x: f64, y: f64) -> Self {
        let now = now_ms();
        Self {
            id,
            color: "amarillo".to_string(),
            mode: MODE_TEXT.to_string(),
            body_plain: String::new(),
            content_json: serde_json::to_string(&Content {
                rich_text: true,
                ..Content::default()
            })
            .unwrap_or_else(|_| "{}".into()),
            body_cipher: None,
            salt: None,
            nonce: None,
            locked: false,
            always_on_top: false,
            visible: true,
            x,
            y,
            width: 300.0,
            height: 260.0,
            reminder_at: None,
            reminder_fired: false,
            attach_kind: None,
            attach_value: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Plaintext content when the note is not encrypted.
    pub fn content(&self) -> Content {
        let has_rich_text = serde_json::from_str::<serde_json::Value>(&self.content_json)
            .ok()
            .and_then(|value| value.get("text").cloned())
            .is_some();
        let mut content: Content = serde_json::from_str(&self.content_json).unwrap_or_default();
        if !has_rich_text {
            content.text = self.body_plain.clone();
        }
        content
    }

    /// First line, used as the title in the notes list.
    pub fn title(&self) -> String {
        let content = self.content();
        let first = if self.mode == MODE_LIST {
            content
                .items
                .iter()
                .find(|item| !item.text.trim().is_empty())
                .map(|item| item.text.clone())
                .unwrap_or_default()
        } else {
            html_to_text(&content.text)
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .to_string()
        };
        first.trim().chars().take(80).collect()
    }
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS notes (
    id              TEXT PRIMARY KEY,
    color           TEXT NOT NULL DEFAULT 'amarillo',
    mode            TEXT NOT NULL DEFAULT 'texto',
    body_plain      TEXT NOT NULL DEFAULT '',
    content_json    TEXT NOT NULL DEFAULT '{}',
    body_cipher     BLOB,
    salt            BLOB,
    nonce           BLOB,
    locked          INTEGER NOT NULL DEFAULT 0,
    always_on_top   INTEGER NOT NULL DEFAULT 0,
    visible         INTEGER NOT NULL DEFAULT 1,
    x               REAL NOT NULL DEFAULT 160,
    y               REAL NOT NULL DEFAULT 140,
    width           REAL NOT NULL DEFAULT 300,
    height          REAL NOT NULL DEFAULT 260,
    reminder_at     INTEGER,
    reminder_fired  INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tags (
    note_id TEXT NOT NULL,
    tag     TEXT NOT NULL,
    PRIMARY KEY (note_id, tag),
    FOREIGN KEY (note_id) REFERENCES notes(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tags_note ON tags(note_id);
CREATE INDEX IF NOT EXISTS idx_notes_reminder ON notes(reminder_at);
"#;

/// Open (or create) the database file and bring the schema up to date.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Idempotent migration driven by `PRAGMA user_version`.
pub fn migrate(conn: &Connection) -> Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA_V1)?;
    }
    if version < 2 {
        migrate_plain_text_to_html(conn)?;
    }
    if version < 3 {
        add_attachment_columns(conn)?;
    }
    if version < 4 {
        create_sync_tables(conn)?;
    }
    if version < SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    // Repairs manually-created databases and makes every migration idempotent.
    conn.execute_batch(SCHEMA_V1)?;
    add_attachment_columns(conn)?;
    create_sync_tables(conn)?;
    Ok(())
}

fn has_column(conn: &Connection, name: &str) -> Result<bool> {
    let mut statement = conn.prepare("PRAGMA table_info(notes)")?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for column in columns {
        if column? == name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn add_attachment_columns(conn: &Connection) -> Result<()> {
    if !has_column(conn, "attach_kind")? {
        conn.execute("ALTER TABLE notes ADD COLUMN attach_kind TEXT", [])?;
    }
    if !has_column(conn, "attach_value")? {
        conn.execute("ALTER TABLE notes ADD COLUMN attach_value TEXT", [])?;
    }
    Ok(())
}

fn create_sync_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_state (
            note_id TEXT PRIMARY KEY,
            last_synced_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sync_tombstones (
            note_id TEXT PRIMARY KEY,
            deleted_at INTEGER NOT NULL,
            device_id TEXT NOT NULL
        );",
    )?;
    Ok(())
}

/// Convert v1 plaintext text notes once. Locked payloads carry their own
/// `rich_text` marker and are converted the first time they are unlocked.
fn migrate_plain_text_to_html(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT id, mode, body_plain, content_json FROM notes WHERE locked = 0 AND mode = ?1",
    )?;
    let rows = statement.query_map(params![MODE_TEXT], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut updates = Vec::new();
    for row in rows {
        let (id, mode, plain, content_json) = row?;
        let mut content: Content = serde_json::from_str(&content_json).unwrap_or_default();
        content.text = plain_to_html(&plain);
        content.rich_text = true;
        updates.push((id, mode, content, plain));
    }
    drop(statement);
    for (id, mode, content, plain) in updates {
        let payload = serde_json::to_string(&content)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        conn.execute(
            "UPDATE notes SET mode = ?2, body_plain = ?3, content_json = ?4 WHERE id = ?1",
            params![id, mode, plain, payload],
        )?;
    }
    Ok(())
}

fn map_note(row: &Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get("id")?,
        color: row.get("color")?,
        mode: row.get("mode")?,
        body_plain: row.get("body_plain")?,
        content_json: row.get("content_json")?,
        body_cipher: row.get("body_cipher")?,
        salt: row.get("salt")?,
        nonce: row.get("nonce")?,
        locked: row.get::<_, i64>("locked")? != 0,
        always_on_top: row.get::<_, i64>("always_on_top")? != 0,
        visible: row.get::<_, i64>("visible")? != 0,
        x: row.get("x")?,
        y: row.get("y")?,
        width: row.get("width")?,
        height: row.get("height")?,
        reminder_at: row.get("reminder_at")?,
        reminder_fired: row.get::<_, i64>("reminder_fired")? != 0,
        attach_kind: row.get("attach_kind")?,
        attach_value: row.get("attach_value")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

pub fn insert_note(conn: &Connection, note: &Note) -> Result<()> {
    conn.execute(
        "INSERT INTO notes (id, color, mode, body_plain, content_json, body_cipher, salt, nonce,
                            locked, always_on_top, visible, x, y, width, height, reminder_at,
                            reminder_fired, attach_kind, attach_value, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
        params![
            note.id,
            note.color,
            note.mode,
            note.body_plain,
            note.content_json,
            note.body_cipher,
            note.salt,
            note.nonce,
            i64::from(note.locked),
            i64::from(note.always_on_top),
            i64::from(note.visible),
            note.x,
            note.y,
            note.width,
            note.height,
            note.reminder_at,
            i64::from(note.reminder_fired),
            note.attach_kind,
            note.attach_value,
            note.created_at,
            note.updated_at,
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Note>> {
    let mut stmt = conn.prepare("SELECT * FROM notes WHERE id = ?1")?;
    let note = stmt.query_row(params![id], map_note).optional()?;
    Ok(note)
}

pub fn require(conn: &Connection, id: &str) -> Result<Note> {
    get(conn, id)?.ok_or(DbError::NotFound)
}

pub fn list(conn: &Connection) -> Result<Vec<Note>> {
    let mut stmt = conn.prepare("SELECT * FROM notes ORDER BY created_at DESC")?;
    let rows = stmt.query_map([], map_note)?;
    let mut notes = Vec::new();
    for note in rows {
        notes.push(note?);
    }
    Ok(notes)
}

pub fn count(conn: &Connection) -> Result<i64> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))?;
    Ok(total)
}

/// Normalize one plaintext payload and derive its searchable projection.
fn plaintext_parts(content: &Content, mode: &str) -> Result<(String, String)> {
    let mut normalized = content.clone();
    normalize_content(&mut normalized, mode);
    let plain = normalized.plain(mode);
    let payload = serde_json::to_string(&normalized)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    Ok((plain, payload))
}

/// Write plaintext content: `body_plain` mirrors the readable text, while
/// `content_json` holds the sanitized HTML and checklist rows.
pub fn save_plain_content(conn: &Connection, id: &str, mode: &str, content: &Content) -> Result<()> {
    let (plain, payload) = plaintext_parts(content, mode)?;
    conn.execute(
        "UPDATE notes SET mode = ?2, body_plain = ?3, content_json = ?4, updated_at = ?5 WHERE id = ?1",
        params![id, mode, plain, payload, now_ms()],
    )?;
    Ok(())
}

/// Write encrypted content and wipe every plaintext column.
pub fn save_cipher(
    conn: &Connection,
    id: &str,
    cipher: &[u8],
    salt: &[u8],
    nonce: &[u8],
) -> Result<()> {
    conn.execute(
        "UPDATE notes SET locked = 1, body_cipher = ?2, salt = ?3, nonce = ?4,
                body_plain = '', content_json = '', updated_at = ?5
         WHERE id = ?1",
        params![id, cipher, salt, nonce, now_ms()],
    )?;
    Ok(())
}

/// Remove the password from a note: store the recovered plaintext and drop the cipher.
pub fn clear_lock(conn: &Connection, id: &str, mode: &str, content: &Content) -> Result<()> {
    let (plain, payload) = plaintext_parts(content, mode)?;
    conn.execute(
        "UPDATE notes SET locked = 0, body_cipher = NULL, salt = NULL, nonce = NULL,
                body_plain = ?2, content_json = ?3, updated_at = ?4
         WHERE id = ?1",
        params![id, plain, payload, now_ms()],
    )?;
    Ok(())
}

pub fn set_mode(conn: &Connection, id: &str, mode: &str) -> Result<()> {
    conn.execute(
        "UPDATE notes SET mode = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, mode, now_ms()],
    )?;
    Ok(())
}

pub fn set_color(conn: &Connection, id: &str, color: &str) -> Result<()> {
    conn.execute(
        "UPDATE notes SET color = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, color, now_ms()],
    )?;
    Ok(())
}

pub fn set_pin(conn: &Connection, id: &str, pin: bool) -> Result<()> {
    conn.execute(
        "UPDATE notes SET always_on_top = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, i64::from(pin), now_ms()],
    )?;
    Ok(())
}

/// Visibility and geometry are local-device state. They deliberately do not
/// advance `updated_at`, so a cloud sync never overwrites another device's layout.
pub fn set_visible(conn: &Connection, id: &str, visible: bool) -> Result<()> {
    conn.execute(
        "UPDATE notes SET visible = ?2 WHERE id = ?1",
        params![id, i64::from(visible)],
    )?;
    Ok(())
}

pub fn set_geometry(
    conn: &Connection,
    id: &str,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<()> {
    conn.execute(
        "UPDATE notes SET x = ?2, y = ?3, width = ?4, height = ?5 WHERE id = ?1",
        params![id, x, y, width, height],
    )?;
    Ok(())
}

pub fn set_attachment(
    conn: &Connection,
    id: &str,
    kind: Option<&str>,
    value: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE notes SET attach_kind = ?2, attach_value = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, kind, value, now_ms()],
    )?;
    Ok(())
}

pub fn sync_last_synced(conn: &Connection, id: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT last_synced_at FROM sync_state WHERE note_id = ?1",
        params![id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn sync_mark_synced(conn: &Connection, id: &str, at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_state (note_id, last_synced_at) VALUES (?1, ?2)
         ON CONFLICT(note_id) DO UPDATE SET last_synced_at = excluded.last_synced_at",
        params![id, at],
    )?;
    Ok(())
}

pub fn sync_tombstones(conn: &Connection) -> Result<Vec<(String, i64, String)>> {
    let mut statement = conn.prepare(
        "SELECT note_id, deleted_at, device_id FROM sync_tombstones ORDER BY deleted_at ASC",
    )?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let mut tombstones = Vec::new();
    for tombstone in rows {
        tombstones.push(tombstone?);
    }
    Ok(tombstones)
}

pub fn sync_set_tombstone(conn: &Connection, id: &str, deleted_at: i64, device_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_tombstones (note_id, deleted_at, device_id) VALUES (?1, ?2, ?3)
         ON CONFLICT(note_id) DO UPDATE SET deleted_at = excluded.deleted_at, device_id = excluded.device_id",
        params![id, deleted_at, device_id],
    )?;
    Ok(())
}

pub fn sync_remove_tombstone(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM sync_tombstones WHERE note_id = ?1", params![id])?;
    Ok(())
}

pub fn sync_prune_tombstones(conn: &Connection, older_than: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM sync_tombstones WHERE deleted_at < ?1",
        params![older_than],
    )?;
    Ok(())
}

/// Set (or clear, with `None`) the reminder. Any change re-arms the note.
pub fn set_reminder(conn: &Connection, id: &str, at: Option<i64>) -> Result<()> {
    conn.execute(
        "UPDATE notes SET reminder_at = ?2, reminder_fired = 0, updated_at = ?3 WHERE id = ?1",
        params![id, at, now_ms()],
    )?;
    Ok(())
}

pub fn mark_reminder_fired(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE notes SET reminder_fired = 1, updated_at = ?2 WHERE id = ?1",
        params![id, now_ms()],
    )?;
    Ok(())
}

/// Notes whose reminder is due and has not fired yet.
pub fn due_reminders(conn: &Connection, now: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM notes
         WHERE reminder_at IS NOT NULL AND reminder_at <= ?1 AND reminder_fired = 0
         ORDER BY reminder_at ASC",
    )?;
    let rows = stmt.query_map(params![now], |row| row.get::<_, String>(0))?;
    let mut ids = Vec::new();
    for id in rows {
        ids.push(id?);
    }
    Ok(ids)
}

pub fn delete_note(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
    Ok(())
}

/// Tags of one note, case-insensitively sorted.
pub fn note_tags(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT tag FROM tags WHERE note_id = ?1 ORDER BY tag COLLATE NOCASE ASC",
    )?;
    let rows = stmt.query_map(params![id], |row| row.get::<_, String>(0))?;
    let mut tags = Vec::new();
    for tag in rows {
        tags.push(tag?);
    }
    Ok(tags)
}

/// All tags grouped by note id.
pub fn all_tags(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare("SELECT note_id, tag FROM tags ORDER BY tag COLLATE NOCASE ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (note_id, tag) = row?;
        map.entry(note_id).or_default().push(tag);
    }
    Ok(map)
}

/// Add a tag unless the note already has it (case-insensitive). Returns the tag list.
pub fn add_tag(conn: &Connection, id: &str, tag: &str) -> Result<Vec<String>> {
    let tag = tag.trim();
    if tag.is_empty() {
        return note_tags(conn, id);
    }
    require(conn, id)?;
    let exists: Option<String> = conn
        .query_row(
            "SELECT tag FROM tags WHERE note_id = ?1 AND lower(tag) = lower(?2)",
            params![id, tag],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        conn.execute(
            "INSERT INTO tags (note_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    note_tags(conn, id)
}

pub fn remove_tag(conn: &Connection, id: &str, tag: &str) -> Result<Vec<String>> {
    conn.execute(
        "DELETE FROM tags WHERE note_id = ?1 AND lower(tag) = lower(?2)",
        params![id, tag],
    )?;
    note_tags(conn, id)
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let value = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    Ok(value)
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDb {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
    }

    fn temp_db() -> TempDb {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("notas.db");
        TempDb { _dir: dir, path }
    }

    fn sample(id: &str) -> Note {
        Note::fresh(id.to_string(), 40.0, 60.0)
    }

    #[test]
    fn create_update_reopen_preserves_everything() {
        let db = temp_db();
        {
            let conn = open(&db.path).expect("open");
            let note = sample("nota-1");
            insert_note(&conn, &note).expect("insert");

            let content = Content {
                text: "Primera línea\nSegunda línea".to_string(),
                items: Vec::new(),
                rich_text: true,
            };
            save_plain_content(&conn, "nota-1", MODE_TEXT, &content).expect("content");
            set_color(&conn, "nota-1", "verde").expect("color");
            set_geometry(&conn, "nota-1", 12.0, 34.0, 320.0, 280.0).expect("geometry");
            add_tag(&conn, "nota-1", "Trabajo").expect("tag");
            add_tag(&conn, "nota-1", "trabajo").expect("dup tag");
            add_tag(&conn, "nota-1", "ideas").expect("tag 2");
        }

        let conn = open(&db.path).expect("reopen");
        let note = require(&conn, "nota-1").expect("load");
        assert_eq!(note.body_plain, "Primera línea\nSegunda línea");
        assert_eq!(note.content().text, "Primera línea\nSegunda línea");
        assert_eq!(note.color, "verde");
        assert_eq!((note.x, note.y, note.width, note.height), (12.0, 34.0, 320.0, 280.0));
        assert_eq!(note.title(), "Primera línea");
        assert_eq!(note_tags(&conn, "nota-1").expect("tags"), vec!["ideas", "Trabajo"]);
    }

    #[test]
    fn delete_removes_note_and_tags() {
        let db = temp_db();
        let conn = open(&db.path).expect("open");
        insert_note(&conn, &sample("nota-2")).expect("insert");
        add_tag(&conn, "nota-2", "borrar").expect("tag");
        assert_eq!(count(&conn).expect("count"), 1);

        delete_note(&conn, "nota-2").expect("delete");
        assert_eq!(count(&conn).expect("count"), 0);
        assert!(get(&conn, "nota-2").expect("get").is_none());
        assert!(note_tags(&conn, "nota-2").expect("tags").is_empty());
    }

    #[test]
    fn locked_row_keeps_no_plaintext() {
        let db = temp_db();
        let conn = open(&db.path).expect("open");
        const SECRET: &str = "SECRETO_V2B";
        let mut note = sample("nota-3");
        note.body_plain = SECRET.to_string();
        insert_note(&conn, &note).expect("insert");
        let html = format!("<div><b>{SECRET}</b></div>");
        let content = Content {
            text: html.clone(),
            items: Vec::new(),
            rich_text: true,
        };
        save_plain_content(&conn, "nota-3", MODE_TEXT, &content).expect("content");
        assert!(require(&conn, "nota-3").expect("load").body_plain.contains(SECRET));

        let (key, sealed) = crate::crypto::seal_with_password(
            "secreto",
            serde_json::to_vec(&content).expect("json").as_slice(),
        )
        .expect("seal");
        save_cipher(&conn, "nota-3", &sealed.cipher, &sealed.salt, &sealed.nonce).expect("cipher");

        let locked = require(&conn, "nota-3").expect("load");
        assert!(locked.locked);
        assert_eq!(locked.body_plain, "");
        assert_eq!(locked.content_json, "");
        assert!(locked.body_cipher.is_some());
        assert!(locked.salt.is_some());
        assert!(locked.nonce.is_some());

        // The bytes on disk must not contain the marker.
        drop(conn);
        let bytes = std::fs::read(&db.path).expect("read db");
        assert!(
            !bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
            "plaintext leaked into the database file"
        );

        // And the ciphertext opens with the right password only.
        let conn = open(&db.path).expect("reopen");
        let row = require(&conn, "nota-3").expect("load");
        let (_, plain) = crate::crypto::open_with_password(
            "secreto",
            row.body_cipher.as_deref().expect("cipher"),
            row.salt.as_deref().expect("salt"),
            row.nonce.as_deref().expect("nonce"),
        )
        .expect("open");
        let recovered: Content = serde_json::from_slice(&plain).expect("json");
        assert_eq!(recovered.text, html);
        let _ = key;
    }

    #[test]
    fn due_reminders_only_returns_pending_and_past() {
        let db = temp_db();
        let conn = open(&db.path).expect("open");
        for id in ["pasada", "futura", "sin-fecha", "ya-disparada"] {
            insert_note(&conn, &sample(id)).expect("insert");
        }
        let now = 1_000_000;
        set_reminder(&conn, "pasada", Some(now - 5_000)).expect("set");
        set_reminder(&conn, "futura", Some(now + 60_000)).expect("set");
        set_reminder(&conn, "ya-disparada", Some(now - 5_000)).expect("set");
        mark_reminder_fired(&conn, "ya-disparada").expect("fired");

        let due = due_reminders(&conn, now).expect("due");
        assert_eq!(due, vec!["pasada".to_string()]);

        // Firing marks it; a second pass returns nothing.
        mark_reminder_fired(&conn, "pasada").expect("fired");
        assert!(due_reminders(&conn, now).expect("due").is_empty());
        // Re-arming with the same time makes it due again.
        set_reminder(&conn, "pasada", Some(now - 1)).expect("rearm");
        assert_eq!(due_reminders(&conn, now).expect("due"), vec!["pasada".to_string()]);
    }

    #[test]
    fn reminder_due_after_restart_fires_once() {
        let db = temp_db();
        {
            let conn = open(&db.path).expect("open");
            insert_note(&conn, &sample("recordar")).expect("insert");
            set_reminder(&conn, "recordar", Some(1_000)).expect("set");
        }
        let conn = open(&db.path).expect("reopen");
        let due = due_reminders(&conn, 5_000).expect("due");
        assert_eq!(due, vec!["recordar".to_string()]);
        mark_reminder_fired(&conn, "recordar").expect("fired");
        // Simulate the next app start: nothing left to fire.
        drop(conn);
        let conn = open(&db.path).expect("reopen");
        assert!(due_reminders(&conn, 5_000).expect("due").is_empty());
    }

    #[test]
    fn mode_conversion_roundtrip() {
        let mut content = Content {
            text: plain_to_html("uno\n\n dos \ntres"),
            items: Vec::new(),
            rich_text: true,
        };
        content.convert_to_list();
        assert_eq!(content.items.len(), 3);
        assert_eq!(content.items[1].text, " dos");
        assert_eq!(content.plain(MODE_LIST), "uno\n dos\ntres");
        content.items[2].done = true;
        content.convert_to_text();
        assert!(content.items.is_empty());
        assert_eq!(content.text, "<div>uno</div><div> dos</div><div>tres</div>");
    }

    #[test]
    fn sanitizer_keeps_only_the_rich_text_allowlist() {
        let clean = sanitize_html(
            r#"<b>Hola</b> <img src=x onerror=alert(1)><script>alert(2)</script><span style="color:#8b0000;position:fixed">rojo</span><a href="javascript:alert(3)">enlace</a>"#,
        );
        assert_eq!(clean, r#"<b>Hola</b> <span style="color:#8b0000">rojo</span>enlace"#);
        assert!(!clean.contains("img"));
        assert!(!clean.contains("script"));
        assert!(!clean.contains("onerror"));
        assert!(!clean.contains("javascript:"));
    }

    #[test]
    fn html_projection_supplies_title_and_searchable_body() {
        let mut note = sample("formato");
        let content = Content {
            text: "<div>Primer <b>título</b></div><div>palabra <i>buscable</i></div>".to_string(),
            items: Vec::new(),
            rich_text: true,
        };
        note.content_json = serde_json::to_string(&content).expect("json");
        note.body_plain = content.plain(MODE_TEXT);
        assert_eq!(note.title(), "Primer título");
        assert_eq!(note.body_plain, "Primer título\npalabra buscable");
    }

    #[test]
    fn migration_escapes_plain_text_once_and_is_idempotent() {
        let db = temp_db();
        let conn = Connection::open(&db.path).expect("raw open");
        conn.execute_batch(SCHEMA_V1).expect("v1 schema");
        conn.pragma_update(None, "user_version", 1).expect("v1 version");
        conn.execute(
            "INSERT INTO notes (id, body_plain, content_json, created_at, updated_at)
             VALUES ('v1', 'uno < dos & tres\ndos', '{\"items\":[]}', 1, 1)",
            [],
        )
        .expect("v1 note");

        migrate(&conn).expect("migrate");
        let once = require(&conn, "v1").expect("migrated");
        assert_eq!(
            once.content().text,
            "<div>uno &lt; dos &amp; tres</div><div>dos</div>"
        );
        assert_eq!(once.body_plain, "uno < dos & tres\ndos");
        migrate(&conn).expect("second migration");
        let twice = require(&conn, "v1").expect("same row");
        assert_eq!(twice.content().text, once.content().text);
    }

    #[test]
    fn migration_is_idempotent() {
        let db = temp_db();
        let conn = open(&db.path).expect("open");
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, SCHEMA_VERSION);
        migrate(&conn).expect("second migrate");
        migrate(&conn).expect("third migrate");
        assert_eq!(count(&conn).expect("count"), 0);
    }
}
