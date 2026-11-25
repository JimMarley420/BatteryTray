//! Mice with the product ID 2255 under the vendor IDs A8A5 (2.4 GHz receiver) and A8A4 (USB cable): the AJAZZ AJ139
//! V2 MC, the K-Snake X11, the Noir M2-NEX, the MCHOSE G7 Pro and others; and the MCHOSE G3 V2 (3837:4245 on its
//! receiver, 3837:418C on its cable). They
//! share one configuration protocol on the vendor collection FF01:0010, 64-byte reports with no report ID in both
//! directions:
//!   request  55 30 A5 CC 2E 00 ..      30 = battery, CC = the low byte of the sum of the bytes after it
//!   reply    AA 30 .. [8] level in percent, [9] not 0 on USB power
//! The mouse also sends reports of its own (AA FA, AA ED), which are skipped. A receiver whose mouse is off or out of
//! range answers with a level of 0. The vendor's web panel writes the request differently, 55 30 A5 0B 2E 01 01, so
//! that form is tried when the first one gets no answer, and the one that worked goes first the next time. The
//! G3 V2's own monitor sends one more 01, 55 30 A5 0B 2E 01 01 01, which goes first on that mouse. Only this read is
//! sent. The IDs don't tell the models apart, so the name comes from the product string (see `receiver_name`).
//!
//! The AJAZZ T520 Max uses A8A5:2266 on the receiver and A8A4:2266 on the cable. Both use FF01:0010.
//! The read-only model report (55 52) identifies the T520 Max before it is queried with form 2 or named.
//! Both connections were verified with firmware 20250830111206-T520-3395.
//!
//! Protocol facts from ajazz-aj139-macos (PROTOCOL.md: checked on an AJ139 V2 MC), OpenMouse (mouse-protocol:
//! src/ksnake/index.ts, the vendor panel's form of the request, read on a Noir M2-NEX) and
//! Wireless-Device-Battery-Monitor (src/drivers/misc_mice.cpp: both forms in this order; an AJ139 V2 MC and an MCHOSE
//! G7 Pro read by users in its issues 40 and 226; src/drivers/mchose_g3.cpp) and MCHOSE-Monitor (docs/HID_PROTOCOL.md:
//! the G3 V2). Own implementation. Not yet verified on hardware by this project,
//! which is why the first exchange goes to the log.

use super::{group_by, hex_trimmed as hex, no_link_reason, one_per_id, receiver_name, remaining_ms, secs, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::{t, tf};
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use crate::sys;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Mice of one kind: their receiver and cable IDs, what to call one whose product string names nobody, and the
/// forms of the request to try, in order (see `request`).
struct Family {
    id: &'static str,
    receiver: (u16, u16),
    cable: Option<(u16, u16)>,
    name: Option<&'static str>,
    forms: &'static [usize],
    firmware_id: Option<&'static [u8]>,
}

static FAMILIES: &[Family] = &[
    Family {
        id: "ajazz:2255",
        receiver: (0xA8A5, 0x2255),
        cable: Some((0xA8A4, 0x2255)),
        name: None,
        forms: &[0, 1],
        firmware_id: None,
    },
    Family {
        id: "ajazz:t520-max",
        receiver: (0xA8A5, 0x2266),
        cable: Some((0xA8A4, 0x2266)),
        name: Some("AJAZZ T520 Max"),
        forms: &[2],
        firmware_id: Some(b"20250830111206-T520-3395"),
    },
    Family {
        id: "mchose:g3v2",
        receiver: (0x3837, 0x4245),
        cable: Some((0x3837, 0x418C)),
        name: Some("MCHOSE G3 V2"),
        forms: &[2, 1],
        firmware_id: None,
    },
];

/// The family these IDs belong to, and whether they are its cable's.
fn family(vid: u16, pid: u16) -> Option<(&'static Family, bool)> {
    FAMILIES.iter().find_map(|f| match (vid, pid) {
        ids if ids == f.receiver => Some((f, false)),
        ids if Some(ids) == f.cable => Some((f, true)),
        _ => None,
    })
}

const VENDOR_PAGE: u16 = 0xFF01;
const VENDOR_USAGE: u16 = 0x0010;
const REQUEST_HEAD: u8 = 0x55;
const REPLY_HEAD: u8 = 0xAA;
const READ_MARK: u8 = 0xA5;
const BATTERY_COMMAND: u8 = 0x30;
const MODEL_COMMAND: u8 = 0x52;
const BATTERY_LENGTH: u8 = 0x2E;
/// The report ID, always 0, and 64 bytes.
const REPORT_LENGTH: usize = 65;
const TIMEOUT_MS: u64 = 700;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub level: i32,
    pub charging: bool,
}

