import AppKit
import GameController
import UserNotifications

/// macOS notifications: "charging complete", "updated", "a new version". Allowed by the user the first time one is
/// shown; shown as banners also while the app is in front. macOS gives UNUserNotificationCenter only to apps signed
/// with a Developer ID, and BatteryTray is signed ad hoc: when it is refused, the notices go through the older
/// NSUserNotificationCenter, which still shows them.
@MainActor
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = Notifier()
    private enum Route { case unknown, asking, modern, legacy }
    private var route = Route.unknown
    /// Notices that came while the answer was awaited: title, text, and whether with the system's sound
    private var waiting: [(String, String, Bool)] = []
    private let legacy: LegacyPosting = LegacyNotices()

    func setUp() {
        UNUserNotificationCenter.current().delegate = self
        // A click on a notice opens the device window
        legacy.setUp { AppDelegate.shared?.openMain() }
    }

    /// `systemSound`: with the sound macOS gives a notification, which the system plays or not as the user set it for
    /// the app in System Settings and as a Focus has it. Without it the notice is silent, as the app's notices were
    /// before a sound could be chosen (any other sound is played by `Sounds`).
    func post(title: String, text: String, systemSound: Bool) {
        switch route {
        case .modern:
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = text
            if systemSound {
                content.sound = .default
            }
            UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: content,
                                                                         trigger: nil))
        case .legacy:
            legacy.post(title: title, text: text, systemSound: systemSound)
        case .asking:
            waiting.append((title, text, systemSound))
        case .unknown:
            route = .asking
            waiting.append((title, text, systemSound))
            UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { granted, error in
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { Notifier.shared.answered(granted, error?.localizedDescription) }
                }
            }
        }
    }

    private func answered(_ granted: Bool, _ error: String?) {
        route = granted ? .modern : .legacy
        Core.log("notifications: \(granted ? "allowed" : "through NSUserNotificationCenter")\(error.map { " (\($0))" } ?? "")")
        let notices = waiting
        waiting = []
        for (title, text, systemSound) in notices {
            post(title: title, text: text, systemSound: systemSound)
        }
    }

    /// Shown, and heard when the notice has a sound, also while the app is in front.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .list, .sound])
    }

    /// A click on a notice opens the device window.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler done: @escaping () -> Void) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { AppDelegate.shared?.openMain() }
            done()
        }
    }
}

/// The older notification center. Its calls are deprecated; reached through this protocol, their deprecation stays
/// inside `LegacyNotices`.
@MainActor
protocol LegacyPosting {
    func setUp(clicked: @escaping () -> Void)
    func post(title: String, text: String, systemSound: Bool)
}

@MainActor
struct LegacyNotices: LegacyPosting {
    /// The center's delegate, which it doesn't keep itself
    private static var delegate: AnyObject?

    @available(macOS, deprecated: 11.0)
    func setUp(clicked: @escaping () -> Void) {
        /// Shown also while the app is in front; a click opens the device window.
        final class Delegate: NSObject, NSUserNotificationCenterDelegate {
            let clicked: () -> Void

            init(clicked: @escaping () -> Void) {
                self.clicked = clicked
            }

            func userNotificationCenter(_ center: NSUserNotificationCenter, shouldPresent notification: NSUserNotification)
                -> Bool {
                true
            }

            func userNotificationCenter(_ center: NSUserNotificationCenter, didActivate notification: NSUserNotification) {
                clicked()
            }
        }
        let delegate = Delegate(clicked: clicked)
        Self.delegate = delegate
        NSUserNotificationCenter.default.delegate = delegate
    }

    @available(macOS, deprecated: 11.0)
    func post(title: String, text: String, systemSound: Bool) {
        let note = NSUserNotification()
        note.title = title
        note.informativeText = text
        if systemSound {
            note.soundName = NSUserNotificationDefaultSoundName
        }
        NSUserNotificationCenter.default.deliver(note)
    }
}

/// The sounds of the reminder and of the "charged" notice, as the core hands them over ("sound"). The system's own:
/// the user's alert sound, and macOS' "Basso" at the critical level, as the Windows app plays the system's Exclamation
/// and Critical Stop. Or a file - one of macOS' own sounds by name, or one the user added - for as many seconds as the
/// core says, 10 at most. One sound at a time; the reminder's ends when the reminder closes.
@MainActor
enum Sounds {
    private static var playing: NSSound?
    private static var limit: Timer?

    static func play(_ effect: [String: Any]) {
        stop()
        guard let path = effect["path"] as? String,
              let sound = NSSound(contentsOfFile: path, byReference: true),
              sound.play()
        else {
            // The system's sound, also for a file that can't be played
            if effect["event"] as? String == "critical", let basso = NSSound(named: NSSound.Name("Basso")) {
                basso.play()
            } else {
                NSSound.beep()
            }
            return
        }
        playing = sound
        let seconds = effect["seconds"] as? Double ?? 10
        limit = Timer.scheduledTimer(withTimeInterval: seconds, repeats: false) { _ in
            MainActor.assumeIsolated { Sounds.stop() }
        }
    }

    static func stop() {
        limit?.invalidate()
        limit = nil
        playing?.stop()
        playing = nil
    }
}

/// The game controllers macOS knows (Xbox, Switch Pro and others over Bluetooth) and their levels, reported to the core
/// every minute and whenever one comes or goes. PlayStation controllers are left to the core, which reads them itself.
@MainActor
final class GameControllers {
    private var timer: Timer?

    func start() {
        GCController.shouldMonitorBackgroundEvents = true
        for name in [NSNotification.Name.GCControllerDidConnect, .GCControllerDidDisconnect] {
            NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { AppDelegate.shared?.reportControllers() }
            }
        }
        timer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { _ in
            MainActor.assumeIsolated { AppDelegate.shared?.reportControllers() }
        }
        report()
    }

    func report() {
        var readings: [[String: Any]] = []
        var seen: [String: Int] = [:]
        for controller in GCController.controllers() {
            guard let battery = controller.battery else { continue }
            let category = controller.productCategory
            if category.localizedCaseInsensitiveContains("DualSense") || category.localizedCaseInsensitiveContains("DualShock") {
                continue
            }
            let name = controller.vendorName ?? category
            let slug = name.lowercased().map { $0.isLetter || $0.isNumber ? $0 : "-" }
            var id = "gc:" + String(slug)
            seen[id, default: 0] += 1
            if let n = seen[id], n > 1 {
                id += "-\(n)"
            }
            var reading: [String: Any] = [
                "id": id, "name": name,
                "charging": battery.batteryState == .charging, "full": battery.batteryState == .full,
            ]
            if battery.batteryState != .unknown || battery.batteryLevel > 0 {
                reading["level"] = Int((battery.batteryLevel * 100).rounded())
            }
            readings.append(reading)
        }
        Core.call(["cmd": "external", "source": "controllers", "readings": readings])
    }
}
