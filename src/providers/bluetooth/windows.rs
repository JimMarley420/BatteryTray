//! The levels Windows knows (Settings > Bluetooth & devices): a device-node property. Windows keeps that property
//! after a device disconnects, so the level only counts while the device is actually connected. Of an LE device it is
//! often stale as well - Windows doesn't follow the level's changes - and it is one number for a device with several
//! batteries. So a connected LE device's level is read from the device itself, from its Battery Services (one per half
//! of a split keyboard), and that of AirPods and Beats earbuds is taken from what they advertise (advertisements.rs):
//! each earbud's and the case's. The property stays for what neither gives.
//! Reading the Battery Service as Microsoft's GATT client guide describes, as BlueGauge (iKineticate, src/bluetooth/
//! ble.rs) and zmk-battery-center (kot149, src-tauri/src/ble.rs, which names ZMK's halves by their descriptor 0x2901)
//! do; own implementation.

use super::advertisements;
use super::kind_by_name;
use super::parts;
use crate::hid::{self, ffi::Guid, PropertyKey};
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::{Error, RuntimeType};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattCharacteristicUuids, GattCommunicationStatus, GattDescriptorUuids, GattDeviceService,
    GattServiceUuids,
};
use windows::Devices::Bluetooth::{
    BluetoothCacheMode, BluetoothClassOfDevice, BluetoothConnectionStatus, BluetoothDevice, BluetoothLEAppearance,
    BluetoothLEAppearanceCategories, BluetoothLEAppearanceSubcategories, BluetoothLEDevice, BluetoothMajorClass,
    BluetoothMinorClass,
};
use windows::Storage::Streams::{DataReader, IBuffer};
use windows::Win32::Foundation::{ERROR_TIMEOUT, E_FAIL};
use windows_future::{AsyncStatus, IAsyncOperation};

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

/// How long Windows may take to find a device it knows.
const LOOKUP_TIME: Duration = Duration::from_secs(5);

/// How long reading a device's Battery Services may take, all of them. A connected device answers within a second;
/// one that has just gone away keeps a read waiting for about 7 seconds, which the poll doesn't wait for.
const GATT_TIME: Duration = Duration::from_secs(5);

/// A device as its nodes describe it: the level Windows has, and for AirPods and Beats earbuds, their product ID.
struct Node {
    address: u64,
    low_energy: bool,
    level: Option<u8>,
    earbuds: Option<u16>,
}

pub fn poll() -> Vec<DeviceReading> {
    // WinRT needs the thread to be in an apartment; a second call on the same thread is harmless
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
    // By address, in the order first seen
    let mut nodes: Vec<Node> = Vec::new();
    for (instance_id, value) in hid::device_nodes(&["BTHENUM", "BTHLE"], BATTERY_KEY) {
        let address = parse_address(&instance_id);
        let earbuds = apple_product(&instance_id).filter(|&p| parts::is_earbuds(p));
        if address == 0 || (value.is_none() && earbuds.is_none()) {
            continue;
        }
        let low_energy = instance_id.to_ascii_uppercase().starts_with("BTHLE");
        let index = match nodes.iter().position(|n| n.address == address) {
            Some(i) => i,
            None => {
                nodes.push(Node { address, low_energy, level: None, earbuds: None });
                nodes.len() - 1
            }
        };
        let node = &mut nodes[index];
        if value.is_some() {
            (node.low_energy, node.level) = (low_energy, value);
        }
        node.earbuds = node.earbuds.or(earbuds);
    }

    let mut result = Vec::new();
    let mut listening = false;
    for node in nodes {
        let Some(device) = describe(node.address, node.low_energy) else { continue };
        let mut reading = DeviceReading {
            online: device.connected,
            level: node.level.filter(|&l| device.connected && l <= 100).map(i32::from),
            ..DeviceReading::new(format!("bt:{:012X}", node.address), device.name, device.kind, "Bluetooth")
        };
        if let Some((level, detail)) = parts::of_parts(&device.battery_service) {
            (reading.level, reading.detail) = (Some(level), detail);
        }
        if let Some(product) = node.earbuds.filter(|_| device.connected) {
            listening = true;
            match advertisements::earbuds(product) {
                Some(earbuds) => {
                    note_once(format!("bluetooth {:012X}: earbuds {product:04X} heard", node.address));
                    reading.level = earbuds.level();
                    reading.detail = earbuds.detail();
                    reading.charging = earbuds.charging();
                }
                None => note_once(format!("bluetooth {:012X}: earbuds {product:04X} not heard", node.address)),
            }
        }
        // Earbuds whose nodes have no level are listed for what they advertise only
        if node.level.is_some() || reading.level.is_some() {
            result.push(reading);
        }
    }
    if !listening {
        advertisements::stop();
    }
    result
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

/// The product ID of an Apple or Beats device in the instance ID of one of its profile nodes, which Windows names
/// after the Device ID record the device gave when it paired - vendor 004C, Apple's from the Bluetooth SIG (source
/// 0001): BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0001004C_PID&2014\7&... A device without that record
/// has _LOCALMFG& there instead.
fn apple_product(instance_id: &str) -> Option<u16> {
    const APPLE_VID: &str = "_VID&0001004C_PID&";
    let id = instance_id.to_ascii_uppercase();
    let at = id.find(APPLE_VID)? + APPLE_VID.len();
    let digits = id.get(at..at + 4).filter(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))?;
    u16::from_str_radix(digits, 16).ok()
}

