//! Starts the app at sign-in through a LaunchAgent: ~/Library/LaunchAgents/io.github.qu1ncky.BatteryTray.plist, which
//! launchd reads when the user signs in (System Settings lists it under General → Login Items). It is only written
//! or deleted here, never loaded or unloaded: loading it would start a second copy right away, and unloading it would
//! stop the running copy when that is the one launchd started.

use super::BACKGROUND_SWITCH;
use crate::log;
use std::fs;
use std::path::PathBuf;

/// The app's bundle identifier, which names the agent too.
pub const LABEL: &str = "io.github.qu1ncky.BatteryTray";

fn agent() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library").join("LaunchAgents").join(format!("{LABEL}.plist"))
}

pub fn is_registered() -> bool {
    agent().exists()
}

pub fn set(enabled: bool) -> bool {
    if enabled {
        register()
    } else {
        unregister()
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The agent for an executable: started once at sign-in, in the user's graphical session, as an app the user works
/// with (no background throttling).
pub fn agent_plist(exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>{BACKGROUND_SWITCH}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        exe = escape(exe)
    )
}

/// A copy that may be gone at the next login: the read-only copy macOS runs an app downloaded from the internet from
/// until it is moved (App Translocation, a new random folder at every start), one on a disk image or another drive.
/// A login item must not point at it; the app offers to move into Applications instead.
pub fn temporary_place(exe: &str) -> bool {
    exe.contains("/AppTranslocation/") || exe.starts_with("/Volumes/")
}

pub fn register() -> bool {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    if temporary_place(&exe) {
        log::write(&format!("register login agent: not for a copy that may be gone at the next login ({exe})"));
        return false;
    }
    let path = agent();
    let result = path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| fs::write(&path, agent_plist(&exe)));
    log::write(&format!("register login agent: {}", result.as_ref().map_or_else(|e| e.to_string(), |_| "ok".into())));
    result.is_ok()
}

/// The login item starts the copy that wrote it. When this copy runs from somewhere else (it was moved to Applications,
/// or another copy was started by hand), the item is pointed at this one: the one the user runs now is the one they
/// mean to start at login. Not a copy that may be gone at the next login (a disk image opened to try a version): the
/// item stays with the copy it has.
pub fn follow_this_copy() {
    let Ok(text) = fs::read_to_string(agent()) else { return };
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    if !exe.is_empty() && !temporary_place(&exe) && !text.contains(&format!("<string>{}</string>", escape(&exe))) {
        log::write(&format!("login agent: now for {exe}"));
        register();
    }
}

pub fn unregister() -> bool {
    let path = agent();
    let result = match fs::remove_file(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    };
    log::write(&format!("unregister login agent: {}", result.as_ref().map_or_else(|e| e.to_string(), |_| "ok".into())));
    result.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_login_item_for_a_copy_that_may_be_gone_at_the_next_login() {
        assert!(temporary_place(
            "/private/var/folders/x/T/AppTranslocation/1F2E/d/BatteryTray.app/Contents/MacOS/BatteryTray"
        ));
        assert!(temporary_place("/Volumes/BatteryTray/BatteryTray.app/Contents/MacOS/BatteryTray"));
        assert!(!temporary_place("/Applications/BatteryTray.app/Contents/MacOS/BatteryTray"));
        assert!(!temporary_place("/Users/me/Applications/BatteryTray.app/Contents/MacOS/BatteryTray"));
    }

    #[test]
    fn the_agent_starts_the_app_in_the_background() {
        let plist = agent_plist("/Applications/Battery & Tray.app/Contents/MacOS/BatteryTray");
        assert!(plist.contains("<string>/Applications/Battery &amp; Tray.app/Contents/MacOS/BatteryTray</string>"));
        assert!(plist.contains("<string>--background</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        // What plutil, macOS' own reader, makes of it
        let file = std::env::temp_dir().join(format!("bt-agent-{}.plist", std::process::id()));
        fs::write(&file, &plist).unwrap();
        let lint = std::process::Command::new("/usr/bin/plutil").arg("-lint").arg(&file).output().unwrap();
        let _ = fs::remove_file(&file);
        assert!(lint.status.success(), "{}", String::from_utf8_lossy(&lint.stdout));
    }
}
