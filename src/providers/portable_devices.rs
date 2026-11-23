//! Android phones and tablets on a cable, through Windows Portable Devices: the COM API behind the phone Explorer
//! shows in "This PC" (the WinRT one, Windows.Devices.Portable, needs an app with package identity). Windows' MTP
//! driver answers WPD_DEVICE_POWER_LEVEL on the device object from MTP's BatteryLevel device property (0x5001), which
//! Android's MTP server keeps current (AOSP, MtpDatabase.java). So a phone is only seen while its USB mode is "File
//! transfer" or "PTP": on "Charging only", where most phones start, it isn't a portable device at all. Phones are told
//! from cameras, players and USB drives by WPD_DEVICE_TYPE, which Windows takes from MTP's Perceived Device Type; most
//! Android builds set it to "mobile handset" (the sys.usb.mtp.device_type property). Nothing is written to the device:
//! it is opened for reading only.

use super::{secs, Provider};
use crate::loc::t;
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use crate::win::wide;
use std::collections::HashSet;
use std::time::Duration;
use windows::core::{w, Result, PCWSTR, PWSTR};
use windows::Win32::Devices::PortableDevices::{
    IPortableDevice, IPortableDeviceKeyCollection, IPortableDeviceManager, IPortableDeviceValues, PortableDeviceFTM,
    PortableDeviceKeyCollection, PortableDeviceManager, PortableDeviceValues, WPD_CLIENT_DESIRED_ACCESS,
    WPD_CLIENT_MAJOR_VERSION, WPD_CLIENT_MINOR_VERSION, WPD_CLIENT_NAME, WPD_CLIENT_REVISION,
    WPD_CLIENT_SECURITY_QUALITY_OF_SERVICE, WPD_DEVICE_FRIENDLY_NAME, WPD_DEVICE_MANUFACTURER, WPD_DEVICE_MODEL,
    WPD_DEVICE_OBJECT_ID, WPD_DEVICE_POWER_LEVEL, WPD_DEVICE_POWER_SOURCE, WPD_DEVICE_SERIAL_NUMBER, WPD_DEVICE_TYPE,
    WPD_DEVICE_TYPE_PHONE, WPD_POWER_SOURCE_EXTERNAL,
};
use windows::Win32::Foundation::{GENERIC_READ, PROPERTYKEY};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

/// SECURITY_IMPERSONATION (winbase.h): lets a driver that needs to impersonate the app do so, as Microsoft's WPD
/// sample asks for, so every driver works.
const SECURITY_IMPERSONATION: u32 = 0x0002_0000;

#[derive(Default)]
pub struct PortableDevicesProvider {
    /// What is already in the log, so a device that refuses every minute is noted once.
    noted: HashSet<String>,
}

impl Provider for PortableDevicesProvider {
    fn name(&self) -> &'static str {
        "Phone/tablet"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        // COM on this provider's own polling thread, in the multithreaded apartment the Bluetooth provider's WinRT
        // uses on its thread; a second call on the same thread is harmless
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let ids = match unsafe { device_ids() } {
            Ok(ids) => ids,
            Err(e) => {
                self.note("list", &format!("no device list: {}", e.message()));
                return Vec::new();
            }
        };
        let mut result = Vec::new();
        for id in ids {
            match unsafe { read(&id) } {
                Ok(reading) => result.extend(reading),
                Err(e) => self.note(&id, &e.message()),
            }
        }
        result
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        secs(60)
    }
}

impl PortableDevicesProvider {
    fn note(&mut self, key: &str, what: &str) {
        if self.noted.insert(key.to_string()) {
            log::write(&format!("phone/tablet {key}: {what}"));
        }
    }
}

/// The PnP IDs of the portable devices attached now.
unsafe fn device_ids() -> Result<Vec<String>> {
    let manager: IPortableDeviceManager = CoCreateInstance(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER)?;
    // The manager may keep the list it found first; a phone just switched to "File transfer" is to be in it
    manager.RefreshDeviceList()?;
    let mut count = 0;
    manager.GetDevices(std::ptr::null_mut(), &mut count)?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut ids = vec![PWSTR::null(); count as usize];
    manager.GetDevices(ids.as_mut_ptr(), &mut count)?;
    Ok(ids.into_iter().map(|id| take(id)).filter(|id| !id.is_empty()).collect())
}

