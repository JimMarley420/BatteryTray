import AppKit
import SwiftUI

/// Everything the windows and the menu bar show, as the core works it out ("snapshot").
struct Snapshot: Decodable, Equatable {
    var lang: String
    var version: String
    var devices: Devices
    var updatedText: String
    var tray: Tray
    var settings: Settings
    var autostart: Bool
    var update: Update
    var overlay: Overlay
    var needsPermission: Bool
}

struct Devices: Decodable, Equatable {
    var pinned: [Device]
    var connected: [Device]
    var offline: [Device]
    var hidden: [Device]
    var noneOnline: Bool
}

/// One device as the list shows it (view.rs: DeviceView).
struct Device: Decodable, Equatable, Identifiable {
    var id: String
    var name: String
    var kind: String
    var subtitle: String
    var connected: Bool
    var pinned: Bool
    var levelText: String
    /// What the level says, which picks its color (`Tone`): "good", "warn", "bad", "charge" or "offline"
    var tone: String
    var charging: Bool
    var indeterminate: Bool
    var fill: Double
    var customName: String
    var ownName: String
    var source: String
    var alerts: Bool
    var low: Int
    var critical: Int
    var ownLevels: Bool
}

struct Tray: Decodable, Equatable {
    var label: String
    var tone: String
    var tooltip: String
    var items: [TrayItem]
}

struct TrayItem: Decodable, Equatable {
    var name: String
    var kind: String
    var level: String
    var tone: String
    var charging: Bool
}

struct Settings: Decodable, Equatable {
    var low: Int
    var critical: Int
    var repeatMinutes: Int
    var sound: Bool
    var notifyCharged: Bool
    var autoClose: Bool
    var autoCloseSeconds: Int
    var autoUpdate: Bool
    var overlayEnabled: Bool
    var overlayMode: Int
    var overlayCorner: Int
    var overlayNames: Bool
    var language: String
    var autostartBannerDismissed: Bool

    enum CodingKeys: String, CodingKey {
        case low, critical, sound, notifyCharged, autoClose, autoCloseSeconds, autoUpdate, overlayEnabled, overlayMode,
             overlayCorner, overlayNames, language, autostartBannerDismissed
        case repeatMinutes = "repeat"
    }
}

struct Update: Decodable, Equatable {
    var status: String
    var checked: String
    var installVisible: Bool
    var installText: String
    var checkEnabled: Bool
    var notes: String
    var bannerVisible: Bool
    var bannerText: String
    var bannerButtonVisible: Bool
}

struct Overlay: Decodable, Equatable {
    var entries: [OverlayEntry]
    var shortcut: Shortcut
}

struct OverlayEntry: Decodable, Equatable, Hashable {
    var kind: String
    var name: String
    var showName: Bool
    var level: String
    var tone: String
    var power: Bool
}

/// The overlay's shortcut: kept as a Win32 key and MOD_* flags, registered by its Mac key code and Carbon modifiers.
struct Shortcut: Decodable, Equatable {
    var key: Int
    var modifiers: Int
    var macKey: Int?
    var carbonModifiers: UInt32
    var caps: [String]
    var label: String
}

