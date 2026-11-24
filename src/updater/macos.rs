//! Updates on macOS: the "BatteryTray-macOS.zip" asset of the latest release holds BatteryTray.app, which replaces the
//! running app bundle. The zip is downloaded next to the bundle and checked, unpacked there by ditto (which keeps the
//! bundle's signature and attributes), and the new bundle must carry the release's version and be signed as the
//! running one is; then the running bundle is renamed to "BatteryTray.old.app" (a running app may be renamed) and the
//! new one takes its name. What is left is deleted on the next start.

use super::{check_digest, user_agent, ReleaseInfo, Version};
use crate::loc::{t, tf};
use crate::sys::{Error, Result};
use crate::{http, log};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const ASSET_NAME: &str = "BatteryTray-macOS.zip";

/// The app bundle this process runs from: ".../BatteryTray.app" for ".../BatteryTray.app/Contents/MacOS/BatteryTray".
fn bundle() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(|e| Error::other(e.to_string()))?;
    let bundle = exe.parent().and_then(Path::parent).and_then(Path::parent).map(Path::to_path_buf);
    match bundle {
        Some(b) if b.extension().is_some_and(|e| e == "app") => Ok(b),
        _ => Err(Error::other(t("upd.err.notBundle"))),
    }
}

/// "BatteryTray" + suffix, next to the running bundle.
fn sibling(suffix: &str) -> Result<PathBuf> {
    let bundle = bundle()?;
    let stem = bundle.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    Ok(bundle.with_file_name(format!("{stem}{suffix}")))
}

/// macOS runs an app downloaded with a browser from a read-only copy until it is moved (App Translocation): nothing
/// next to it can be written, by anybody.
fn translocated(bundle: &Path) -> bool {
    bundle.to_string_lossy().contains("/AppTranslocation/")
}

/// True when this process may replace its own bundle: one in a folder the user can write to (the Applications folder
/// of an administrator, the home folder). Otherwise it takes an administrator's password (`install_elevated`).
pub fn can_install_in_place() -> bool {
    let Ok(probe) = sibling(".write-test") else { return false };
    fs::write(&probe, b"").and_then(|_| fs::remove_file(&probe)).is_ok()
}

pub fn install(release: &ReleaseInfo, progress: &dyn Fn(i32)) -> Result<()> {
    let bundle = bundle()?;
    if translocated(&bundle) {
        return Err(Error::other(t("upd.err.translocated")));
    }
    let zip = sibling(".new.zip")?;
    let unpacked = sibling(".new")?;
    let old = sibling(".old.app")?;
    clean_up();

    http::download(&release.exe_url, &user_agent(), &zip, release.exe_size, progress)?;
    let fresh = check_digest(&zip, release).and_then(|_| unpack(&zip, &unpacked));
    let _ = fs::remove_file(&zip);
    let checked = fresh.and_then(|app| {
        verify(&app, release)?;
        verify_signature(&app, &bundle)?;
        Ok(app)
    });
    let fresh = match checked {
        Ok(app) => app,
        Err(e) => {
            let _ = fs::remove_dir_all(&unpacked);
            return Err(e);
        }
    };

    let io = |e: std::io::Error| Error::other(e.to_string());
    fs::rename(&bundle, &old).map_err(io)?;
    if let Err(e) = fs::rename(&fresh, &bundle) {
        let _ = fs::rename(&old, &bundle); // put the working version back
        let _ = fs::remove_dir_all(&unpacked);
        return Err(io(e));
    }
    let _ = fs::remove_dir_all(&unpacked);
    log::write(&format!("update: {} -> {} installed at {}", Version::current(), release.version, bundle.display()));
    Ok(())
}

/// Unpacks the zip into `folder` and returns the app bundle in it.
fn unpack(zip: &Path, folder: &Path) -> Result<PathBuf> {
    let _ = fs::remove_dir_all(folder);
    let status = Command::new("/usr/bin/ditto").arg("-x").arg("-k").arg(zip).arg(folder).status();
    if !status.is_ok_and(|s| s.success()) {
        return Err(Error::other(t("upd.err.unpack")));
    }
    let app = fs::read_dir(folder)
        .map_err(|e| Error::other(e.to_string()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|e| e == "app"));
    app.ok_or_else(|| Error::other(t("upd.err.unpack")))
}

fn verify(app: &Path, release: &ReleaseInfo) -> Result<()> {
    let version = bundle_version(app);
    if version.as_deref().and_then(Version::parse) != Some(release.version) {
        return Err(Error::other(tf("upd.err.version", &[&release.tag, &version.unwrap_or_default()])));
    }
    Ok(())
}

/// The new app is signed as the running one is. macOS ties the permissions it gave the app (Bluetooth, Input
/// Monitoring) to the app's signature, the project's own certificate: a version signed otherwise would lose them on
/// every Mac it reached. A copy of the app signed ad hoc (a developer's build, whose requirement is its own code
/// hash) has nothing to compare, and any release may replace it.
fn verify_signature(app: &Path, running: &Path) -> Result<()> {
    match designated_requirement(running) {
        Some(requirement) if requirement.contains("certificate") => {
            if satisfies(app, &requirement) {
                Ok(())
            } else {
                log::write(&format!("update: {} isn't signed to satisfy {requirement}", app.display()));
                Err(Error::other(t("upd.err.signature")))
            }
        }
        _ => Ok(()),
    }
}

/// What a bundle's signature requires of the code that may stand for it ("designated => ..." of `codesign -d -r-`;
/// "# designated => cdhash ..." when it is only implied, as for an ad hoc signature).
fn designated_requirement(bundle: &Path) -> Option<String> {
    let out = Command::new("/usr/bin/codesign").args(["-d", "-r-"]).arg(bundle).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    text.lines()
        .find_map(|line| line.trim_start_matches("# ").strip_prefix("designated => "))
        .map(|r| r.trim().to_string())
}