#[derive(Default)]
pub struct AjazzMouseProvider {
    /// The request form each mouse answered last, by device path: it is the one tried first.
    answered: HashMap<String, usize>,
    logged: HashSet<String>,
}

impl Provider for AjazzMouseProvider {
    fn name(&self) -> &'static str {
        "AJAZZ / K-Snake"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let mut result = Vec::new();
        let found = hid::enumerate(|v, p| family(v, p).is_some());
        for ((vid, pid), collections) in group_by(found, |i| (i.vendor_id, i.product_id)) {
            let Some((family, cable)) = family(vid, pid) else { continue };
            // The T520 Max web driver sends 63 payload bytes. HidDevice pads to the collection's report length.
            let minimum_length = if family.firmware_id.is_some() { 64 } else { REPORT_LENGTH };
            let vendor = collections.iter().find(|i| {
                i.usage_page == VENDOR_PAGE
                    && i.usage == VENDOR_USAGE
                    && i.output_length >= minimum_length
                    && i.input_length >= minimum_length
            });
            let mut trace = Vec::new();
            let mut battery = None;
            let mut error = None;
            match vendor {
                None => error = Some(t(no_link_reason(&collections))),
                Some(v) => match self.query(family, v, &mut trace) {
                    Ok(b) => battery = b,
                    Err(e) => {
                        trace.push(format!("error: {e}"));
                        error = Some(tf("d.error", &[&e]));
                    }
                },
            }
            let product = vendor
                .and_then(|v| v.product.as_deref())
                .or_else(|| collections.iter().find_map(|i| i.product.as_deref()));
            self.log_once((vid, pid), &collections, vendor, product, &trace, battery.is_some());
            let name = if family.firmware_id.is_some() {
                // Only a verified connection gets this name. Keep the row offline during timeouts.
                if !vendor.is_some_and(|v| self.answered.contains_key(&v.path)) {
                    continue;
                }
                family.name.unwrap().into()
            } else {
                receiver_name::mouse(product, family.name)
            };

            let reading = DeviceReading {
                online: battery.is_some(),
                level: battery.and_then(|b| b.reported_level(cable && family.firmware_id.is_some())),
                charging: battery.is_some_and(|b| b.charging),
                detail: if battery.is_none() { Some(error.unwrap_or_else(|| t("d.asleep"))) } else { None },
                ..DeviceReading::new(family.id, name, DeviceKind::Mouse, t(if cable { "via.cable" } else { "via.24" }))
            };
            // The T520 Max's receiver can still hold an old reading when its cable is connected.
            if cable && family.firmware_id.is_some() && reading.online {
                result.insert(0, reading);
            } else {
                result.push(reading);
            }
        }
        // A mouse charging on its cable while its receiver stays plugged in is listed twice under one Id, and the
        // receiver finds it off: keep the reading that reached the mouse
        one_per_id(result, |r| r.online)
    }

    fn delay(&self, last: &[DeviceReading]) -> Duration {
        if last.iter().any(|r| r.online) {
            secs(60)
        } else {
            secs(30)
        }
    }
}

