//! HID on macOS, over IOKit declared by hand. IOKit hands a USB interface over as one device with one report
//! descriptor; Windows makes a collection of each of its top-level collections, and the providers are written for
//! those. So each device is split here into the collections Windows would list, named by paths of the form Windows
//! gives them - `\\?\hid#vid_046d&pid_c539&mi_02&col01#m&1000052a1&0&0000#{4d1e55b2-..}`, the "m&..." part being the
//! device's IORegistry entry ID - and each collection reads only its own input reports.
//!
//! Input reports arrive by callback on a run loop: every open device has a thread running one, and the reports go
//! into a queue of 32, as the Windows HID driver keeps them, which `read` waits on and `flush` empties. Writes are
//! asynchronous with a timeout, so a device that never takes an output report costs its timeout and no more.
//!
//! A device with a keyboard collection can only be opened with the user's "Input Monitoring" permission. Without it
//! its collections are listed as access denied, and nothing tries to open them: that would only bring up the
//! system's prompt at a moment the user didn't choose.

use super::descriptor::{self, Collection, Kind};
use super::{HidInfo, UsageValue};
use crate::mac::{self, CFTypeRef, Cf};
use crate::sys::{Error, Result};
use std::collections::VecDeque;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const INTERFACE_GUID: &str = "{4d1e55b2-f16f-11cf-88cb-001111000030}";
const BLUETOOTH_CLASSIC: &str = "{00001124-0000-1000-8000-00805f9b34fb}";
const BLUETOOTH_LE: &str = "{00001812-0000-1000-8000-00805f9b34fb}";
/// As many input reports as the Windows HID driver keeps for a reader by default.
const QUEUE: usize = 32;

const IO_RETURN_SUCCESS: i32 = 0;
const IO_RETURN_TIMEOUT: i32 = 0xE000_02D6_u32 as i32;
const IO_RETURN_ABORTED: i32 = 0xE000_02EB_u32 as i32;
const USB_TRANSACTION_TIMEOUT: i32 = 0xE000_4051_u32 as i32;
const IO_RETURN_UNSUPPORTED: i32 = 0xE000_02C7_u32 as i32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transport {
    Usb,
    BluetoothClassic,
    BluetoothLe,
}

/// What a device's IORegistry entry says about it.
struct Device {
    entry_id: u64,
    vendor_id: u16,
    product_id: u16,
    vendor_source: u16,
    transport: Transport,
    /// The USB interface number, when the USB device has more than one interface (only then Windows names it)
    interface: Option<u8>,
    product: Option<String>,
    descriptor: descriptor::Descriptor,
}

impl Device {
    fn read(service: u32) -> Option<Self> {
        let number = |key: &str| property(service, key).and_then(|v| mac::number_of(v.0));
        let vendor_id = number("VendorID")? as u16;
        let product_id = number("ProductID").unwrap_or(0) as u16;
        let transport_name = property(service, "Transport").and_then(|v| mac::string_of(v.0)).unwrap_or_default();
        let transport = if transport_name.contains("Bluetooth") {
            if transport_name.contains("Low Energy") || transport_name.contains("LE") {
                Transport::BluetoothLe
            } else {
                Transport::BluetoothClassic
            }
        } else {
            Transport::Usb
        };
        let mut entry_id = 0u64;
        unsafe { IORegistryEntryGetRegistryEntryID(service, &mut entry_id) };
        let raw = property(service, "ReportDescriptor").and_then(|v| mac::data_of(v.0)).unwrap_or_default();
        Some(Self {
            entry_id,
            vendor_id,
            product_id,
            vendor_source: number("VendorIDSource").filter(|&s| s == 1 || s == 2).unwrap_or(2) as u16,
            transport,
            interface: if transport == Transport::Usb { usb_interface(service) } else { None },
            product: property(service, "Product").and_then(|v| mac::string_of(v.0)).filter(|p| !p.is_empty()),
            descriptor: descriptor::parse(&raw),
        })
    }

