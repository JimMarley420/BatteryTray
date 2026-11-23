//! Apple's Magic Keyboards on Bluetooth, whose level Windows doesn't show. The keyboard keeps its battery in input
//! report 90: `90 SS LL`, LL the level in percent, SS status bits of which bit 1 is charging (the one capture
//! published, of a Magic Trackpad on its cable, has `90 03 64`: present, charging, 100%). The report is fetched
//! with HidD_GetInputReport, a GET_REPORT the keyboard answers without changing anything, from the first of the
//! keyboard's collections that takes it, its vendor collections first (where Linux finds the report over USB).
//! Sources: WinMagicBattery (APP/BatteryMonitor.cs: the report and its byte 2; APP/DeviceScanner.cs: the two vendor
//! IDs), Linux (drivers/hid/hid-apple.c and hid-ids.h: the product IDs, and the same report fetched over USB; its
//! layout from the 2026 patches on hid-input's battery query). Own implementation, not yet verified on hardware by
//! this project, which is why the first exchange with a keyboard goes to the log.
//!
//! Bluetooth only: on its cable the keyboard charges. On a Mac the system shows the level (the Bluetooth provider),
//! so this provider runs on Windows only. The Magic Mouse and Magic Trackpad are left out, as the source this follows
//! reads keyboards only, and so are Apple's older Wireless Keyboards, which keep their level elsewhere.

use super::{contains_ignore_case, group_by, hex_trimmed as hex, no_link_reason, secs, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::t;
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use std::collections::HashSet;
use std::time::Duration;

/// Apple's vendor ID as the Bluetooth SIG assigns it, which the keyboards give over Bluetooth, and as the USB-IF
/// does, which a keyboard may give instead.
const APPLE_VIDS: [u16; 2] = [0x004C, 0x05AC];
const BATTERY_REPORT: u8 = 0x90;

const MODELS: [(u16, &str); 8] = [
    (0x0267, "Magic Keyboard"),
    (0x026C, "Magic Keyboard with Numeric Keypad"),
    (0x029C, "Magic Keyboard"),
    (0x029A, "Magic Keyboard with Touch ID"),
    (0x029F, "Magic Keyboard with Touch ID and Numeric Keypad"),
    (0x0320, "Magic Keyboard"),
    (0x0321, "Magic Keyboard with Touch ID"),
    (0x0322, "Magic Keyboard with Touch ID and Numeric Keypad"),
];

fn model(pid: u16) -> Option<&'static str> {
    MODELS.iter().find(|m| m.0 == pid).map(|m| m.1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub level: i32,
    pub charging: bool,
    pub full: bool,
}

/// The battery from input report 90. A level outside 1-100 is no reading.
pub fn parse(r: &[u8]) -> Option<Battery> {
    let [BATTERY_REPORT, status, level, ..] = *r else { return None };
    if !(1..=100).contains(&level) {
        return None;
    }
    let charger = status & 0x02 != 0;
    Some(Battery { level: level as i32, charging: charger && level < 100, full: charger && level == 100 })
}

#[derive(Default)]
pub struct AppleKeyboardProvider {
    /// What this session's log already has: see `log_once`.
    logged: HashSet<String>,
}

impl Provider for AppleKeyboardProvider {
    fn name(&self) -> &'static str {
        "Apple keyboards"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let found: Vec<HidInfo> = hid::enumerate(|v, p| APPLE_VIDS.contains(&v) && model(p).is_some())
            .into_iter()
            .filter(|i| contains_ignore_case(&i.path, "{00001124-"))
            .collect();
        let mut result: Vec<DeviceReading> = Vec::new();
        for ((vid, pid), collections) in group_by(found, |i| (i.vendor_id, i.product_id)) {
            let Some(name) = model(pid) else { continue };
            // Two keyboards of one model get #2, #3...
            let mut id = format!("apple:{pid:04x}");
            let mut name = name.to_string();
            let same = result.iter().filter(|r| r.id.starts_with(&id)).count();
            if same > 0 {
                id = format!("{id}#{}", same + 1);
                name = format!("{name} #{}", same + 1);
            }
            let template = DeviceReading::new(id, name, DeviceKind::Keyboard, "Bluetooth");

            let mut asked = Vec::new();
            let battery = ask(&collections, &mut asked);
            self.log_once(vid, pid, &collections, &asked, battery.is_some());
            result.push(match battery {
                Some(Battery { level, charging, full }) => {
                    DeviceReading { online: true, level: Some(level), charging, full_on_charger: full, ..template }
                }
                None if asked.is_empty() => DeviceReading { detail: Some(t(no_link_reason(&collections))), ..template },
                None => DeviceReading { detail: Some(t("d.offOrAway")), ..template },
            });
        }
        result
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        // The level moves by a percent in hours; asking less often keeps the requests from mattering to a keyboard
        // that sleeps
        secs(5 * 60)
    }
}

