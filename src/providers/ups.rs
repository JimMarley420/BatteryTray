//! A UPS's battery, as Windows itself knows it. A UPS on a USB cable is a HID Power Device, and Windows' HID battery
//! driver (hidbatt) adds it to the battery class next to a laptop's own battery, marked short-term
//! (BATTERY_IS_SHORT_TERM: there for a power cut, not for everyday use). The batteries are asked as Microsoft's
//! "Enumerating Battery Devices" does: SetupAPI for their interfaces, then IOCTL_BATTERY_QUERY_TAG,
//! IOCTL_BATTERY_QUERY_INFORMATION and IOCTL_BATTERY_QUERY_STATUS on each, opened for reading only (the three ask for no
//! more). A battery that isn't short-term is the computer's own, which Windows shows itself, and is left out.

use super::{secs, Provider};
use crate::hid::{self, ffi::Guid};
use crate::loc::t;
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use crate::win::{self, CloseHandle, INVALID_HANDLE};
use std::collections::HashSet;
use std::ffi::c_void;
use std::mem::size_of;
use std::time::Duration;
use windows::core::{Error, Result};
use windows::Win32::Foundation::{GENERIC_READ, HANDLE};
use windows::Win32::System::Power::{
    BatteryDeviceName, BatteryInformation, BatteryUniqueID, BATTERY_CHARGING, BATTERY_DISCHARGING, BATTERY_INFORMATION,
    BATTERY_IS_SHORT_TERM, BATTERY_QUERY_INFORMATION, BATTERY_STATUS, BATTERY_TAG_INVALID, BATTERY_UNKNOWN_CAPACITY,
    BATTERY_WAIT_STATUS, IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_STATUS, IOCTL_BATTERY_QUERY_TAG,
};
use windows::Win32::System::IO::DeviceIoControl;

/// GUID_DEVICE_BATTERY: the device interface every battery of the battery class has.
const BATTERY_INTERFACE: Guid =
    Guid { data1: 0x72631E54, data2: 0x78A4, data3: 0x11D0, data4: [0xBC, 0xF7, 0x00, 0xAA, 0x00, 0xB7, 0xB3, 0x2A] };

#[derive(Default)]
pub struct UpsProvider {
    /// The batteries whose failures are already in the log, so one that fails every poll is noted once.
    noted: HashSet<String>,
}

impl Provider for UpsProvider {
    fn name(&self) -> &'static str {
        "UPS"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let mut result = Vec::new();
        for path in hid::interface_paths(&BATTERY_INTERFACE) {
            match read(&path) {
                Ok(reading) => result.extend(reading),
                Err(e) => {
                    if self.noted.insert(path.clone()) {
                        log::write(&format!("UPS {path}: {}", e.message()));
                    }
                }
            }
        }
        result
    }

    /// Sooner than the peripherals: on a power cut the level falls in minutes.
    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        secs(30)
    }
}

/// One battery's reading; None for one that isn't a UPS's, or holds no battery right now.
fn read(path: &str) -> Result<Option<DeviceReading>> {
    let battery = Battery::open(path)?;
    // No wait: a battery that isn't in its slot now has no tag to give
    let mut tag = BATTERY_TAG_INVALID;
    battery.ask(IOCTL_BATTERY_QUERY_TAG, &0u32, &mut tag)?;
    if tag == BATTERY_TAG_INVALID {
        return Ok(None);
    }
    let mut query =
        BATTERY_QUERY_INFORMATION { BatteryTag: tag, InformationLevel: BatteryInformation, ..Default::default() };
    let mut info = BATTERY_INFORMATION::default();
    battery.ask(IOCTL_BATTERY_QUERY_INFORMATION, &query, &mut info)?;
    if info.Capabilities & BATTERY_IS_SHORT_TERM == 0 {
        return Ok(None);
    }
    let mut status = BATTERY_STATUS::default();
    battery.ask(
        IOCTL_BATTERY_QUERY_STATUS,
        &BATTERY_WAIT_STATUS { BatteryTag: tag, ..Default::default() },
        &mut status,
    )?;
    query.InformationLevel = BatteryDeviceName;
    let name = battery.text(&query);
    query.InformationLevel = BatteryUniqueID;
    let unique = battery.text(&query);
    Ok(Some(DeviceReading {
        online: true,
        level: level(status.Capacity, info.FullChargedCapacity),
        charging: status.PowerState & BATTERY_CHARGING != 0,
        // Never "full on the charger": a UPS stays on the mains, and the notice that it may be unplugged is not for it
        detail: (status.PowerState & BATTERY_DISCHARGING != 0).then(|| t("d.onBattery")),
        ..DeviceReading::new(
            id(&unique, path),
            if name.is_empty() { t("name.ups") } else { name },
            DeviceKind::Other,
            t("via.cable"),
        )
    }))
}

