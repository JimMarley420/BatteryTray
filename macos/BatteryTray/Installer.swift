import AppKit

/// Offers, once, to move the app into Applications when it was opened from elsewhere: straight from the disk image,
/// from Downloads, or from the read-only copy macOS runs an app downloaded from the internet from until it is moved
/// (App Translocation). From there it can't update itself, and its login item would point at a place that goes away.
@MainActor
enum Installer {
    private static let askedKey = "MoveToApplicationsAsked"

    /// True when the app is about to start again from Applications (this copy quits).
    static func offerToMove(_ model: AppModel) -> Bool {
        let here = Bundle.main.bundleURL.resolvingSymlinksInPath()
        let homeApps = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications")
        let inApplications = ["/Applications", homeApps.path].contains { here.path.hasPrefix($0 + "/") }
        if inApplications || UserDefaults.standard.bool(forKey: askedKey) {
            return false
        }
        UserDefaults.standard.set(true, forKey: askedKey)

        let alert = NSAlert()
        alert.messageText = model.t("move.title")
        alert.informativeText = model.t("move.text")
        alert.addButton(withTitle: model.t("move.yes"))
        alert.addButton(withTitle: model.t("move.no"))
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return false }

        // Applications when this user may write there (an administrator), else their own Applications folder
        let fm = FileManager.default
        let folder = fm.isWritableFile(atPath: "/Applications") ? URL(fileURLWithPath: "/Applications") : homeApps
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
        // The copy that was opened goes to the Trash too, unless it can't (a disk image, the translocated copy)
        if fm.isDeletableFile(atPath: here.path), !here.path.hasPrefix("/Volumes/"),
           !here.path.contains("/AppTranslocation/") {
            try? fm.trashItem(at: here, resultingItemURL: nil)
        }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.createsNewApplicationInstance = true
        configuration.arguments = ["--moved"]
        // The new copy waits for this one to be gone before it starts for real, so this one doesn't wait for it: it
        // steps aside once the launch is on its way
        NSWorkspace.shared.openApplication(at: target, configuration: configuration) { _, _ in }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { NSApp.terminate(nil) }
        return true
    }
}