impl Battery {
    fn reported_level(self, t520_cable: bool) -> Option<i32> {
        // This firmware can report 100 on USB power even when it reported 51% just before.
        // Its charging flag remains useful but does not distinguish this reply from a full battery.
        (!(t520_cable && self.charging && self.level == 100)).then_some(self.level)
    }
}

impl AjazzMouseProvider {
    /// Asks for the battery; None when nothing answered or the reply has no level.
    fn query(&mut self, family: &Family, vendor: &HidInfo, trace: &mut Vec<String>) -> sys::Result<Option<Battery>> {
        let dev = HidDevice::open(vendor)?;
        if let Some(expected_firmware) = family.firmware_id {
            let Some(reply) = ask(&dev, &model_request(), |r| is_reply(r, MODEL_COMMAND), trace)? else {
                return Ok(None);
            };
            let Some(firmware) = firmware_id(&reply) else { return Ok(None) };
            if firmware != expected_firmware {
                self.answered.remove(&vendor.path);
                return Ok(None);
            }
        }
        let accepts_reply = if family.firmware_id.is_some() { is_t520_battery_reply } else { is_battery_reply };
        let forms = family.forms;
        let answered = self.answered.get(&vendor.path);
        let first = answered.and_then(|form| forms.iter().position(|f| f == form)).unwrap_or(0);
        for i in 0..forms.len() {
            let tried = forms[(first + i) % forms.len()];
            if let Some(reply) = ask(&dev, &request(tried), accepts_reply, trace)? {
                self.answered.insert(vendor.path.clone(), tried);
                return Ok(parse(&reply));
            }
        }
        Ok(None)
    }

