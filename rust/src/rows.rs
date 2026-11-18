//! Devices as the app shows them, and the two decisions made from them: a finished charge to announce
//! (Core/ChargeDoneWatcher.cs) and low battery reminders due (Core/AlertManager.cs).

use crate::reading::{DeviceKind, DeviceReading};
use crate::settings::AppSettings;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A device as the UI shows it: what the provider reports now, plus the last level we ever saw.
#[derive(Clone, Debug)]
pub struct DeviceRow {
    pub id: String,
    pub current: Option<DeviceReading>,
    pub last: Option<DeviceReading>,
    pub name: String,
}

impl DeviceRow {
    pub fn any(&self) -> &DeviceReading {
        self.current.as_ref().or(self.last.as_ref()).expect("a row has a current or a last reading")
    }

    pub fn kind(&self) -> DeviceKind {
        self.any().kind
    }

    pub fn online(&self) -> bool {
        self.current.as_ref().is_some_and(|c| c.online)
    }
}

/// Every device known: what the providers report now, plus devices only remembered from before; by kind, then name.
pub fn rows(
    current: Vec<DeviceReading>,
    remembered: &[(String, Option<DeviceReading>)],
    settings: &AppSettings,
) -> Vec<DeviceRow> {
    let mut rows: Vec<DeviceRow> = Vec::new();
    for r in current {
        match rows.iter_mut().find(|row| row.id == r.id) {
            Some(row) => row.current = Some(r),
            None => rows.push(DeviceRow { id: r.id.clone(), current: Some(r), last: None, name: String::new() }),
        }
    }
    for (id, last) in remembered {
        match rows.iter_mut().find(|row| &row.id == id) {
            Some(row) => row.last = last.clone(),
            None if last.is_some() => {
                rows.push(DeviceRow { id: id.clone(), current: None, last: last.clone(), name: String::new() })
            }
            None => {}
        }
    }
    for row in &mut rows {
        row.name = settings.display_name(&row.id, &row.any().name);
    }
    rows.sort_by_key(|r| (r.kind() as u8, r.name.to_lowercase()));
    rows
}

/// Spots the moment a charge finishes: a device that was charging reports itself full with the charger still in.
/// Only devices that say so count - many report nothing new until the cable comes out. One announcement per charge:
/// chargers top a full battery up now and then, which must not repeat it.
#[derive(Default)]
pub struct ChargeDoneWatcher {
    was_charging: HashMap<String, bool>,
    announced: HashSet<String>,
}

impl ChargeDoneWatcher {
    const REARM_PERCENT: i32 = 90; // below this a new charge may be announced again

    /// The rows whose charge finished since the last call.
    pub fn finished<'a>(&mut self, rows: &'a [DeviceRow]) -> Vec<&'a DeviceRow> {
        let mut done = Vec::new();
        for row in rows {
            let Some(r) = &row.current else { continue };
            if !r.online {
                // Turned off or out of range: whatever it does meanwhile wasn't seen
                self.was_charging.remove(&row.id);
                continue;
            }
            if r.level.is_some_and(|l| l <= Self::REARM_PERCENT) {
                self.announced.remove(&row.id);
            }
            let was = self.was_charging.insert(row.id.clone(), r.charging).unwrap_or(false);
            // Already full when first seen (e.g. the app started overnight) is no news
            if was && r.full_on_charger && self.announced.insert(row.id.clone()) {
                done.push(row);
            }
        }
        done
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertItem {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub level: i32,
    pub critical: bool,
}

#[derive(Default)]
struct AlertState {
    snoozed_until: Option<Instant>,
    muted: bool,
    critical_seen: bool,
}

/// Decides which devices need a low battery reminder right now. Each device alerts at its own level when one is set,
/// and at the global one otherwise. A reminder stays due until the user snoozes it (it comes back after the repeat
/// interval) or mutes it (quiet until the device is charged). Reaching the critical level breaks through a snooze or
/// mute once. Charging, or a level clearly above the threshold, re-arms everything.
#[derive(Default)]
pub struct AlertManager {
    states: HashMap<String, AlertState>,
}

impl AlertManager {
    const REARM_MARGIN: i32 = 5;

    pub fn due(&mut self, rows: &[DeviceRow], settings: &AppSettings, now: Instant) -> Vec<AlertItem> {
        let mut due = Vec::new();
        for row in rows {
            let Some(r) = &row.current else { continue }; // provider lost the device: keep its state as is
            let ds = settings.device(&row.id);
            let low = settings.low_for(&row.id);
            if ds.hidden || !ds.alerts || r.charging || r.level.is_some_and(|l| l > low + Self::REARM_MARGIN) {
                self.states.remove(&row.id);
                continue;
            }
            let Some(level) = r.level.filter(|&l| r.online && l <= low) else { continue };

            let st = self.states.entry(row.id.clone()).or_default();
            let critical = level <= settings.critical_for(&row.id);
            if critical && !st.critical_seen {
                st.critical_seen = true;
                st.muted = false;
                st.snoozed_until = None;
            }
            if st.muted || st.snoozed_until.is_some_and(|until| now < until) {
                continue;
            }
            due.push(AlertItem { id: row.id.clone(), name: row.name.clone(), kind: row.kind(), level, critical });
        }
        due
    }

    pub fn snooze(&mut self, ids: &[String], duration: Duration) {
        for id in ids {
            self.states.entry(id.clone()).or_default().snoozed_until = Some(Instant::now() + duration);
        }
    }

    pub fn mute_until_charged(&mut self, ids: &[String]) {
        for id in ids {
            self.states.entry(id.clone()).or_default().muted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(level: i32, charging: bool, full: bool) -> DeviceRow {
        let r = DeviceReading {
            online: true,
            level: Some(level),
            charging,
            full_on_charger: full,
            ..DeviceReading::new("m", "Mouse", DeviceKind::Mouse, "2.4 GHz")
        };
        DeviceRow { id: "m".into(), current: Some(r), last: None, name: "Mouse".into() }
    }

    #[test]
    fn a_finished_charge_is_announced_once() {
        let mut w = ChargeDoneWatcher::default();
        assert!(w.finished(&[row(100, false, true)]).is_empty(), "already full when first seen");
        assert!(w.finished(&[row(80, true, false)]).is_empty());
        assert_eq!(w.finished(&[row(100, false, true)]).len(), 1);
        assert!(w.finished(&[row(99, true, false)]).is_empty());
        assert!(w.finished(&[row(100, false, true)]).is_empty(), "a top-up is no news");
    }

    #[test]
    fn reminder_snooze_and_critical() {
        let settings = AppSettings::default(); // 20% and 10%
        let mut alerts = AlertManager::default();
        let now = Instant::now();
        assert!(alerts.due(&[row(25, false, false)], &settings, now).is_empty());
        assert_eq!(alerts.due(&[row(18, false, false)], &settings, now).len(), 1);
        alerts.snooze(&["m".into()], Duration::from_secs(600));
        assert!(alerts.due(&[row(17, false, false)], &settings, Instant::now()).is_empty(), "snoozed");
        let critical = alerts.due(&[row(9, false, false)], &settings, Instant::now());
        assert!(critical.len() == 1 && critical[0].critical, "critical breaks through the snooze");
        alerts.mute_until_charged(&["m".into()]);
        assert!(alerts.due(&[row(8, false, false)], &settings, Instant::now()).is_empty(), "muted");
        assert!(alerts.due(&[row(8, true, false)], &settings, Instant::now()).is_empty(), "charging re-arms");
        assert_eq!(alerts.due(&[row(8, false, false)], &settings, Instant::now()).len(), 1);
    }
}
