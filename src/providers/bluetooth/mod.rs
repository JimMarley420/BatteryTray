//! Bluetooth devices whose battery level the system itself knows: on Windows the level in Settings > Bluetooth &
//! devices, on macOS the one the Bluetooth menu shows. Their IDs are "bt:" and the device address on both. On Windows
//! a connected device's own Battery Services and AirPods' advertisements tell more, where the device has them.

#[cfg(windows)]
mod advertisements;
#[cfg(target_os = "macos")]
mod macos;
// Pure, so the Mac runs its tests too
#[cfg(any(windows, test))]
mod parts;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
use windows as imp;

#[cfg(windows)]
pub use windows::parse_address;

use super::{secs, Provider};
use crate::reading::{DeviceKind, DeviceReading};
use std::time::Duration;

pub struct BluetoothProvider;

impl Provider for BluetoothProvider {
    fn name(&self) -> &'static str {
        "Bluetooth"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        imp::poll()
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        secs(60)
    }
}

pub fn kind_by_name(name: &str) -> DeviceKind {
    let n = name.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if any(&["mouse", "мыш"]) {
        DeviceKind::Mouse
    } else if any(&["controller", "gamepad", "xbox", "dualsense", "dualshock", "joy"]) {
        DeviceKind::Gamepad
    } else if any(&["keyboard", "клав", "rainy", "keychron"]) {
        DeviceKind::Keyboard
    } else if any(&["headset", "headphone", "buds", "barracuda", "kraken", "arctis", "airpods", "наушн"]) {
        DeviceKind::Headset
    } else {
        DeviceKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_from_the_name() {
        assert_eq!(kind_by_name("Razer Barracuda X"), DeviceKind::Headset);
        assert_eq!(kind_by_name("Беспроводная мышь"), DeviceKind::Mouse);
        assert_eq!(kind_by_name("Xbox Wireless Controller"), DeviceKind::Gamepad);
        assert_eq!(kind_by_name("Keychron K2"), DeviceKind::Keyboard);
    }
}
