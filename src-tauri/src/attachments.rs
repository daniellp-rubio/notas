//! Window attachment rules for notes.
//!
//! Foreground observation runs on a sleeping worker thread; it never creates a
//! WebView directly.  The stored `visible` field remains the user's intent,
//! while this module temporarily shows or hides attached note windows.

use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::db::{self, Note};
use crate::state::AppState;
use crate::windows;

pub const KIND_EXE: &str = "exe";
pub const KIND_TITLE: &str = "title";
const INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub title: String,
    pub exe: String,
}

pub fn rule_label(note: &Note) -> Option<String> {
    match (note.attach_kind.as_deref(), note.attach_value.as_deref()) {
        (Some(KIND_EXE), Some(value)) => Some(value.to_ascii_uppercase()),
        (Some(KIND_TITLE), Some(value)) => Some(value.to_string()),
        _ => None,
    }
}

pub fn matches(kind: &str, value: &str, foreground: &Candidate) -> bool {
    match kind {
        KIND_EXE => foreground.exe.eq_ignore_ascii_case(value),
        KIND_TITLE => foreground
            .title
            .to_ascii_lowercase()
            .contains(&value.to_ascii_lowercase()),
        _ => false,
    }
}

pub fn list_candidates() -> Result<Vec<Candidate>, String> {
    platform::list_candidates()
}

fn foreground() -> Option<Candidate> {
    platform::foreground()
}

/// A low-frequency poll is intentionally used instead of keeping a global
/// WinEvent callback alive. It has no work while the foreground is unchanged,
/// and keeps WebView calls on their normal event-loop path.
pub fn start(app: AppHandle) {
    thread::Builder::new()
        .name("notas-attachments".to_string())
        .spawn(move || loop {
            refresh(&app);
            thread::sleep(INTERVAL);
        })
        .ok();
}

pub fn refresh(app: &AppHandle) {
    let active = foreground();
    // Do not hide a note while any of our windows has focus. This lets the
    // user edit an attached note without the foreground transition hiding it.
    if platform::foreground_is_ours() {
        return;
    }
    let notes = {
        let state = app.state::<AppState>();
        let Ok(conn) = state.conn() else { return };
        match db::list(&conn) {
            Ok(notes) => notes,
            Err(error) => {
                eprintln!("[notas] no se pudieron leer reglas de ventana: {error}");
                return;
            }
        }
    };
    for note in notes {
        let (Some(kind), Some(value)) = (note.attach_kind.as_deref(), note.attach_value.as_deref()) else {
            continue;
        };
        let show = note.visible && active.as_ref().is_some_and(|current| matches(kind, value, current));
        if show {
            windows::show_attached_note(app, &note.id);
        } else {
            windows::hide_attached_note(app, &note.id, note.always_on_top);
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;
    use std::path::Path;

    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    use super::Candidate;

    struct Collector {
        candidates: Vec<Candidate>,
        executables: HashMap<u32, String>,
    }

    fn details(hwnd: HWND, executables: &mut HashMap<u32, String>) -> Option<Candidate> {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return None;
            }
            let length = GetWindowTextLengthW(hwnd);
            if length <= 0 {
                return None;
            }
            let mut title = vec![0u16; length as usize + 1];
            let written = GetWindowTextW(hwnd, &mut title);
            if written <= 0 {
                return None;
            }
            title.truncate(written as usize);
            let title = String::from_utf16_lossy(&title).trim().to_string();
            if title.is_empty() {
                return None;
            }
            let mut process_id = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut process_id));
            if process_id == std::process::id() {
                return None;
            }
            let exe = if let Some(exe) = executables.get(&process_id) {
                exe.clone()
            } else {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
                let mut path = vec![0u16; 32_768];
                let mut size = path.len() as u32;
                let ok = QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    PWSTR(path.as_mut_ptr()),
                    &mut size,
                )
                .is_ok();
                let _ = CloseHandle(process);
                if !ok || size == 0 {
                    return None;
                }
                path.truncate(size as usize);
                let path = String::from_utf16_lossy(&path);
                let exe = Path::new(&path).file_name()?.to_string_lossy().to_string();
                executables.insert(process_id, exe.clone());
                exe
            };
            Some(Candidate { title, exe })
        }
    }

    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let collector = &mut *(data.0 as *mut Collector);
        if let Some(candidate) = details(hwnd, &mut collector.executables) {
            collector.candidates.push(candidate);
        }
        BOOL(1)
    }

    pub fn list_candidates() -> Result<Vec<Candidate>, String> {
        let mut collector = Collector {
            candidates: Vec::new(),
            executables: HashMap::new(),
        };
        unsafe {
            EnumWindows(
                Some(visit),
                LPARAM((&mut collector as *mut Collector) as isize),
            )
            .map_err(|error| error.to_string())?;
        }
        collector
            .candidates
            .sort_by_key(|candidate| candidate.title.to_lowercase());
        collector.candidates.dedup_by(|left, right| {
            left.title == right.title && left.exe.eq_ignore_ascii_case(&right.exe)
        });
        Ok(collector.candidates)
    }

    pub fn foreground() -> Option<Candidate> {
        unsafe { details(GetForegroundWindow(), &mut HashMap::new()) }
    }

    pub fn foreground_is_ours() -> bool {
        unsafe {
            let mut process_id = 0;
            GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut process_id));
            process_id == std::process::id()
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Candidate;

    pub fn list_candidates() -> Result<Vec<Candidate>, String> {
        Err("Pegar a una ventana solo está disponible en Windows".to_string())
    }

    pub fn foreground() -> Option<Candidate> {
        None
    }

    pub fn foreground_is_ours() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_is_case_insensitive_for_programs_and_titles() {
        let current = Candidate {
            title: "Proyecto - Bloc de notas".to_string(),
            exe: "NOTEPAD.EXE".to_string(),
        };
        assert!(matches(KIND_EXE, "notepad.exe", &current));
        assert!(matches(KIND_TITLE, "proyecto - BLOC", &current));
        assert!(!matches(KIND_TITLE, "otro", &current));
    }
}