/// What WinRT says of a device: its name, kind, whether it is connected, and for a connected LE device, the levels of
/// its Battery Services, each with its name.
struct Device {
    name: String,
    kind: DeviceKind,
    connected: bool,
    battery_service: Vec<(String, u8)>,
}

fn describe(address: u64, low_energy: bool) -> Option<Device> {
    match describe_device(address, low_energy) {
        Ok(found) => found,
        Err(e) => {
            log::write(&format!("bluetooth {address:012X}: {}", e.message()));
            None
        }
    }
}

fn describe_device(address: u64, low_energy: bool) -> windows::core::Result<Option<Device>> {
    let deadline = Instant::now() + LOOKUP_TIME;
    if low_energy {
        // A device Windows doesn't know comes back as null, which is an error with no code here
        let le = match wait(BluetoothLEDevice::FromBluetoothAddressAsync(address)?, deadline) {
            Ok(le) => le,
            Err(e) if e.code().is_ok() => return Ok(None),
            Err(e) => return Err(e),
        };
        let name = le.Name()?.to_string();
        let kind = kind_of_le(&name, le.Appearance().ok());
        let connected = le.ConnectionStatus()? == BluetoothConnectionStatus::Connected;
        // Only a connected device is read: an uncached read of one that isn't would wait for it to come
        let battery_service = if connected { battery_service(&le, address) } else { Vec::new() };
        let _ = le.Close();
        return Ok(Some(Device { name, kind, connected, battery_service }));
    }
    let classic = match wait(BluetoothDevice::FromBluetoothAddressAsync(address)?, deadline) {
        Ok(d) => d,
        Err(e) if e.code().is_ok() => return Ok(None),
        Err(e) => return Err(e),
    };
    let name = classic.Name()?.to_string();
    let kind = kind_of_classic(&name, classic.ClassOfDevice().ok());
    let connected = classic.ConnectionStatus()? == BluetoothConnectionStatus::Connected;
    let _ = classic.Close();
    Ok(Some(Device { name, kind, connected, battery_service: Vec::new() }))
}

/// The levels of a connected LE device's Battery Services (0x180F, Battery Level 0x2A19), read from the device now,
/// with the names the device gives them; none when it has no such service or doesn't answer. The services and
/// characteristics are Windows' cached ones, which never connect a device. Nothing is written: no descriptor, no
/// subscription to the level's changes.
fn battery_service(le: &BluetoothLEDevice, address: u64) -> Vec<(String, u8)> {
    let read = read_battery_service(le);
    match &read {
        Ok(levels) => note_once(format!("bluetooth {address:012X}: {} battery service level(s)", levels.len())),
        Err(e) => note_once(format!("bluetooth {address:012X}: battery service: {}", e.message())),
    }
    read.unwrap_or_default()
}

fn read_battery_service(le: &BluetoothLEDevice) -> windows::core::Result<Vec<(String, u8)>> {
    let deadline = Instant::now() + GATT_TIME;
    let battery = GattServiceUuids::Battery()?;
    let found = wait(le.GetGattServicesForUuidWithCacheModeAsync(battery, BluetoothCacheMode::Cached)?, deadline)?;
    check(found.Status()?)?;
    let services: Vec<GattDeviceService> = found.Services()?.into_iter().collect();
    let read = read_levels(&services, deadline);
    for service in &services {
        let _ = service.Close();
    }
    read
}

