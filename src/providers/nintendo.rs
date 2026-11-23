//! Nintendo Switch Pro Controller and Joy-Con on Bluetooth. They carry their battery in the header of their full
//! input reports, so nothing is sent: reports are read for a moment, as for the PlayStation controllers.
//!   Reports 30 (the full state), 31 (the same with NFC/IR data) and 21 (a reply to a subcommand) start
//!   `id timer BC ..`. BC's high nibble is the battery: bits 7-5 the level in five steps (0 empty, 1 critical,
//!   2 low, 3 medium, 4 full; above 4 undefined), bit 4 charging. Its low nibble says how the controller is
//!   connected.
//! Over Bluetooth the controller sends the short report 3F, without the battery, until a game or Steam switches it
//! to the full report with a subcommand. Nothing of that is sent from here, so the level shows only while something
//! keeps the controller in full mode. On USB it reports only after a handshake that changes its USB state, and it
//! charges there anyway, so it isn't read on its cable. Third-party pads in Switch mode that take these IDs (8BitDo's
//! Pro 2, for one) send the same header and are read as the controller whose IDs they took.
//! Sources: dekuNukem's Nintendo_Switch_Reverse_Engineering (bluetooth_hid_notes.md, "Standard input report
//! format"), SDL (src/joystick/hidapi/SDL_hidapi_switch.c, HandleFullControllerState: the percentage as level / 8 in
//! the nibble's terms), Linux (drivers/hid/hid-nintendo.c, joycon_parse_report) and HaloBattery
//! (providers/nintendo.py). Own implementation, not yet verified on hardware by this project.

use super::{contains_ignore_case, remaining_ms, secs, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::{t, tf};
use crate::reading::{DeviceKind, DeviceReading};
use crate::sys;
use std::time::{Duration, Instant};

const NINTENDO_VID: u16 = 0x057E;
/// The full report comes 60 times a second (120 from the Pro Controller), so a quarter of a second sees several.
const TIMEOUT_MS: u64 = 250;

const MODELS: [(u16, &str); 3] =
    [(0x2006, "Joy-Con (L)"), (0x2007, "Joy-Con (R)"), (0x2009, "Nintendo Switch Pro Controller")];

fn model(pid: u16) -> Option<&'static str> {
    MODELS.iter().find(|m| m.0 == pid).map(|m| m.1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub level: i32,
    pub charging: bool,
    pub full: bool,
}

pub struct NintendoProvider;

impl Provider for NintendoProvider {
    fn name(&self) -> &'static str {
        "Nintendo Switch"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let mut result: Vec<DeviceReading> = Vec::new();
        let pads = hid::enumerate(|v, p| v == NINTENDO_VID && model(p).is_some());
        for info in pads.iter().filter(|i| contains_ignore_case(&i.path, "{00001124-")).filter(|i| {
            i.access_denied || (i.usage_page == 0x01 && matches!(i.usage, 0x04 | 0x05) && i.input_length >= 3)
        }) {
            let Some(name) = model(info.product_id) else { continue };
            // Several identical controllers get #2, #3... in the order Windows lists them
            let mut id = format!("switch:{:04x}", info.product_id);
            let mut name = name.to_string();
            let same = result.iter().filter(|r| r.id.starts_with(&id)).count();
            if same > 0 {
                id = format!("{id}#{}", same + 1);
                name = format!("{name} #{}", same + 1);
            }

            let mut reading = DeviceReading {
                detail: info.access_denied.then(|| t("d.busy")),
                ..DeviceReading::new(id, name, DeviceKind::Gamepad, "Bluetooth")
            };
            if !info.access_denied {
                reading = match read(info, reading.clone()) {
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

fn read(info: &HidInfo, template: DeviceReading) -> sys::Result<DeviceReading> {
    let dev = HidDevice::open_read_only(info)?;
    dev.flush();
    let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS);
    let mut any = false;
    while Instant::now() < deadline {
        let Some(r) = dev.read(remaining_ms(deadline))? else { break };
        any = true;
        if let Some(Battery { level, charging, full }) = parse(&r) {
            return Ok(DeviceReading {
                online: true,
                level: Some(level),
                estimate: true,
                charging,
                full_on_charger: full,
                detail: Some(t("d.fiveLevels")),
                ..template
            });
        }
    }
    // Reports arrive, but none carries the battery: the short report of the simple mode
    Ok(if any { DeviceReading { online: true, detail: Some(t("d.waitFullMode")), ..template } } else { template })
}

/// The battery from a full report or a subcommand reply; None for any other report and for a level the protocol
/// doesn't have. The level is shown as SDL shows it: 0, 25, 50, 75 or 100.
pub fn parse(r: &[u8]) -> Option<Battery> {
    if r.len() < 3 || !matches!(r[0], 0x21 | 0x30 | 0x31) {
        return None;
    }
    let step = (r[2] >> 5) as i32;
    if step > 4 {
        return None;
    }
    let charging = r[2] & 0x10 != 0;
    Some(Battery { level: step * 25, charging: charging && step < 4, full: charging && step == 4 })
}

#[cfg(test)]
mod tests {
    //! No capture of these reports is published: they are put together from dekuNukem's table of the header, with
    //! the timer byte HaloBattery's tests use.
    use super::*;
    use crate::providers::pad;

    fn report(id: u8, battery: u8) -> Vec<u8> {
        pad(49, &[id, 0x5A, battery])
    }

    #[test]
    fn the_high_nibble_is_the_level_and_the_charging_bit() {
        // Full on battery, connected as a Pro Controller (low nibble 0)
        assert_eq!(parse(&report(0x30, 0x80)), Some(Battery { level: 100, charging: false, full: false }));
        // Medium, a Joy-Con (low nibble E)
        assert_eq!(parse(&report(0x30, 0x6E)), Some(Battery { level: 75, charging: false, full: false }));
        // Low and charging, USB powered (low nibble 1)
        assert_eq!(parse(&report(0x21, 0x51)), Some(Battery { level: 50, charging: true, full: false }));
        // Full on the charger
        assert_eq!(parse(&report(0x31, 0x91)), Some(Battery { level: 100, charging: false, full: true }));
        assert_eq!(parse(&report(0x30, 0x00)), Some(Battery { level: 0, charging: false, full: false }));
    }

    #[test]
    fn other_reports_and_undefined_levels_are_no_reading() {
        // The simple mode's report has no battery
        assert_eq!(parse(&pad(12, &[0x3F, 0x00, 0x00, 0x08])), None);
        assert_eq!(parse(&report(0x30, 0xA0)), None, "a level above full");
        assert_eq!(parse(&[0x30, 0x5A]), None, "too short");
    }
}