/// The bundle's signature is valid and meets the requirement.
fn satisfies(bundle: &Path, requirement: &str) -> bool {
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "-R"])
        .arg(format!("={requirement}"))
        .arg(bundle)
        .stderr(std::process::Stdio::null())
        .status();
    status.is_ok_and(|s| s.success())
}

/// The CFBundleShortVersionString of an app bundle's Info.plist.
pub fn bundle_version(app: &Path) -> Option<String> {
    let plist = app.join("Contents").join("Info.plist");
    let out = Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// Removes what a previous update left behind.
pub fn clean_up() {
    for suffix in [".old.app", ".new"] {
        if let Ok(leftover) = sibling(suffix) {
            let _ = fs::remove_dir_all(leftover);
        }
    }
    if let Ok(zip) = sibling(".new.zip") {
        let _ = fs::remove_file(zip);
    }
}

/// Installs through a copy of this app run as root (`--update`), which asks for an administrator's password as macOS
/// does: an app in a folder only administrators can write to. True when it installed the update.
pub fn install_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let quoted = exe.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("do shell script (quoted form of \"{quoted}\") & \" --update\" with administrator privileges");
    let status = Command::new("/usr/bin/osascript").arg("-e").arg(script).status();
    status.is_ok_and(|s| s.success())
}

/// SHA-256 by macOS' own implementation (CommonCrypto).
pub fn sha256(data: &[u8]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    // One call takes at most 4 GB
    let length = u32::try_from(data.len()).map_err(|_| Error::other("SHA-256: more than 4 GB"))?;
    unsafe { CC_SHA256(data.as_ptr(), length, out.as_mut_ptr()) };
    Ok(out)
}

extern "C" {
    fn CC_SHA256(data: *const u8, length: u32, digest: *mut u8) -> *mut u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::hex;

    /// The digest the release would publish for a file.
    fn digest_of(path: &Path) -> String {
        hex(&sha256(&fs::read(path).unwrap()).unwrap())
    }

    /// A zip as the release has it: unpacked, found and checked as an update is.
    #[test]
    fn a_zip_of_an_app_bundle_is_unpacked_and_checked() {
        let root = std::env::temp_dir().join(format!("bt-update-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let contents = root.join("src").join("BatteryTray.app").join("Contents");
        fs::create_dir_all(&contents).unwrap();
        fs::write(
            contents.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
            <key>CFBundleShortVersionString</key><string>9.8.7</string></dict></plist>"#,
        )
        .unwrap();
        let zip = root.join("BatteryTray-macOS.zip");
        let zipped = Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(root.join("src").join("BatteryTray.app"))
            .arg(&zip)
            .status()
            .unwrap();
        assert!(zipped.success());
        let release = ReleaseInfo {
            version: Version(9, 8, 7),
            tag: "v9.8.7".into(),
            notes: String::new(),
            page_url: String::new(),
            exe_url: String::new(),
            exe_size: fs::metadata(&zip).unwrap().len(),
            sha256: digest_of(&zip),
        };
        check_digest(&zip, &release).unwrap();
        let app = unpack(&zip, &root.join("out")).unwrap();
        assert_eq!(app.file_name().unwrap(), "BatteryTray.app");
        verify(&app, &release).unwrap();
        let older = ReleaseInfo { version: Version(9, 8, 8), tag: "v9.8.8".into(), ..release.clone() };
        assert!(verify(&app, &older).is_err(), "a bundle of another version is refused");
        let wrong = ReleaseInfo { sha256: "00".into(), ..release };
        assert!(check_digest(&zip, &wrong).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    /// A bundle signed ad hoc with this identifier.
    fn signed_bundle(root: &Path, name: &str, identifier: &str) -> PathBuf {
        let app = root.join(format!("{name}.app"));
        let macos = app.join("Contents").join("MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(
            app.join("Contents").join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
                <key>CFBundleIdentifier</key><string>{identifier}</string>
                <key>CFBundleExecutable</key><string>{name}</string></dict></plist>"#
            ),
        )
        .unwrap();
        fs::copy("/usr/bin/true", macos.join(name)).unwrap();
        let signed = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&app).status().unwrap();
        assert!(signed.success());
        app
    }

    /// The new version must meet the running app's requirement when that names a certificate; an ad hoc build's
    /// requirement (its own code hash) asks nothing of it.
    #[test]
    fn an_update_must_be_signed_as_the_running_app() {
        let root = std::env::temp_dir().join(format!("bt-signature-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let running = signed_bundle(&root, "Running", "io.test.batterytray");
        let fresh = signed_bundle(&root, "Fresh", "io.test.batterytray");
        assert!(satisfies(&fresh, r#"identifier "io.test.batterytray""#));
        assert!(!satisfies(&fresh, r#"identifier "io.test.other""#));
        let ad_hoc = designated_requirement(&running).unwrap();
        assert!(ad_hoc.starts_with("cdhash"), "{ad_hoc}");
        verify_signature(&fresh, &running).unwrap();
        // A certificate the new one isn't signed with
        let wanted =
            r#"identifier "io.test.batterytray" and certificate root = H"0000000000000000000000000000000000000000""#;
        assert!(!satisfies(&fresh, wanted));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_copy_that_macos_runs_from_a_read_only_place_says_so() {
        assert!(translocated(Path::new("/private/var/folders/x/AppTranslocation/1234/d/BatteryTray.app")));
        assert!(!translocated(Path::new("/Applications/BatteryTray.app")));
        // The test binary isn't in a bundle
        assert!(bundle().is_err());
    }
}
