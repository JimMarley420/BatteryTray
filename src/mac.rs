//! Small macOS helpers shared by the macOS parts: CoreFoundation values, IOKit's error texts, the app's data folder,
//! the system language and version. CoreFoundation is declared by hand, as win.rs declares Win32.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFIndex = isize;

const UTF8: u32 = 0x0800_0100;
const NUMBER_SINT64: CFIndex = 4;

/// An owned CoreFoundation object, released when dropped.
pub struct Cf(pub CFTypeRef);

impl Cf {
    /// Takes over an object that came from a Create or Copy function; None for NULL.
    pub fn new(object: CFTypeRef) -> Option<Self> {
        (!object.is_null()).then_some(Self(object))
    }

    pub fn string(text: &str) -> Self {
        let c = CString::new(text.replace('\0', "")).unwrap_or_default();
        Self(unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) })
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

// The objects are immutable or only touched through thread-safe calls
unsafe impl Send for Cf {}

/// A CFString's text; None for NULL or another type.
pub fn string_of(value: CFTypeRef) -> Option<String> {
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return None;
        }
        let fast = CFStringGetCStringPtr(value, UTF8);
        if !fast.is_null() {
            return Some(CStr::from_ptr(fast).to_string_lossy().into_owned());
        }
        let length = CFStringGetLength(value);
        let size = CFStringGetMaximumSizeForEncoding(length, UTF8) + 1;
        let mut buf = vec![0u8; size.max(1) as usize];
        if CFStringGetCString(value, buf.as_mut_ptr() as *mut c_char, size, UTF8) == 0 {
            return None;
        }
        Some(CStr::from_ptr(buf.as_ptr() as *const c_char).to_string_lossy().into_owned())
    }
}

/// A CFNumber's value as an integer; None for NULL or another type.
pub fn number_of(value: CFTypeRef) -> Option<i64> {
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFNumberGetTypeID() {
            return None;
        }
        let mut n: i64 = 0;
        (CFNumberGetValue(value, NUMBER_SINT64, &mut n as *mut i64 as *mut c_void) != 0).then_some(n)
    }
}

/// A CFData's bytes; None for NULL or another type.
pub fn data_of(value: CFTypeRef) -> Option<Vec<u8>> {
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFDataGetTypeID() {
            return None;
        }
        let length = CFDataGetLength(value);
        let bytes = CFDataGetBytePtr(value);
        if bytes.is_null() || length <= 0 {
            return Some(Vec::new());
        }
        Some(std::slice::from_raw_parts(bytes, length as usize).to_vec())
    }
}

/// A CFDictionary's value for a text key; NULL when the dictionary is NULL, of another type or has no such key.
pub fn value_of(dictionary: CFTypeRef, key: &str) -> CFTypeRef {
    unsafe {
        if dictionary.is_null() || CFGetTypeID(dictionary) != CFDictionaryGetTypeID() {
            return std::ptr::null();
        }
        let key = Cf::string(key);
        CFDictionaryGetValue(dictionary, key.0)
    }
}

/// A CFArray's items, owned by the array; empty for NULL or another type.
pub fn items_of(array: CFTypeRef) -> Vec<CFTypeRef> {
    unsafe {
        if array.is_null() || CFGetTypeID(array) != CFArrayGetTypeID() {
            return Vec::new();
        }
        (0..CFArrayGetCount(array)).map(|i| CFArrayGetValueAtIndex(array, i)).collect()
    }
}

/// IOKit's and the kernel's text for a return code ("(iokit/common) not permitted").
pub fn system_message(code: u32) -> String {
    let text = unsafe { mach_error_string(code as i32) };
    let text =
        if text.is_null() { String::new() } else { unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned() };
    if text.is_empty() || text.contains("unknown error code") {
        format!("Unknown error (0x{code:x})")
    } else {
        format!("{text} (0x{code:x})")
    }
}

/// ~/Library/Application Support/BatteryTray: settings.json, state.json and the log.
pub fn app_data_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Library").join("Application Support").join("BatteryTray")
}

/// The first of the user's preferred languages (System Settings → General → Language & Region): "ru", "zh" or "en".
pub fn ui_language() -> &'static str {
    let first = unsafe {
        let languages = Cf::new(CFLocaleCopyPreferredLanguages());
        languages.and_then(|list| {
            (CFArrayGetCount(list.0) > 0).then(|| string_of(CFArrayGetValueAtIndex(list.0, 0))).flatten()
        })
    };
    language_of(first.as_deref().unwrap_or("en"))
}

/// "ru", "zh" or "en" for a language tag such as "ru-RU" or "zh-Hans-CN".
pub fn language_of(tag: &str) -> &'static str {
    match tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase().as_str() {
        "ru" => "ru",
        "zh" => "zh",
        _ => "en",
    }
}

pub fn is_root() -> bool {
    unsafe { geteuid() == 0 }
}

/// The macOS version as the system names it: "26.6.2".
pub fn macos_version() -> String {
    let mut buf = [0u8; 64];
    let mut size = buf.len();
    let name = c"kern.osproductversion";
    let ok =
        unsafe { sysctlbyname(name.as_ptr(), buf.as_mut_ptr() as *mut c_void, &mut size, std::ptr::null_mut(), 0) };
    if ok != 0 || size == 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..size]).trim_end_matches('\0').to_string()
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFRelease(object: CFTypeRef);
    pub fn CFRetain(object: CFTypeRef) -> CFTypeRef;
    pub fn CFGetTypeID(object: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFStringCreateWithCString(alloc: CFAllocatorRef, text: *const c_char, encoding: u32) -> CFStringRef;
    fn CFStringGetCStringPtr(string: CFStringRef, encoding: u32) -> *const c_char;
    fn CFStringGetLength(string: CFStringRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
    fn CFStringGetCString(string: CFStringRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> u8;
    fn CFNumberGetValue(number: CFTypeRef, kind: CFIndex, value: *mut c_void) -> u8;
    fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFLocaleCopyPreferredLanguages() -> CFTypeRef;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
}

extern "C" {
    fn mach_error_string(code: i32) -> *const c_char;
    fn geteuid() -> u32;
    fn sysctlbyname(
        name: *const c_char,
        old: *mut c_void,
        old_len: *mut usize,
        new: *mut c_void,
        new_len: usize,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags() {
        assert_eq!(language_of("ru-RU"), "ru");
        assert_eq!(language_of("zh-Hans-CN"), "zh");
        assert_eq!(language_of("en-US"), "en");
        assert_eq!(language_of("de"), "en");
        assert_eq!(language_of(""), "en");
    }

    #[test]
    fn strings_numbers_and_errors() {
        let s = Cf::string("Заряд 85%");
        assert_eq!(string_of(s.0).as_deref(), Some("Заряд 85%"));
        assert_eq!(number_of(s.0), None);
        assert!(system_message(0xE00002E2).contains("not permitted"), "{}", system_message(0xE00002E2));
        assert!(macos_version().starts_with(|c: char| c.is_ascii_digit()), "{}", macos_version());
        println!("macOS {}, language {}", macos_version(), ui_language());
    }
}
