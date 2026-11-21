//! The levels macOS knows: what `system_profiler SPBluetoothDataType` reports for the connected devices, which is
//! what the Bluetooth menu shows - a mouse's or keyboard's own level, a headset's, the two earbuds and the case of
//! AirPods. A paired device that isn't connected has no level there; once this session has had one from it, it is
//! listed as offline, as Windows lists a device whose level it still holds.

use super::kind_by_name;
use crate::loc::tf;
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The devices this session has had a level from.
static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn poll() -> Vec<DeviceReading> {
    let Some(report) = system_profiler() else { return Vec::new() };
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashSet::new);
    let readings = readings(&report, seen);
    for r in &readings {
        if r.level.is_some() {
            seen.insert(r.id.clone());
        }
    }
    readings
}

/// What system_profiler reports, as JSON; None when it couldn't be run or said nothing in 20 seconds.
fn system_profiler() -> Option<Value> {
    let mut child = Command::new("/usr/sbin/system_profiler")
        .args(["SPBluetoothDataType", "-json", "-detailLevel", "basic"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| log::write(&format!("bluetooth: system_profiler: {e}")))
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().ok()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            log::write("bluetooth: system_profiler didn't answer in 20 s");
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = reader.join().ok()?.ok()?;
    serde_json::from_str(&text).ok()
}

/// The connected devices that report a level, and the devices `seen` before that are no longer connected.
pub fn readings(report: &Value, seen: &HashSet<String>) -> Vec<DeviceReading> {
    let mut result = Vec::new();
    let controllers = report.get("SPBluetoothDataType").and_then(Value::as_array).cloned().unwrap_or_default();
    for controller in &controllers {
        for (list, connected) in [("device_connected", true), ("device_not_connected", false)] {
            let devices = controller.get(list).and_then(Value::as_array).cloned().unwrap_or_default();
            // Each entry is an object with one key: the device's name
            for (name, device) in devices.iter().filter_map(Value::as_object).flat_map(|o| o.iter()) {
                let Some(address) = device.get("device_address").and_then(Value::as_str).and_then(parse_address) else {
                    continue;
                };
                let id = format!("bt:{address:012X}");
                let battery = Battery::of(device);
                if connected && battery.level().is_none() {
                    continue; // nothing to show: a phone, a speaker without a level
                }
                if !connected && !seen.contains(&id) {
                    continue;
                }
                let kind = kind_of(device.get("device_minorType").and_then(Value::as_str), name);
                result.push(DeviceReading {
                    online: connected,
                    level: battery.level().filter(|_| connected),
                    detail: battery.detail().filter(|_| connected),
                    ..DeviceReading::new(id, name.clone(), kind, "Bluetooth")
                });
            }
        }
    }
    result
}

/// "AA:BB:CC:DD:EE:FF" (or with dashes) as a number.
pub fn parse_address(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (digits.len() == 12).then(|| u64::from_str_radix(&digits, 16).ok()).flatten()
}

/// A device's levels: its own, or those of the two earbuds and their case.
#[derive(Default, Debug, PartialEq)]
struct Battery {
    main: Option<i32>,
    left: Option<i32>,
    right: Option<i32>,
    case: Option<i32>,
}

impl Battery {
    fn of(device: &Value) -> Self {
        let percent = |key: &str| {
            let text = device.get(key).and_then(Value::as_str)?;
            let n = text.trim().trim_end_matches('%').trim().parse::<i32>().ok()?;
            (0..=100).contains(&n).then_some(n)
        };
        Self {
            main: percent("device_batteryLevelMain"),
            left: percent("device_batteryLevelLeft"),
            right: percent("device_batteryLevelRight"),
            case: percent("device_batteryLevelCase"),
        }
    }

    /// The device's own level, or that of the earbud that runs out first.
    fn level(&self) -> Option<i32> {
        self.main.or(match (self.left, self.right) {
            (Some(l), Some(r)) => Some(l.min(r)),
            (one, other) => one.or(other),
        })
    }

    /// The earbuds and the case, each with its level.
    fn detail(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(l) = self.left {
            parts.push(tf("d.budLeft", &[&l]));
        }
        if let Some(r) = self.right {
            parts.push(tf("d.budRight", &[&r]));
        }
        if let Some(c) = self.case {
            parts.push(tf("d.case", &[&c]));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

fn kind_of(minor_type: Option<&str>, name: &str) -> DeviceKind {
    match minor_type.unwrap_or("").to_ascii_lowercase().as_str() {
        "headphones" | "headset" | "speaker" | "loudspeaker" | "portable audio" | "hifi audio" | "car audio" => {
            DeviceKind::Headset
        }
        "mouse" | "trackpad" | "pointing device" => DeviceKind::Mouse,
        "keyboard" | "keypad" => DeviceKind::Keyboard,
        "gamepad" | "game controller" | "joystick" => DeviceKind::Gamepad,
        _ => kind_by_name(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Value {
        serde_json::json!({ "SPBluetoothDataType": [{
            "controller_properties": { "controller_address": "BC:D0:74:00:00:01" },
            "device_connected": [
                { "MX Ergo": { "device_address": "D1:2C:26:38:38:90", "device_batteryLevelMain": "85%",
                    "device_minorType": "Mouse" } },
                { "AirPods Pro": { "device_address": "70:AE:D5:00:00:02", "device_batteryLevelLeft": "100%",
                    "device_batteryLevelRight": "90%", "device_batteryLevelCase": "45%",
                    "device_minorType": "Headphones" } },
                { "iPhone": { "device_address": "B4:55:75:00:00:03", "device_rssi": "-50" } }
            ],
            "device_not_connected": [
                { "Keychron K6": { "device_address": "DC:2C:26:00:00:04", "device_minorType": "Keyboard" } },
                { "Xbox Wireless Controller": { "device_address": "AC:8E:BD:00:00:05", "device_minorType": "Gamepad" } }
            ]
        }]})
    }

    #[test]
    fn levels_of_the_connected_devices() {
        let seen = HashSet::from(["bt:DC2C26000004".to_string()]);
        let all = readings(&report(), &seen);
        let shown: Vec<_> = all.iter().map(|r| (r.id.as_str(), r.name.as_str(), r.kind, r.online, r.level)).collect();
        assert_eq!(
            shown,
            [
                ("bt:D12C26383890", "MX Ergo", DeviceKind::Mouse, true, Some(85)),
                ("bt:70AED5000002", "AirPods Pro", DeviceKind::Headset, true, Some(90)),
                // Not connected: only the one this session had a level from
                ("bt:DC2C26000004", "Keychron K6", DeviceKind::Keyboard, false, None),
            ]
        );
        assert_eq!(all[1].detail.as_deref(), Some("left 100%, right 90%, case 45%"));
        assert_eq!(all[0].detail, None);
        assert!(all.iter().all(|r| r.via == "Bluetooth"));
    }

    #[test]
    fn one_earbud_and_odd_values() {
        let one = Battery::of(&serde_json::json!({ "device_batteryLevelRight": "70 %" }));
        assert_eq!((one.level(), one.right), (Some(70), Some(70)));
        let odd =
            Battery::of(&serde_json::json!({ "device_batteryLevelMain": "120%", "device_batteryLevelCase": "x" }));
        assert_eq!(odd, Battery::default());
        assert_eq!(parse_address("AC-8E-BD-00-00-05"), Some(0xAC8EBD000005));
        assert_eq!(parse_address("AC:8E"), None);
        assert!(readings(&serde_json::json!({}), &HashSet::new()).is_empty());
    }

    /// The real thing, on this Mac.
    #[test]
    fn this_macs_devices() {
        for r in poll() {
            println!("{r}");
        }
    }
}