    /// The path Windows would give collection `index`: the "&mi_" of a composite USB device, the "&col" of an
    /// interface with more than one collection, and the Bluetooth forms of the device part.
    fn path(&self, index: usize) -> String {
        let col = if self.descriptor.collections.len() > 1 { format!("&col{:02x}", index + 1) } else { String::new() };
        let (vid, pid) = (self.vendor_id, self.product_id);
        let device = match self.transport {
            Transport::Usb => {
                let mi = self.interface.map(|i| format!("&mi_{i:02x}")).unwrap_or_default();
                format!("vid_{vid:04x}&pid_{pid:04x}{mi}{col}")
            }
            Transport::BluetoothClassic => {
                format!("{BLUETOOTH_CLASSIC}_vid&{:04x}{vid:04x}_pid&{pid:04x}{col}", self.vendor_source)
            }
            Transport::BluetoothLe => {
                format!("{BLUETOOTH_LE}_dev_vid&{:02x}{vid:04x}_pid&{pid:04x}_rev&0000{col}", self.vendor_source)
            }
        };
        format!(r"\\?\hid#{device}#m&{:x}&0&{index:04x}#{INTERFACE_GUID}", self.entry_id)
    }

    /// A keyboard or keypad collection: opening the device needs "Input Monitoring".
    fn has_keyboard(&self) -> bool {
        self.descriptor.collections.iter().any(|c| c.usage_page == 0x01 && matches!(c.usage, 0x06 | 0x07))
    }

    fn info(&self, index: usize, denied: bool) -> HidInfo {
        let c = &self.descriptor.collections[index];
        let mut info = HidInfo {
            path: self.path(index),
            vendor_id: self.vendor_id,
            product_id: self.product_id,
            access_denied: denied,
            ..Default::default()
        };
        // As on Windows, a collection that can't be opened shows neither its name, its usage nor its lengths
        if !denied {
            info.product = self.product.clone();
            info.usage_page = c.usage_page;
            info.usage = c.usage;
            info.input_length = c.input_length;
            info.output_length = c.output_length;
            info.feature_length = c.feature_length;
            info.output_report_id = c.output_report_id;
        }
        info
    }
}

/// Every HID collection of the devices whose IDs `matches` accepts.
pub fn enumerate(matches: impl Fn(u16, u16) -> bool) -> Vec<HidInfo> {
    let mut result = Vec::new();
    let allowed = input_monitoring_allowed();
    for service in services() {
        let quick = |key: &str| property(service.0, key).and_then(|v| mac::number_of(v.0));
        let (Some(vid), pid) = (quick("VendorID"), quick("ProductID").unwrap_or(0)) else { continue };
        if !matches(vid as u16, pid as u16) {
            continue;
        }
        let Some(device) = Device::read(service.0) else { continue };
        let denied = device.has_keyboard() && !allowed;
        for index in 0..device.descriptor.collections.len() {
            result.push(device.info(index, denied));
        }
    }
    result
}

/// Whether the user has given this app "Input Monitoring" (System Settings → Privacy & Security), which opening a
/// device with a keyboard collection needs.
pub fn input_monitoring_allowed() -> bool {
    const LISTEN_EVENT: u32 = 1;
    const GRANTED: u32 = 0;
    unsafe { IOHIDCheckAccess(LISTEN_EVENT) == GRANTED }
}

/// Asks for "Input Monitoring": the system's prompt the first time, and from then on its answer (the user changes it
/// in System Settings). True when it is given.
pub fn request_input_monitoring() -> bool {
    const LISTEN_EVENT: u32 = 1;
    unsafe { IOHIDRequestAccess(LISTEN_EVENT) }
}

/// An IORegistry object, released when dropped.
struct Object(u32);

impl Drop for Object {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { IOObjectRelease(self.0) };
        }
    }
}

fn services() -> Vec<Object> {
    let mut list = Vec::new();
    unsafe {
        let mut iterator = 0u32;
        // The matching dictionary is consumed by the call
        if IOServiceGetMatchingServices(0, IOServiceMatching(c"IOHIDDevice".as_ptr()), &mut iterator) != 0 {
            return list;
        }
        let iterator = Object(iterator);
        loop {
            let service = IOIteratorNext(iterator.0);
            if service == 0 {
                break;
            }
            list.push(Object(service));
        }
    }
    list
}

fn property(entry: u32, key: &str) -> Option<Cf> {
    let key = Cf::string(key);
    Cf::new(unsafe { IORegistryEntryCreateCFProperty(entry, key.0, std::ptr::null(), 0) })
}