/// A battery's handle, opened for reading and closed when dropped.
struct Battery(isize);

impl Battery {
    fn open(path: &str) -> Result<Self> {
        let handle = win::open_path(path, GENERIC_READ.0, 0);
        if handle == INVALID_HANDLE {
            return Err(Error::from_win32());
        }
        Ok(Self(handle))
    }

    /// Sends `input` with an IOCTL and fills `output` with the answer; the number of bytes written to it.
    fn ask<I, O>(&self, code: u32, input: &I, output: &mut O) -> Result<u32> {
        let mut written = 0;
        unsafe {
            DeviceIoControl(
                HANDLE(self.0 as *mut c_void),
                code,
                Some(input as *const I as *const c_void),
                size_of::<I>() as u32,
                Some(output as *mut O as *mut c_void),
                size_of::<O>() as u32,
                Some(&mut written),
                None,
            )?;
        }
        Ok(written)
    }

    /// A text the battery answers to `query` (its name, its unique ID), empty when it has none. hidbatt hands the text
    /// over without the NUL at its end, so its length is the number of bytes written.
    fn text(&self, query: &BATTERY_QUERY_INFORMATION) -> String {
        let mut text = [0u16; 256];
        match self.ask(IOCTL_BATTERY_QUERY_INFORMATION, query, &mut text) {
            Ok(bytes) => win::from_wide(&text[..(bytes as usize / 2).min(text.len())]).trim().to_string(),
            Err(_) => String::new(),
        }
    }
}

impl Drop for Battery {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// The charge in percent: what the battery holds of what it holds when full (in mWh, or both relative), rounded.
fn level(capacity: u32, full: u32) -> Option<i32> {
    if capacity == BATTERY_UNKNOWN_CAPACITY || full == 0 || full == BATTERY_UNKNOWN_CAPACITY {
        return None;
    }
    let percent = (capacity as u64 * 100 + full as u64 / 2) / full as u64;
    Some(percent.min(100) as i32)
}

/// The battery's unique ID, which hidbatt makes of the UPS's serial number, manufacturer and product, so it stays the
/// same on another port; its interface path when it has none.
fn id(unique: &str, path: &str) -> String {
    if unique.is_empty() {
        format!("ups:{}", path.to_ascii_lowercase())
    } else {
        format!("ups:{unique}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_is_the_share_of_the_full_charge() {
        assert_eq!(level(86_400, 86_400), Some(100));
        assert_eq!(level(43_000, 86_400), Some(50));
        assert_eq!(level(57, 100), Some(57));
        // A fresh battery can hold a little more than its last full charge
        assert_eq!(level(87_000, 86_400), Some(100));
        assert_eq!(level(BATTERY_UNKNOWN_CAPACITY, 86_400), None);
        assert_eq!(level(43_000, 0), None);
        assert_eq!(level(43_000, BATTERY_UNKNOWN_CAPACITY), None);
    }

    #[test]
    fn the_unique_id_else_the_path() {
        let path = r"\\?\HID#VID_051D&PID_0002#7&1A2B&0&0000#{72631E54}";
        assert_eq!(id("3B1234X56789APCBack-UPS", path), "ups:3B1234X56789APCBack-UPS");
        assert_eq!(id("", path), r"ups:\\?\hid#vid_051d&pid_0002#7&1a2b&0&0000#{72631e54}");
    }
}
