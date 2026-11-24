import AppKit
import Combine
import SwiftUI

/// The app around the core: the menu bar item, the windows, the reminder, the overlay and its shortcut, notices and
/// sounds, the timers, and what the core asks for.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    nonisolated static let openNotification = Notification.Name("io.github.qu1ncky.BatteryTray.open")
    static private(set) weak var shared: AppDelegate?

    let model = AppModel.shared
    private let background: Bool
    private let demo: Bool
    private(set) var statusItem: StatusItemController?
    private var mainWindow: MainWindowController?
    private var settingsWindow: SettingsWindowController?
    private let reminder = ReminderController(test: false)
    private var testReminder: ReminderController?
    let overlay = OverlayController()
    private let gameControllers = GameControllers()
    let bleBatteries = BleBatteries()
    private let deviceWatch = DeviceWatch()
    private var language = ""
    private var alertTimer: Timer?
    private var saveTimer: Timer?
    private var clockTimer: Timer?
    private var quitting = false
    /// `--snapshot`: the folder the windows are drawn into, and the language
    var snapshot: (String, String?)?

    init(background: Bool, demo: Bool) {
        self.background = background
        self.demo = demo
        super.init()
        AppDelegate.shared = self
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        model.perform = { [weak self] effects in self?.perform(effects) }
        let started = Core.call(["cmd": "start", "demo": demo])
        model.loadTexts()
        model.refresh()
        MainMenu.install(model)
        statusItem = StatusItemController(model: model)
        overlay.reminderFrame = { [weak self] in self?.reminder.frame }
        overlay.apply(model.snapshot)
        Notifier.shared.setUp()
        gameControllers.start()
        deviceWatch.start()
        perform(started["effects"] as? [[String: Any]] ?? [])
        // Opened from the disk image or Downloads: once, the offer to move into Applications (this copy then quits)
        if !demo, snapshot == nil, Installer.offerToMove(model) {
            return
        }
        // Bluetooth asks for its permission once, from the copy that stays. The demo's pictures need no Bluetooth
        if !demo {
            bleBatteries.start()
        }
        // Moved into Applications because the login item was turned on from a place that may be gone at the next login
        if !demo, CommandLine.arguments.contains("--autostart") {
            model.send(["cmd": "autostart", "on": true])
        }

        // Snoozes run out between polls
        alertTimer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { _ in
            MainActor.assumeIsolated { AppModel.shared.send(["cmd": "tick"]) }
        }
        DistributedNotificationCenter.default().addObserver(
            forName: AppDelegate.openNotification, object: nil, queue: .main
        ) { _ in
            MainActor.assumeIsolated { AppDelegate.shared?.openMain() }
        }
        model.$snapshot.sink { [weak self] snapshot in self?.snapshotChanged(snapshot) }.store(in: &subscriptions)
        // The login start shows nothing; started any other way, the window opens
        if !background {
            openMain()
        }
        if let (folder, language) = snapshot {
            // Once the demo's devices have been read
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                Snapshots(folder: folder, language: language, app: self).run()
            }
        }
    }

    private var subscriptions = Set<AnyCancellable>()

    /// A double click on the app in Finder while it runs: its window comes up.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        openMain()
        return false
    }

    /// Opened again from Finder, Spotlight or `open` while it runs: macOS only brings a menu bar app to the front, with
    /// nothing to show. As a second launch, the device window comes up. Its own windows activate the app too, but then
    /// one of them is open already.
    func applicationDidBecomeActive(_ notification: Notification) {
        if mainWindow == nil && settingsWindow == nil && !quitting {
            openMain()
        }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        tearDown()
        return .terminateNow
    }

    /// The polling threads or the update service have news.
    func coreHasNews() {
        model.send(["cmd": "pump"])
    }

    private func snapshotChanged(_ snapshot: Snapshot?) {
        guard let snapshot else { return }
        statusItem?.update(snapshot.tray)
        overlay.apply(snapshot)
        if snapshot.lang != language {
            language = snapshot.lang
            MainMenu.install(model)
            mainWindow?.languageChanged()
            settingsWindow?.languageChanged()
        }
    }

    func reportControllers() {
        gameControllers.report()
    }

    /// What the core asks for, in order.
    func perform(_ effects: [[String: Any]]) {
        for effect in effects {
            switch effect["type"] as? String {
            case "notice":
                Notifier.shared.post(title: effect["title"] as? String ?? "", text: effect["text"] as? String ?? "")
            case "sound":
                Sounds.play(critical: effect["critical"] as? Bool ?? false)
            case "alert":
                if effect["test"] as? Bool == true {
                    showTestReminder(effect)
                } else {
                    reminder.show(effect)
                    overlay.place()
                }
            case "alert_close":
                reminder.close()
                overlay.place()
            case "save_later":
                saveTimer?.invalidate()
                saveTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: false) { _ in
                    MainActor.assumeIsolated { _ = Core.call(["cmd": "save"]) }
                }
            case "restart":
                restart(background: effect["background"] as? Bool ?? true)
            default:
                break
            }
        }
    }

    private func showTestReminder(_ effect: [String: Any]) {
        testReminder?.close()
        let sample = ReminderController(test: true)
        sample.show(effect)
        testReminder = sample
    }

    /// Opens the device window (or brings it to the front), with nothing left open in it from before.
    func openMain() {
        if quitting { return }
        if mainWindow == nil {
            mainWindow = MainWindowController(model: model, anchor: statusItem?.buttonFrame) { [weak self] in
                self?.mainWindow = nil
                self?.windowsChanged()
            }
        }
        model.refresh()
        mainWindow?.show()
        windowsChanged()
    }

    func openSettings() {
        if quitting { return }
        if settingsWindow == nil {
            settingsWindow = SettingsWindowController(model: model) { [weak self] in
                self?.settingsWindow = nil
                self?.model.recordingShortcut = false
                self?.windowsChanged()
            }
        }
        model.refresh()
        settingsWindow?.show()
        windowsChanged()
    }

    /// The app stays a menu bar app with its windows open too, out of the Dock: a window is made in the space the user
    /// is in only while the app has no Dock icon, so with one the settings opened from the device window over an app in
    /// full screen went to the desktop, out of sight. While a window is open the device
    /// list's "N min ago" stays fresh; an automatic update waits.
    func windowsChanged() {
        let open = mainWindow != nil || settingsWindow != nil
        if open {
            NSApp.activate(ignoringOtherApps: true)
            if clockTimer == nil {
                clockTimer = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { _ in
                    MainActor.assumeIsolated { AppModel.shared.refresh() }
                }
            }
        } else {
            clockTimer?.invalidate()
            clockTimer = nil
        }
        reportWindows()
    }

    func reportWindows() {
        let open = mainWindow != nil || settingsWindow != nil
        Core.call(["cmd": "windows", "main_open": open, "overlay_timed": overlay.isTimedVisible])
    }

    func quit() {
        NSApp.terminate(nil)
    }

    private func tearDown() {
        if quitting { return }
        quitting = true
        bleBatteries.stop()
        alertTimer?.invalidate()
        saveTimer?.invalidate()
        overlay.hide()
        overlay.release()
        Core.call(["cmd": "quit"])
        reminder.close()
        testReminder?.close()
    }

    /// The new app is in place: it starts once this one has gone (one copy at a time), opening its window if this
    /// one had it open.
    private func restart(background: Bool) {
        let bundle = Bundle.main.bundlePath
        let script = "while kill -0 $1 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open \"$2\" --args $3"
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/bin/sh")
        task.arguments = ["-c", script, "sh", "\(getpid())", bundle, background ? "--background" : ""]
        try? task.run()
        quit()
    }
}