/// The interface number of a USB HID device whose USB device has more than one interface.
fn usb_interface(service: u32) -> Option<u8> {
    const RECURSIVELY: u32 = 1;
    const PARENTS: u32 = 2;
    let key = Cf::string("bInterfaceNumber");
    let number = Cf::new(unsafe {
        IORegistryEntrySearchCFProperty(service, c"IOService".as_ptr(), key.0, std::ptr::null(), RECURSIVELY | PARENTS)
    })
    .and_then(|v| mac::number_of(v.0))?;
    (usb_interface_count(service) > 1).then_some(number as u8)
}

/// How many interfaces the USB device above this entry has.
fn usb_interface_count(service: u32) -> usize {
    unsafe {
        let mut entry = Object(0);
        let mut current = service;
        // Up the service plane to the USB device
        for _ in 0..8 {
            let mut parent = 0u32;
            if IORegistryEntryGetParentEntry(current, c"IOService".as_ptr(), &mut parent) != 0 {
                return 0;
            }
            entry = Object(parent);
            current = parent;
            if IOObjectConformsTo(parent, c"IOUSBHostDevice".as_ptr()) != 0
                || IOObjectConformsTo(parent, c"IOUSBDevice".as_ptr()) != 0
            {
                break;
            }
        }
        let mut children = 0u32;
        if IORegistryEntryGetChildIterator(entry.0, c"IOService".as_ptr(), &mut children) != 0 {
            return 0;
        }
        let children = Object(children);
        let mut count = 0;
        loop {
            let child = IOIteratorNext(children.0);
            if child == 0 {
                break;
            }
            let child = Object(child);
            if IOObjectConformsTo(child.0, c"IOUSBHostInterface".as_ptr()) != 0
                || IOObjectConformsTo(child.0, c"IOUSBInterface".as_ptr()) != 0
            {
                count += 1;
            }
        }
        count
    }
}

/// The IORegistry entry ID and the collection index in a path this module wrote.
fn parse_path(path: &str) -> Option<(u64, usize)> {
    let part = path.split('#').nth(2)?;
    let mut fields = part.split('&');
    if fields.next()? != "m" {
        return None;
    }
    let entry_id = u64::from_str_radix(fields.next()?, 16).ok()?;
    let index = usize::from_str_radix(fields.nth(1)?, 16).ok()?;
    Some((entry_id, index))
}

/// The open IOHIDDevice of a path, with its descriptor.
struct Handle {
    device: CFTypeRef,
    uses_report_ids: bool,
    collection: Collection,
}

unsafe impl Send for Handle {}

impl Handle {
    fn open(info: &HidInfo) -> Result<Self> {
        let cannot =
            |code: u32| Error::with_message(code, format!("Cannot open {}: {}", info.path, mac::system_message(code)));
        let (entry_id, index) =
            parse_path(&info.path).ok_or_else(|| Error::other(format!("Cannot open {}", info.path)))?;
        if info.access_denied {
            return Err(cannot(0xE000_02E2)); // kIOReturnNotPermitted, what opening it would answer
        }
        unsafe {
            let service = Object(IOServiceGetMatchingService(0, IORegistryEntryIDMatching(entry_id)));
            if service.0 == 0 {
                return Err(cannot(0xE000_02C0)); // kIOReturnNoDevice: it went away
            }
            let device = IOHIDDeviceCreate(std::ptr::null(), service.0);
            if device.is_null() {
                return Err(cannot(0xE000_02C0));
            }
            let raw =
                mac::data_of(IOHIDDeviceGetProperty(device, Cf::string("ReportDescriptor").0)).unwrap_or_default();
            let parsed = descriptor::parse(&raw);
            let Some(collection) = parsed.collections.get(index).cloned() else {
                mac::CFRelease(device);
                return Err(cannot(0xE000_02C0));
            };
            let status = IOHIDDeviceOpen(device, 0);
            if status != IO_RETURN_SUCCESS {
                mac::CFRelease(device);
                return Err(cannot(status as u32));
            }
            Ok(Self { device, uses_report_ids: parsed.uses_report_ids, collection })
        }
    }

    /// Sends a report (its first byte the report ID, 0 when the device has none) synchronously.
    fn set(&self, kind: u32, report: &[u8]) -> bool {
        let (id, data) = split_id(report);
        unsafe { IOHIDDeviceSetReport(self.device, kind, id as isize, data.as_ptr(), data.len() as isize) == 0 }
    }

