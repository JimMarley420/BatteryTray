//! Catch-all for any USB device that declares the standard HID "Battery Strength" field (Generic Device Controls page
//! 0x06, usage 0x20) in a feature report - common on generic 2.4 GHz receivers - which Windows never shows.
//! Only feature reports are read (a GET_REPORT has no side effects), so
//! devices with a dedicated provider and those whose level the system shows itself are skipped.

use super::{contains_ignore_case, one_per_id, secs, Provider};
use crate::hid::{self, HidInfo, UsageValue};
use crate::loc::t;
use crate::reading::{DeviceKind, DeviceReading};
use std::time::Duration;

const GENERIC_DEVICE_PAGE: u16 = 0x06;
const BATTERY_STRENGTH: u16 = 0x20;

/// Vendors read by their own providers. A device that a row of the tables in `devices` has read is left alone too.
const OWN_PROVIDERS: [u16; 7] = [0x1532, 0x1038, 0x046D, 0x054C, 0x03F0, 0x0951, 0x1B1C];

pub struct StandardHidBatteryProvider;

impl Provider for StandardHidBatteryProvider {
    fn name(&self) -> &'static str {
        "HID Battery Strength"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let mut result = Vec::new();
        for info in hid::enumerate_unclaimed(|v, p| !OWN_PROVIDERS.contains(&v) && !super::devices::read_here(v, p)) {
            if info.access_denied || info.feature_length == 0 || shown_by_the_system(&info) {
                continue;
            }
            // A device that refuses the request just isn't listed
            let Some(level) = read_level(&info) else { continue };
            let name =
                info.product.clone().unwrap_or_else(|| format!("HID {:04X}:{:04X}", info.vendor_id, info.product_id));
            result.push(DeviceReading {
                online: true,
                level: Some(level),
                ..DeviceReading::new(
                    format!("hid:{:04x}:{:04x}", info.vendor_id, info.product_id),
                    name,
                    kind_of(&info),
                    t("via.receiver"),
                )
            });
        }
        // One entry per device even if several of its collections carry the field
        one_per_id(result, |_| false)
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        secs(2 * 60)
    }
}

/// Bluetooth LE devices: Windows shows those itself. macOS shows every Bluetooth device's level that it knows,
/// classic ones too, and the Bluetooth provider lists them from there.
fn shown_by_the_system(info: &HidInfo) -> bool {
    if cfg!(target_os = "macos") {
        info.is_bluetooth()
    } else {
        contains_ignore_case(&info.path, "{00001812-")
    }
}

fn kind_of(info: &HidInfo) -> DeviceKind {
    if info.usage_page != 0x01 {
        return DeviceKind::Other;
    }
    match info.usage {
        0x02 => DeviceKind::Mouse,
        0x06 => DeviceKind::Keyboard,
        0x04 | 0x05 => DeviceKind::Gamepad,
        _ => DeviceKind::Other,
    }
}

fn read_level(info: &HidInfo) -> Option<i32> {
    let value = hid::feature_value(info, GENERIC_DEVICE_PAGE, BATTERY_STRENGTH)?;
    percent(value)
}

/// The level a field's value stands for: most devices declare 0-100 (or 0-255); a missing range is taken as a
/// percentage. A level outside 1-100 is no reading.
fn percent(value: UsageValue) -> Option<i32> {
    let (min, max) = if value.max <= value.min { (0, 100) } else { (value.min, value.max) };
    // Math.Round in C# rounds halves to even
    let percent = ((value.raw as i64 - min as i64) as f64 * 100.0 / (max - min) as f64).round_ties_even() as i32;
    (1..=100).contains(&percent).then_some(percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_from_the_fields_range() {
        let at = |raw, min, max| percent(UsageValue { raw, min, max });
        assert_eq!(at(85, 0, 100), Some(85));
        assert_eq!(at(128, 0, 255), Some(50));
        assert_eq!(at(64, 0, -1), Some(64), "a one-byte 255 read as -1: no range, a percentage");
        assert_eq!(at(0, 0, 100), None);
        assert_eq!(at(101, 0, 100), None);
    }
}
