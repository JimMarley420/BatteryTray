//! Light or dark, as Windows has it. The app's windows follow the app mode (Settings, Personalization, Colors); the
//! tray icon's number and the tray menu follow the Windows mode, the taskbar's, as they sit on the taskbar.
//! Both are read from the registry and watched there, so a change re-colours everything at once. The registry rather
//! than the WM_SETTINGCHANGE Windows broadcasts on a change: the app usually runs elevated, and a message from a
//! program without admin rights (Settings) reaches its windows only if UIPI lets it through; the registry works
//! whatever the rights, and without a window.

use std::cell::Cell;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{
    RegCloseKey, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY,
    REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, RRF_RT_REG_DWORD,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

const THEMES: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes");
const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// The app's windows
    pub apps_light: bool,
    /// The taskbar, and with it the tray icon's number and the tray menu
    pub taskbar_light: bool,
}

impl Mode {
    /// From AppsUseLightTheme and SystemUsesLightTheme. A value that isn't there means what Windows does without it:
    /// light apps, a dark taskbar (Windows 10 before 1903 had only the app mode, and its taskbar was dark).
    fn from_values(apps: Option<u32>, system: Option<u32>) -> Self {
        Mode { apps_light: apps.is_none_or(|v| v != 0), taskbar_light: system.is_some_and(|v| v != 0) }
    }
}

thread_local! {
    /// The mode the app shows, on the UI thread
    static SHOWN: Cell<Option<Mode>> = const { Cell::new(None) };
}

/// The mode the app shows: read when first asked, then changed only by `reread`, so every part of the app agrees.
pub fn current() -> Mode {
    SHOWN.with(|shown| {
        let mode = shown.get().unwrap_or_else(read);
        shown.set(Some(mode));
        mode
    })
}

/// Reads the mode again: the new one when it changed.
pub fn reread() -> Option<Mode> {
    let mode = read();
    SHOWN.with(|shown| (shown.replace(Some(mode)) != Some(mode)).then_some(mode))
}

fn read() -> Mode {
    Mode::from_values(value(w!("AppsUseLightTheme")), value(w!("SystemUsesLightTheme")))
}

fn value(name: PCWSTR) -> Option<u32> {
    let (mut data, mut size) = (0u32, 4u32);
    let found = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PERSONALIZE,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    found.is_ok().then_some(data)
}

/// Calls `changed`, on a thread of its own, after every change to Windows' theme settings, for as long as the app
/// runs. The whole Themes key is watched: Personalize may only be made the first time the mode is changed.
pub fn watch(changed: impl Fn() + Send + 'static) {
    let spawned = std::thread::Builder::new().name("theme".into()).spawn(move || unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, THEMES, None, KEY_NOTIFY, &mut key).is_err() {
            crate::log::write("theme: the theme settings can't be watched");
            return;
        }
        let Ok(event) = CreateEventW(None, false, false, None) else {
            let _ = RegCloseKey(key);
            return;
        };
        // Armed again before `changed` has the values read: a change made meanwhile ends the next wait at once
        let arm = || {
            RegNotifyChangeKeyValue(key, true, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET, Some(event), true)
                .is_ok()
        };
        let mut armed = arm();
        // Once now: a change made while the app started, before the watch was armed, would otherwise wait for the
        // next one (a theme scheduler switching at logon, as the app starts with Windows)
        if armed {
            changed();
        }
        while armed && WaitForSingleObject(event, INFINITE) == WAIT_OBJECT_0 {
            armed = arm();
            changed();
        }
        let _ = CloseHandle(event);
        let _ = RegCloseKey(key);
    });
    if let Err(e) = spawned {
        crate::log::write(&format!("theme: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_value_means_light_apps_and_a_dark_taskbar() {
        let mode = |apps, system| {
            let m = Mode::from_values(apps, system);
            (m.apps_light, m.taskbar_light)
        };
        assert_eq!(mode(Some(0), Some(0)), (false, false));
        assert_eq!(mode(Some(1), Some(1)), (true, true));
        assert_eq!(mode(Some(1), Some(0)), (true, false));
        assert_eq!(mode(Some(0), Some(1)), (false, true));
        assert_eq!(mode(None, None), (true, false));
    }
}
