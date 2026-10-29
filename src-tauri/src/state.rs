//! Process-wide state: the SQLite connection and the derived keys of the notes
//! that were unlocked during this session.
//!
//! Keys never leave the Rust process: they are not written anywhere, and they
//! are zeroed when the note is locked again or the process exits.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

use crate::crypto::KEY_LEN;

/// A derived key that wipes itself when dropped.
pub struct SessionKey([u8; KEY_LEN]);

impl SessionKey {
    pub fn new(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl Drop for SessionKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub struct AppState {
    db: Mutex<Connection>,
    keys: Mutex<HashMap<String, SessionKey>>,
    exiting: AtomicBool,
}

impl AppState {
    pub fn new(conn: Connection) -> Self {
        Self {
            db: Mutex::new(conn),
            keys: Mutex::new(HashMap::new()),
            exiting: AtomicBool::new(false),
        }
    }

    pub fn conn(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.db
            .lock()
            .map_err(|_| "No se pudo acceder a la base de datos".to_string())
    }

    pub fn key(&self, note_id: &str) -> Option<[u8; KEY_LEN]> {
        self.keys
            .lock()
            .ok()
            .and_then(|keys| keys.get(note_id).map(|key| *key.bytes()))
    }

    pub fn has_key(&self, note_id: &str) -> bool {
        self.keys
            .lock()
            .map(|keys| keys.contains_key(note_id))
            .unwrap_or(false)
    }

    pub fn remember_key(&self, note_id: &str, key: [u8; KEY_LEN]) {
        if let Ok(mut keys) = self.keys.lock() {
            keys.insert(note_id.to_string(), SessionKey::new(key));
        }
    }

    /// Forget the key of one note, or all keys when `note_id` is `None`.
    pub fn forget_key(&self, note_id: Option<&str>) {
        if let Ok(mut keys) = self.keys.lock() {
            match note_id {
                Some(id) => {
                    keys.remove(id);
                }
                None => keys.clear(),
            }
        }
    }

    pub fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::SeqCst)
    }

    pub fn set_exiting(&self) {
        self.exiting.store(true, Ordering::SeqCst);
        self.forget_key(None);
    }
}
