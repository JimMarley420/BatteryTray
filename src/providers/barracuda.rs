//! Razer Barracuda X (2022) over its 2.4 GHz dongle.
//! Protocol, framing and charge detection ported from github.com/mehdibouchami/razer-barracuda-x-battery
//! (MIT License, Copyright (c) 2026 Mehdi Bouchami).

use super::{remaining_ms, secs, sleep_ms, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::{t, tf};
use crate::log;
use crate::monitor::{no_push, Pusher};
use crate::reading::{DeviceKind, DeviceReading};
use crate::state::StateStore;
use crate::sys;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ID: &str = "razer:barracuda-x";
const DEVICE_NAME: &str = "Razer Barracuda X";
const NOT_CHARGING_CHECKS: i32 = 3; // polls in a row without charging before the level may move again
const CHARGER_RISE_MV: i32 = 15; // a charger lifts the voltage at least this much above the resting one
const HOLD_MV: i32 = 4195; // the charger's last stage holds the battery at exactly 4.20 V
const FULL_REST_MV: i32 = 4145; // a full battery right after the charger stops: 4.155 V (measured)
const SETTLE_QUIET: Duration = secs(50);
const SETTLE_MAX: Duration = secs(5 * 60);

pub struct RazerBarracudaProvider {
    state: Arc<StateStore>,
    push: Pusher,
    charging_sticky: bool,
    not_charging_streak: i32,
    resting_mv: Option<i32>, // median voltage of the last poll on battery; unknown after the headset was away
    events: Option<HeadsetEvents>,
    seeded: bool,

    // The current stay on the cable, as the voltage shows it
    cable_since: Option<Instant>,
    cable_start_mv: i32,
    saw_hold: bool,
    full: bool,
    level_before_cable: Option<i32>,
    last_level: Option<i32>,
    unplugged_at: Option<Instant>,
    settling: bool,
    completed: bool,
    completed_shown: bool,
}

impl RazerBarracudaProvider {
    pub fn new(state: Arc<StateStore>) -> Self {
        Self {
            state,
            push: no_push(),
            charging_sticky: false,
            not_charging_streak: 0,
            resting_mv: None,
            events: None,
            seeded: false,
            cable_since: None,
            cable_start_mv: 0,
            saw_hold: false,
            full: false,
            level_before_cable: None,
            last_level: None,
            unplugged_at: None,
            settling: false,
            completed: false,
            completed_shown: false,
        }
    }

    fn reading(level: Option<i32>, charging: bool, full: bool, detail: Option<String>) -> DeviceReading {
        DeviceReading {
            online: true,
            level,
            charging,
            full_on_charger: full,
            detail,
            ..DeviceReading::new(ID, DEVICE_NAME, DeviceKind::Headset, t("via.24"))
        }
    }

    fn offline(detail: Option<String>) -> DeviceReading {
        DeviceReading { detail, ..DeviceReading::new(ID, DEVICE_NAME, DeviceKind::Headset, t("via.24")) }
    }

    /// On the cable no level is shown: the voltage is the charger's, and the headset's own number is no better - it
    /// just climbs 1% per 20 s check towards the 100% that 4.20 V means. What the voltage does show is the charger's
    /// stage: rising, then held at exactly 4.20 V for the last stretch, then let go once the battery is full -
    /// that is "charged".
    fn on_cable(&mut self, r: &BatteryReading) -> DeviceReading {
        self.settling = false;
        let since = *self.cable_since.get_or_insert_with(|| {
            self.cable_start_mv = r.millivolts;
            self.saw_hold = false;
            self.full = false;
            self.completed = false;
            self.level_before_cable = self.last_level;
            Instant::now()
        });
        let hold = r.millivolts >= HOLD_MV;
        if hold {
            self.saw_hold = true;
            self.full = false; // a top-up of a full battery starts with a new hold
        } else if self.saw_hold && r.millivolts < CHARGE_VOLTAGE_MV {
            self.full = true;
        } else if !self.saw_hold
            && since.elapsed() > secs(10 * 60)
            && r.millivolts >= 4100
            && r.millivolts - self.cable_start_mv < 10
        {
            self.full = true; // plugged in already full: the charger never started, the voltage just sits there
        }

        if self.full {
            self.level_before_cable = Some(100); // a top-up from here starts full
            return Self::reading(Some(100), false, true, Some(t("d.unplugOk")));
        }
        let stage = t(if hold { "d.almostFull" } else { "d.cableHidden" });
        let detail = match self.level_before_cable {
            Some(before) => format!("{stage} · {}", tf("d.wasBefore", &[&before])),
            None => stage,
        };
        Self::reading(None, true, false, Some(detail))
    }

    /// On battery: the headset's own number, which it sends whenever it moves. Right after the cable comes out that
    /// number slides down 1% per 20 s check while the voltage settles (92, 91, 90, 89, then quiet), so it is held
    /// back until it has been still for a while. Until the headset has sent a number the level is unknown: the
    /// voltage gives none worth showing (a curve of our own was off by 20-30%).
    fn on_battery(&mut self, r: &BatteryReading, ev: &Snapshot) -> DeviceReading {
        if self.cable_since.take().is_some() {
            // The headset says "not charging" both when the cable comes out and when its charger stops at full with
            // the cable still in. After the last stage a full battery rests at 4.155 V, one unplugged early well
            // below (4.13 V at ~90%) - that tells "charged" from "unplugged".
            if self.saw_hold && r.millivolts >= FULL_REST_MV {
                self.completed = true;
                self.completed_shown = false;
            } else {
                self.unplugged_at = Some(Instant::now());
            }
        }
        if self.completed {
            // "Charged" until the headset's own number moves off 100: then it's in use on battery
            if !self.completed_shown || ev.percent.unwrap_or(100) >= 100 {
                self.completed_shown = true;
                self.last_level = Some(100);
                return Self::reading(Some(100), false, true, Some(t("d.unplugOk")));
            }
            self.completed = false;
        }
        self.settling = match self.unplugged_at {
            Some(u) => {
                u.elapsed() < SETTLE_MAX
                    && (u.elapsed() < SETTLE_QUIET || ev.percent_time.is_some_and(|p| p.elapsed() < SETTLE_QUIET))
            }
            None => false,
        };
        if self.settling {
            return Self::reading(None, false, false, Some(t("d.settling")));
        }
        self.unplugged_at = None;

        match ev.percent {
            Some(own) => {
                self.last_level = Some(own);
                Self::reading(Some(own), false, false, None)
            }
            None => Self::reading(None, false, false, Some(t("d.levelUnknown"))),
        }
    }

    /// Fallback until the headset has reported the charger itself. Charging starts when the samples swing and the
    /// voltage also rose above the resting one: a load or a glitch can make the samples swing, but only a charger
    /// lifts the voltage. (With no resting voltage yet the swing alone has to do.) Near full the charger holds a
    /// flat 4.20 V, caught by the voltage threshold. Detection can miss a check, and a single miss would let the
    /// charger-inflated voltage update the level, so charging sticks until several checks in a row say otherwise.
    fn is_charging(&mut self, r: &BatteryReading) -> bool {
        let swing = r.charging
            && (self.charging_sticky
                || r.millivolts >= CHARGE_VOLTAGE_MV
                || self.resting_mv.is_none_or(|rest| r.median >= rest + CHARGER_RISE_MV));
        let charging = if swing {
            self.charging_sticky = true;
            self.not_charging_streak = 0;
            true
        } else if self.charging_sticky && {
            self.not_charging_streak += 1;
            self.not_charging_streak < NOT_CHARGING_CHECKS
        } {
            true
        } else {
            self.charging_sticky = false;
            false
        };
        if r.charging && !charging {
            let samples = r.samples.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
            log::write(&format!(
                "Barracuda: swing without a rise ignored, samples {samples}, resting {} mV",
                self.resting_mv.map(|v| v.to_string()).unwrap_or_default()
            ));
        }
        if !charging {
            self.resting_mv = Some(r.median);
        }
        charging
    }
}

impl Provider for RazerBarracudaProvider {
    fn name(&self) -> &'static str {
        DEVICE_NAME
    }

    fn set_pusher(&mut self, push: Pusher) {
        self.push = push;
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let all = hid::enumerate(|v, p| v == RAZER_VID && DONGLE_PIDS.contains(&p));
        if all.is_empty() {
            self.events = None; // dongle unplugged
            return Vec::new();
        }

        let candidates = control_interfaces(&all);
        if candidates.is_empty() {
            let why = if all.iter().any(|i| i.access_denied) { "d.needsAdmin" } else { "d.receiverSilent" };
            return vec![Self::offline(Some(t(why)))];
        }

        let listening = self.events.as_ref().is_some_and(|e| e.running() && e.path() == candidates[0].path);
        if !listening {
            self.events = None;
            let events = HeadsetEvents::start(candidates[0].clone(), self.push.clone());
            // The headset only sends its level when it changes. After a quick restart of the app (an update) the
            // number it sent just before is still right, and better than no level until the next change.
            if !self.seeded {
                if let Some(before) = self.state.get(ID) {
                    if let (false, false, Some(last)) = (before.estimate, before.charging, before.level) {
                        if chrono::Local::now() - before.time < chrono::Duration::minutes(2) {
                            events.seed(last);
                        }
                    }
                }
            }
            self.seeded = true;
            self.events = Some(events);
        }
        let ev = self.events.as_ref().map(|e| e.current()).unwrap_or_default();

        // On battery with the headset's own level known, a couple of samples just confirm it's on; on the cable the
        // voltage shows the charger's stage, so take the full set
        let samples = if ev.percent.is_some() && ev.charger == Some(false) { 2 } else { CHARGE_SAMPLES };
        let r = match read(&candidates, samples) {
            Ok(r) => r,
            Err(why) => {
                self.resting_mv = None;
                return vec![Self::offline(why)];
            }
        };

        let on_cable = match ev.charger {
            Some(plugged) => {
                self.charging_sticky = plugged;
                self.not_charging_streak = 0;
                if !plugged {
                    self.resting_mv = Some(r.median);
                }
                plugged
            }
            None => self.is_charging(&r), // until the headset has said, the voltage tells whether a charger is on
        };
        vec![if on_cable { self.on_cable(&r) } else { self.on_battery(&r, &ev) }]
    }

    fn delay(&self, last: &[DeviceReading]) -> Duration {
        match last.first() {
            None => secs(15),
            Some(r) if r.online => secs(if self.settling { 15 } else { 60 }),
            // Headset off or on Bluetooth: look again soon. No access won't fix itself, so check rarely.
            Some(r) if r.detail.as_deref() == Some(t("d.needsAdmin").as_str()) => secs(5 * 60),
            Some(_) => secs(15),
        }
    }
}

