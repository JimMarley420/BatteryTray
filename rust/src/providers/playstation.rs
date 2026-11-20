//! Sony DualSense and DualShock 4 (Providers/PlayStationProvider.cs). They stream their state as input reports, the
//! battery included, so nothing is sent - one report is read. Layout from the Linux driver
//! (drivers/hid/hid-playstation.c):
//!   DualSense: status byte = data[53] (USB report 0x01) or data[54] (Bluetooth report 0x31); low nibble 0-10 =
//!     level in tens, high nibble 0 = discharging, 1 = charging, 2 = charging complete.
//!   DualShock 4: status bytes = data[30..31] (USB report 0x01) or data[32..33] (Bluetooth report 0x11); low nibble =
//!     level, bit 4 = cable plugged in (then 0-10 = charging, 11 = complete); wireless adapter: bit 2 of the second
//!     byte = no controller connected.
//! Over Bluetooth the controller sends the short report 0x01 without the battery until a game or Steam switches it
//! to the full report; switching it from here could confuse a running game, so that is left alone.

use super::{contains_ignore_case, remaining_ms, secs, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::{t, tf};
use crate::reading::{DeviceKind, DeviceReading};
use crate::win;
use std::time::{Duration, Instant};

const SONY_VID: u16 = 0x054C;
const TIMEOUT_MS: u64 = 250;

const MODELS: [(u16, &str, bool); 5] = [
    (0x0CE6, "DualSense", true),
    (0x0DF2, "DualSense Edge", true),
    (0x05C4, "DualShock 4", false),
    (0x09CC, "DualShock 4", false),
    (0x0BA0, "DualShock 4", false), // Sony wireless adapter
];

fn model(pid: u16) -> Option<(&'static str, bool)> {
    MODELS.iter().find(|m| m.0 == pid).map(|m| (m.1, m.2))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Battery {
    Level { level: i32, charging: bool, full: bool },
    NoController,
    Error,
}

pub struct PlayStationProvider;

impl Provider for PlayStationProvider {
    fn name(&self) -> &'static str {
        "PlayStation"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let mut result: Vec<DeviceReading> = Vec::new();
        let pads = hid::enumerate(|v, p| v == SONY_VID && model(p).is_some());
        for info in
            pads.iter().filter(|i| i.access_denied || (i.usage_page == 0x01 && i.usage == 0x05 && i.input_length > 0))
        {
            let Some((name, dual_sense)) = model(info.product_id) else { continue };
            let bluetooth = contains_ignore_case(&info.path, "{00001124-");
            let adapter = !bluetooth && info.product_id == 0x0BA0;
            let via =
                if bluetooth { "Bluetooth".to_string() } else { t(if adapter { "via.adapter" } else { "via.cable" }) };
            // Several identical controllers get #2, #3... in the order Windows lists them
            let mut id = format!("ps:{:04x}:{}", info.product_id, if bluetooth { "bt" } else { "usb" });
            let mut name = name.to_string();
            let same = result.iter().filter(|r| r.id.starts_with(&id)).count();
            if same > 0 {
                id = format!("{id}#{}", same + 1);
                name = format!("{name} #{}", same + 1);
            }

            let mut reading = DeviceReading {
                detail: info.access_denied.then(|| t("d.busy")),
                ..DeviceReading::new(id, name, DeviceKind::Gamepad, via)
            };
            if !info.access_denied {
                reading = match read(info, dual_sense, bluetooth, adapter, reading.clone()) {
                    Ok(r) => r,
                    Err(e) => DeviceReading { detail: Some(tf("d.error", &[&e])), ..reading },
                };
            }
            result.push(reading);
        }
        result
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        secs(60)
    }
}

fn read(
    info: &HidInfo,
    dual_sense: bool,
    bluetooth: bool,
    adapter: bool,
    template: DeviceReading,
) -> win::Result<DeviceReading> {
    let dev = HidDevice::open_read_only(info)?;
    dev.flush();
    let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS);
    let mut any = false;
    while Instant::now() < deadline {
        let Some(r) = dev.read(remaining_ms(deadline))? else { break };
        any = true;
        let battery =
            if dual_sense { parse_dual_sense(&r, bluetooth) } else { parse_dual_shock4(&r, bluetooth, adapter) };
        match battery {
            None => continue,
            Some(Battery::NoController) => return Ok(DeviceReading { detail: Some(t("d.noController")), ..template }),
            Some(Battery::Error) => {
                return Ok(DeviceReading { online: true, detail: Some(t("d.batteryError")), ..template });
            }
            Some(Battery::Level { level, charging, full }) => {
                return Ok(DeviceReading {
                    online: true,
                    level: Some(level),
                    charging,
                    full_on_charger: full,
                    detail: None,
                    ..template
                });
            }
        }
    }
    // Reports arrive, but none carries the battery: the short Bluetooth report
    Ok(if any { DeviceReading { online: true, detail: Some(t("d.waitFullMode")), ..template } } else { template })
}

