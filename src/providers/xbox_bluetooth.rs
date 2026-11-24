//! Xbox controllers on Bluetooth Classic, as the firmware they shipped with connects them: the Xbox Wireless
//! Controller of the One S (045E:02E0, 02FD after its 2017 update) and the Elite Series 2 (0B05). With the firmware
//! that moved them to Bluetooth LE, Windows shows their level itself (the Bluetooth provider); on Bluetooth Classic it
//! has none for them, and the XInput provider leaves Bluetooth controllers alone, so their own battery report is read
//! here.
//!
//! The battery is input report 04 of the gamepad collection (01:05), declared as Generic Device Controls / Battery
//! Strength (06:20): `04 FF`, one byte of flags. Bit 7: the controller is on (clear only in the report it sends as
//! it shuts down). Bit 4: charging. Bits 3-2: the power, 0 USB, 1 disposable batteries, 2 the Play & Charge pack.
//! Bits 1-0: the level, 0 critical, 1 low, 2 medium, 3 full; always 0 on USB power.
//! Sources: xpadneo (docs/reports/xb1s_battery_event.md; docs/descriptors/xb1s_windows.md and xbe2_linux.md for the
//! report in the descriptor; hid-xpadneo/src/xpadneo/power.c for how the flags are read) and SDL
//! (src/joystick/hidapi/SDL_hidapi_xboxone.c, HIDAPI_DriverXboxOne_HandleBatteryState). Own implementation, not yet
//! verified on hardware by this project, which is why the first report of each kind goes to the log.
//!
//! Nothing is sent. None of the sources asks for the report or says how often it comes: they take it as an event,
//! when it arrives. So it is listened for on a read-only handle kept open between polls, and the last flags heard
//! stand until the next report brings others - the controller is taken to report every change of its flags. A
//! controller silent for longer than it stays on unused has turned itself off, and what it said before is forgotten.

use super::{contains_ignore_case, hex_trimmed as hex, secs, ClearOnDrop, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::{t, tf};
use crate::log;
use crate::monitor::{no_push, Pusher};
use crate::reading::{DeviceKind, DeviceReading};
use crate::sys;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MICROSOFT_VID: u16 = 0x045E;
const BATTERY_REPORT: u8 = 0x04;
/// An Xbox controller turns itself off after 15 minutes without being used, so one that has been silent for longer
/// is off, whether or not its last report said so.
const QUIET: Duration = secs(16 * 60);

const MODELS: [(u16, &str); 3] = [(0x02E0, "name.xbox"), (0x02FD, "name.xbox"), (0x0B05, "Xbox Elite Series 2")];

fn model(pid: u16) -> Option<&'static str> {
    MODELS.iter().find(|m| m.0 == pid).map(|m| m.1)
}

/// XInput's four battery levels and these flags' (critical or empty, low, medium, full) as a rough percentage.
pub(crate) fn four_level_percent(level: u8) -> i32 {
    match level {
        0 => 5,
        1 => 15,
        2 => 50,
        _ => 100,
    }
}

/// How a controller is powered, from the flags of its battery report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    /// On USB power, which hides the level.
    Usb { charging: bool },
    /// On disposable batteries or the Play & Charge pack, at one of four levels (0 critical .. 3 full).
    Battery { level: u8, charging: bool },
}

pub fn power(flags: u8) -> Power {
    let charging = flags & 0x10 != 0;
    if (flags >> 2) & 0x03 == 0 {
        Power::Usb { charging }
    } else {
        Power::Battery { level: flags & 0x03, charging }
    }
}

/// What a controller's reports say at a moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pad {
    /// Nothing heard lately: off, out of range, or on and not used since the listening began.
    Silent,
    /// Its last battery report said it was shutting down.
    ShutDown,
    /// It is on, with the flags of its last battery report when one has come.
    On(Option<u8>),
}

/// The reports of one controller so far. No I/O and no clock of its own, so it can be fed by hand.
#[derive(Default)]
struct Heard {
    flags: Option<u8>,
    last: Option<Instant>,
    /// The report IDs seen: the first report of each goes to the log.
    ids: Vec<u8>,
}

impl Heard {
    /// Takes one input report; true when what a poll would show has changed. Lines worth logging go to `log`.
    fn feed(&mut self, report: &[u8], now: Instant, log: &mut Vec<String>) -> bool {
        let Some(&id) = report.first() else { return false };
        let before = self.pad(now);
        // A controller back from a silence or a shutdown starts afresh: its old flags are no longer true
        if !matches!(before, Pad::On(_)) {
            self.flags = None;
        }
        if id == BATTERY_REPORT && report.len() >= 2 {
            self.flags = Some(report[1]);
        }
        self.last = Some(now);
        if !self.ids.contains(&id) {
            self.ids.push(id);
            log.push(format!("first report {id:02x}: {}", hex(report)));
        }
        self.pad(now) != before
    }