/// What the headset reported by itself so far. `percent_time`: when the level last changed (it only sends changes).
#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub percent: Option<i32>,
    pub percent_time: Option<Instant>,
    pub charger: Option<bool>,
    pub charger_changed: Option<Instant>,
}

const NOTIFICATION_FRAME: u8 = 0x08;
const LINK_EVENT: u8 = 0x20;
const BATTERY_EVENT: u8 = 0x21;
const CHARGER_EVENT: u8 = 0x2A;

impl Snapshot {
    /// Takes one notification [id 02 01 value]; true when it is news.
    pub fn handle(&mut self, id: u8, value: u8) -> bool {
        if id == LINK_EVENT && value <= 1 {
            // The level stays, but the cable may have come out while the headset was off and the charger is only
            // reported along with a changed level: back to the voltage for that
            self.charger = None;
            log::write(&format!("Barracuda: headset {}", if value == 1 { "connected" } else { "disconnected" }));
            true
        } else if id == BATTERY_EVENT && value <= 100 {
            let news = self.percent != Some(value as i32);
            self.percent = Some(value as i32);
            if news {
                self.percent_time = Some(Instant::now());
            }
            news
        } else if id == CHARGER_EVENT && value <= 1 {
            let plugged = value == 1;
            let news = self.charger != Some(plugged);
            self.charger = Some(plugged);
            if news {
                self.charger_changed = Some(Instant::now());
                log::write(&format!(
                    "Barracuda: headset reports the cable {}",
                    if plugged { "plugged in" } else { "unplugged" }
                ));
            }
            news
        } else {
            false
        }
    }

