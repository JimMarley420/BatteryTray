//! SteelSeries wireless mice, over their 2.4 GHz receiver or the cable. The
//! battery command is an output report on the mouse's vendor interface, answered by an input report. Commands from
//! rivalcfg (github.com/flozz/rivalcfg):
//!   Aerox 3/5/9 Wireless, Prime (Mini) Wireless: 0x92 over the cable, 0xD2 (0x92 | 0x40) over 2.4 GHz; the reply's
//!     second data byte b holds charging in bit 7 and level = ((b & 0x7F) - 1) * 5.
//!   Aerox 3 Wireless Gen 2: the same commands, a different reply: [command, status, level, 0], status 0 on battery,
//!     1 charging, 2 charged, the level in percent. The level is FF for about a minute after the mouse goes on or
//!     off the cable, and an asleep or switched-off mouse answers 40 FF 00 00 instead (rivalcfg issue 276).
//!   Rival 3 Wireless (and Gen 2), Rival 650 Wireless: AA 01; reply [level, ?, charging].
//! A Gen 1 Aerox or Rival with a level outside 0-100 is off. Read-only; not yet verified on hardware by this project.

use super::{group_by, one_per_id, remaining_ms, secs, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::{t, tf};
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use crate::sys;
use std::collections::HashSet;
use std::time::{Duration, Instant};

const STEELSERIES_VID: u16 = 0x1038;
const TIMEOUT_MS: u64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Aerox,
    AeroxGen2,
    Rival,
}

/// What a mouse answered. `level` is None while the mouse doesn't know it yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub level: Option<i32>,
    pub charging: bool,
    pub full: bool,
}

struct Model {
    key: &'static str,
    name: &'static str,
    interface: i32,
    protocol: Protocol,
    wireless: bool,
}

const fn m(key: &'static str, name: &'static str, interface: i32, protocol: Protocol, wireless: bool) -> Model {
    Model { key, name, interface, protocol, wireless }
}

static MODELS: &[(u16, Model)] = &[
    (0x1838, m("aerox3", "SteelSeries Aerox 3 Wireless", 3, Protocol::Aerox, true)),
    (0x1878, m("aerox3", "SteelSeries Aerox 3 Wireless", 3, Protocol::Aerox, true)),
    (0x183A, m("aerox3", "SteelSeries Aerox 3 Wireless", 3, Protocol::Aerox, false)),
    (0x187A, m("aerox3", "SteelSeries Aerox 3 Wireless", 3, Protocol::Aerox, false)),
    (0x1890, m("aerox3gen2", "SteelSeries Aerox 3 Wireless Gen 2", 3, Protocol::AeroxGen2, true)),
    (0x1892, m("aerox3gen2", "SteelSeries Aerox 3 Wireless Gen 2", 3, Protocol::AeroxGen2, false)),
    (0x1852, m("aerox5", "SteelSeries Aerox 5 Wireless", 3, Protocol::Aerox, true)),
    (0x185C, m("aerox5", "SteelSeries Aerox 5 Wireless", 3, Protocol::Aerox, true)),
    (0x1860, m("aerox5", "SteelSeries Aerox 5 Wireless", 3, Protocol::Aerox, true)),
    (0x1854, m("aerox5", "SteelSeries Aerox 5 Wireless", 3, Protocol::Aerox, false)),
    (0x185E, m("aerox5", "SteelSeries Aerox 5 Wireless", 3, Protocol::Aerox, false)),
    (0x1862, m("aerox5", "SteelSeries Aerox 5 Wireless", 3, Protocol::Aerox, false)),
    (0x1858, m("aerox9", "SteelSeries Aerox 9 Wireless", 3, Protocol::Aerox, true)),
    (0x1874, m("aerox9", "SteelSeries Aerox 9 Wireless", 3, Protocol::Aerox, true)),
    (0x185A, m("aerox9", "SteelSeries Aerox 9 Wireless", 3, Protocol::Aerox, false)),
    (0x1876, m("aerox9", "SteelSeries Aerox 9 Wireless", 3, Protocol::Aerox, false)),
    (0x1840, m("prime", "SteelSeries Prime Wireless", 3, Protocol::Aerox, true)),
    (0x1842, m("prime", "SteelSeries Prime Wireless", 3, Protocol::Aerox, false)),
    (0x1848, m("primemini", "SteelSeries Prime Mini Wireless", 3, Protocol::Aerox, true)),
    (0x184A, m("primemini", "SteelSeries Prime Mini Wireless", 3, Protocol::Aerox, false)),
    (0x1830, m("rival3", "SteelSeries Rival 3 Wireless", 3, Protocol::Rival, true)),
    (0x1872, m("rival3gen2", "SteelSeries Rival 3 Wireless Gen 2", 3, Protocol::Rival, true)),
    (0x1726, m("rival650", "SteelSeries Rival 650 Wireless", 0, Protocol::Rival, true)),
    (0x172B, m("rival650", "SteelSeries Rival 650 Wireless", 0, Protocol::Rival, false)),
];