    fn pad(&self, now: Instant) -> Pad {
        match self.last {
            Some(last) if now.saturating_duration_since(last) < QUIET => match self.flags {
                Some(flags) if flags & 0x80 == 0 => Pad::ShutDown,
                flags => Pad::On(flags),
            },
            _ => Pad::Silent,
        }
    }
}

struct Shared {
    inner: Mutex<(Heard, Option<String>)>, // what was heard, and the error that stopped the listener
    stopped: AtomicBool,
    running: AtomicBool,
}

/// Reads one controller's gamepad collection on its own thread into a `Heard`.
struct Listener {
    shared: Arc<Shared>,
}

impl Listener {
    fn start(info: HidInfo, label: String, changed: Pusher) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new((Heard::default(), None)),
            stopped: AtomicBool::new(false),
            running: AtomicBool::new(true),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("Xbox controller reports".into());
        if thread.spawn(move || Self::listen(info, label, s, changed)).is_err() {
            shared.running.store(false, Ordering::Relaxed);
        }
        Self { shared }
    }

    fn running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    fn pad(&self) -> Pad {
        self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()).0.pad(Instant::now())
    }

    fn error(&self) -> Option<String> {
        self.shared.inner.lock().unwrap_or_else(|e| e.into_inner()).1.clone()
    }

    /// The listening thread owns the handle and closes it itself, so no read is ever left pending on a closed handle.
    fn listen(info: HidInfo, label: String, shared: Arc<Shared>, changed: Pusher) {
        // However it ends, a panic included: a listener that isn't running is started again by the next poll
        let _running = ClearOnDrop(&shared.running);
        let result = (|| -> sys::Result<()> {
            let dev = HidDevice::open_read_only(&info)?;
            let mut lines = Vec::new();
            while !shared.stopped.load(Ordering::Relaxed) {
                let Some(report) = dev.read(1000)? else { continue };
                let news =
                    shared.inner.lock().unwrap_or_else(|e| e.into_inner()).0.feed(&report, Instant::now(), &mut lines);
                for line in lines.drain(..) {
                    log::write(&format!("{label}: {line}"));
                }
                if news {
                    changed();
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            // Windows' own text ("Access is denied.") rather than "Cannot open <a long device path>"
            shared.inner.lock().unwrap_or_else(|e| e.into_inner()).1 =
                Some(e.code.map(sys::system_message).unwrap_or(e.message));
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Relaxed);
    }
}

pub struct XboxBluetoothProvider {
    push: Pusher,
    listeners: HashMap<String, Listener>, // by collection path
    /// The listeners' errors this session's log already has.
    logged: HashSet<String>,
}

impl Default for XboxBluetoothProvider {
    fn default() -> Self {
        Self { push: no_push(), listeners: HashMap::new(), logged: HashSet::new() }
    }
}

impl XboxBluetoothProvider {
    /// What the controller on this collection has said, and the error that stopped its listener if one did. A
    /// listener that has stopped (the collection couldn't be read, or went away and is back) is started again.
    fn state(&mut self, info: &HidInfo, name: &str) -> (Pad, Option<String>) {
        let error = match self.listeners.get(&info.path) {
            Some(listener) if listener.running() => return (listener.pad(), None),
            Some(listener) => listener.error(),
            None => None,
        };
        let label = format!("{name} {:04x}:{:04x}", info.vendor_id, info.product_id);
        if let Some(error) = &error {
            if self.logged.insert(format!("{} {error}", info.path)) {
                log::write(&format!("{label}: cannot read the gamepad collection: {error}"));
            }
        }
        self.listeners.insert(info.path.clone(), Listener::start(info.clone(), label, self.push.clone()));
        (Pad::Silent, error)
    }
}

impl Provider for XboxBluetoothProvider {
    fn name(&self) -> &'static str {
        "Xbox on Bluetooth Classic"
    }

    fn set_pusher(&mut self, push: Pusher) {
        self.push = push;
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let pads: Vec<HidInfo> = hid::enumerate(|v, p| v == MICROSOFT_VID && model(p).is_some())
            .into_iter()
            .filter(|i| contains_ignore_case(&i.path, "{00001124-"))
            .filter(|i| i.access_denied || (i.usage_page == 0x01 && i.usage == 0x05 && i.input_length >= 2))
            .collect();
        // Dropping a listener stops it
        self.listeners.retain(|path, _| pads.iter().any(|i| &i.path == path));

        let mut result: Vec<DeviceReading> = Vec::new();
        for info in &pads {
            let Some(name) = model(info.product_id) else { continue };
            // Several identical controllers get #2, #3... in the order Windows lists them
            let mut id = format!("xbox:{:04x}", info.product_id);
            let mut name = t(name);
            let same = result.iter().filter(|r| r.id.starts_with(&id)).count();
            if same > 0 {
                id = format!("{id}#{}", same + 1);
                name = format!("{name} #{}", same + 1);
            }
            let template = DeviceReading::new(id, name.clone(), DeviceKind::Gamepad, "Bluetooth");
            if info.access_denied {
                result.push(DeviceReading { detail: Some(t("d.busy")), ..template });
                continue;
            }
            let (pad, error) = self.state(info, &name);
            result.push(reading(pad, error, template));
        }
        result
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        // The listeners push every change; polling is for controllers that come and go
        secs(60)
    }
}

fn reading(pad: Pad, error: Option<String>, template: DeviceReading) -> DeviceReading {
    if let Some(error) = error {
        return DeviceReading { detail: Some(tf("d.error", &[&error])), ..template };
    }
    match pad {
        Pad::Silent => template,
        Pad::ShutDown => DeviceReading { detail: Some(t("d.offOrAway")), ..template },
        Pad::On(None) => DeviceReading { online: true, detail: Some(t("d.levelUnknown")), ..template },
        Pad::On(Some(flags)) => match power(flags) {
            Power::Usb { charging } => {
                DeviceReading { online: true, charging, detail: Some(t("d.cableHidden")), ..template }
            }
            Power::Battery { level, charging } => DeviceReading {
                online: true,
                level: Some(four_level_percent(level)),
                estimate: true,
                charging,
                detail: Some(t("d.fourLevels")),
                ..template
            },
        },
    }
}

#[cfg(test)]
mod tests {
    //! No capture of report 04 is published: the flags here are put together from xpadneo's table of its bits.
    use super::*;
    use crate::providers::pad;

    /// Report 04 as Windows returns it: padded with zeros to the collection's longest input report (16 bytes).
    fn battery(flags: u8) -> Vec<u8> {
        pad(16, &[BATTERY_REPORT, flags])
    }

    #[test]
    fn flags_give_the_power_and_the_level() {
        // On, Play & Charge pack, full
        assert_eq!(power(0x8B), Power::Battery { level: 3, charging: false });
        // On, disposable batteries, low
        assert_eq!(power(0x85), Power::Battery { level: 1, charging: false });
        // On, on USB, charging the pack: the level bits are 0 and mean nothing
        assert_eq!(power(0x90), Power::Usb { charging: true });
        assert_eq!(power(0x80), Power::Usb { charging: false });
    }

    #[test]
    fn four_levels_as_xinput_shows_them() {
        assert_eq!([0, 1, 2, 3].map(four_level_percent), [5, 15, 50, 100]);
    }

    #[test]
    fn the_last_battery_report_stands_while_the_controller_reports() {
        let start = Instant::now();
        let mut heard = Heard::default();
        let mut log = Vec::new();
        assert_eq!(heard.pad(start), Pad::Silent);
        // A state report first: on, level not known yet
        assert!(heard.feed(&pad(16, &[0x01, 0x00, 0x80]), start, &mut log));
        assert_eq!(heard.pad(start), Pad::On(None));
        assert!(heard.feed(&battery(0x8A), start, &mut log));
        // State reports later on change nothing
        let later = start + secs(5 * 60);
        assert!(!heard.feed(&pad(16, &[0x01, 0x00, 0x81]), later, &mut log));
        assert_eq!(heard.pad(later), Pad::On(Some(0x8A)));
        assert_eq!(log.len(), 2, "the first report of each kind: {log:?}");
    }

    #[test]
    fn a_shutdown_or_a_long_silence_is_off_and_forgets_the_flags() {
        let start = Instant::now();
        let mut heard = Heard::default();
        let mut log = Vec::new();
        heard.feed(&battery(0x89), start, &mut log);
        assert!(heard.feed(&battery(0x09), start, &mut log));
        assert_eq!(heard.pad(start), Pad::ShutDown);
        // Back on: what it said before the shutdown is gone until its next battery report
        assert!(heard.feed(&pad(16, &[0x01]), start + secs(60), &mut log));
        assert_eq!(heard.pad(start + secs(60)), Pad::On(None));

        heard.feed(&battery(0x8B), start + secs(60), &mut log);
        let silent = start + secs(60) + QUIET;
        assert_eq!(heard.pad(silent), Pad::Silent);
        assert!(heard.feed(&pad(16, &[0x01]), silent, &mut log));
        assert_eq!(heard.pad(silent), Pad::On(None));
    }

    #[test]
    fn readings_from_what_was_heard() {
        let template = DeviceReading::new("xbox:02fd", "Xbox", DeviceKind::Gamepad, "Bluetooth");
        let r = reading(Pad::On(Some(0x86)), None, template.clone());
        assert!(r.online && r.estimate && !r.charging);
        assert_eq!(r.level, Some(50));
        let r = reading(Pad::On(Some(0x90)), None, template.clone());
        assert!(r.online && r.charging);
        assert_eq!(r.level, None, "USB power hides the level");
        let r = reading(Pad::Silent, None, template.clone());
        assert!(!r.online && r.level.is_none());
        let r = reading(Pad::ShutDown, None, template);
        assert!(!r.online && r.level.is_none());
    }
}
