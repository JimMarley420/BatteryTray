//! Updates from GitHub Releases: the latest release's asset for this system replaces the running app - the exe on
//! Windows, the app bundle (from a zip) on macOS. The new copy is downloaded straight next to the running one, so an
//! install that only administrators can write to never passes through a folder other programs can write to. It must
//! match the SHA-256 digest GitHub publishes for the asset and carry the release's version (a mislabeled asset would
//! otherwise update forever). The old copy is swapped out under another name and deleted on the next start.
//! The C# app does exactly the same on Windows, which is how its installs move to this one: the Rust exe is just a
//! release asset with the right name, digest and FileVersion.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{can_install_in_place, clean_up, install, install_elevated, sha256, ASSET_NAME};
#[cfg(windows)]
pub use windows::{can_install_in_place, clean_up, file_version, install, install_elevated, sha256, ASSET_NAME};

use crate::http;
use crate::loc::tf;
use crate::sys::{Error, Result};
use std::fmt;
use std::path::Path;

pub const REPOSITORY: &str = "qu1ncky/BatteryTray";
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

/// The newest published release and its asset for this system (`exe_url` and `exe_size` are the zip's on macOS).
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

pub(crate) fn user_agent() -> String {
    format!("BatteryTray/{}", Version::current()) // GitHub's API requires one
}

/// The latest non-prerelease release, or None when it has no usable asset for this system.
pub fn get_latest() -> Result<Option<ReleaseInfo>> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let text = http::get_text(&url, &user_agent(), Some("application/vnd.github+json"))?;
    let root: serde_json::Value = serde_json::from_str(&text).map_err(|e| Error::other(e.to_string()))?;
    Ok(parse_release(&root))
}

pub fn parse_release(root: &serde_json::Value) -> Option<ReleaseInfo> {
    parse_release_for(root, ASSET_NAME)
}

/// The release with its asset of this name.
pub fn parse_release_for(root: &serde_json::Value, asset_name: &str) -> Option<ReleaseInfo> {
    let tag = root.get("tag_name")?.as_str()?.to_string();
    let version = Version::parse(tag.trim_start_matches(['v', 'V']))?;
    let asset =
        root.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(|n| n.as_str()) == Some(asset_name))?;
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

/// The downloaded file has the size and the SHA-256 digest GitHub publishes for the asset.
pub(crate) fn check_digest(path: &Path, release: &ReleaseInfo) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|e| Error::other(e.to_string()))?;
    if bytes.len() as u64 != release.exe_size {
        return Err(Error::other(tf("upd.err.size", &[&bytes.len(), &release.exe_size])));
    }
    if hex(&sha256(&bytes)?) != release.sha256 {
        return Err(Error::other(tf("upd.err.hash", &[])));
    }
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
                  "browser_download_url": "https://github.com/qu1ncky/BatteryTray/releases/download/v1.8.6/BatteryTray.exe" },
                { "name": "BatteryTray-macOS.zip", "digest": "sha256:0123", "size": 9123456,
                  "browser_download_url": "https://github.com/qu1ncky/BatteryTray/releases/download/v1.8.6/BatteryTray-macOS.zip" }
            ]
        });
        let r = parse_release_for(&json, "BatteryTray.exe").unwrap();
        assert_eq!((r.version, r.exe_size, r.sha256.as_str()), (Version(1, 8, 6), 78123456, "abcdef"));
        let mac = parse_release_for(&json, "BatteryTray-macOS.zip").unwrap();
        assert_eq!((mac.exe_size, mac.sha256.as_str()), (9123456, "0123"));
        assert_eq!(parse_release(&json).map(|r| r.exe_url.ends_with(ASSET_NAME)), Some(true));
        let mut no_digest = json.clone();
        no_digest["assets"][1]["digest"] = serde_json::Value::Null;
        assert_eq!(parse_release_for(&no_digest, "BatteryTray.exe"), None, "no digest, no update");
    }

    #[test]
    fn sha256_of_abc() {
        assert_eq!(hex(&sha256(b"abc").unwrap()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