    /// Reads a report into a buffer of `length` bytes, its first byte the report ID as on Windows.
    fn get(&self, kind: u32, report_id: u8, length: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; length];
        buf[0] = report_id;
        // Without report IDs the device's data starts at the first byte, which on Windows is the zero ID
        let (skip, mut size) = if report_id == 0 { (1, length as isize - 1) } else { (0, length as isize) };
        let status = unsafe {
            IOHIDDeviceGetReport(self.device, kind, report_id as isize, buf.as_mut_ptr().add(skip), &mut size)
        };
        (status == IO_RETURN_SUCCESS).then_some(buf)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            IOHIDDeviceClose(self.device, 0);
            mac::CFRelease(self.device);
        }
    }
}

/// The report ID and the bytes IOKit is given: without report IDs, the report without its leading zero.
fn split_id(report: &[u8]) -> (u8, &[u8]) {
    match report.first() {
        Some(0) | None => (0, report.get(1..).unwrap_or(&[])),
        Some(&id) => (id, report),
    }
}

/// What the run loop's callback shares with the readers.
struct Inbox {
    reports: Mutex<VecDeque<Vec<u8>>>,
    arrived: Condvar,
    /// The input report IDs of the collection (None: the descriptor has no report IDs, every report is its own)
    ids: Option<Vec<u8>>,
    length: usize,
}

impl Inbox {
    /// One input report from IOKit: kept when it belongs to the collection, as Windows would hand it over - the ID
    /// first (0 without report IDs), padded to the collection's input length.
    fn take(&self, report: &[u8]) {
        let mut buf = match &self.ids {
            Some(ids) => {
                if !report.first().is_some_and(|id| ids.contains(id)) {
                    return;
                }
                report.to_vec()
            }
            None => std::iter::once(0).chain(report.iter().copied()).collect(),
        };
        if buf.len() < self.length {
            buf.resize(self.length, 0);
        }
        let mut reports = self.reports.lock().unwrap_or_else(|e| e.into_inner());
        if reports.len() == QUEUE {
            reports.pop_front();
        }
        reports.push_back(buf);
        self.arrived.notify_all();
    }
}

extern "C" fn on_input(
    context: *mut c_void,
    result: i32,
    _sender: *mut c_void,
    _kind: u32,
    _report_id: u32,
    report: *mut u8,
    length: isize,
) {
    if result != IO_RETURN_SUCCESS || report.is_null() || length <= 0 {
        return;
    }
    // The context is the Inbox the device holds for as long as the callback is registered
    let inbox = unsafe { &*(context as *const Inbox) };
    inbox.take(unsafe { std::slice::from_raw_parts(report, length as usize) });
}

/// The thread whose run loop the device's callbacks run on. It is stopped through a source of its own: a signal stays
/// pending until the loop next runs, so a stop that comes between two runs isn't lost (CFRunLoopStop would be), and
/// the loop, which only its thread keeps alive, is held here until the thread has ended.
struct RunLoop {
    run_loop: usize,
    wake: usize,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

extern "C" fn woken(_info: *const c_void) {}

impl RunLoop {
    fn start(device: usize) -> Option<Self> {
        let mut context = SourceContext { version: 0, perform: Some(woken), ..SourceContext::default() };
        let wake = unsafe { CFRunLoopSourceCreate(std::ptr::null(), 0, &mut context) } as usize;
        if wake == 0 {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::channel();
        let flag = stop.clone();
        let thread = std::thread::Builder::new().name("hid input".into()).spawn(move || unsafe {
            let run_loop = CFRunLoopGetCurrent();
            mac::CFRetain(run_loop);
            CFRunLoopAddSource(run_loop, wake as CFTypeRef, kCFRunLoopDefaultMode);
            IOHIDDeviceScheduleWithRunLoop(device as CFTypeRef, run_loop, kCFRunLoopDefaultMode);
            let _ = sender.send(run_loop as usize);
            // Back after each report or wake-up, to look at the flag
            while !flag.load(Ordering::Acquire) {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, 1);
            }
            IOHIDDeviceUnscheduleFromRunLoop(device as CFTypeRef, run_loop, kCFRunLoopDefaultMode);
            CFRunLoopRemoveSource(run_loop, wake as CFTypeRef, kCFRunLoopDefaultMode);
        });
        let started = thread.ok().and_then(|thread| receiver.recv().ok().map(|run_loop| (thread, run_loop)));
        match started {
            Some((thread, run_loop)) => Some(Self { run_loop, wake, stop, thread: Some(thread) }),
            None => {
                unsafe { mac::CFRelease(wake as CFTypeRef) };
                None
            }
        }
    }
}

impl Drop for RunLoop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        unsafe {
            CFRunLoopSourceSignal(self.wake as CFTypeRef);
            CFRunLoopWakeUp(self.run_loop as CFTypeRef);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            mac::CFRelease(self.wake as CFTypeRef);
            mac::CFRelease(self.run_loop as CFTypeRef);
        }
    }
}

