//! Updates on Windows: the "BatteryTray.exe" asset of the latest release replaces the running exe. The new exe is
//! downloaded into the exe's own folder, checked, and swapped in: the running exe is renamed to *.old.exe (Windows
//! allows renaming a running exe) and the new one takes its name. The old file is deleted on the next start.

use super::{check_digest, user_agent, ReleaseInfo, Version};
use crate::loc::tf;
use crate::win::{self, wide, Error, Result};
use crate::{autostart, http, log};
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

pub const ASSET_NAME: &str = "BatteryTray.exe";

fn exe() -> Result<PathBuf> {
    std::env::current_exe().map_err(|e| Error::other(e.to_string()))
}

/// "BatteryTray" + suffix, next to the running exe.
fn sibling(suffix: &str) -> Result<PathBuf> {
    let exe = exe()?;
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    Ok(exe.with_file_name(format!("{stem}{suffix}")))
}

/// True when this process may replace its own exe: a portable copy, or an elevated one in Program Files. Otherwise
/// an elevated copy of the exe has to do it (`--update`).
pub fn can_install_in_place() -> bool {
    let Ok(probe) = sibling(".write-test") else { return false };
    fs::write(&probe, b"").and_then(|_| fs::remove_file(&probe)).is_ok()
}

pub fn install(release: &ReleaseInfo, progress: &dyn Fn(i32)) -> Result<()> {
    let exe = exe()?;
    let fresh = sibling(".new.exe")?;
    let old = sibling(".old.exe")?;
    clean_up();

    http::download(&release.exe_url, &user_agent(), &fresh, release.exe_size, progress)?;
    if let Err(e) = verify(&fresh, release) {
        let _ = fs::remove_file(&fresh);
        return Err(e);
    }

    let io = |e: std::io::Error| Error::other(e.to_string());
    fs::rename(&exe, &old).map_err(io)?;
    if let Err(e) = fs::rename(&fresh, &exe) {
        let _ = fs::rename(&old, &exe); // put the working version back
        return Err(io(e));
    }
    log::write(&format!("update: {} -> {} installed at {}", Version::current(), release.version, exe.display()));
    Ok(())
}

/// Removes what a previous update left behind (the old exe can only go once it has exited).
pub fn clean_up() {
    for suffix in [".old.exe", ".new.exe"] {
        if let Ok(leftover) = sibling(suffix) {
            // Still running (the previous version is exiting): the next start removes it
            let _ = fs::remove_file(leftover);
        }
    }
}

fn verify(path: &Path, release: &ReleaseInfo) -> Result<()> {
    check_digest(path, release)?;
    let file_version = file_version(path);
    if file_version.as_deref().and_then(Version::parse) != Some(release.version) {
        return Err(Error::other(tf("upd.err.version", &[&release.tag, &file_version.unwrap_or_default()])));
    }
    Ok(())
}

/// Installs through an elevated copy of this exe (`--update`, a UAC prompt): an install in Program Files without
/// admin rights. True when it installed the update.
pub fn install_elevated() -> bool {
    autostart::run_self_elevated("--update")
}

/// SHA-256 by Windows' own implementation (CNG).
pub fn sha256(data: &[u8]) -> Result<[u8; 32]> {
    const BCRYPT_SHA256_ALG_HANDLE: usize = 0x41;
    let mut out = [0u8; 32];
    let status = unsafe {
        BCryptHash(BCRYPT_SHA256_ALG_HANDLE, null_mut(), 0, data.as_ptr(), data.len() as u32, out.as_mut_ptr(), 32)
    };
    if status != 0 {
        return Err(Error::other(format!("SHA-256 failed: 0x{status:08x}")));
    }
    Ok(out)
}

/// The FileVersion string of an exe's version resource, as .NET's FileVersionInfo reads it.
pub fn file_version(path: &Path) -> Option<String> {
    let name = wide(&path.to_string_lossy());
    unsafe {
        let mut ignored = 0u32;
        let size = GetFileVersionInfoSizeW(name.as_ptr(), &mut ignored);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(name.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
            return None;
        }
        let query = |sub: &str| -> Option<(*const u8, u32)> {
            let mut ptr: *mut c_void = null_mut();
            let mut len = 0u32;
            let ok = VerQueryValueW(data.as_ptr() as *const c_void, wide(sub).as_ptr(), &mut ptr, &mut len);
            (ok != 0 && !ptr.is_null() && len > 0).then_some((ptr as *const u8, len))
        };
        // The first translation, then the usual ones
        let mut tables = Vec::new();
        if let Some((ptr, len)) = query("\\VarFileInfo\\Translation") {
            if len >= 4 {
                let lang = u16::from_le_bytes([*ptr, *ptr.add(1)]);
                let codepage = u16::from_le_bytes([*ptr.add(2), *ptr.add(3)]);
                tables.push(format!("{lang:04x}{codepage:04x}"));
            }
        }
        tables.extend(["040904b0", "040904e4", "04090000"].map(String::from));
        for table in tables {
            if let Some((ptr, len)) = query(&format!("\\StringFileInfo\\{table}\\FileVersion")) {
                let chars = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                return Some(win::from_wide(chars));
            }
        }
        None
    }
}

#[link(name = "bcrypt", kind = "raw-dylib")]
extern "system" {
    fn BCryptHash(
        algorithm: usize,
        secret: *mut u8,
        secret_len: u32,
        input: *const u8,
        input_len: u32,
        output: *mut u8,
        output_len: u32,
    ) -> i32;
}

#[link(name = "version", kind = "raw-dylib")]
extern "system" {
    fn GetFileVersionInfoSizeW(name: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(name: *const u16, handle: u32, len: u32, data: *mut c_void) -> i32;
    fn VerQueryValueW(block: *const c_void, sub_block: *const u16, buffer: *mut *mut c_void, len: *mut u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::get_latest;

    /// The real thing: downloads the latest release's exe from GitHub (tens of MB) and checks it as an update would.
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn latest_release_downloads_and_verifies() {
        let release = get_latest().unwrap().expect("the latest release has an exe asset");
        let path = std::env::temp_dir().join("BatteryTray-update-test.exe");
        let percent = std::sync::atomic::AtomicI32::new(-1);
        let progress = |p| percent.store(p, std::sync::atomic::Ordering::Relaxed);
        http::download(&release.exe_url, &user_agent(), &path, release.exe_size, &progress).unwrap();
        let checked = verify(&path, &release);
        let version = file_version(&path);
        let _ = fs::remove_file(&path);
        checked.unwrap();
        assert_eq!(percent.load(std::sync::atomic::Ordering::Relaxed), 100);
        assert_eq!(version.as_deref().and_then(Version::parse), Some(release.version));
    }
}