    /// Takes the notifications out of one input report of the dongle; true when any of them is news.
    pub fn process(&mut self, report: &[u8], report_id: u8) -> bool {
        let mut news = false;
        for f in frames(report, report_id) {
            if f.len() >= 14 && f[2] == NOTIFICATION_FRAME && f[11] == 0x02 {
                news |= self.handle(f[10], f[13]);
            }
        }
        news
    }
}

struct EventsShared {
    snapshot: Mutex<Snapshot>,
    stopped: AtomicBool,
    running: AtomicBool,
}

/// What the headset reports by itself through the dongle, found by listening while plugging the cable in and out
/// and switching the headset off and on. Notification frames (RACE frame type 0x08) with the payload
/// [id 02 01 value]: 0x20 headset link (01 connected, 00 lost); 0x21 the headset's own battery percentage, and right
/// after it 0x2A charging (01 charging, 00 not: the cable came out, or the charger stopped at full with the cable
/// in). The headset checks about every 20 s and sends the pair when the level differs from the one it sent last;
/// switched off and on it stays quiet if the level is unchanged, so the last level heard stays right across a
/// power cycle. It is the only level shown, and keeps counting while charging. Passive: it only reads,
/// on its own handle.
struct HeadsetEvents {
    info: HidInfo,
    shared: Arc<EventsShared>,
}