/// Every Battery Level of the services. The lowest of them is the device's, so one that can't be read fails them all.
fn read_levels(services: &[GattDeviceService], deadline: Instant) -> windows::core::Result<Vec<(String, u8)>> {
    let battery_level = GattCharacteristicUuids::BatteryLevel()?;
    let mut levels = Vec::new();
    for service in services {
        let found = wait(
            service.GetCharacteristicsForUuidWithCacheModeAsync(battery_level, BluetoothCacheMode::Cached)?,
            deadline,
        )?;
        check(found.Status()?)?;
        for characteristic in found.Characteristics()? {
            let read = wait(characteristic.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?, deadline)?;
            check(read.Status()?)?;
            if let Some(&level) = bytes(&read.Value()?)?.first() {
                levels.push((characteristic, level));
            }
        }
    }
    // A name only tells several levels apart
    let several = levels.len() > 1;
    let name = |c: &GattCharacteristic| if several { name_of(c, deadline).unwrap_or_default() } else { String::new() };
    Ok(levels.into_iter().map(|(c, level)| (name(&c), level)).collect())
}

/// A characteristic's user description (descriptor 0x2901): the one Windows keeps, else read from the device.
fn name_of(characteristic: &GattCharacteristic, deadline: Instant) -> windows::core::Result<String> {
    let kept = characteristic.UserDescription()?.to_string();
    if !kept.is_empty() {
        return Ok(kept);
    }
    let user_description = GattDescriptorUuids::CharacteristicUserDescription()?;
    let found = wait(
        characteristic.GetDescriptorsForUuidWithCacheModeAsync(user_description, BluetoothCacheMode::Cached)?,
        deadline,
    )?;
    check(found.Status()?)?;
    let Some(descriptor) = found.Descriptors()?.into_iter().next() else { return Ok(String::new()) };
    let read = wait(descriptor.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?, deadline)?;
    check(read.Status()?)?;
    Ok(String::from_utf8_lossy(&bytes(&read.Value()?)?).into_owned())
}

pub(super) fn bytes(buffer: &IBuffer) -> windows::core::Result<Vec<u8>> {
    let mut bytes = vec![0; buffer.Length()? as usize];
    DataReader::FromBuffer(buffer)?.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

/// A GATT call's status, as an error unless it succeeded.
fn check(status: GattCommunicationStatus) -> windows::core::Result<()> {
    let what = match status {
        GattCommunicationStatus::Success => return Ok(()),
        GattCommunicationStatus::Unreachable => "unreachable",
        GattCommunicationStatus::ProtocolError => "protocol error",
        GattCommunicationStatus::AccessDenied => "access denied",
        _ => "failed",
    };
    Err(Error::new(E_FAIL, what))
}

/// The result of an asynchronous WinRT call, waited for until `deadline` and cancelled if it isn't done by then: its
/// own get() would wait as long as the call takes.
fn wait<T: RuntimeType + 'static>(call: IAsyncOperation<T>, deadline: Instant) -> windows::core::Result<T> {
    while call.Status()? == AsyncStatus::Started {
        if Instant::now() >= deadline {
            let _ = call.Cancel();
            return Err(Error::new(ERROR_TIMEOUT.to_hresult(), "no answer in time"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    call.GetResults()
}

/// Writes a line to the log the first time it comes up in this session: what each poll finds would fill it.
pub(super) fn note_once(text: String) {
    static NOTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    if noted.get_or_insert_with(HashSet::new).insert(text.clone()) {
        log::write(&text);
    }
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
    fn apples_product_id_from_the_device_id_record() {
        let airpods =
            r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_VID&0001004C_PID&2014\7&2B2A3C4D&0&A0B1C2D3E4F5_C00000000";
        assert_eq!(apple_product(airpods), Some(0x2014));
        assert_eq!(parse_address(airpods), 0xA0B1C2D3E4F5);
        assert_eq!(
            apple_product(r"bthenum\{0000110b-0000-1000-8000-00805f9b34fb}_vid&0001004c_pid&200e\7&1"),
            Some(0x200E)
        );
        // Apple's USB vendor ID (source 0002), as a Magic Mouse has it; no Device ID record; cut short
        assert_eq!(apple_product(r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&000205AC_PID&030D\7&1"), None);
        assert_eq!(apple_product(r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&0002\7&1"), None);
        assert_eq!(apple_product(r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_VID&0001004C_PID&20"), None);
    }
}
