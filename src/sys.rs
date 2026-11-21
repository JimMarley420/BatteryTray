//! What the core asks of the operating system, the same on Windows and macOS: errors as the system words them, the
//! folder the app keeps its files in, the system's language, whether the process has administrator rights.

use std::fmt;
use std::path::PathBuf;

/// What failed: the system's error code when there is one (Win32 on Windows, IOReturn on macOS), and the text shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: Option<u32>,
    pub message: String,
}

impl Error {
    /// The system's own text for the code ("Access is denied."), as C#'s Win32Exception gives it on Windows.
    pub fn os(code: u32) -> Self {
        Self { code: Some(code), message: system_message(code) }
    }

    /// The calling thread's last Win32 error.
    #[cfg(windows)]
    pub fn last() -> Self {
        Self::os(crate::win::last_error())
    }

    pub fn with_message(code: u32, message: impl Into<String>) -> Self {
        Self { code: Some(code), message: message.into() }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self { code: None, message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The system's text for an error code.
pub fn system_message(code: u32) -> String {
    #[cfg(windows)]
    return crate::win::system_message(code);
    #[cfg(target_os = "macos")]
    return crate::mac::system_message(code);
}

/// Where settings.json, state.json and the log are: %LOCALAPPDATA%\BatteryTray on Windows,
/// ~/Library/Application Support/BatteryTray on macOS.
pub fn app_data_folder() -> PathBuf {
    #[cfg(windows)]
    return crate::win::app_data_folder();
    #[cfg(target_os = "macos")]
    return crate::mac::app_data_folder();
}

/// The language the system shows its own texts in: "ru", "zh" or "en".
pub fn ui_language() -> &'static str {
    #[cfg(windows)]
    return crate::win::ui_language();
    #[cfg(target_os = "macos")]
    return crate::mac::ui_language();
}

/// True when this process runs with administrator rights: an elevated token on Windows, root on macOS.
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    return crate::win::is_elevated();
    #[cfg(target_os = "macos")]
    return crate::mac::is_root();
}

/// The macOS version as the system names it: "26.6.2".
#[cfg(target_os = "macos")]
pub fn macos_version() -> String {
    crate::mac::macos_version()
}