fn model(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|(p, _)| *p == pid).map(|(_, m)| m)
}

#[derive(Default)]
pub struct SteelSeriesProvider {
    logged: HashSet<String>,
}

impl Provider for SteelSeriesProvider {
    fn name(&self) -> &'static str {
        "SteelSeries"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let mut result = Vec::new();
        let found = hid::enumerate(|v, p| v == STEELSERIES_VID && model(p).is_some());
        for (pid, collections) in group_by(found, |i| i.product_id) {
            let Some(model) = model(pid) else { continue };
            let mut asked: Vec<(String, Vec<String>)> = Vec::new();
            let mut battery = None;
            let mut error = None;
            // The vendor collection of the configuration interface; a single-interface device has no "&mi_"
            for info in collections.iter().filter(|i| i.interface_number().max(0) == model.interface) {
                if info.access_denied {
                    error = Some(t("d.needsAdmin"));
                    continue;
                }
                if info.usage_page < 0xFF00 || info.output_length == 0 || info.input_length == 0 {
                    continue;
                }
                let mut trace = Vec::new();
                match query(info, model, &mut trace) {
                    Ok(b) => battery = b,
                    Err(e) => {
                        trace.push(format!("error: {e}"));
                        error = Some(tf("d.error", &[&e]));
                    }
                }
                asked.push((info.path.clone(), trace));
                if battery.is_some() {
                    break;
                }
            }
            self.log_once(pid, &collections, &asked, battery.is_some());

            let detail = match battery {
                None => Some(error.unwrap_or_else(|| t("d.asleep"))),
                Some(Battery { level: None, .. }) => Some(t("d.levelUnknown")),
                Some(_) => None,
            };
            result.push(DeviceReading {
                online: battery.is_some(),
                level: battery.and_then(|b| b.level),
                charging: battery.is_some_and(|b| b.charging),
                full_on_charger: battery.is_some_and(|b| b.full),
                detail,
                ..DeviceReading::new(
                    format!("steelseries:{}", model.key),
                    model.name,
                    DeviceKind::Mouse,
                    t(if model.wireless { "via.24" } else { "via.cable" }),
                )
            });
        }
        // A mouse charging on its cable while its receiver stays plugged in is listed twice under one Id, and the
        // receiver finds it off: keep the reading that reached the mouse
        one_per_id(result, |r| r.online)
    }

    fn delay(&self, last: &[DeviceReading]) -> Duration {
        // A Gen 2 that doesn't know its level yet will within a minute of going on or off the cable
        if last.iter().any(|r| r.online && r.level.is_none()) {
            return secs(20);
        }
        if last.iter().any(|r| r.online) {
            secs(60)
        } else {
            secs(20)
        }
    }
}

impl SteelSeriesProvider {
    /// Logs how a mouse was queried - its HID collections, the request and the replies - the first time it is found
    /// off and the first time it is found on in a session. None of these mice is tested yet, so when one isn't
    /// reached or reads wrong, these lines show which collection answered and with what bytes.
    fn log_once(&mut self, pid: u16, collections: &[HidInfo], asked: &[(String, Vec<String>)], online: bool) {
        let id = format!("{STEELSERIES_VID:04x}:{pid:04x}");
        if !self.logged.insert(format!("{id}{online}")) {
            return;
        }
        log::write(&format!(
            "SteelSeries {id}: found {}, {} HID collections",
            if online { "on" } else { "off" },
            collections.len()
        ));
        for info in collections {
            let trace =
                asked.iter().find(|a| a.0 == info.path).map(|a| a.1.join("  ")).unwrap_or_else(|| "not asked".into());
            log::write(&format!("  mi={} {info}: {trace}", info.interface_number()));
        }
    }
}

fn query(info: &HidInfo, model: &Model, trace: &mut Vec<String>) -> sys::Result<Option<Battery>> {
    let dev = HidDevice::open(info)?;
    dev.flush();
    let command = match model.protocol {
        Protocol::Rival => 0xAA,
        _ if model.wireless => 0xD2,
        _ => 0x92,
    };
    // Byte 0 is the report ID (the device uses none)
    let report: Vec<u8> = if model.protocol == Protocol::Rival { vec![0, 0xAA, 0x01] } else { vec![0, command] };
    trace.push(format!("> {}", hex(&report)));
    if !dev.write(&report, TIMEOUT_MS as u32)? {
        trace.push("(not sent)".into());
        return Ok(None);
    }

    // rivalcfg takes the next input report. If the reply echoes the command, prefer that one, so an unrelated event
    // report arriving first can't be mistaken for the answer.
    let mut first = None;
    let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS);
    while Instant::now() < deadline {
        let Some(r) = dev.read(remaining_ms(deadline))? else {
            trace.push("< nothing".into());
            break;
        };
        trace.push(format!("< {}", hex(&r)));
        if r.len() > 1 && r[1] == command {
            return Ok(parse(&r, model.protocol, command));
        }
        first.get_or_insert(r);
    }
    Ok(first.and_then(|r| parse(&r, model.protocol, command)))
}

