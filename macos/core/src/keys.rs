//! The overlay's shortcut on a Mac. It is kept as on Windows - a Win32 virtual key and MOD_* flags, ⌘ being MOD_WIN -
//! so settings.json reads the same and the rules in settings.rs apply; this maps it to and from the Mac's own key
//! codes (the kVK_* of a key's place on the keyboard, so a shortcut is the same on every layout) and Carbon's
//! modifiers, and names it as macOS menus do: ⌃⌥⇧⌘ in that order, then the key.

use batterytray::loc::t;
use batterytray::settings::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

/// (kVK_* code, Win32 virtual key, name). Names left empty are a letter or digit, named by the virtual key itself.
const KEYS: &[(u16, i32, &str)] = &[
    (0x00, 0x41, ""),
    (0x0B, 0x42, ""),
    (0x08, 0x43, ""),
    (0x02, 0x44, ""),
    (0x0E, 0x45, ""),
    (0x03, 0x46, ""),
    (0x05, 0x47, ""),
    (0x04, 0x48, ""),
    (0x22, 0x49, ""),
    (0x26, 0x4A, ""),
    (0x28, 0x4B, ""),
    (0x25, 0x4C, ""),
    (0x2E, 0x4D, ""),
    (0x2D, 0x4E, ""),
    (0x1F, 0x4F, ""),
    (0x23, 0x50, ""),
    (0x0C, 0x51, ""),
    (0x0F, 0x52, ""),
    (0x01, 0x53, ""),
    (0x11, 0x54, ""),
    (0x20, 0x55, ""),
    (0x09, 0x56, ""),
    (0x0D, 0x57, ""),
    (0x07, 0x58, ""),
    (0x10, 0x59, ""),
    (0x06, 0x5A, ""),
    (0x1D, 0x30, ""),
    (0x12, 0x31, ""),
    (0x13, 0x32, ""),
    (0x14, 0x33, ""),
    (0x15, 0x34, ""),
    (0x17, 0x35, ""),
    (0x16, 0x36, ""),
    (0x1A, 0x37, ""),
    (0x1C, 0x38, ""),
    (0x19, 0x39, ""),
    (0x7A, 0x70, "F1"),
    (0x78, 0x71, "F2"),
    (0x63, 0x72, "F3"),
    (0x76, 0x73, "F4"),
    (0x60, 0x74, "F5"),
    (0x61, 0x75, "F6"),
    (0x62, 0x76, "F7"),
    (0x64, 0x77, "F8"),
    (0x65, 0x78, "F9"),
    (0x6D, 0x79, "F10"),
    (0x67, 0x7A, "F11"),
    (0x6F, 0x7B, "F12"),
    (0x69, 0x7C, "F13"),
    (0x6B, 0x7D, "F14"),
    (0x71, 0x7E, "F15"),
    (0x6A, 0x7F, "F16"),
    (0x40, 0x80, "F17"),
    (0x4F, 0x81, "F18"),
    (0x50, 0x82, "F19"),
    (0x5A, 0x83, "F20"),
    (0x31, 0x20, "space"),
    (0x24, 0x0D, "↩"),
    (0x30, 0x09, "⇥"),
    (0x35, 0x1B, "⎋"),
    (0x33, 0x08, "⌫"),
    (0x75, 0x2E, "⌦"),
    (0x72, 0x2D, "Help"),
    (0x73, 0x24, "↖"),
    (0x77, 0x23, "↘"),
    (0x74, 0x21, "⇞"),
    (0x79, 0x22, "⇟"),
    (0x7B, 0x25, "←"),
    (0x7E, 0x26, "↑"),
    (0x7C, 0x27, "→"),
    (0x7D, 0x28, "↓"),
    (0x1B, 0xBD, "-"),
    (0x18, 0xBB, "="),
    (0x21, 0xDB, "["),
    (0x1E, 0xDD, "]"),
    (0x2A, 0xDC, "\\"),
    (0x29, 0xBA, ";"),
    (0x27, 0xDE, "'"),
    (0x2B, 0xBC, ","),
    (0x2F, 0xBE, "."),
    (0x2C, 0xBF, "/"),
    (0x32, 0xC0, "`"),
    (0x52, 0x60, "0"),
    (0x53, 0x61, "1"),
    (0x54, 0x62, "2"),
    (0x55, 0x63, "3"),
    (0x56, 0x64, "4"),
    (0x57, 0x65, "5"),
    (0x58, 0x66, "6"),
    (0x59, 0x67, "7"),
    (0x5B, 0x68, "8"),
    (0x5C, 0x69, "9"),
    (0x43, 0x6A, "*"),
    (0x45, 0x6B, "+"),
    (0x4E, 0x6D, "-"),
    (0x41, 0x6E, "."),
    (0x4B, 0x6F, "/"),
    (0x47, 0x0C, "⌧"),
];

