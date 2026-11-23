//! Starting the app when the user signs in: a Task Scheduler task on Windows, a LaunchAgent on macOS. Either starts it
//! with `--background`, in the tray or the menu bar without opening its window.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{follow_this_copy, is_registered, register, set, unregister};
#[cfg(windows)]
pub use windows::{
    install, install_here, installed_exe, is_installed, is_registered, register, run_self_elevated, run_task, set,
    unregister, TASK_NAME,
};

/// Passed at sign-in: start without opening the window.
pub const BACKGROUND_SWITCH: &str = "--background";
