import AppKit
import GameController
import UserNotifications

/// macOS notifications: "charging complete", "updated", "a new version". Allowed by the user the first time one is
/// shown; shown as banners also while the app is in front.
@MainActor
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = Notifier()
    private var asked = false

    func setUp() {
        UNUserNotificationCenter.current().delegate = self
    }

    func post(title: String, text: String) {
        let center = UNUserNotificationCenter.current()
        let send = {
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = text
            center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
        if asked {
            send()
            return
        }
        asked = true
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            if granted { send() }
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
            AppDelegate.shared?.openMain()
            done()
        }
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
