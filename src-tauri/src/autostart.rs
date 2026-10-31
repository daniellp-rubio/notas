//! "Iniciar con Windows": launch the app when the user logs in.
//!
//! The state lives in the operating system, never in our own settings: a
//! shortcut `Notas.lnk` in the user's Startup folder
//! (`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup`). Every decision
//! reads whether that shortcut exists, so the tray check and the settings window
//! can never drift from what Windows will actually do.
//!
//! A Startup-folder shortcut is used instead of a `HKCU\...\Run` value: an
//! unsigned program that writes itself into the `Run` key is the exact pattern
//! Microsoft Defender's behavioral model flags as persistence malware
//! (`Behavior:Win32/Persistence.A!ml`), and it quarantined Notas for it. Older
//! versions wrote that `Run` value; it is migrated once to the shortcut.
//!
//! The first run enables it once and records that it did, so a later opt-out is
//! never overridden. Failures are logged and the app keeps running.

use rusqlite::Connection;
use tauri::AppHandle;

use crate::db;

/// `meta` key: the first-run default was already applied. Written only after a
/// successful enable, so a failed attempt is retried on the next start.
const INITIALIZED_KEY: &str = "autostart_initialized";

/// What Windows will do at the next sign-in.
pub fn is_enabled(_app: &AppHandle) -> Result<bool, String> {
    platform::shortcut_exists()
}

/// Enable or disable autostart and return the state the OS reports afterwards,
/// which is what the caller must show: an error only means "nothing changed".
pub fn set_enabled(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let changed = if enabled {
        platform::create_shortcut()
    } else {
        platform::remove_shortcut()
    };
    if let Err(error) = changed {
        eprintln!("[notas] no se pudo cambiar el inicio con Windows: {error}");
    }
    is_enabled(app)
}

/// Move a `Run` value left by an older version to the Startup shortcut, then
/// apply the first-run default once. It never overrides a later user choice.
pub fn ensure_initial_default(app: &AppHandle, conn: &Connection) {
    match platform::remove_legacy_run_value() {
        Ok(true) => {
            if let Err(error) = platform::create_shortcut() {
                eprintln!("[notas] no se pudo migrar el inicio con Windows: {error}");
            }
        }
        Ok(false) => {}
        Err(error) => eprintln!("[notas] no se pudo leer el inicio antiguo: {error}"),
    }

    match db::meta_get(conn, INITIALIZED_KEY) {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(error) => {
            eprintln!("[notas] no se pudo leer el estado del inicio con Windows: {error}");
            return;
        }
    }

    if let Err(error) = platform::create_shortcut() {
        eprintln!("[notas] no se pudo activar el inicio con Windows: {error}");
    }
    if !is_enabled(app).unwrap_or(false) {
        return;
    }
    if let Err(error) = db::meta_set(conn, INITIALIZED_KEY, "1") {
        eprintln!("[notas] no se pudo guardar el estado del inicio con Windows: {error}");
    }
}

#[cfg(windows)]
mod platform {
    use std::path::PathBuf;

    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, HKEY_CURRENT_USER,
    };
    use windows::Win32::UI::Shell::{
        FOLDERID_Startup, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT,
    };

    const SHORTCUT_NAME: &str = "Notas.lnk";
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_VALUE: &str = crate::APP_NAME;

    fn startup_dir() -> Result<PathBuf, String> {
        unsafe {
            let raw = SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None)
                .map_err(|error| error.to_string())?;
            let path = raw.to_string().map_err(|error| error.to_string());
            CoTaskMemFree(Some(raw.0 as *const _));
            Ok(PathBuf::from(path?))
        }
    }

    fn shortcut_path() -> Result<PathBuf, String> {
        Ok(startup_dir()?.join(SHORTCUT_NAME))
    }

    pub fn shortcut_exists() -> Result<bool, String> {
        Ok(shortcut_path()?.exists())
    }

    /// Write `Notas.lnk` pointing at the running executable. Runs COM on a
    /// short-lived thread so it never depends on how the caller's thread was
    /// initialized (Tauri's main thread already owns an apartment).
    pub fn create_shortcut() -> Result<(), String> {
        let target = std::env::current_exe().map_err(|error| error.to_string())?;
        let link = shortcut_path()?;
        std::thread::spawn(move || unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|error| error.to_string())?;
            let result = (|| -> windows::core::Result<()> {
                let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                shell.SetPath(&HSTRING::from(target.as_os_str()))?;
                if let Some(dir) = target.parent() {
                    shell.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
                }
                shell.SetDescription(&HSTRING::from("Notas"))?;
                let file: IPersistFile = shell.cast()?;
                file.Save(PCWSTR(HSTRING::from(link.as_os_str()).as_ptr()), true)?;
                Ok(())
            })();
            CoUninitialize();
            result.map_err(|error| error.to_string())
        })
        .join()
        .map_err(|_| "el hilo del acceso directo falló".to_string())?
    }

    pub fn remove_shortcut() -> Result<(), String> {
        match std::fs::remove_file(shortcut_path()?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Delete the `Run` value written by Notas 1.x/2.0.x. Returns whether it
    /// existed, so the caller can keep the user's "start with Windows" choice.
    pub fn remove_legacy_run_value() -> Result<bool, String> {
        let status = unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_VALUE))
        };
        if status.is_ok() {
            Ok(true)
        } else if status == ERROR_FILE_NOT_FOUND {
            Ok(false)
        } else {
            Err(format!("error {}", status.0))
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn shortcut_exists() -> Result<bool, String> {
        Ok(false)
    }
    pub fn create_shortcut() -> Result<(), String> {
        Err("solo disponible en Windows".to_string())
    }
    pub fn remove_shortcut() -> Result<(), String> {
        Ok(())
    }
    pub fn remove_legacy_run_value() -> Result<bool, String> {
        Ok(false)
    }
}