    /// Logs the mouse's HID collections and what was sent and received, the first time it is found off and the
    /// first time it is found on in a session, as the other mice do.
    fn log_once(
        &mut self,
        id: (u16, u16),
        collections: &[HidInfo],
        vendor: Option<&HidInfo>,
        product: Option<&str>,
        trace: &[String],
        online: bool,
    ) {
        let key = format!("{:04x}:{:04x}", id.0, id.1);
        if !self.logged.insert(format!("{key}{online}")) {
            return;
        }
        log::write(&format!(
            "AJAZZ / K-Snake {key}: found {}, {} HID collections, product \"{}\"",
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

/// Sends the request and waits for a reply with the expected command, skipping unsolicited reports.
fn ask(
    dev: &HidDevice,
    request: &[u8],
    accepts_reply: impl Fn(&[u8]) -> bool,
    trace: &mut Vec<String>,
) -> sys::Result<Option<Vec<u8>>> {
    dev.flush();
    trace.push(format!("> {}", hex(request)));
    if !dev.write(request, TIMEOUT_MS as u32)? {
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
        if accepts_reply(&r) {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

fn model_request() -> [u8; REPORT_LENGTH] {
    let mut report = [0; REPORT_LENGTH];
    report[..6].copy_from_slice(&[0, REQUEST_HEAD, MODEL_COMMAND, 1, 0, 0x1A]);
    report
}

fn firmware_id(reply: &[u8]) -> Option<&[u8]> {
    if !is_reply(reply, MODEL_COMMAND) {
        return None;
    }
    let length = usize::from(*reply.get(9)?);
    let field = reply.get(10..10 + length)?;
    let id = field.split(|&byte| byte == 0).next()?;
    (!id.is_empty()).then_some(id)
}

/// The battery request, report ID first. Form 0 has the checksum the protocol notes describe, form 1 is what the
/// vendor's web panel sends, form 2 what the MCHOSE G3 V2 is asked with.
pub fn request(form: usize) -> Vec<u8> {
    let mut r = vec![0u8; REPORT_LENGTH];
    r[1] = REQUEST_HEAD;
    r[2] = BATTERY_COMMAND;
    r[3] = READ_MARK;
    r[5] = BATTERY_LENGTH;
    if form == 0 {
        r[4] = r[5..].iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    } else {
        r[4] = 0x0B;
        r[6] = 0x01;
        r[7] = 0x01;
        if form == 2 {
            r[8] = 0x01;
        }
    }
    r
}

/// The answer to the battery request, report ID first, and not one of the mouse's own reports.
pub fn is_battery_reply(r: &[u8]) -> bool {
    r.len() >= 11 && is_reply(r, BATTERY_COMMAND)
}

fn is_t520_battery_reply(r: &[u8]) -> bool {
    // The checksum byte varies by connection and battery level.
    is_battery_reply(r) && r[3] == READ_MARK && r[5..9] == [0x0A, 1, 1, 1] && r[10] <= 1
}

fn is_reply(r: &[u8], command: u8) -> bool {
    r.get(1..3) == Some(&[REPLY_HEAD, command])
}

/// What a battery reply says; None for a level over 100 and for 0 off the cable, which is the receiver answering for
/// a mouse it can't reach.
pub fn parse(r: &[u8]) -> Option<Battery> {
    if !is_battery_reply(r) {
        return None;
    }
    let level = r[9] as i32;
    let charging = r[10] != 0;
    if level > 100 || (level == 0 && !charging) {
        return None;
    }
    Some(Battery { level, charging })
}

#[cfg(test)]
mod tests {
    //! The battery read of the mice on A8A5:2255 / A8A4:2255 (AJAZZ AJ139 V2 MC and others): 64-byte reports with no
    //! report ID, so everything here is one byte further than in the protocol notes, after the ID that Windows adds.
    use super::*;

    /// A report with the report ID 0 and the given bytes after it, zero padded.
    fn report(body: &[u8]) -> Vec<u8> {
        let mut r = vec![0u8; 65];
        r[1..=body.len()].copy_from_slice(body);
        r
    }

    fn battery_reply(level: u8, power: u8) -> Vec<u8> {
        report(&[0xAA, 0x30, 0xA5, 0x00, 0x00, 0x00, 0x00, 0x00, level, power])
    }

    fn b(level: i32, charging: bool) -> Option<Battery> {
        Some(Battery { level, charging })
    }

    #[test]
    fn request_with_the_checksum_of_the_notes() {
        // ajazz-aj139-macos: 55 30 A5 CHK 2E, CHK = the low byte of the sum of bytes 4 to 63
        assert_eq!(request(0), report(&[0x55, 0x30, 0xA5, 0x2E, 0x2E]));
    }

    #[test]
    fn request_of_the_web_panel() {
        // OpenMouse, src/ksnake/index.ts: GET_BATTERY with the tail a5 0b 2e 01 01
        assert_eq!(request(1), report(&[0x55, 0x30, 0xA5, 0x0B, 0x2E, 0x01, 0x01]));
    }

    #[test]
    fn request_of_the_g3_v2() {
        // MCHOSE-Monitor, docs/HID_PROTOCOL.md: 55 30 A5 0B 2E 01 01 01
        assert_eq!(request(2), report(&[0x55, 0x30, 0xA5, 0x0B, 0x2E, 0x01, 0x01, 0x01]));
    }

    #[test]
    fn requests_are_whole_reports() {
        assert_eq!(request(0).len(), 65);
        assert_eq!(request(1).len(), 65);
        assert_eq!(request(2).len(), 65);
    }

    #[test]
    fn ids_name_the_family_and_the_cable() {
        assert!(family(0xA8A5, 0x2255).is_some_and(|(f, cable)| f.id == "ajazz:2255" && !cable));
        assert!(family(0xA8A4, 0x2255).is_some_and(|(f, cable)| f.id == "ajazz:2255" && cable));
        assert!(family(0x3837, 0x4245).is_some_and(|(f, cable)| f.id == "mchose:g3v2" && !cable));
        assert!(family(0x3837, 0x418C).is_some_and(|(f, cable)| f.forms[0] == 2 && cable));
        assert!(family(0x3837, 0x2255).is_none());
    }

    #[test]
    fn t520_max_checks_the_model_on_both_connections_and_reads_charging() {
        let (family, cable) = family(0xA8A5, 0x2266).unwrap();
        assert!(!cable);
        let (usb, cable) = super::family(0xA8A4, 0x2266).unwrap();
        assert!(cable);
        assert_eq!(usb.id, family.id);
        assert_eq!(family.name, Some("AJAZZ T520 Max"));
        assert_eq!(family.forms, &[2]);
        assert_eq!(model_request().as_slice(), report(&[0x55, 0x52, 0x01, 0x00, 0x1A]));

        let mut model = report(&[0xAA, 0x52, 0x01, 0x00, 0x1A, 0, 0, 0, 0x19]);
        model[10..34].copy_from_slice(b"20250830111206-T520-3395");
        assert_eq!(firmware_id(&model), family.firmware_id);
        assert_eq!(firmware_id(&model[..33]), None);
        model[33] = b'1';
        assert_ne!(firmware_id(&model), family.firmware_id);
        model[9] = 0;
        assert_eq!(firmware_id(&model), None);

        let battery = report(&[0xAA, 0x30, 0xA5, 0x0B, 0x0A, 1, 1, 1, 0x27, 0]);
        assert!(is_t520_battery_reply(&battery));
        assert_eq!(parse(&battery), b(39, false));

        let cable_battery = report(&[0xAA, 0x30, 0xA5, 0x41, 0x0A, 1, 1, 1, 0x33, 1]);
        assert!(is_t520_battery_reply(&cable_battery));
        let charging = parse(&cable_battery).unwrap();
        assert!(charging.charging);
        assert_eq!(charging.reported_level(true), Some(51));

        let cable_at_100 = report(&[0xAA, 0x30, 0xA5, 0x72, 0x0A, 1, 1, 1, 0x64, 1]);
        assert!(is_t520_battery_reply(&cable_at_100));
        let ambiguous = parse(&cable_at_100).unwrap();
        assert!(ambiguous.charging);
        assert_eq!(ambiguous.reported_level(true), None);
        assert_eq!(ambiguous.reported_level(false), Some(100));
    }

    #[test]
    fn level_on_battery() {
        assert_eq!(parse(&battery_reply(63, 0)), b(63, false));
    }

    #[test]
    fn on_usb_power_it_is_charging() {
        assert_eq!(parse(&battery_reply(80, 1)), b(80, true));
    }

    #[test]
    fn full_and_empty_on_the_cable() {
        assert_eq!(parse(&battery_reply(100, 1)), b(100, true));
        assert_eq!(parse(&battery_reply(0, 1)), b(0, true));
    }

    #[test]
    fn zero_from_the_receiver_is_a_mouse_it_cannot_reach() {
        assert_eq!(parse(&battery_reply(0, 0)), None);
    }

    #[test]
    fn level_over_100_is_no_reading() {
        assert_eq!(parse(&battery_reply(101, 0)), None);
        assert_eq!(parse(&battery_reply(0xFF, 0)), None);
    }

    #[test]
    fn the_mouse_own_reports_are_not_the_reply() {
        let dpi = report(&[0xAA, 0xFA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40]);
        assert!(!is_battery_reply(&dpi));
        assert!(!is_battery_reply(&report(&[0xAA, 0xED, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40])));
        assert_eq!(parse(&dpi), None);
    }

    #[test]
    fn the_request_coming_back_is_not_the_reply() {
        assert!(!is_battery_reply(&request(0)));
        assert!(!is_battery_reply(&request(1)));
    }

    #[test]
    fn a_short_report_is_not_the_reply() {
        assert!(!is_battery_reply(&report(&[0xAA, 0x30])[..10]));
        assert!(!is_battery_reply(&[]));
    }
}