/// One device's reading; None for a device that isn't an Android phone or tablet or has no level to give.
unsafe fn read(pnp_id: &str) -> Result<Option<DeviceReading>> {
    if apple_vendor(pnp_id) {
        return Ok(None);
    }
    let device: IPortableDevice = CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER)?;
    let path = wide(pnp_id);
    device.Open(PCWSTR(path.as_ptr()), &client_information()?)?;
    let values = device_values(&device);
    let _ = device.Close();
    let values = values?;

    let number = |key: &PROPERTYKEY| values.GetUnsignedIntegerValue(key).ok();
    let text = |key: &PROPERTYKEY| values.GetStringValue(key).map(|s| take(s)).unwrap_or_default();
    if number(&WPD_DEVICE_TYPE) != Some(WPD_DEVICE_TYPE_PHONE.0 as u32)
        || text(&WPD_DEVICE_MANUFACTURER).to_lowercase().contains("apple")
    {
        return Ok(None);
    }
    let Some(level) = number(&WPD_DEVICE_POWER_LEVEL).filter(|l| *l <= 100) else { return Ok(None) };
    let external = number(&WPD_DEVICE_POWER_SOURCE) == Some(WPD_POWER_SOURCE_EXTERNAL.0 as u32);
    let (charging, full_on_charger) = charge_state(level, external);
    let name = [text(&WPD_DEVICE_FRIENDLY_NAME), text(&WPD_DEVICE_MODEL)]
        .into_iter()
        .map(|n| n.trim().to_string())
        .find(|n| !n.is_empty())
        .unwrap_or_else(|| t("name.phone"));
    let id = device_id(&text(&WPD_DEVICE_SERIAL_NUMBER), pnp_id);
    Ok(Some(DeviceReading {
        online: true,
        level: Some(level as i32),
        charging,
        full_on_charger,
        ..DeviceReading::new(id, name.clone(), kind_by_name(&name), t("via.cable"))
    }))
}

/// Who opens the device and how: this app, for reading only.
unsafe fn client_information() -> Result<IPortableDeviceValues> {
    let client: IPortableDeviceValues = CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)?;
    let version = |part: &str| part.parse().unwrap_or(0);
    client.SetStringValue(&WPD_CLIENT_NAME, w!("BatteryTray"))?;
    client.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, version(env!("CARGO_PKG_VERSION_MAJOR")))?;
    client.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, version(env!("CARGO_PKG_VERSION_MINOR")))?;
    client.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, version(env!("CARGO_PKG_VERSION_PATCH")))?;
    client.SetUnsignedIntegerValue(&WPD_CLIENT_DESIRED_ACCESS, GENERIC_READ.0)?;
    client.SetUnsignedIntegerValue(&WPD_CLIENT_SECURITY_QUALITY_OF_SERVICE, SECURITY_IMPERSONATION)?;
    Ok(client)
}

/// The device object's properties this provider reads, in one request. A property the device doesn't have comes back
/// as an error in its place, and the request still succeeds.
unsafe fn device_values(device: &IPortableDevice) -> Result<IPortableDeviceValues> {
    let keys: IPortableDeviceKeyCollection =
        CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER)?;
    for key in [
        &WPD_DEVICE_TYPE,
        &WPD_DEVICE_MANUFACTURER,
        &WPD_DEVICE_POWER_LEVEL,
        &WPD_DEVICE_POWER_SOURCE,
        &WPD_DEVICE_FRIENDLY_NAME,
        &WPD_DEVICE_MODEL,
        &WPD_DEVICE_SERIAL_NUMBER,
    ] {
        keys.Add(key)?;
    }
    device.Content()?.Properties()?.GetValues(WPD_DEVICE_OBJECT_ID, &keys)
}

