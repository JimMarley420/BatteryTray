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
    /// Notices that came while the answer was awaited
    private var waiting: [(String, String)] = []
    private let legacy: LegacyPosting = LegacyNotices()

    func setUp() {
        UNUserNotificationCenter.current().delegate = self
        // A click on a notice opens the device window
        legacy.setUp { AppDelegate.shared?.openMain() }
    }

    func post(title: String, text: String) {
        switch route {
        case .modern:
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = text
            UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: content,
                                                                         trigger: nil))
        case .legacy:
            legacy.post(title: title, text: text)
        case .asking:
            waiting.append((title, text))
        case .unknown:
            route = .asking
            waiting.append((title, text))
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
        for (title, text) in notices {
            post(title: title, text: text)
        }
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .list])
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
    func post(title: String, text: String)
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
    func post(title: String, text: String) {
        let note = NSUserNotification()
        note.title = title
        note.informativeText = text
        NSUserNotificationCenter.default.deliver(note)
    }
}

/// The reminder's sounds, as the Windows app plays the system's Exclamation and Critical Stop: the user's alert sound,
/// and macOS' "Basso" when the level is critical.
enum Sounds {
    static func play(critical: Bool) {
        if critical, let sound = NSSound(named: NSSound.Name("Basso")) {
            sound.play()
        } else {
            NSSound.beep()
        }
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
        Core.call(["cmd": "external", "readings": readings])
    }
}