impl AppleKeyboardProvider {
    /// Logs a keyboard's collections and what each answered, the first time it is found on and the first time it is
    /// found off in a session.
    fn log_once(&mut self, vid: u16, pid: u16, collections: &[HidInfo], asked: &[(String, String)], online: bool) {
        let id = format!("{vid:04x}:{pid:04x}");
        if !self.logged.insert(format!("{id}{online}")) {
            return;
        }
        log::write(&format!(
            "Apple keyboard {id}: found {}, {} HID collections",
            if online { "on" } else { "off" },
            collections.len()
        ));
        for info in collections {
            let answer = asked.iter().find(|a| a.0 == info.path).map_or("not asked", |a| a.1.as_str());
            log::write(&format!("  {info}: {answer}"));
        }
    }
}

/// Asks the keyboard's collections for report 90, vendor collections first, until one gives it. `asked` gets each
/// collection asked and what came back.
fn ask(collections: &[HidInfo], asked: &mut Vec<(String, String)>) -> Option<Battery> {
    let mut candidates: Vec<&HidInfo> =
        collections.iter().filter(|i| !i.access_denied && i.input_length >= 3).collect();
    candidates.sort_by_key(|i| if i.usage_page >= 0xFF00 { 0 } else { 1 });
    for info in candidates {
        // Windows holds the keyboard collection for itself and refuses it to a handle that reads or writes, which
        // would pass for missing admin rights; one without access rights may still ask over the control channel
        let held = info.usage_page == 0x01 && matches!(info.usage, 0x02 | 0x06 | 0x07);
        let open = if held { HidDevice::open_features_only(info) } else { HidDevice::open_sync(info) };
        let dev = match open {
            Ok(dev) => dev,
            Err(e) => {
                asked.push((info.path.clone(), format!("error: {e}")));
                continue;
            }
        };
        let report = dev.get_input_report(BATTERY_REPORT);
        asked.push((info.path.clone(), report.as_deref().map_or_else(|| "nothing".into(), hex)));
        if let Some(battery) = report.as_deref().and_then(parse) {
            return Some(battery);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    //! `90 03 64` is the capture published with Linux's 2026 patch on the battery query (a Magic Trackpad on its
    //! cable); the others are put together from it and WinMagicBattery's reading of byte 2.
    use super::*;
    use crate::providers::pad;

    #[test]
    fn the_level_is_byte_2_and_bit_1_the_charger() {
        assert_eq!(parse(&[0x90, 0x03, 0x64]), Some(Battery { level: 100, charging: false, full: true }));
        assert_eq!(parse(&[0x90, 0x03, 0x4A]), Some(Battery { level: 74, charging: true, full: false }));
        assert_eq!(parse(&[0x90, 0x01, 0x4A]), Some(Battery { level: 74, charging: false, full: false }));
        // On Bluetooth Linux saw a status of 04 on a battery at 74%: not the charger
        assert_eq!(parse(&[0x90, 0x04, 0x4A]), Some(Battery { level: 74, charging: false, full: false }));
        // Windows pads the report to the collection's longest input report
        assert_eq!(parse(&pad(8, &[0x90, 0x01, 0x2D])), Some(Battery { level: 45, charging: false, full: false }));
    }

    #[test]
    fn other_reports_and_impossible_levels_are_no_reading() {
        assert_eq!(parse(&[0x91, 0x01, 0x40]), None);
        assert_eq!(parse(&[0x90, 0x01, 0x00]), None);
        assert_eq!(parse(&[0x90, 0x01, 0x65]), None);
        assert_eq!(parse(&[0x90, 0x01]), None, "too short");
    }
}