/// Reads the battery from an input report as Windows returns it, report ID first; None = the mouse is off.
pub fn parse(r: &[u8], protocol: Protocol, command: u8) -> Option<Battery> {
    // r[0] is the report ID; rivalcfg's data[n] is r[n + 1]
    match protocol {
        Protocol::Aerox => {
            if r.len() < 3 {
                return None;
            }
            let level = ((r[2] & 0x7F) as i32 - 1) * 5;
            (0..=100).contains(&level).then_some(Battery {
                level: Some(level),
                charging: r[2] & 0x80 != 0,
                full: false,
            })
        }
        Protocol::AeroxGen2 => {
            // Anything but the echo of the command, 40 FF 00 00 included, means the mouse is asleep or off
            if r.len() < 4 || r[1] != command {
                return None;
            }
            let (status, level) = (r[2], r[3]);
            Some(Battery { level: (level <= 100).then_some(level as i32), charging: status == 1, full: status == 2 })
        }
        Protocol::Rival => {
            if r.len() < 4 {
                return None;
            }
            let level = r[1] as i32;
            (0..=100).contains(&level).then_some(Battery { level: Some(level), charging: r[3] != 0, full: false })
        }
    }
}

/// A report for the log as rivalcfg prints it: without the report ID in front and the zeros that pad it, and no
/// more than its first 32 bytes.
fn hex(report: &[u8]) -> String {
    let used = report.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    if used <= 1 {
        return "00".into();
    }
    let shown = used.min(33) - 1;
    let text = super::hex(&report[1..1 + shown]);
    if used > 33 {
        text + " ..."
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    //! The Aerox 3 Wireless Gen 2 replies, as a Gen 2 owner logged them over a full discharge and charge in rivalcfg
    //! issue 276. Windows puts the report ID (none, so 00) in front of what rivalcfg prints.
    use super::*;

    const WIRELESS: u8 = 0xD2;
    const CABLE: u8 = 0x92;

    fn gen2(command: u8, data: &[u8]) -> Option<Battery> {
        let mut report = vec![0u8; 65];
        report[1..1 + data.len()].copy_from_slice(data);
        parse(&report, Protocol::AeroxGen2, command)
    }

    fn b(level: Option<i32>, charging: bool, full: bool) -> Option<Battery> {
        Some(Battery { level, charging, full })
    }

    #[test]
    fn on_battery_over_the_receiver() {
        for (level, percent) in [(0x62, 98), (0x36, 54), (0x03, 3), (0x00, 0)] {
            assert_eq!(gen2(WIRELESS, &[0xD2, 0x00, level, 0x00]), b(Some(percent), false, false));
        }
    }

    #[test]
    fn charging_on_the_cable() {
        assert_eq!(gen2(CABLE, &[0x92, 0x01, 0x23, 0x00]), b(Some(35), true, false));
        assert_eq!(gen2(CABLE, &[0x92, 0x01, 0x64, 0x00]), b(Some(100), true, false));
    }

    #[test]
    fn charged_on_the_cable() {
        assert_eq!(gen2(CABLE, &[0x92, 0x02, 0x64, 0x00]), b(Some(100), false, true));
    }

    #[test]
    fn level_not_known_right_after_going_on_or_off_the_cable() {
        assert_eq!(gen2(CABLE, &[0x92, 0x01, 0xFF, 0x00]), b(None, true, false));
        assert_eq!(gen2(WIRELESS, &[0xD2, 0x00, 0xFF, 0x00]), b(None, false, false));
    }

    #[test]
    fn asleep_or_off_when_the_reply_is_not_the_echo() {
        assert_eq!(gen2(WIRELESS, &[0x40, 0xFF, 0x00, 0x00]), None);
        assert_eq!(gen2(CABLE, &[0x11, 0x00]), None);
    }

    #[test]
    fn too_short_is_no_reading() {
        assert_eq!(parse(&[0x00, 0xD2, 0x00], Protocol::AeroxGen2, WIRELESS), None);
    }

    #[test]
    fn first_aerox_still_reads_its_own_way() {
        // Bit 7 is charging, the rest is (level / 5) + 1: 0x95 = charging, 100%
        assert_eq!(parse(&[0x00, 0xD2, 0x95], Protocol::Aerox, WIRELESS), b(Some(100), true, false));
        assert_eq!(parse(&[0x00, 0x92, 0x0B], Protocol::Aerox, CABLE), b(Some(50), false, false));
    }
}
