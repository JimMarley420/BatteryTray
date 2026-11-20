//! Bluetooth devices whose battery level Windows itself knows (the level in Settings > Bluetooth & devices)
//! (Providers/BluetoothProvider.cs). Windows keeps that property after a device disconnects, so the level only counts
//! while the device is actually connected.

use super::{secs, Provider};
use crate::hid::{self, ffi::Guid, PropertyKey};
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use std::time::Duration;
use windows::Devices::Bluetooth::{
    BluetoothClassOfDevice, BluetoothConnectionStatus, BluetoothDevice, BluetoothLEAppearance,
    BluetoothLEAppearanceCategories, BluetoothLEAppearanceSubcategories, BluetoothLEDevice, BluetoothMajorClass,
    BluetoothMinorClass,
};

/// DEVPKEY_Bluetooth_Battery: one byte, 0-100, on the device node that carries the level (the LE device node, or the
/// Hands-Free AG node of a classic headset).
const BATTERY_KEY: PropertyKey = PropertyKey {
    format_id: Guid {
        data1: 0x104EA319,
        data2: 0x6EE2,
        data3: 0x4701,
        data4: [0xBD, 0x47, 0x8D, 0xDB, 0xF4, 0x25, 0xBB, 0xE5],
    },
    id: 2,
};

pub struct BluetoothProvider;

impl Provider for BluetoothProvider {
    fn name(&self) -> &'static str {
        "Bluetooth"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        // WinRT needs the thread to be in an apartment; a second call on the same thread is harmless
        unsafe {
            let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
        }
        // By address, in the order first seen: (address, low energy, level)
        let mut levels: Vec<(u64, bool, u8)> = Vec::new();
        for (instance_id, value) in hid::byte_property(&["BTHENUM", "BTHLE"], BATTERY_KEY) {
            let address = parse_address(&instance_id);
            if address == 0 {
                continue;
            }
            let low_energy = instance_id.to_ascii_uppercase().starts_with("BTHLE");
            match levels.iter_mut().find(|l| l.0 == address) {
                Some(l) => *l = (address, low_energy, value),
                None => levels.push((address, low_energy, value)),
            }
        }

        let mut result = Vec::new();
        for (address, low_energy, level) in levels {
            let Some((name, kind, connected)) = describe(address, low_energy) else { continue };
            result.push(DeviceReading {
                online: connected,
                level: (connected && level <= 100).then_some(level as i32),
                ..DeviceReading::new(format!("bt:{address:012X}"), name, kind, "Bluetooth")
            });
        }
        result
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        secs(60)
    }
}

/// The device address inside an instance ID such as BTHLE\DEV_A1B2C3D4E5F6\7&1234ABC&0&A1B2C3D4E5F6 or
/// BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&0002\7&12345678&0&112233445566_C00000000. It is the
/// last group of exactly 12 hex digits: a profile node's ID starts with the service GUID, which ends in 12 digits too.
pub fn parse_address(instance_id: &str) -> u64 {
    let mut last = None;
    let bytes = instance_id.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_hexdigit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i - start == 12 {
            last = Some(&instance_id[start..i]);
        }
    }
    last.and_then(|s| u64::from_str_radix(s, 16).ok()).unwrap_or(0)
}

fn describe(address: u64, low_energy: bool) -> Option<(String, DeviceKind, bool)> {
    match describe_device(address, low_energy) {
        Ok(found) => found,
        Err(e) => {
            log::write(&format!("bluetooth {address:012X}: {}", e.message()));
            None
        }
    }
}