/// A string COM handed over, freed once copied.
unsafe fn take(text: PWSTR) -> String {
    if text.is_null() {
        return String::new();
    }
    let copy = text.to_string().unwrap_or_default();
    CoTaskMemFree(Some(text.0 as *const _));
    copy
}

/// Apple's USB vendor ID in a PnP ID. iPhones and iPads show up as portable devices too, for their photos; they are
/// not this provider's to read, so they aren't even opened. One whose ID doesn't say is known by its manufacturer.
fn apple_vendor(pnp_id: &str) -> bool {
    pnp_id.to_ascii_lowercase().contains("vid_05ac")
}

/// The device's serial number when it has a real one: the PnP ID changes with the USB mode (File transfer alone, or
/// with USB debugging) and, for a device without a serial, with the port. Android answers "????????" when its build
/// keeps the serial from the MTP server, so that and "unknown" don't count.
fn device_id(serial: &str, pnp_id: &str) -> String {
    let serial = serial.trim();
    if serial.chars().any(|c| c.is_ascii_alphanumeric()) && !serial.eq_ignore_ascii_case("unknown") {
        format!("mtp:{serial}")
    } else {
        format!("mtp:{}", pnp_id.to_ascii_lowercase())
    }
}

/// MTP has no tablet type, so a tablet is told by its name: "Galaxy Tab S9", "Pixel Tablet", "Xiaomi Pad 6",
/// "MatePad 11".
fn kind_by_name(name: &str) -> DeviceKind {
    let name = name.to_lowercase();
    let tablet = name.contains("планшет")
        || name.contains("平板")
        || name.split(|c: char| !c.is_alphanumeric()).any(|w| w == "tab" || w == "tablet" || w.ends_with("pad"));
    if tablet {
        DeviceKind::Tablet
    } else {
        DeviceKind::Phone
    }
}

/// (charging, full on the charger) from the level and whether the device runs on external power, which is all WPD
/// says about charging. A device that doesn't say is taken as neither.
fn charge_state(level: u32, external: bool) -> (bool, bool) {
    (external && level < 100, external && level >= 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_serial_is_the_id_else_the_pnp_id() {
        let pnp = r"\\?\USB#VID_18D1&PID_4EE2&MI_00#7&2A1B3C4D&0&0000#{6ac27878-a6fa-4155-ba85-f98f491d4f33}";
        assert_eq!(device_id("1A2B3C4D5E", pnp), "mtp:1A2B3C4D5E");
        assert_eq!(device_id(" R58M1234ABC ", pnp), "mtp:R58M1234ABC");
        let by_path = format!("mtp:{}", pnp.to_ascii_lowercase());
        assert_eq!(device_id("????????", pnp), by_path);
        assert_eq!(device_id("unknown", pnp), by_path);
        assert_eq!(device_id("", pnp), by_path);
    }

    #[test]
    fn iphones_are_known_by_apples_vendor_id() {
        assert!(apple_vendor(r"\\?\usb#vid_05ac&pid_12a8#00008030001234560123#{6ac27878}"));
        assert!(apple_vendor(r"\\?\USB#VID_05AC&PID_12AB#abc#{6ac27878}"));
        assert!(!apple_vendor(r"\\?\usb#vid_18d1&pid_4ee1#1a2b3c#{6ac27878}"));
    }

    #[test]
    fn tablets_by_name() {
        for tablet in ["Galaxy Tab S9", "Pixel Tablet", "Xiaomi Pad 6", "HUAWEI MatePad 11", "Мой планшет"] {
            assert_eq!(kind_by_name(tablet), DeviceKind::Tablet, "{tablet}");
        }
        for phone in ["Pixel 8", "Galaxy S24", "Redmi Note 12", "Nothing Phone (2)", "Tablo phone"] {
            assert_eq!(kind_by_name(phone), DeviceKind::Phone, "{phone}");
        }
    }

    #[test]
    fn charging_only_on_external_power() {
        assert_eq!(charge_state(57, true), (true, false));
        assert_eq!(charge_state(100, true), (false, true));
        assert_eq!(charge_state(57, false), (false, false));
        assert_eq!(charge_state(100, false), (false, false));
    }
}
