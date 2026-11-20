//! Updates from GitHub Releases (Core/Updater.cs): the "BatteryTray.exe" asset of the latest release replaces the
//! running exe. The new exe is downloaded straight into the exe's own folder, so an install in Program Files never
//! passes through a folder other programs can write to. It must match the SHA-256 digest GitHub publishes for the
//! asset and carry the release's version (a mislabeled asset would otherwise update forever). Then it is swapped in:
//! the running exe is renamed to *.old.exe (Windows allows renaming a running exe) and the new one takes its name.
//! The old file is deleted on the next start.
//! The C# app does exactly the same, which is how its installs move to this one: the Rust exe is just a release
//! asset with the right name, digest and FileVersion.

use crate::loc::tf;
use crate::win::{self, wide, Error, Result};
use crate::{http, log};
use std::ffi::c_void;
use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

pub const REPOSITORY: &str = "qu1ncky/BatteryTray";
const ASSET_NAME: &str = "BatteryTray.exe";

/// Major.minor.build, as the C# app compares versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// "1.8.6", "1.8" or "1.8.6.0" (as .NET's Version.TryParse; the fourth part is dropped).
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.trim().split('.').collect();
        if !(2..=4).contains(&parts.len()) {
            return None;
        }
        let mut numbers = Vec::new();
        for p in &parts {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            numbers.push(p.parse::<u32>().ok()?);
        }
        Some(Self(numbers[0], numbers[1], numbers.get(2).copied().unwrap_or(0)))
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the package version is a version")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The newest published release and its exe asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: Version,
    pub tag: String,
    pub notes: String,
    pub page_url: String,
    pub exe_url: String,
    pub exe_size: u64,
    pub sha256: String,
}

fn user_agent() -> String {
    format!("BatteryTray/{}", Version::current()) // GitHub's API requires one
}

/// The latest non-prerelease release, or None when it has no usable exe asset.
pub fn get_latest() -> Result<Option<ReleaseInfo>> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let text = http::get(&url, &user_agent(), Some("application/vnd.github+json"))?.text()?;
    let root: serde_json::Value = serde_json::from_str(&text).map_err(|e| Error::other(e.to_string()))?;
    Ok(parse_release(&root))
}

pub fn parse_release(root: &serde_json::Value) -> Option<ReleaseInfo> {
    let tag = root.get("tag_name")?.as_str()?.to_string();
    let version = Version::parse(tag.trim_start_matches(['v', 'V']))?;
    let asset =
        root.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(|n| n.as_str()) == Some(ASSET_NAME))?;
    let digest = asset.get("digest")?.as_str()?;
    if digest.len() < 7 || !digest[..7].eq_ignore_ascii_case("sha256:") {
        return None;
    }
    Some(ReleaseInfo {
        version,
        notes: root.get("body").and_then(|b| b.as_str()).unwrap_or_default().to_string(),
        page_url: root.get("html_url").and_then(|u| u.as_str()).unwrap_or_default().to_string(),
        exe_url: asset.get("browser_download_url").and_then(|u| u.as_str()).unwrap_or_default().to_string(),
        exe_size: asset.get("size").and_then(|s| s.as_u64())?,
        sha256: digest[7..].to_ascii_lowercase(),
        tag,
    })
}

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

    download(release, &fresh, progress)?;
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

fn download(release: &ReleaseInfo, path: &Path, progress: &dyn Fn(i32)) -> Result<()> {
    let mut response = http::get(&release.exe_url, &user_agent(), None)?;
    let mut file = File::create(path).map_err(|e| Error::other(e.to_string()))?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done: u64 = 0;
    let mut last_percent = -1;
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| Error::other(e.to_string()))?;
        done += n as u64;
        let percent = (done * 100 / release.exe_size.max(1)) as i32;
        if percent != last_percent {
            last_percent = percent;
            progress(percent);
        }
    }
    file.sync_all().map_err(|e| Error::other(e.to_string()))
}

fn verify(path: &Path, release: &ReleaseInfo) -> Result<()> {
    let bytes = fs::read(path).map_err(|e| Error::other(e.to_string()))?;
    if bytes.len() as u64 != release.exe_size {
        return Err(Error::other(tf("upd.err.size", &[&bytes.len(), &release.exe_size])));
    }
    if hex(&sha256(&bytes)?) != release.sha256 {
        return Err(Error::other(tf("upd.err.hash", &[])));
    }
    let file_version = file_version(path);
    if file_version.as_deref().and_then(Version::parse) != Some(release.version) {
        return Err(Error::other(tf("upd.err.version", &[&release.tag, &file_version.unwrap_or_default()])));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

    #[test]
    fn versions_as_dotnet_parses_them() {
        assert_eq!(Version::parse("1.8.6"), Some(Version(1, 8, 6)));
        assert_eq!(Version::parse("1.8.6.0"), Some(Version(1, 8, 6)));
        assert_eq!(Version::parse("2.0"), Some(Version(2, 0, 0)));
        assert_eq!(Version::parse("2"), None);
        assert_eq!(Version::parse("1.8.x"), None);
        assert!(Version(2, 0, 0) > Version(1, 99, 0) && Version(1, 99, 0) > Version(1, 8, 6));
    }

    #[test]
    fn release_from_githubs_json() {
        let json = serde_json::json!({
            "tag_name": "v1.8.6", "body": "notes", "html_url": "https://github.com/qu1ncky/BatteryTray/releases/tag/v1.8.6",
            "assets": [
                { "name": "BatteryTray-1.8.6.zip", "digest": "sha256:00", "size": 1, "browser_download_url": "z" },
                { "name": "BatteryTray.exe", "digest": "sha256:ABCDEF", "size": 78123456,
                  "browser_download_url": "https://github.com/qu1ncky/BatteryTray/releases/download/v1.8.6/BatteryTray.exe" }
            ]
        });
        let r = parse_release(&json).unwrap();
        assert_eq!((r.version, r.exe_size, r.sha256.as_str()), (Version(1, 8, 6), 78123456, "abcdef"));
        let mut no_digest = json.clone();
        no_digest["assets"][1]["digest"] = serde_json::Value::Null;
        assert_eq!(parse_release(&no_digest), None, "no digest, no update");
    }

    /// The real thing: downloads the latest release's exe from GitHub (tens of MB) and checks it as an update would.
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn latest_release_downloads_and_verifies() {
        let release = get_latest().unwrap().expect("the latest release has an exe asset");
        let path = std::env::temp_dir().join("BatteryTray-update-test.exe");
        let percent = std::sync::atomic::AtomicI32::new(-1);
        download(&release, &path, &|p| percent.store(p, std::sync::atomic::Ordering::Relaxed)).unwrap();
        let checked = verify(&path, &release);
        let version = file_version(&path);
        let _ = fs::remove_file(&path);
        checked.unwrap();
        assert_eq!(percent.load(std::sync::atomic::Ordering::Relaxed), 100);
        assert_eq!(version.as_deref().and_then(Version::parse), Some(release.version));
    }

    #[test]
    fn sha256_of_abc() {
        assert_eq!(hex(&sha256(b"abc").unwrap()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