fn describe_device(address: u64, low_energy: bool) -> windows::core::Result<Option<(String, DeviceKind, bool)>> {
    if low_energy {
        // A device Windows doesn't know comes back as null, which is an error with no code here
        let le = match BluetoothLEDevice::FromBluetoothAddressAsync(address)?.get() {
            Ok(le) => le,
            Err(e) if e.code().is_ok() => return Ok(None),
            Err(e) => return Err(e),
        };
        let name = le.Name()?.to_string();
        let kind = kind_of_le(&name, le.Appearance().ok());
        let connected = le.ConnectionStatus()? == BluetoothConnectionStatus::Connected;
        let _ = le.Close();
        return Ok(Some((name, kind, connected)));
    }
    let classic = match BluetoothDevice::FromBluetoothAddressAsync(address)?.get() {
        Ok(d) => d,
        Err(e) if e.code().is_ok() => return Ok(None),
        Err(e) => return Err(e),
    };
    let name = classic.Name()?.to_string();
    let kind = kind_of_classic(&name, classic.ClassOfDevice().ok());
    let connected = classic.ConnectionStatus()? == BluetoothConnectionStatus::Connected;
    let _ = classic.Close();
    Ok(Some((name, kind, connected)))
}

fn kind_of_le(name: &str, appearance: Option<BluetoothLEAppearance>) -> DeviceKind {
    let hid_kind = (|| -> windows::core::Result<Option<DeviceKind>> {
        let Some(a) = appearance else { return Ok(None) };
        if a.Category()? != BluetoothLEAppearanceCategories::HumanInterfaceDevice()? {
            return Ok(None);
        }
        let sub = a.SubCategory()?;
        Ok(if sub == BluetoothLEAppearanceSubcategories::Keyboard()? {
            Some(DeviceKind::Keyboard)
        } else if sub == BluetoothLEAppearanceSubcategories::Mouse()? {
            Some(DeviceKind::Mouse)
        } else if sub == BluetoothLEAppearanceSubcategories::Gamepad()?
            || sub == BluetoothLEAppearanceSubcategories::Joystick()?
        {
            Some(DeviceKind::Gamepad)
        } else {
            None
        })
    })();
    hid_kind.ok().flatten().unwrap_or_else(|| kind_by_name(name))
}

fn kind_of_classic(name: &str, cod: Option<BluetoothClassOfDevice>) -> DeviceKind {
    let by_class = (|| -> windows::core::Result<Option<DeviceKind>> {
        let Some(cod) = cod else { return Ok(None) };
        let major = cod.MajorClass()?;
        if major == BluetoothMajorClass::AudioVideo {
            return Ok(Some(DeviceKind::Headset));
        }
        if major == BluetoothMajorClass::Peripheral {
            // Bits 6-7 of the minor class: 01 = keyboard, 10 = pointing device
            match (cod.RawValue()? >> 6) & 0x3 {
                1 => return Ok(Some(DeviceKind::Keyboard)),
                2 => return Ok(Some(DeviceKind::Mouse)),
                _ => {}
            }
            let minor = cod.MinorClass()?;
            if minor == BluetoothMinorClass::PeripheralGamepad || minor == BluetoothMinorClass::PeripheralJoystick {
                return Ok(Some(DeviceKind::Gamepad));
            }
        }
        Ok(None)
    })();
    by_class.ok().flatten().unwrap_or_else(|| kind_by_name(name))
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
    fn address_is_the_last_twelve_digit_group() {
        assert_eq!(parse_address(r"BTHLE\DEV_A1B2C3D4E5F6\7&1234ABC&0&A1B2C3D4E5F6"), 0xA1B2C3D4E5F6);
        assert_eq!(
            parse_address(
                r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&0002\7&12345678&0&112233445566_C00000000"
            ),
            0x112233445566
        );
        assert_eq!(parse_address(r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&0002\7&1"), 0x00805F9B34FB);
        assert_eq!(parse_address("nothing here"), 0);
    }

    #[test]
    fn kind_from_the_name() {
        assert_eq!(kind_by_name("Razer Barracuda X"), DeviceKind::Headset);
        assert_eq!(kind_by_name("Беспроводная мышь"), DeviceKind::Mouse);
        assert_eq!(kind_by_name("Xbox Wireless Controller"), DeviceKind::Gamepad);
        assert_eq!(kind_by_name("Keychron K2"), DeviceKind::Keyboard);
    }
}