impl HeadsetEvents {
    fn start(info: HidInfo, changed: Pusher) -> Self {
        let shared = Arc::new(EventsShared {
            snapshot: Mutex::new(Snapshot::default()),
            stopped: AtomicBool::new(false),
            running: AtomicBool::new(true),
        });
        let s = shared.clone();
        let i = info.clone();
        let started =
            std::thread::Builder::new().name("Barracuda events".into()).spawn(move || Self::listen(i, s, changed));
        if started.is_err() {
            shared.running.store(false, Ordering::Relaxed);
        }
        Self { info, shared }
    }

    fn path(&self) -> &str {
        &self.info.path
    }

    fn running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    fn current(&self) -> Snapshot {
        *self.shared.snapshot.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A level remembered from just before a restart of the app, used until the headset sends one.
    fn seed(&self, level: i32) {
        let mut s = self.shared.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        s.percent.get_or_insert(level);
    }

    /// The listening thread owns the handle and closes it itself, so no read is ever left pending on a closed
    /// handle; it stops within a second of being asked.
    fn listen(info: HidInfo, shared: Arc<EventsShared>, changed: Pusher) {
        // However it ends, a panic included: a listener that isn't running is started again by the next poll
        let _running = super::ClearOnDrop(&shared.running);
        let result = (|| -> sys::Result<()> {
            let dev = HidDevice::open_read_only(&info)?;
            let report_id = report_id(&info);
            while !shared.stopped.load(Ordering::Relaxed) {
                if let Some(r) = dev.read(1000)? {
                    let news = shared.snapshot.lock().unwrap_or_else(|e| e.into_inner()).process(&r, report_id);
                    if news {
                        changed();
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            if !shared.stopped.load(Ordering::Relaxed) {
                log::write(&format!("Barracuda events: {e}"));
            }
        }
    }
}

impl Drop for HeadsetEvents {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatteryReading {
    /// Lowest sample: a charger lifts every sample, so the minimum is closest to the resting voltage.
    pub millivolts: i32,
    /// The samples swing like on a charger, or the voltage is above any resting voltage.
    pub charging: bool,
    pub median: i32,
    pub samples: Vec<i32>,
}

// The headset is an Airoha chip; the dongle bridges USB HID to it. We send the Airoha "RACE" command GET_BATTERY
// (50 41 06 seq 01 00 31) and the reply carries the battery voltage in millivolts. The dongle must be switched to
// "remote mode" (50 41 0E seq 02 E1 01) first so it forwards the command to the headset; we switch back to local
// mode afterwards (E1 00), as Razer's tool does. Two dongle bridge chips exist, with different HID framing
// (identified by the vendor collection's report ID):
//   0x02 - YS-Tech bridge (PID 0x0550): [02 80 len crcLo crcHi payload...], CRC-16/XMODEM over the whole 64-byte
//          report; the write is acknowledged via GetInputReport ('O' ok / 'F' busy, retried); replies arrive as
//          input reports [02 len payload...].
//   0x01 - Macronix bridge (PID 0x0552): [01 80 len payload...] after an "app mode" handshake; replies are
//          [01 80 len payload...].
// Only read-only status commands are sent; settings and firmware are never written.

pub const RAZER_VID: u16 = 0x1532;
pub const DONGLE_PIDS: [u16; 2] = [0x0550, 0x0552];
const VENDOR_USAGE_PAGE: u16 = 0xFF00;
const TIMEOUT_MS: u64 = 800;
const RACE_GET_BATTERY: u8 = 0x31;
pub const CHARGE_SAMPLES: usize = 7; // spread over ~2 s: consecutive samples can match while charging
const CHARGE_JITTER_MV: i32 = 12; // spread across samples: ~3 mV on battery, 30-45 mV while charging
pub const CHARGE_VOLTAGE_MV: i32 = 4180; // above any resting voltage, so the charger must be connected

static SEQUENCE: AtomicU8 = AtomicU8::new(0);

/// Receives every raw packet sent (">") and received ("<").
pub type TraceHook = fn(&str, &[u8]);

/// Optional hook for the packets, for diagnostics.
pub static TRACE: Mutex<Option<TraceHook>> = Mutex::new(None);

fn trace(what: &str, bytes: &[u8]) {
    if let Some(f) = *TRACE.lock().unwrap_or_else(|e| e.into_inner()) {
        f(what, bytes);
    }
}

/// Vendor interfaces that can carry the command, most likely first (the bridge uses 64-byte reports). Writing to
/// the wrong one fails with ERROR_INVALID_PARAMETER.
pub fn control_interfaces(all: &[HidInfo]) -> Vec<HidInfo> {
    let mut list: Vec<HidInfo> = all
        .iter()
        .filter(|i| !i.access_denied && i.usage_page == VENDOR_USAGE_PAGE && i.output_length > 0 && i.input_length > 0)
        .cloned()
        .collect();
    list.sort_by_key(|i| if i.input_length == 64 && i.output_length == 64 { 0 } else { 1 });
    list
}

/// Tries each candidate interface until one answers. The error is None when the headset is just away (an
/// interface talked without error), otherwise the last I/O error as shown.
pub fn read(candidates: &[HidInfo], samples: usize) -> Result<BatteryReading, Option<String>> {
    let mut error = None;
    let mut answered = false;
    for info in candidates {
        match HidDevice::open(info).and_then(|dev| query(&dev, samples)) {
            Ok(Some(reading)) => return Ok(reading),
            Ok(None) => answered = true,
            Err(e) => {
                trace(&format!("! {e}"), &[]);
                error = Some(e.message);
            }
        }
    }
    match error {
        Some(e) if !answered => Err(Some(tf("d.error", &[&e]))),
        _ => Err(None),
    }
}

/// Report ID of the vendor collection: 0x02 = YS-Tech bridge, otherwise Macronix. Falls back to the product ID
/// when the descriptor couldn't be read.
pub fn report_id(info: &HidInfo) -> u8 {
    if info.output_report_id != 0 {
        info.output_report_id
    } else if info.product_id == 0x0550 {
        0x02
    } else {
        0x01
    }
}

/// None when the dongle is present but the headset doesn't answer (e.g. powered off).
fn query(dev: &HidDevice, sample_count: usize) -> sys::Result<Option<BatteryReading>> {
    dev.flush();
    if report_id(&dev.info) != 0x02 && !enter_macronix_app_mode(dev)? {
        return Ok(None);
    }

    // Remote mode makes the dongle forward commands to the headset; in local mode the dongle answers GET_BATTERY
    // itself with a meaningless value.
    match race(dev, 0x0E, &[0x02, 0xE1, 0x01])? {
        Some(reply) if reply[12] == 0x00 => {}
        _ => return Ok(None),
    }

    // The headset reports voltage only - no charging flag and no percentage. While charging, the voltage is raised
    // by the charger and swings by tens of millivolts between samples; on battery it is steady within a few
    // millivolts. So take a few samples and use their spread to tell the two apart.
    let sampled = (|| -> sys::Result<Vec<i32>> {
        let mut samples = Vec::new();
        for i in 0..sample_count {
            if i > 0 {
                sleep_ms(350);
            }
            // Response: [12] status, [13..14] millivolts (LE)
            let Some(reply) = race(dev, 0x06, &[0x01, 0x00, RACE_GET_BATTERY])? else { continue };
            if reply.len() < 15 || reply[12] != 0x00 {
                continue;
            }
            let sample = reply[13] as i32 | (reply[14] as i32) << 8;
            if (2500..=4500).contains(&sample) {
                samples.push(sample);
            }
        }
        Ok(samples)
    })();
    // Back to local mode whatever happened; its own failure wins, as C#'s finally did
    race(dev, 0x0E, &[0x02, 0xE1, 0x00])?;
    let samples = sampled?;
    if samples.is_empty() {
        return Ok(None);
    }
    Ok(Some(evaluate(samples)))
}

/// On battery the samples sit within 3-6 mV (one or two ADC steps). A single stray sample - seen once while a USB
/// device was being plugged in - must not count as a swing, so with enough samples the lowest and the highest one
/// are left out.
pub fn evaluate(samples: Vec<i32>) -> BatteryReading {
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let trim = if sorted.len() >= 5 { 1 } else { 0 };
    let low = sorted[trim];
    let high = sorted[sorted.len() - 1 - trim];
    BatteryReading {
        millivolts: low,
        charging: high - low >= CHARGE_JITTER_MV || low >= CHARGE_VOLTAGE_MV,
        median: sorted[sorted.len() / 2],
        samples,
    }
}

/// Sends an Airoha RACE command (50 41 group seq body...) and returns the matching response payload
/// (50 49 ... [10] group, [11] 0x80|seq, [12] status ...), or None.
fn race(dev: &HidDevice, group: u8, body: &[u8]) -> sys::Result<Option<Vec<u8>>> {
    let seq = next_sequence();
    let mut packet = vec![0x50, 0x41, group, seq];
    packet.extend_from_slice(body);
    let matches = |p: &[u8]| p.len() >= 13 && p[0] == 0x50 && p[1] == 0x49 && p[10] == group && p[11] == (0x80 | seq);
    if report_id(&dev.info) == 0x02 {
        send_ys_tech(dev, &packet, &matches)
    } else {
        send_macronix(dev, &packet, &matches)
    }
}

fn next_sequence() -> u8 {
    // 1..127, as Razer's tool does
    let mut current = SEQUENCE.load(Ordering::Relaxed);
    loop {
        let next = current % 127 + 1;
        match SEQUENCE.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => current = actual,
        }
    }
}

// ---- YS-Tech bridge (report ID 0x02) ----

fn send_ys_tech(dev: &HidDevice, packet: &[u8], matches: &dyn Fn(&[u8]) -> bool) -> sys::Result<Option<Vec<u8>>> {
    let mut frame = vec![0u8; dev.info.output_length.max(5 + packet.len())];
    frame[0] = 0x02;
    frame[1] = 0x80;
    frame[2] = packet.len() as u8;
    frame[5..5 + packet.len()].copy_from_slice(packet);
    let crc = crc16_xmodem(&frame);
    frame[3] = crc as u8;
    frame[4] = (crc >> 8) as u8;

    let mut accepted = false;
    for attempt in 0..5 {
        if accepted {
            break;
        }
        if attempt > 0 {
            sleep_ms(50);
        }
        trace(">", &frame[..5 + packet.len()]);
        if !dev.write(&frame, TIMEOUT_MS as u32)? {
            return Ok(None);
        }
        let Some(status) = dev.get_input_report(0x02) else { return Ok(None) };
        match status.get(1) {
            Some(b'O') => accepted = true,
            Some(b'F') => {}
            _ => return Ok(None),
        }
    }
    if !accepted {
        return Ok(None);
    }

    read_until(dev, &|r| {
        if r.len() < 2 || r[0] != 0x02 || r[1] as usize > r.len() - 2 {
            return None;
        }
        find_frame(r, 2, r[1] as usize, matches)
    })
}

pub fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

// ---- Macronix bridge (report ID 0x01) ----

fn enter_macronix_app_mode(dev: &HidDevice) -> sys::Result<bool> {
    let id = report_id(&dev.info);
    if !dev.write(&[id, 0x40], TIMEOUT_MS as u32)? {
        return Ok(false);
    }
    let mode = read_until(dev, &|r| (r.len() >= 4 && r[0] == id && r[1] == 0x40).then(|| r.to_vec()))?;
    Ok(mode.is_some_and(|m| m[2] == 0x01 && m[3] == 0x01))
}

fn send_macronix(dev: &HidDevice, packet: &[u8], matches: &dyn Fn(&[u8]) -> bool) -> sys::Result<Option<Vec<u8>>> {
    let id = report_id(&dev.info);
    let mut frame = vec![id, 0x80, packet.len() as u8];
    frame.extend_from_slice(packet);
    trace(">", &frame);
    if !dev.write(&frame, TIMEOUT_MS as u32)? {
        return Ok(None);
    }
    read_until(dev, &|r| {
        if r.len() < 3 || r[0] != id || r[1] != 0x80 || r[2] as usize > r.len() - 3 {
            return None;
        }
        find_frame(r, 3, r[2] as usize, matches)
    })
}

/// One input report can carry several RACE frames back to back: 50 49, frame type (01 answer, 08 notification),
/// 5 more bytes, a 16-bit payload length, then the payload. The headset also sends notifications by itself,
/// sometimes in bursts, so the answer we're waiting for is often not the first frame in the report - walk them all.
fn find_frame(report: &[u8], offset: usize, length: usize, matches: &dyn Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
    split(report, offset, length).into_iter().find(|f| matches(f))
}

/// The RACE frames in one input report of the dongle, whatever they are.
pub fn frames(r: &[u8], report_id: u8) -> Vec<Vec<u8>> {
    if report_id == 0x02 {
        if r.len() >= 2 && r[0] == 0x02 && r[1] as usize <= r.len() - 2 {
            return split(r, 2, r[1] as usize);
        }
        return Vec::new();
    }
    if r.len() >= 3 && r[0] == report_id && r[1] == 0x80 && r[2] as usize <= r.len() - 3 {
        return split(r, 3, r[2] as usize);
    }
    Vec::new()
}

fn split(report: &[u8], offset: usize, length: usize) -> Vec<Vec<u8>> {
    const HEADER_LENGTH: usize = 10; // 50 49 + 6 bytes, of which the last two are the payload length
    let mut frames = Vec::new();
    let end = (offset + length).min(report.len());
    let mut pos = offset;
    while pos + HEADER_LENGTH <= end {
        if report[pos] != 0x50 || report[pos + 1] != 0x49 {
            break;
        }
        let payload = report[pos + 8] as usize | (report[pos + 9] as usize) << 8;
        let frame_end = pos + HEADER_LENGTH + payload;
        if frame_end > end {
            break;
        }
        frames.push(report[pos..frame_end].to_vec());
        pos = frame_end;
    }
    frames
}

/// Reads input reports until `extract` returns something, or the timeout expires. The dongle interleaves
/// unrelated reports, which are skipped.
fn read_until(dev: &HidDevice, extract: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> sys::Result<Option<Vec<u8>>> {
    let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS);
    while Instant::now() < deadline {
        let Some(r) = dev.read(remaining_ms(deadline))? else { return Ok(None) };
        trace("<", &r);
        if let Some(result) = extract(&r) {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: u8, value: u8) -> Vec<u8> {
        // 50 49, type 08, 5 bytes, payload length 4 (LE), payload [id 02 01 value]
        vec![0x50, 0x49, 0x08, 0, 0, 0, 0, 0, 0x04, 0x00, id, 0x02, 0x01, value]
    }

    #[test]
    fn ys_tech_report_with_two_notifications() {
        let mut report = vec![0x02, 28];
        report.extend(notification(BATTERY_EVENT, 92));
        report.extend(notification(CHARGER_EVENT, 1));
        report.resize(64, 0);
        assert_eq!(frames(&report, 0x02).len(), 2);
        let mut s = Snapshot::default();
        assert!(s.process(&report, 0x02));
        assert_eq!((s.percent, s.charger), (Some(92), Some(true)));
        assert!(!s.process(&report, 0x02), "the same values again are no news");
    }

    #[test]
    fn macronix_framing() {
        let mut report = vec![0x01, 0x80, 14];
        report.extend(notification(BATTERY_EVENT, 40));
        assert_eq!(frames(&report, 0x01).len(), 1);
        assert!(frames(&report, 0x02).is_empty());
    }

    #[test]
    fn frame_longer_than_the_report_is_cut_off() {
        let mut report = vec![0x02, 14];
        report.extend(&notification(BATTERY_EVENT, 40)[..12]);
        assert!(frames(&report, 0x02).is_empty());
    }

    #[test]
    fn link_event_forgets_the_charger() {
        let mut s = Snapshot { charger: Some(true), percent: Some(80), ..Default::default() };
        assert!(s.handle(LINK_EVENT, 1));
        assert_eq!((s.percent, s.charger), (Some(80), None));
        assert!(!s.handle(0x55, 1));
    }

    #[test]
    fn swing_or_high_voltage_means_charging() {
        assert!(!evaluate(vec![4010, 4012, 4011, 4013, 4040]).charging, "one stray sample is trimmed");
        assert!(evaluate(vec![4010, 4030, 4045, 4020, 4050]).charging);
        assert!(evaluate(vec![4190, 4191]).charging);
        assert_eq!(evaluate(vec![4012, 4010, 4011]).millivolts, 4010);
    }

    #[test]
    fn crc_xmodem() {
        assert_eq!(crc16_xmodem(b"123456789"), 0x31C3);
    }

    #[test]
    fn sequence_runs_from_1_to_127() {
        SEQUENCE.store(126, Ordering::Relaxed);
        assert_eq!(next_sequence(), 127);
        assert_eq!(next_sequence(), 1);
    }
}