/// CFRunLoopSourceContext, version 0: only `perform` is used.
#[repr(C)]
#[derive(Default)]
struct SourceContext {
    version: isize,
    info: usize,
    retain: usize,
    release: usize,
    copy_description: usize,
    equal: usize,
    hash: usize,
    schedule: usize,
    cancel: usize,
    perform: Option<extern "C" fn(*const c_void)>,
}

/// Read/write handle to one HID collection.
pub struct HidDevice {
    pub info: HidInfo,
    // Dropped in this order: the run loop stops before the device closes, and the callback's buffers outlive both
    run_loop: Option<RunLoop>,
    handle: Handle,
    inbox: Arc<Inbox>,
    _buffer: Box<[u8]>,
}

impl HidDevice {
    pub fn open(info: &HidInfo) -> Result<Self> {
        let handle = Handle::open(info)?;
        let inbox = Arc::new(Inbox {
            reports: Mutex::new(VecDeque::new()),
            arrived: Condvar::new(),
            ids: handle.uses_report_ids.then(|| handle.collection.input_ids.clone()),
            length: info.input_length,
        });
        let size = property_number(handle.device, "MaxInputReportSize").unwrap_or(0).max(info.input_length as i64 + 1);
        let mut buffer = vec![0u8; size.max(64) as usize].into_boxed_slice();
        unsafe {
            IOHIDDeviceRegisterInputReportCallback(
                handle.device,
                buffer.as_mut_ptr(),
                buffer.len() as isize,
                on_input,
                Arc::as_ptr(&inbox) as *mut c_void,
            );
        }
        let run_loop = RunLoop::start(handle.device as usize);
        if run_loop.is_none() {
            return Err(Error::other(format!("Cannot open {}: no thread for its input", info.path)));
        }
        Ok(Self { info: info.clone(), run_loop, handle, inbox, _buffer: buffer })
    }

    /// Only input reports (enough for devices that stream their state).
    pub fn open_read_only(info: &HidInfo) -> Result<Self> {
        Self::open(info)
    }

    /// For devices that only take feature reports.
    pub fn open_sync(info: &HidInfo) -> Result<Self> {
        Self::open(info)
    }

    /// Feature reports on a collection Windows holds exclusively; macOS holds none, so this is an ordinary open.
    pub fn open_features_only(info: &HidInfo) -> Result<Self> {
        Self::open(info)
    }

