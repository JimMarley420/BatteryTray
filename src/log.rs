//! Small rolling log (log.txt, the previous one kept as log.old.txt). The console tool writes
//! to a folder of its own and echoes every line, so it never mixes with the C# app's log.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const MAX_BYTES: u64 = 512 * 1024;

static FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);
static ECHO: AtomicBool = AtomicBool::new(false);

pub fn set_folder(folder: PathBuf) {
    *FOLDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(folder);
}

/// Also print every line to stderr.
pub fn set_echo(echo: bool) {
    ECHO.store(echo, Ordering::Relaxed);
}

pub fn folder() -> PathBuf {
    FOLDER.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(default_folder)
}

/// Where the lines go when no folder was set: the app's data folder. The unit tests have one of their own, so a test
/// run never writes to the log of the app installed on the same machine.
fn default_folder() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join("BatteryTray-tests")
    } else {
        crate::sys::app_data_folder()
    }
}

pub fn write(text: &str) {
    let line = format!("{}  {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), text);
    // Held while writing, so lines from several threads never interleave
    let guard = FOLDER.lock().unwrap_or_else(|e| e.into_inner());
    if ECHO.load(Ordering::Relaxed) {
        eprintln!("{line}");
    }
    let folder = guard.clone().unwrap_or_else(default_folder);
    // Logging must never break the app
    let _ = (|| -> std::io::Result<()> {
        fs::create_dir_all(&folder)?;
        let path = folder.join("log.txt");
        if fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
            let old = folder.join("log.old.txt");
            let _ = fs::remove_file(&old);
            fs::rename(&path, &old)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(format!("{line}\r\n").as_bytes())
    })();
}
