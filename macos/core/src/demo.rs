//! `--demo`: the app with the devices of the Windows app's pictures instead of the real ones, for pictures of the Mac
//! app (README, docs/development.md). Its settings and state live in a folder of their own in the temporary folder,
//! made afresh at each start, so the user's are never touched.

use batterytray::loc::{t, tf};
use batterytray::monitor::Pusher;
use batterytray::providers::Provider;
use batterytray::reading::{DeviceKind, DeviceReading};
use batterytray::settings::AppSettings;
use batterytray::state::StateStore;
use chrono::{Local, TimeDelta};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const BARRACUDA: &str = "razer:barracuda-x";
pub const XBOX: &str = "bt:xbox-wireless-controller";
pub const ATTACK_SHARK: &str = "rongyuan:attack-shark-r3";
pub const RAINY: &str = "sonix:rainy-75";
pub const MX_KEYS: &str = "logitech:mx-keys";
pub const AIRPODS: &str = "bt:airpods-pro";

pub fn folder() -> PathBuf {
    std::env::temp_dir().join("BatteryTray-demo")
}

/// The demo's settings and state, written into an empty folder: two pinned devices (one away, one with its own
/// levels), a keyboard without reminders, one device hidden, two remembered from before.
pub fn prepare(folder: &Path) {
    let _ = std::fs::remove_dir_all(folder);
    let mut settings = AppSettings::default();
    settings.device_mut(BARRACUDA).pinned = true;
    settings.device_mut(BARRACUDA).low_percent = Some(30);
    settings.device_mut(BARRACUDA).critical_percent = Some(15);
    settings.device_mut(XBOX).pinned = true;
    settings.device_mut(RAINY).alerts = false;
    settings.device_mut(AIRPODS).hidden = true;
    settings.last_run_version = Some(env!("CARGO_PKG_VERSION").into());
    settings.save_to(&folder.join("settings.json"));

    let state = StateStore::load_from(folder.join("state.json"));
    let now = Local::now();
    let was = |id: &str, name: &str, kind: DeviceKind, via: String, level: i32, ago: TimeDelta| DeviceReading {
        online: true,
        level: Some(level),
        time: now - ago,
        ..DeviceReading::new(id, name, kind, via)
    };
    state.update(&was(
        XBOX,
        "Xbox Wireless Controller",
        DeviceKind::Gamepad,
        "Bluetooth".into(),
        80,
        TimeDelta::days(3),
    ));
    state.update(&was(MX_KEYS, "Logitech MX Keys", DeviceKind::Keyboard, t("via.receiver"), 64, TimeDelta::hours(2)));
    state.save();
}

/// Reports the demo's devices on the air, as providers would.
pub struct DemoProvider;

impl Provider for DemoProvider {
    fn name(&self) -> &'static str {
        "Demo"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let on = |id: &str, name: &str, kind: DeviceKind, via: String, level: Option<i32>| DeviceReading {
            online: true,
            level,
            ..DeviceReading::new(id, name, kind, via)
        };
        vec![
            on(BARRACUDA, "Razer Barracuda X", DeviceKind::Headset, t("via.24"), Some(37)),
            DeviceReading {
                charging: true,
                detail: Some(tf("d.wasBefore", &[&47])),
                ..on(ATTACK_SHARK, "Attack Shark R3", DeviceKind::Mouse, t("via.cable"), None)
            },
            DeviceReading {
                estimate: true,
                detail: Some(t("d.estimate")),
                ..on(RAINY, "Rainy 75", DeviceKind::Keyboard, t("via.24"), Some(15))
            },
            DeviceReading {
                detail: Some([tf("d.budLeft", &[&90]), tf("d.budRight", &[&70])].join(", ")),
                ..on(AIRPODS, "AirPods Pro", DeviceKind::Headset, "Bluetooth".into(), Some(70))
            },
        ]
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        Duration::from_secs(60)
    }

    fn set_pusher(&mut self, _push: Pusher) {}
}
