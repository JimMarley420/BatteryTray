//! Wireless keyboards on the SONiX 2.4 GHz dongle 0C45:FEFE ("2.4G Dongle"): the Attack Shark X85 (issue #21), the
//! EWEADN X87, AJAZZ AK-series boards and others. The dongle has two vendor
//! collections with the usage FF60:0061; the one with 33-byte reports (report ID 0 and 32 bytes) takes the battery
//! request, and it is all this sends:
//!   request  20 01 00 .. 00 21      the last byte is the sum of the others
//!   reply    20 01 00 LL ..         LL level in percent
//! The vendor's Windows tool for these boards asks the same way every 15 s and reads nothing but the level, so there
//! is no charging sign. A keyboard that is asleep or off doesn't answer, or the dongle answers for it with a level of
//! 0. Some boards that share these IDs have other firmware and never give a level this way.
//! Protocol facts from vigil-watt (docs/hardware/eweadn.md: the EWEADN X87's vendor tool read statically and the
//! exchange measured on its 0C45:FEFE dongle; the X87 has the X85's USB IDs and the same vendor tool),
//! sonix-keyboard-studio and Wireless-Device-Battery-Monitor (src/drivers/sonix.cpp). Own implementation. Not yet
//! verified on hardware by this project, which is why the first exchange goes to the log.

use super::{no_link_reason, receiver_name, remaining_ms, secs, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::{t, tf};
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use crate::win;
use std::collections::HashSet;
use std::time::{Duration, Instant};

const VID: u16 = 0x0C45;
const PID: u16 = 0xFEFE;
const VENDOR_PAGE: u16 = 0xFF60;
const VENDOR_USAGE: u16 = 0x0061;
const BATTERY_COMMAND: u8 = 0x20;
const BATTERY_ARGUMENT: u8 = 0x01;
/// The report ID, always 0, and 32 bytes.
const REPORT_LENGTH: usize = 33;
const TIMEOUT_MS: u64 = 500;
const ID: &str = "sonix:0c45:fefe";

#[derive(Default)]
pub struct SonixKeyboardProvider {
    logged: HashSet<bool>,
}

impl Provider for SonixKeyboardProvider {
    fn name(&self) -> &'static str {
        "SONiX 2.4G"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let collections = hid::enumerate(|v, p| v == VID && p == PID);
        if collections.is_empty() {
            return Vec::new();
        }

        // The other FF60:0061 collection has 65-byte reports and takes no battery request
        let vendor = collections.iter().find(|i| {
            i.usage_page == VENDOR_PAGE
                && i.usage == VENDOR_USAGE
                && i.output_length == REPORT_LENGTH
                && i.input_length == REPORT_LENGTH
        });
        let mut trace = Vec::new();
        let mut level = None;
        let mut error = None;
        match vendor {
            None => error = Some(t(no_link_reason(&collections))),
            Some(v) => match query(v, &mut trace) {
                Ok(l) => level = l,
                Err(e) => {
                    trace.push(format!("error: {e}"));
                    error = Some(tf("d.error", &[&e]));
                }
            },
        }
        let product =
            vendor.and_then(|v| v.product.as_deref()).or_else(|| collections.iter().find_map(|i| i.product.as_deref()));
        self.log_once(&collections, vendor, product, &trace, level.is_some());

        vec![DeviceReading {
            online: level.is_some(),
            level,
            detail: if level.is_none() { Some(error.unwrap_or_else(|| t("d.asleep"))) } else { None },
            ..DeviceReading::new(
                ID,
                receiver_name::keyboard(product, Some(&t("name.fa09Keyboard"))),
                DeviceKind::Keyboard,
                t("via.24"),
            )
        }]
    }

    /// The level changes slowly; a keyboard that didn't answer is asked again sooner, to catch it waking up.
    fn delay(&self, last: &[DeviceReading]) -> Duration {
        if last.iter().any(|r| r.online) {
            secs(300)
        } else {
            secs(60)
        }
    }
}

impl SonixKeyboardProvider {
    /// Logs the dongle's HID collections and what was sent and received, the first time the keyboard is found off
    /// and the first time it is found on in a session, as the other keyboards and mice do.
    fn log_once(
        &mut self,
        collections: &[HidInfo],
        vendor: Option<&HidInfo>,
        product: Option<&str>,
        trace: &[String],
        online: bool,
    ) {
        if !self.logged.insert(online) {
            return;
        }
        log::write(&format!(
            "SONiX {VID:04x}:{PID:04x}: found {}, {} HID collections, product \"{}\"",
            if online { "on" } else { "off" },
            collections.len(),
            product.unwrap_or_default()
        ));
        for info in collections {
            let role = if vendor.is_some_and(|v| v.path == info.path) { "asked" } else { "not used" };
            log::write(&format!("  mi={} {info}: {role}", info.interface_number()));
        }
        log::write(&format!("  {}", if trace.is_empty() { "nothing sent".into() } else { trace.join("  ") }));
    }
}

/// Asks for the level; None when the keyboard didn't answer or the reply has none.
fn query(vendor: &HidInfo, trace: &mut Vec<String>) -> win::Result<Option<i32>> {
    let dev = HidDevice::open(vendor)?;
    // The first request can go unanswered while the link wakes up
    let reply = match ask(&dev, trace)? {
        Some(r) => Some(r),
        None => ask(&dev, trace)?,
    };
    Ok(reply.and_then(|r| parse_level(&r)))
}