/// Carbon's modifier bits, as RegisterEventHotKey takes them.
const CMD_KEY: u32 = 0x0100;
const SHIFT_KEY: u32 = 0x0200;
const OPTION_KEY: u32 = 0x0800;
const CONTROL_KEY: u32 = 0x1000;

/// The virtual key of a Mac key code; None for a key a shortcut can't have (a modifier, fn, the volume keys).
pub fn virtual_key(mac_key: u16) -> Option<i32> {
    KEYS.iter().find(|k| k.0 == mac_key).map(|k| k.1)
}

pub fn mac_key(virtual_key: i32) -> Option<u16> {
    KEYS.iter().find(|k| k.1 == virtual_key).map(|k| k.0)
}

/// MOD_* flags from the modifiers held.
pub fn modifiers(command: bool, control: bool, option: bool, shift: bool) -> i32 {
    (if option { MOD_ALT } else { 0 })
        | (if control { MOD_CONTROL } else { 0 })
        | (if shift { MOD_SHIFT } else { 0 })
        | (if command { MOD_WIN } else { 0 })
}

pub fn carbon_modifiers(modifiers: i32) -> u32 {
    (if modifiers & MOD_WIN != 0 { CMD_KEY } else { 0 })
        | (if modifiers & MOD_SHIFT != 0 { SHIFT_KEY } else { 0 })
        | (if modifiers & MOD_ALT != 0 { OPTION_KEY } else { 0 })
        | (if modifiers & MOD_CONTROL != 0 { CONTROL_KEY } else { 0 })
}

/// The parts of a shortcut as key caps: ["⌃", "⌥", "V"].
pub fn parts(key: i32, modifiers: i32) -> Vec<String> {
    let mut parts = Vec::new();
    for (flag, symbol) in [(MOD_CONTROL, "⌃"), (MOD_ALT, "⌥"), (MOD_SHIFT, "⇧"), (MOD_WIN, "⌘")] {
        if modifiers & flag != 0 {
            parts.push(symbol.to_string());
        }
    }
    parts.push(key_name(key));
    parts
}

/// "⌃⌥V", as a Mac menu writes it.
pub fn label(key: i32, modifiers: i32) -> String {
    parts(key, modifiers).concat()
}

fn key_name(key: i32) -> String {
    match KEYS.iter().find(|k| k.1 == key) {
        Some((_, _, "")) => char::from(key as u8).to_string(),
        Some((_, _, "space")) => t("key.space"),
        Some((_, _, name)) => name.to_string(),
        None => format!("0x{key:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use batterytray::settings::DEFAULT_HOTKEY;

    #[test]
    fn the_default_shortcut_on_a_mac() {
        let (key, modifiers) = DEFAULT_HOTKEY;
        assert_eq!(mac_key(key), Some(0x09), "V");
        assert_eq!(carbon_modifiers(modifiers), CONTROL_KEY | OPTION_KEY);
        assert_eq!(label(key, modifiers), "⌃⌥V");
        assert_eq!(parts(0x42, MOD_WIN | MOD_SHIFT | MOD_ALT | MOD_CONTROL), ["⌃", "⌥", "⇧", "⌘", "B"]);
        assert_eq!(label(0x74, 0), "F5");
        assert_eq!(label(0x20, MOD_WIN | MOD_ALT), "⌥⌘Space");
    }

    #[test]
    fn every_key_goes_both_ways_once() {
        for &(mac, vk, _) in KEYS {
            assert_eq!(virtual_key(mac), Some(vk), "{mac:#x}");
            assert_eq!(mac_key(vk), Some(mac), "{vk:#x}");
        }
        assert_eq!(virtual_key(0x37), None, "⌘ alone is no key");
        assert_eq!(modifiers(true, false, true, false), MOD_WIN | MOD_ALT);
    }
}
