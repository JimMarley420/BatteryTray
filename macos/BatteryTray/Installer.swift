import AppKit

/// Offers to move the app into Applications when it was opened from elsewhere: straight from the disk image, from
/// Downloads, or from the read-only copy macOS runs an app downloaded from the internet from until it is moved (App
/// Translocation). From there it can't update itself, and its login item would point at a place that goes away. Asked
/// once, at the first start; and again when the login item is turned on from a place that may be gone at the next login.
@MainActor
enum Installer {
    private static let askedKey = "MoveToApplicationsAsked"
    private static let quarantine = "com.apple.quarantine"

    private static var here: URL {
        Bundle.main.bundleURL.resolvingSymlinksInPath()
    }

    private static var homeApplications: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications")
    }

    private static var inApplications: Bool {
        ["/Applications", homeApplications.path].contains { here.path.hasPrefix($0 + "/") }
    }

    /// The app runs from a place that may be gone at the next login: the translocated copy (a new random folder at
    /// every start), a disk image, another drive.
    static var runsFromTemporaryPlace: Bool {
        here.path.contains("/AppTranslocation/") || here.path.hasPrefix("/Volumes/")
    }

    /// At the first start: true when the app is about to start again from Applications (this copy quits).
    static func offerToMove(_ model: AppModel) -> Bool {
        if inApplications || UserDefaults.standard.bool(forKey: askedKey) {
            return false
        }
        UserDefaults.standard.set(true, forKey: askedKey)
        return move(model, arguments: [])
    }

    /// "Open at login" turned on. From a place that may be gone at the next login the app first moves into
    /// Applications, and the moved copy turns it on (`--autostart`); declined, it stays off.
    static func enableAutostart(_ model: AppModel) {
        if runsFromTemporaryPlace && !inApplications {
            _ = move(model, arguments: ["--autostart"])
        } else {
            model.send(["cmd": "autostart", "on": true])
        }
    }

    /// Asks, copies the app into Applications and starts the copy with these arguments; true when this copy quits.
    private static func move(_ model: AppModel, arguments: [String]) -> Bool {
        let alert = NSAlert()
        alert.messageText = model.t("move.title")
        alert.informativeText = model.t("move.text")
        alert.addButton(withTitle: model.t("move.yes"))
        alert.addButton(withTitle: model.t("move.no"))
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return false }

        // Applications when this user may write there (an administrator), else their own Applications folder
        let fm = FileManager.default
        let folder = fm.isWritableFile(atPath: "/Applications") ? URL(fileURLWithPath: "/Applications") : homeApplications
        let target = folder.appendingPathComponent(here.lastPathComponent)
        do {
            try fm.createDirectory(at: folder, withIntermediateDirectories: true)
            if fm.fileExists(atPath: target.path) {
                // An older copy: to the Trash, where it can still be taken back from
                try fm.trashItem(at: target, resultingItemURL: nil)
            }
            try fm.copyItem(at: here, to: target)
        } catch {
            let failed = NSAlert()
            failed.messageText = model.tf("move.failed", error.localizedDescription)
            failed.runModal()
            return false
        }
        // The copy keeps the quarantine of the download, and macOS would run it from a read-only place again, where it
        // can't update itself. The user has opened this app already, so its copy needs no more checks
        removeQuarantine(target)
        // The copy that was opened goes to the Trash too, unless it can't (a disk image, the translocated copy)
        let source = here
        if fm.isDeletableFile(atPath: source.path), !source.path.hasPrefix("/Volumes/"),
           !source.path.contains("/AppTranslocation/") {
            try? fm.trashItem(at: source, resultingItemURL: nil)
        }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.createsNewApplicationInstance = true
        configuration.arguments = ["--moved"] + arguments
        // The new copy waits for this one to be gone before it starts for real, so this one doesn't wait for it: it
        // steps aside once the launch is on its way
        NSWorkspace.shared.openApplication(at: target, configuration: configuration) { _, _ in }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { NSApp.terminate(nil) }
        return true
    }

    /// Takes the quarantine attribute off a bundle and everything in it.
    static func removeQuarantine(_ bundle: URL) {
        var paths = [bundle.path]
        if let inside = FileManager.default.enumerator(atPath: bundle.path) {
            for case let relative as String in inside {
                paths.append(bundle.appendingPathComponent(relative).path)
            }
        }
        for path in paths {
            removexattr(path, quarantine, XATTR_NOFOLLOW)
        }
    }
}
