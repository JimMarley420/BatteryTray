// No console window: a tray app
#![windows_subsystem = "windows"]

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(batterytray::app::run(&args));
}

/// On macOS the app is the Swift one in macos/, built around this crate's core.
#[cfg(not(windows))]
fn main() {
    eprintln!("BatteryTray for macOS is built from macos/ (see docs/development.md)");
    std::process::exit(2);
}