/// The app's state for the views: the latest snapshot and the texts of its language. Every change goes to the core,
/// which answers with what to do; then everything is shown afresh.
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    @Published private(set) var snapshot: Snapshot?
    @Published private(set) var texts: [String: String] = [:]
    /// The device whose name is being typed: the list keeps it where it is shown until the typing ends
    @Published var renamingID: String?
    /// The device whose menu is open: one at a time
    @Published var menuID: String?
    /// The shortcut field is recording: pressing the overlay's shortcut does nothing meanwhile
    @Published var recordingShortcut = false
    /// Why the shortcut doesn't work or wasn't taken, until the next change
    @Published var shortcutError = ""

    /// The effects the core asks for; set by the app delegate.
    var perform: ([[String: Any]]) -> Void = { _ in }

    func t(_ key: String) -> String {
        texts[key] ?? key
    }

    /// The text with {0}, {1}... filled in.
    func tf(_ key: String, _ args: Any...) -> String {
        var text = t(key)
        for (i, arg) in args.enumerated() {
            text = text.replacingOccurrences(of: "{\(i)}", with: "\(arg)")
        }
        return text
    }

    /// Asks the core for everything afresh (and for the texts, when the language changed).
    func refresh() {
        guard var fresh = Core.decode(Snapshot.self, ["cmd": "snapshot"]) else { return }
        if texts.isEmpty || fresh.lang != snapshot?.lang {
            loadTexts()
        }
        if let id = renamingID, let old = snapshot {
            fresh.devices = holdRenaming(id, old: old.devices, fresh: fresh.devices)
        }
        if fresh != snapshot {
            snapshot = fresh
        }
    }

    func loadTexts() {
        if let answer = Core.call(["cmd": "texts"])["texts"] as? [String: String] {
            texts = answer
        }
    }

    /// Sends a command and does what the core answers.
    func send(_ command: [String: Any]) {
        let answer = Core.call(command)
        if let effects = answer["effects"] as? [[String: Any]] {
            perform(effects)
        }
        refresh()
    }

    func set(_ key: String, _ value: Any) {
        send(["cmd": "set", "key": key, "value": value])
    }

    /// Keeps the device whose name is being typed in the block it was shown in, at its place there, with its fresh
    /// data: the field stays where the user is typing.
    private func holdRenaming(_ id: String, old: Devices, fresh: Devices) -> Devices {
        let oldBlocks = [old.pinned, old.connected, old.offline]
        guard let home = oldBlocks.firstIndex(where: { $0.contains { $0.id == id } }),
              let index = oldBlocks[home].firstIndex(where: { $0.id == id })
        else { return fresh }
        var blocks = [fresh.pinned, fresh.connected, fresh.offline]
        guard let from = blocks.firstIndex(where: { $0.contains { $0.id == id } }), from != home,
              let at = blocks[from].firstIndex(where: { $0.id == id })
        else { return fresh }
        let device = blocks[from].remove(at: at)
        blocks[home].insert(device, at: min(index, blocks[home].count))
        var held = fresh
        (held.pinned, held.connected, held.offline) = (blocks[0], blocks[1], blocks[2])
        return held
    }
}

/// A level's color by its tone (view.rs: Tone), the same as the Windows app's: in the dark appearance the bright colors
/// the app always had, in the light one colors dark enough to read as text on a light background.
enum Tone {
    /// The tone's color in the light and in the dark appearance, as 0xRRGGBB in sRGB
    private static func shades(_ tone: String) -> (light: Int, dark: Int)? {
        switch tone {
        case "good": return (light: 0x0F7B0F, dark: 0x6CCB5F)
        case "warn": return (light: 0x9D5D00, dark: 0xF7C04A)
        case "bad": return (light: 0xC42B1C, dark: 0xFF6B6B)
        case "charge": return (light: 0x005FB8, dark: 0x60CDFF)
        case "offline": return (light: 0x8A8A8A, dark: 0x7A7A7A)
        default: return nil
        }
    }

    private static func srgb(_ hex: Int) -> NSColor {
        NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }

    /// The tone's color in the appearance it is drawn in, picked each time it is drawn: a window's, or for the menu bar
    /// item's number the menu bar's own, which isn't always the windows' one. An unknown tone is grey.
    static func nsColor(_ tone: String) -> NSColor {
        guard let rgb = shades(tone) else { return .secondaryLabelColor }
        return NSColor(name: nil) { appearance in
            Self.srgb(appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? rgb.dark : rgb.light)
        }
    }

    /// The same for SwiftUI. `alwaysDark`: the dark appearance's color whatever the window's, for the overlay, which is
    /// dark over anything.
    static func color(_ tone: String, alwaysDark: Bool = false) -> Color {
        guard alwaysDark, let rgb = shades(tone) else { return Color(nsColor: nsColor(tone)) }
        return Color(nsColor: srgb(rgb.dark))
    }
}

/// The SF Symbol for a kind of device.
func symbol(_ kind: String) -> String {
    switch kind {
    case "headset": return "headphones"
    case "mouse": return "computermouse"
    case "keyboard": return "keyboard"
    case "gamepad": return "gamecontroller"
    case "phone": return "iphone"
    case "tablet": return "ipad"
    default: return "dot.radiowaves.left.and.right"
    }
}

/// A view's own value, kept across its updates: what @State holds. This SDK makes @State a macro, whose plugin only
/// comes with Xcode; the app builds with the command line tools alone.
final class Local<Value>: ObservableObject {
    @Published var value: Value

    init(_ value: Value) {
        self.value = value
    }
}
