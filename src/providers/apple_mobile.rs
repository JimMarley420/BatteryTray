//! iPhones and iPads on a cable, as Finder sees them. MobileDevice, the private framework Finder, Xcode and
//! coconutBattery use, reports the devices usbmuxd has; one that already trusts this Mac answers lockdownd's battery
//! domain in a session. Read only over USB: a device on Wi-Fi is reached over the network, which macOS 15 puts behind
//! the Local Network permission, a prompt not worth an iPhone's level. A device that doesn't trust this Mac is left
//! alone: pairing would ask on the phone. The framework is looked up when the app runs, so a macOS without it only
//! goes without these devices.

use super::Provider;
use crate::loc::t;
use crate::log;
use crate::mac::{bool_of, number_of, string_of, value_of, CFRetain, CFTypeRef, Cf};
use crate::monitor::{self, Pusher};
use crate::reading::{DeviceKind, DeviceReading};
use std::collections::HashSet;
use std::ffi::{c_char, c_void, CString};
use std::sync::{Mutex, Once, OnceLock};
use std::time::Duration;

pub struct AppleMobileProvider;

impl Provider for AppleMobileProvider {
    fn name(&self) -> &'static str {
        "iPhone/iPad"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let Some(api) = api() else { return Vec::new() };
        subscribe(api);
        let devices: Vec<Device> = DEVICES.lock().unwrap_or_else(|e| e.into_inner()).clone();
        devices.iter().filter_map(|d| read(api, d.0)).collect()
    }

    fn delay(&self, _last: &[DeviceReading]) -> Duration {
        Duration::from_secs(60)
    }

    fn set_pusher(&mut self, push: Pusher) {
        *PUSH.lock().unwrap_or_else(|e| e.into_inner()) = Some(push);
        if let Some(api) = api() {
            subscribe(api);
        }
    }
}

/// An AMDeviceRef the framework reported on USB, retained while it is attached.
#[derive(Clone, Copy, PartialEq)]
struct Device(CFTypeRef);

// Only passed to the framework's calls, which talk to usbmuxd and lockdownd and take any thread
unsafe impl Send for Device {}

static DEVICES: Mutex<Vec<Device>> = Mutex::new(Vec::new());
static PUSH: Mutex<Option<Pusher>> = Mutex::new(None);
/// The devices whose failed reads are already in the log, so a locked or untrusted phone is noted once.
static NOTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

const ATTACHED: u32 = 1;
const DETACHED: u32 = 2;
const USB: i32 = 1;

#[repr(C)]
struct Note {
    device: CFTypeRef,
    message: u32,
    subscription: *mut c_void,
}

type Callback = extern "C" fn(note: *const Note, context: *mut c_void);

type Subscribe = unsafe extern "C" fn(Callback, u32, u32, *mut c_void, *mut *mut c_void) -> i32;
type DeviceCall = unsafe extern "C" fn(CFTypeRef) -> i32;
type CopyValue = unsafe extern "C" fn(CFTypeRef, CFTypeRef, CFTypeRef) -> CFTypeRef;

struct Api {
    subscribe: Subscribe,
    connect: DeviceCall,
    disconnect: DeviceCall,
    is_paired: DeviceCall,
    validate_pairing: DeviceCall,
    start_session: DeviceCall,
    stop_session: DeviceCall,
    interface_type: DeviceCall,
    copy_value: CopyValue,
}

/// The framework's calls; None when it isn't there.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let paths = [
            "/Library/Apple/System/Library/PrivateFrameworks/MobileDevice.framework/MobileDevice",
            "/System/Library/PrivateFrameworks/MobileDevice.framework/MobileDevice",
        ];
        let handle = paths.iter().find_map(|path| {
            let path = CString::new(*path).ok()?;
            let handle = dlopen(path.as_ptr(), RTLD_NOW);
            (!handle.is_null()).then_some(handle)
        });
        let Some(handle) = handle else {
            log::write("iPhone/iPad: no MobileDevice framework");
            return None;
        };
        let symbol = |name: &str| {
            let name = CString::new(name).ok()?;
            let f = dlsym(handle, name.as_ptr());
            (!f.is_null()).then_some(f)
        };
        macro_rules! load {
            ($name:literal, $kind:ty) => {
                match symbol($name) {
                    Some(f) => std::mem::transmute::<*mut c_void, $kind>(f),
                    None => {
                        log::write(concat!("iPhone/iPad: MobileDevice has no ", $name));
                        return None;
                    }
                }
            };
        }
        Some(Api {
            subscribe: load!("AMDeviceNotificationSubscribe", Subscribe),
            connect: load!("AMDeviceConnect", DeviceCall),
            disconnect: load!("AMDeviceDisconnect", DeviceCall),
            is_paired: load!("AMDeviceIsPaired", DeviceCall),
            validate_pairing: load!("AMDeviceValidatePairing", DeviceCall),
            start_session: load!("AMDeviceStartSession", DeviceCall),
            stop_session: load!("AMDeviceStopSession", DeviceCall),
            interface_type: load!("AMDeviceGetInterfaceType", DeviceCall),
            copy_value: load!("AMDeviceCopyValue", CopyValue),
        })
    })
    .as_ref()
}