/// Sends the battery request and waits for its reply; None when it didn't come.
fn ask(dev: &HidDevice, trace: &mut Vec<String>) -> win::Result<Option<Vec<u8>>> {
    dev.flush();
    let request = request();
    trace.push(format!("> {}", hex(&request)));
    if !dev.write(&request, TIMEOUT_MS as u32)? {
        trace.push("(not sent)".into());
        return Ok(None);
    }
    let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS);
    while Instant::now() < deadline {
        let Some(r) = dev.read(remaining_ms(deadline))? else {
            trace.push("< nothing".into());
            return Ok(None);
        };
        trace.push(format!("< {}", hex(&r)));
        if is_reply(&r) {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

/// 00 20 01 00 .. and the sum of the bytes before it, as the vendor tool sends it.
pub fn request() -> Vec<u8> {
    let mut r = vec![0u8; REPORT_LENGTH];
    r[1] = BATTERY_COMMAND;
    r[2] = BATTERY_ARGUMENT;
    r[REPORT_LENGTH - 1] = r[..REPORT_LENGTH - 1].iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    r
}

/// The answer to the battery request, report ID first: the command comes back with a 0 after it.
pub fn is_reply(r: &[u8]) -> bool {
    r.len() >= 5 && r[1] == BATTERY_COMMAND && r[2] == BATTERY_ARGUMENT && r[3] == 0x00
}

/// The level a reply carries; None for 0 (the dongle answering for a keyboard that's off, or the request coming
/// back) and for anything over 100.
pub fn parse_level(r: &[u8]) -> Option<i32> {
    if !is_reply(r) {
        return None;
    }
    Some(r[4] as i32).filter(|l| (1..=100).contains(l))
}

/// A report for the log: without the zeros that pad it. The request shows whole, its checksum being the last byte.
fn hex(report: &[u8]) -> String {
    let used = report.iter().rposition(|&b| b != 0).map_or(1, |i| i + 1);
    super::hex(&report[..used.min(report.len())])
}

#[cfg(test)]
mod tests {
    //! The battery read of keyboards on the SONiX dongle 0C45:FEFE (Attack Shark X85, EWEADN X87 and others): 33-byte
    //! reports, report ID 0 first. The exchange is the one vigil-watt (docs/hardware/eweadn.md) measured on an EWEADN
    //! X87: 00 20 01 00 .. 21 out, 00 20 01 00 64 .. back.
    use super::*;

    /// A 33-byte report starting with the given bytes, zero padded.
    fn report(head: &[u8]) -> Vec<u8> {
        let mut r = vec![0u8; 33];
        r[..head.len()].copy_from_slice(head);
        r
    }

    #[test]
    fn request_is_the_captured_one() {
        let mut expected = report(&[0x00, 0x20, 0x01]);
        expected[32] = 0x21;
        assert_eq!(request(), expected);
    }

    #[test]
    fn eweadn_x87_at_full() {
        let mut r = report(&[0x00, 0x20, 0x01, 0x00, 0x64]);
        r[32] = 0x85;
        assert_eq!(parse_level(&r), Some(100));
    }

    #[test]
    fn level_in_percent() {
        assert_eq!(parse_level(&report(&[0x00, 0x20, 0x01, 0x00, 0x49])), Some(73));
        assert_eq!(parse_level(&report(&[0x00, 0x20, 0x01, 0x00, 0x01])), Some(1));
    }

    #[test]
    fn zero_is_a_keyboard_that_is_off() {
        assert_eq!(parse_level(&report(&[0x00, 0x20, 0x01, 0x00, 0x00])), None);
    }

    #[test]
    fn the_request_coming_back_is_no_reading() {
        assert_eq!(parse_level(&request()), None);
    }

    #[test]
    fn level_over_100_is_no_reading() {
        assert_eq!(parse_level(&report(&[0x00, 0x20, 0x01, 0x00, 0x65])), None);
        assert_eq!(parse_level(&report(&[0x00, 0x20, 0x01, 0x00, 0xFF])), None);
    }

    #[test]
    fn other_reports_are_not_the_reply() {
        assert!(!is_reply(&report(&[0x00, 0x55, 0x10, 0x14, 0x00, 0x00, 0x01, 0x00, 0x00, 0x40])));
        assert!(!is_reply(&report(&[0x00, 0x20, 0x02, 0x00, 0x64])));
        assert!(!is_reply(&report(&[0x00, 0x20, 0x01, 0x01, 0x64])));
        assert_eq!(parse_level(&report(&[0x00, 0x0F, 0x01, 0x01, 0x11])), None);
    }

    #[test]
    fn a_short_report_is_not_the_reply() {
        assert!(!is_reply(&[0x00, 0x20, 0x01, 0x00]));
        assert!(!is_reply(&[]));
    }

    #[test]
    fn the_whole_request_goes_to_the_log() {
        assert!(hex(&request()).ends_with(" 21"));
        assert_eq!(hex(&report(&[0x00, 0x20, 0x01, 0x00, 0x64])), "00 20 01 00 64");
        assert_eq!(hex(&report(&[])), "00");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn keyboard_name_from_the_dongle() {
        let fallback = t("name.fa09Keyboard");
        assert_eq!(receiver_name::keyboard(Some("2.4G Dongle"), Some(&fallback)), "Keyboard on a 2.4G receiver");
        assert_eq!(receiver_name::keyboard(Some("Neo Ergo 2.4G"), None), "Neo Ergo");
        assert_eq!(receiver_name::keyboard(Some("AK820MAX 2.4G"), None), "AK820MAX keyboard");
        assert_eq!(receiver_name::keyboard(Some("AJAZZ AK820MAX"), None), "AJAZZ AK820MAX");
        assert_eq!(receiver_name::keyboard(None, None), "Keyboard");
    }
}