    pub fn flush(&self) {
        self.inbox.reports.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Fetches an input report over the control pipe (GET_REPORT).
    pub fn get_input_report(&self, report_id: u8) -> Option<Vec<u8>> {
        if self.info.input_length == 0 {
            return None;
        }
        self.handle.get(INPUT, report_id, self.info.input_length)
    }

    pub fn set_feature(&self, report: &[u8]) -> bool {
        self.handle.set(FEATURE, report)
    }

    pub fn get_feature(&self, report_id: u8) -> Option<Vec<u8>> {
        self.get_feature_of(report_id, self.info.feature_length)
    }

    /// Reads a feature report into a buffer of `length` bytes: for a report shorter than the collection's longest
    /// one, on a device that refuses to be asked for more than the report has.
    pub fn get_feature_of(&self, report_id: u8, length: usize) -> Option<Vec<u8>> {
        if self.info.feature_length == 0 || length == 0 {
            return None;
        }
        self.handle.get(FEATURE, report_id, length)
    }

    /// Writes an output report padded to the collection's length; false when it wasn't taken in time.
    pub fn write(&self, report: &[u8], timeout_ms: u32) -> Result<bool> {
        if self.info.output_length == 0 {
            return Ok(false); // the collection takes no output reports
        }
        let mut buf = vec![0u8; self.info.output_length];
        let n = report.len().min(buf.len());
        buf[..n].copy_from_slice(&report[..n]);
        let (id, data) = split_id(&buf);
        let sent = Arc::new(Sent::default());
        // The callback's reference: given back by the callback, or kept for good if it never comes
        let context = Arc::into_raw(sent.clone()) as *mut c_void;
        let status = unsafe {
            IOHIDDeviceSetReportWithCallback(
                self.handle.device,
                OUTPUT,
                id as isize,
                data.as_ptr(),
                data.len() as isize,
                timeout_ms as f64 / 1000.0,
                on_sent,
                context,
            )
        };
        if status != IO_RETURN_SUCCESS {
            unsafe { drop(Arc::from_raw(context as *const Sent)) };
            if status == IO_RETURN_UNSUPPORTED {
                // A transport without asynchronous output: sent and waited for
                return Ok(self.handle.set(OUTPUT, &buf));
            }
            return Err(Error::os(status as u32));
        }
        // IOKit answers by the timeout it was given; a little longer for the callback to come round
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64 + 250);
        let mut result = sent.result.lock().unwrap_or_else(|e| e.into_inner());
        while result.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            result = sent.done.wait_timeout(result, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        // A report not taken in time comes back timed out, or aborted by the timeout on some transports
        match result.unwrap_or(IO_RETURN_TIMEOUT) {
            IO_RETURN_SUCCESS => Ok(true),
            IO_RETURN_TIMEOUT | IO_RETURN_ABORTED | USB_TRANSACTION_TIMEOUT => Ok(false),
            code => Err(Error::os(code as u32)),
        }
    }

    /// The next input report, or None on timeout.
    pub fn read(&self, timeout_ms: u32) -> Result<Option<Vec<u8>>> {
        if self.info.input_length == 0 {
            return Ok(None);
        }
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let mut reports = self.inbox.reports.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(report) = reports.pop_front() {
                return Ok(Some(report));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            reports = self.inbox.arrived.wait_timeout(reports, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

impl Drop for HidDevice {
    fn drop(&mut self) {
        // No more callbacks into the buffers before they go: the run loop stops, then the device closes
        self.run_loop = None;
    }
}

/// The answer to an asynchronous write.
#[derive(Default)]
struct Sent {
    result: Mutex<Option<i32>>,
    done: Condvar,
}

extern "C" fn on_sent(
    context: *mut c_void,
    result: i32,
    _sender: *mut c_void,
    _kind: u32,
    _report_id: u32,
    _report: *mut u8,
    _length: isize,
) {
    let sent = unsafe { Arc::from_raw(context as *const Sent) };
    *sent.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
    sent.done.notify_all();
}

fn property_number(device: CFTypeRef, key: &str) -> Option<i64> {
    let key = Cf::string(key);
    mac::number_of(unsafe { IOHIDDeviceGetProperty(device, key.0) })
}

/// The value of a feature report's field with this usage, and the field's logical range. The descriptor says
/// whether the collection has such a field before anything is opened.
pub fn feature_value(info: &HidInfo, usage_page: u16, usage: u16) -> Option<UsageValue> {
    let (entry_id, index) = parse_path(&info.path)?;
    if info.access_denied {
        return None;
    }
    let field = unsafe {
        let service = Object(IOServiceGetMatchingService(0, IORegistryEntryIDMatching(entry_id)));
        if service.0 == 0 {
            return None;
        }
        let raw = property(service.0, "ReportDescriptor").and_then(|v| mac::data_of(v.0))?;
        let parsed = descriptor::parse(&raw);
        let collection = parsed.collections.get(index)?;
        collection.fields.iter().find(|f| f.kind == Kind::Feature && f.bit_of(usage_page, usage).is_some())?.clone()
    };
    let bit = field.bit_of(usage_page, usage)?;
    let handle = Handle::open(info).ok()?;
    let report = handle.get(FEATURE, field.report_id, info.feature_length.max(1))?;
    let raw = descriptor::value_at(&report, bit, field.size)?;
    Some(UsageValue { raw, min: field.logical_min, max: field.logical_max })
}

const INPUT: u32 = 0;
const OUTPUT: u32 = 1;
const FEATURE: u32 = 2;

type ReportCallback = extern "C" fn(*mut c_void, i32, *mut c_void, u32, u32, *mut u8, isize);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
    fn IOServiceGetMatchingServices(main_port: u32, matching: CFTypeRef, iterator: *mut u32) -> i32;
    fn IOServiceGetMatchingService(main_port: u32, matching: CFTypeRef) -> u32;
    fn IORegistryEntryIDMatching(entry_id: u64) -> CFTypeRef;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOObjectConformsTo(object: u32, class_name: *const c_char) -> u32;
    fn IORegistryEntryGetRegistryEntryID(entry: u32, entry_id: *mut u64) -> i32;
    fn IORegistryEntryCreateCFProperty(entry: u32, key: CFTypeRef, allocator: CFTypeRef, options: u32) -> CFTypeRef;
    fn IORegistryEntrySearchCFProperty(
        entry: u32,
        plane: *const c_char,
        key: CFTypeRef,
        allocator: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;
    fn IORegistryEntryGetParentEntry(entry: u32, plane: *const c_char, parent: *mut u32) -> i32;
    fn IORegistryEntryGetChildIterator(entry: u32, plane: *const c_char, iterator: *mut u32) -> i32;
    fn IOHIDDeviceCreate(allocator: CFTypeRef, service: u32) -> CFTypeRef;
    fn IOHIDDeviceOpen(device: CFTypeRef, options: u32) -> i32;
    fn IOHIDDeviceClose(device: CFTypeRef, options: u32) -> i32;
    fn IOHIDDeviceGetProperty(device: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn IOHIDDeviceGetReport(device: CFTypeRef, kind: u32, report_id: isize, report: *mut u8, length: *mut isize)
        -> i32;
    fn IOHIDDeviceSetReport(device: CFTypeRef, kind: u32, report_id: isize, report: *const u8, length: isize) -> i32;
    fn IOHIDDeviceSetReportWithCallback(
        device: CFTypeRef,
        kind: u32,
        report_id: isize,
        report: *const u8,
        length: isize,
        timeout: f64,
        callback: ReportCallback,
        context: *mut c_void,
    ) -> i32;
    fn IOHIDDeviceRegisterInputReportCallback(
        device: CFTypeRef,
        report: *mut u8,
        length: isize,
        callback: ReportCallback,
        context: *mut c_void,
    );
    fn IOHIDDeviceScheduleWithRunLoop(device: CFTypeRef, run_loop: CFTypeRef, mode: CFTypeRef);
    fn IOHIDDeviceUnscheduleFromRunLoop(device: CFTypeRef, run_loop: CFTypeRef, mode: CFTypeRef);
    fn IOHIDCheckAccess(request: u32) -> u32;
    fn IOHIDRequestAccess(request: u32) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: CFTypeRef;
    fn CFRunLoopGetCurrent() -> CFTypeRef;
    fn CFRunLoopRunInMode(mode: CFTypeRef, seconds: f64, return_after_source: u8) -> i32;
    fn CFRunLoopWakeUp(run_loop: CFTypeRef);
    fn CFRunLoopSourceCreate(allocator: CFTypeRef, order: isize, context: *mut SourceContext) -> CFTypeRef;
    fn CFRunLoopAddSource(run_loop: CFTypeRef, source: CFTypeRef, mode: CFTypeRef);
    fn CFRunLoopRemoveSource(run_loop: CFTypeRef, source: CFTypeRef, mode: CFTypeRef);
    fn CFRunLoopSourceSignal(source: CFTypeRef);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::{interface_number, parse_ids};

    fn device(transport: Transport, interface: Option<u8>, collections: usize) -> Device {
        Device {
            entry_id: 0x1000_052a1,
            vendor_id: 0x046D,
            product_id: 0xC539,
            vendor_source: 2,
            transport,
            interface,
            product: None,
            descriptor: descriptor::Descriptor {
                collections: vec![Collection::default(); collections],
                uses_report_ids: true,
            },
        }
    }

    /// The paths read back as Windows paths do: IDs, interface, transport, and what the providers group by.
    #[test]
    fn paths_in_the_windows_form() {
        let usb = device(Transport::Usb, Some(2), 3);
        let path = usb.path(1);
        assert_eq!(
            path,
            r"\\?\hid#vid_046d&pid_c539&mi_02&col02#m&1000052a1&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        );
        assert_eq!((parse_ids(&path), interface_number(&path)), (Some((0x046D, 0xC539)), 2));
        assert_eq!(parse_path(&path), Some((0x1000_052a1, 1)));
        // The collections of one interface share what the Logitech and ASUS providers group by
        let key = crate::providers::logitech::channel_key;
        assert_eq!(key(&usb.path(0)), key(&usb.path(2)));
        assert_eq!(crate::providers::devices::asus::instance(&usb.path(0)), "m&1000052a1&0");
        let single = device(Transport::Usb, None, 1).path(0);
        assert_eq!((single.contains("&col"), interface_number(&single)), (false, -1));

        let classic = device(Transport::BluetoothClassic, None, 1).path(0);
        let le = device(Transport::BluetoothLe, None, 2).path(1);
        for p in [&classic, &le] {
            let info = HidInfo { path: p.clone(), ..Default::default() };
            assert!(info.is_bluetooth(), "{p}");
            assert_eq!(parse_ids(p), Some((0x046D, 0xC539)), "{p}");
            assert_eq!(parse_path(p).map(|p| p.0), Some(0x1000_052a1), "{p}");
        }
        assert!(!HidInfo { path, ..Default::default() }.is_bluetooth());
    }

    #[test]
    fn reports_reach_the_collection_they_belong_to() {
        let inbox = |ids: Option<Vec<u8>>| Inbox {
            reports: Mutex::new(VecDeque::new()),
            arrived: Condvar::new(),
            ids,
            length: 8,
        };
        let numbered = inbox(Some(vec![0x10, 0x11]));
        numbered.take(&[0x10, 1, 2]);
        numbered.take(&[0x20, 1, 2]);
        numbered.take(&[0x11, 3]);
        let got: Vec<_> = numbered.reports.lock().unwrap().iter().cloned().collect();
        assert_eq!(got, [vec![0x10, 1, 2, 0, 0, 0, 0, 0], vec![0x11, 3, 0, 0, 0, 0, 0, 0]]);
        // Without report IDs the zero ID goes first, as Windows reads it
        let plain = inbox(None);
        plain.take(&[0xAA, 0xBB]);
        assert_eq!(plain.reports.lock().unwrap()[0], [0, 0xAA, 0xBB, 0, 0, 0, 0, 0]);
        // The oldest go when nobody reads
        for n in 0..40u8 {
            plain.take(&[n]);
        }
        let queue = plain.reports.lock().unwrap();
        assert_eq!((queue.len(), queue[0][1]), (QUEUE, 8));
    }

    #[test]
    fn report_ids_are_sent_as_iokit_takes_them() {
        assert_eq!(split_id(&[0, 1, 2]), (0, &[1u8, 2][..]));
        assert_eq!(split_id(&[0x10, 1, 2]), (0x10, &[0x10u8, 1, 2][..]));
    }

    /// The real thing, on this Mac: a collection is opened, read with a timeout, asked for a feature report and closed,
    /// again and again, without hanging or leaking a thread. `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn this_macs_devices_open_read_and_close() {
        let all = enumerate(|_, _| true);
        let readable = all.iter().filter(|i| !i.access_denied && i.usage_page >= 0xFF00 && i.input_length > 0);
        for info in readable.take(4) {
            for _ in 0..3 {
                let started = Instant::now();
                let dev = HidDevice::open(info).unwrap_or_else(|e| panic!("{info}: {e}"));
                let report = dev.read(150).unwrap();
                dev.flush();
                let feature = (info.feature_length > 0).then(|| dev.get_feature(0)).flatten();
                drop(dev);
                let took = started.elapsed();
                println!(
                    "{info}: read {:?}, feature {:?}, {took:?}",
                    report.map(|r| r.len()),
                    feature.map(|f| f.len())
                );
                assert!(took < Duration::from_secs(3), "{info} took {took:?}");
            }
        }
    }

    /// The real thing, on this Mac: every HID device is listed with its collections, and none of them is opened.
    #[test]
    fn this_macs_devices() {
        let all = enumerate(|_, _| true);
        for info in &all {
            println!("{info} {}", info.path);
        }
        assert!(all.iter().all(|i| parse_path(&i.path).is_some() && parse_ids(&i.path).is_some()));
    }
}