/// Asks the framework, once, to report the devices that come and go, on a thread of its own that runs its run loop.
fn subscribe(api: &'static Api) {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new().name("iPhone/iPad".into()).spawn(move || unsafe {
            let mut subscription = std::ptr::null_mut();
            let code = (api.subscribe)(on_note, 0, 0, std::ptr::null_mut(), &mut subscription);
            if code != 0 {
                log::write(&format!("iPhone/iPad: AMDeviceNotificationSubscribe: {code:#x}"));
                return;
            }
            let mode = Cf::string("kCFRunLoopDefaultMode");
            loop {
                // 1 = kCFRunLoopRunFinished: no source on this thread's run loop (the framework may deliver on the
                // main one), so it waits rather than spins
                if CFRunLoopRunInMode(mode.0, 1.0, 0) == 1 {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        });
        if let Err(e) = spawned {
            log::write(&format!("iPhone/iPad: no thread for the notifications: {e}"));
        }
    });
}

/// A device came or went. Only USB ones are kept: a Wi-Fi one would be reached over the network.
extern "C" fn on_note(note: *const Note, _context: *mut c_void) {
    let Some(api) = api() else { return };
    let Some(note) = (unsafe { note.as_ref() }) else { return };
    if note.device.is_null() {
        return;
    }
    let mut devices = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
    match note.message {
        ATTACHED if unsafe { (api.interface_type)(note.device) } == USB => {
            if !devices.contains(&Device(note.device)) {
                devices.push(Device(unsafe { CFRetain(note.device) }));
            }
            drop(devices);
            // Read now rather than within a minute, unless the settings say to wait for the schedule
            if monitor::polls_on_connect() {
                if let Some(push) = PUSH.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    push();
                }
            }
        }
        DETACHED => {
            if let Some(at) = devices.iter().position(|d| *d == Device(note.device)) {
                // Released when the list lets go of it: a poll may still hold a copy of the pointer, so the retain
                // taken on arrival is kept (one reference for the life of the app per device that came)
                devices.remove(at);
            }
        }
        _ => {}
    }
}

/// One device's level, read in a lockdown session; None when it can't be read (not trusted, locked, gone).
fn read(api: &Api, device: CFTypeRef) -> Option<DeviceReading> {
    unsafe {
        if (api.connect)(device) != 0 {
            return None;
        }
        let reading = session(api, device);
        (api.disconnect)(device);
        reading
    }
}

unsafe fn session(api: &Api, device: CFTypeRef) -> Option<DeviceReading> {
    let value = |domain: Option<&str>, key: Option<&str>| {
        let domain = domain.map(Cf::string);
        let key = key.map(Cf::string);
        Cf::new((api.copy_value)(
            device,
            domain.as_ref().map_or(std::ptr::null(), |d| d.0),
            key.as_ref().map_or(std::ptr::null(), |k| k.0),
        ))
    };
    let udid = value(None, Some("UniqueDeviceID")).and_then(|v| string_of(v.0))?;
    // Never paired: pairing would put "Trust this computer?" on the phone, which isn't for this app to do
    if (api.is_paired)(device) == 0 {
        note_once(&udid, "doesn't trust this Mac");
        return None;
    }
    let validated = (api.validate_pairing)(device);
    let started = if validated == 0 { (api.start_session)(device) } else { validated };
    if started != 0 {
        note_once(&udid, &format!("no session ({started:#x}), locked?"));
        return None;
    }
    let battery = value(Some("com.apple.mobile.battery"), None);
    let name = value(None, Some("DeviceName")).and_then(|v| string_of(v.0));
    let class = value(None, Some("DeviceClass")).and_then(|v| string_of(v.0));
    (api.stop_session)(device);
    let battery = battery?;
    let field = |key: &str| value_of(battery.0, key);
    let level = number_of(field("BatteryCurrentCapacity")).filter(|n| (0..=100).contains(n))? as i32;
    let charging = bool_of(field("BatteryIsCharging")).unwrap_or(false);
    let plugged = bool_of(field("ExternalConnected")).unwrap_or(false);
    let full = bool_of(field("FullyCharged")).unwrap_or(false);
    let class = class.unwrap_or_default();
    let kind = if class.eq_ignore_ascii_case("iPad") { DeviceKind::Tablet } else { DeviceKind::Phone };
    let name = name.filter(|n| !n.trim().is_empty()).unwrap_or(if class.is_empty() { "iPhone".into() } else { class });
    Some(DeviceReading {
        online: true,
        level: Some(level),
        charging: charging && !full,
        full_on_charger: plugged && full,
        ..DeviceReading::new(format!("ios:{udid}"), name, kind, t("via.cable"))
    })
}

fn note_once(udid: &str, what: &str) {
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    if noted.get_or_insert_with(HashSet::new).insert(udid.to_string()) {
        log::write(&format!("iPhone/iPad {udid}: {what}"));
    }
}

const RTLD_NOW: i32 = 2;

extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopRunInMode(mode: CFTypeRef, seconds: f64, return_after_source_handled: u8) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real thing, on this Mac: the iPhones and iPads on a cable that trust it.
    #[test]
    fn this_macs_phones() {
        let mut provider = AppleMobileProvider;
        provider.poll();
        // The framework reports the attached devices right after the subscription
        std::thread::sleep(Duration::from_secs(3));
        for r in provider.poll() {
            println!("{r}");
            assert!(r.id.starts_with("ios:") && r.level.is_some());
        }
    }
}