pub fn parse_dual_sense(r: &[u8], bluetooth: bool) -> Option<Battery> {
    let at = if bluetooth { 54 } else { 53 };
    if r.len() <= at || r[0] != if bluetooth { 0x31 } else { 0x01 } {
        return None;
    }
    let (data, status) = ((r[at] & 0x0F) as i32, r[at] >> 4);
    Some(match status {
        0 => Battery::Level { level: (data * 10 + 5).min(100), charging: false, full: false },
        1 => Battery::Level { level: (data * 10 + 5).min(100), charging: true, full: false },
        2 => Battery::Level { level: 100, charging: false, full: true },
        _ => Battery::Error, // 0xA/0xB voltage or temperature error, 0xF charging error
    })
}

pub fn parse_dual_shock4(r: &[u8], bluetooth: bool, adapter: bool) -> Option<Battery> {
    let at = if bluetooth { 32 } else { 30 };
    if r.len() <= at + 1 || r[0] != if bluetooth { 0x11 } else { 0x01 } {
        return None;
    }
    if adapter && r[at + 1] & 0x04 != 0 {
        return Some(Battery::NoController);
    }
    let data = (r[at] & 0x0F) as i32;
    let cable = r[at] & 0x10 != 0;
    if !cable {
        return Some(Battery::Level {
            level: if data < 10 { data * 10 + 5 } else { 100 },
            charging: false,
            full: false,
        });
    }
    Some(match data {
        0..=9 => Battery::Level { level: data * 10 + 5, charging: true, full: false },
        10 => Battery::Level { level: 100, charging: true, full: false },
        11 => Battery::Level { level: 100, charging: false, full: true },
        _ => Battery::Error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_sense_usb_and_bluetooth() {
        let mut usb = vec![0u8; 64];
        usb[0] = 0x01;
        usb[53] = 0x17; // charging, 70-79%
        assert_eq!(parse_dual_sense(&usb, false), Some(Battery::Level { level: 75, charging: true, full: false }));
        let mut bt = vec![0u8; 78];
        bt[0] = 0x31;
        bt[54] = 0x2A;
        assert_eq!(parse_dual_sense(&bt, true), Some(Battery::Level { level: 100, charging: false, full: true }));
        assert_eq!(parse_dual_sense(&usb[..10], false), None, "the short report");
    }

    #[test]
    fn dual_shock4_on_the_adapter_without_a_controller() {
        let mut r = vec![0u8; 64];
        r[0] = 0x01;
        r[31] = 0x04;
        assert_eq!(parse_dual_shock4(&r, false, true), Some(Battery::NoController));
        r[31] = 0;
        r[30] = 0x1B;
        assert_eq!(
            parse_dual_shock4(&r, false, true),
            Some(Battery::Level { level: 100, charging: false, full: true })
        );
    }
}
