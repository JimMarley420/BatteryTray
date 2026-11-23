//! HID enumeration and I/O with timeouts: what every provider is built on. The collections are described the same on
//! both systems, and Windows' device paths are the form they are named in - the providers group a device's
//! collections and tell USB from Bluetooth by the path. On macOS the paths are written in that form (hid/macos.rs).
//! The I/O behaves the same too: a read waits for the next input report of its own collection, at most as long as it
//! is given; a write that isn't taken in time comes back false, never late.

#[cfg(target_os = "macos")]
mod descriptor;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{enumerate, feature_value, input_monitoring_allowed, request_input_monitoring, HidDevice};

/// On macOS every collection is offered to the app: nothing here is refused for missing admin rights (see the Windows
/// side), so the catch-all enumeration is the usual one.
#[cfg(target_os = "macos")]
pub use macos::enumerate as enumerate_unclaimed;

/// Windows only: on macOS admin rights never let the app read a device it can't otherwise.
#[cfg(target_os = "macos")]
pub fn admin_rights_would_help() -> bool {
    false
}
#[cfg(windows)]
pub(crate) use windows::ffi;
#[cfg(windows)]
pub use windows::{
    admin_rights_would_help, device_nodes, enumerate, enumerate_unclaimed, feature_value, HidDevice, PropertyKey,
};

use std::fmt;

/// A field's value as a feature report holds it, and the field's logical range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageValue {
    pub raw: u32,
    pub min: i32,
    pub max: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HidInfo {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_length: usize,
    pub output_length: usize,
    pub feature_length: usize,
    pub output_report_id: u8,
    pub product: Option<String>,
    /// The collection exists but this process may not open it: its security descriptor on Windows, the user's
    /// "Input Monitoring" permission on macOS (which every device with a keyboard collection needs).
    pub access_denied: bool,
}

impl HidInfo {
    /// USB interface number from the device path ("...&mi_02..."), or -1.
    pub fn interface_number(&self) -> i32 {
        interface_number(&self.path)
    }

    /// The collection belongs to a device connected over Bluetooth (classic HID or LE), not over USB.
    pub fn is_bluetooth(&self) -> bool {
        let path = self.path.to_ascii_lowercase();
        path.contains("{00001124-") || path.contains("{00001812-")
    }
}

impl fmt::Display for HidInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04X}:{:04X} page=0x{:04X} usage=0x{:04X} in={} out={} feat={} outId=0x{:02X}{}",
            self.vendor_id,
            self.product_id,
            self.usage_page,
            self.usage,
            self.input_length,
            self.output_length,
            self.feature_length,
            self.output_report_id,
            if self.access_denied { " ACCESS DENIED" } else { "" }
        )
    }
}

fn hex_value(s: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(s).ok()?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}

/// USB interface number from a device path ("...&mi_02..."), or -1.
pub fn interface_number(path: &str) -> i32 {
    let lower = path.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(at) = lower[from..].find("&mi_") {
        let start = from + at + 4;
        if let Some(v) = bytes.get(start..start + 2).and_then(hex_value) {
            return v as i32;
        }
        from = from + at + 1;
    }
    -1
}

/// Vendor and product ID from a device path. USB: vid_1532&pid_0550; Bluetooth HID: _vid&0002054c_pid&0ce6;
/// Bluetooth LE: _vid&02045e_pid&0b13.
pub fn parse_ids(path: &str) -> Option<(u16, u16)> {
    let lower = path.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut from = 0;
    while let Some(at) = lower[from..].find("vid") {
        let after = from + at + 3;
        for prefix in ["_", "&0001", "&0002", "&01", "&02"] {
            if !lower[after..].starts_with(prefix) {
                continue;
            }
            let v = after + prefix.len();
            let Some(vid) = b.get(v..v + 4).and_then(hex_value) else { continue };
            let sep = v + 4;
            if !matches!(b.get(sep), Some(b'&') | Some(b'_')) || b.get(sep + 1..sep + 4) != Some(b"pid") {
                continue;
            }
            if !matches!(b.get(sep + 4), Some(b'&') | Some(b'_')) {
                continue;
            }
            let p = sep + 5;
            let Some(pid) = b.get(p..p + 4).and_then(hex_value) else { continue };
            return Some((vid as u16, pid as u16));
        }
        from = from + at + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_usb_and_bluetooth_paths() {
        assert_eq!(
            parse_ids(r"\\?\hid#vid_1532&pid_0550&mi_03#7&1a2b&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"),
            Some((0x1532, 0x0550))
        );
        assert_eq!(
            parse_ids(r"\\?\hid#{00001124-0000-1000-8000-00805f9b34fb}_vid&0002054c_pid&0ce6#9&2&0&0000#{4d1e55b2}"),
            Some((0x054C, 0x0CE6))
        );
        assert_eq!(
            parse_ids(
                r"\\?\hid#{00001812-0000-1000-8000-00805f9b34fb}_dev_vid&02045e_pid&0b13_rev&0509#a&1&0&0000#{x}"
            ),
            Some((0x045E, 0x0B13))
        );
        assert_eq!(parse_ids(r"\\?\HID#VID_046D&PID_C539&MI_02&Col01#8&abc#{x}"), Some((0x046D, 0xC539)));
        assert_eq!(parse_ids(r"\\?\hid#acpi0c50#3&1&0#{x}"), None);
    }

    #[test]
    fn bluetooth_paths_are_told_from_usb_ones() {
        let at = |path: &str| HidInfo { path: path.to_string(), ..HidInfo::default() }.is_bluetooth();
        assert!(at(r"\\?\hid#{00001124-0000-1000-8000-00805f9b34fb}_vid&0002054c_pid&0ce6#9&2&0&0000#{4d1e55b2}"));
        assert!(at(r"\\?\HID#{00001812-0000-1000-8000-00805F9B34FB}_dev_vid&02045e_pid&0b13_rev&0509#a&1&0&0000#{x}"));
        assert!(!at(r"\\?\hid#vid_1532&pid_0550&mi_03#7&1a2b&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"));
    }

    #[test]
    fn interface_number_from_path() {
        assert_eq!(interface_number(r"\\?\hid#vid_3151&pid_402d&mi_02#8&1&0&0000#{x}"), 2);
        assert_eq!(interface_number(r"\\?\HID#VID_046D&PID_C539&MI_0A&Col01#x"), 10);
        assert_eq!(interface_number(r"\\?\hid#vid_1532&pid_0550#8&1&0&0000#{x}"), -1);
    }
}
