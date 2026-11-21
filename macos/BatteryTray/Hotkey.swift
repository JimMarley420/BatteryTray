import AppKit
import Carbon.HIToolbox
import SwiftUI

/// The overlay's global shortcut, through Carbon's RegisterEventHotKey: it works whatever app is in front and needs no
/// permission. As on Windows, a new shortcut is registered before the old one is released (ids 1 and 2 take turns),
/// so a combination another app holds never costs the working one.
@MainActor
final class HotkeyBinding {
    private(set) var active: Shortcut?
    private var activeID: UInt32 = 0
    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    /// Called with the id of the shortcut pressed
    var pressed: (UInt32) -> Void = { _ in }

    private static let signature: OSType = 0x4254_7279 // "BTry"

    init() {
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, event, _ in
            var id = EventHotKeyID()
            let status = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                                           nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
            if status == noErr && id.signature == HotkeyBinding.signature {
                let number = id.id
                DispatchQueue.main.async { AppDelegate.shared?.overlay.hotkeyPressed(number) }
            }
            return noErr
        }, 1, &spec, nil, &handler)
    }

    /// Whether a press with this id is the current shortcut (not one queued before a change).
    func isCurrent(_ id: UInt32) -> Bool {
        active != nil && id == activeID
    }

    /// Takes the shortcut; false when it can't be registered (the previous one is kept).
    func tryChange(_ shortcut: Shortcut) -> Bool {
        guard let key = shortcut.macKey else { return false }
        if active.map({ $0.key == shortcut.key && $0.modifiers == shortcut.modifiers }) == true {
            return true
        }
        let next: UInt32 = activeID == 1 ? 2 : 1
        var fresh: EventHotKeyRef?
        let status = RegisterEventHotKey(UInt32(key), shortcut.carbonModifiers,
                                         EventHotKeyID(signature: HotkeyBinding.signature, id: next),
                                         GetApplicationEventTarget(), 0, &fresh)
        guard status == noErr, let fresh else {
            Core.log("overlay shortcut \(shortcut.label) not registered: \(status)")
            return false
        }
        if let ref {
            UnregisterEventHotKey(ref)
        }
        ref = fresh
        activeID = next
        active = shortcut
        return true
    }

    func clear() {
        if let ref {
            UnregisterEventHotKey(ref)
        }
        ref = nil
        active = nil
    }
}

/// The shortcut field: the combination as key caps; clicked, it records the next one pressed (Esc stops, Tab leaves),
/// which the app takes when it is valid and free.
struct ShortcutRecorder: View {
    @EnvironmentObject var model: AppModel
    let shortcut: Shortcut
    @StateObject private var monitor = Local<Any?>(nil)

    var body: some View {
        let recording = model.recordingShortcut
        Button {
            recording ? stop() : start()
        } label: {
            HStack(spacing: 3) {
                if recording {
                    Text(model.t("overlay.recording")).foregroundStyle(.secondary)
                } else {
                    ForEach(Array(shortcut.caps.enumerated()), id: \.offset) { _, cap in
                        Text(cap)
                            .font(.system(size: 12, weight: .medium))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(RoundedRectangle(cornerRadius: 4).fill(Color(nsColor: .controlBackgroundColor)))
                            .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(Color(nsColor: .separatorColor)))
                    }
                }
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 3)
            .frame(minWidth: 120)
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(recording ? Color.accentColor : .clear, lineWidth: 2))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(model.t("overlay.shortcut")): \(shortcut.label)")
        .onDisappear(perform: stop)
    }

    private func start() {
        model.shortcutError = ""
        model.recordingShortcut = true
        // Local monitors run on the main thread
        monitor.value = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [self] event in
            let (key, flags) = (Int(event.keyCode), event.modifierFlags.rawValue)
            let passOn = MainActor.assumeIsolated { record(key: key, flags: NSEvent.ModifierFlags(rawValue: flags)) }
            return passOn ? event : nil
        }
    }

    private func stop() {
        if let installed = monitor.value {
            NSEvent.removeMonitor(installed)
        }
        monitor.value = nil
        model.recordingShortcut = false
    }

    /// A key pressed while recording; nothing reaches the window meanwhile but Tab. True: the key goes on.
    private func record(key: Int, flags: NSEvent.ModifierFlags) -> Bool {
        switch key {
        case kVK_Escape:
            stop()
            return false
        case kVK_Tab where flags.intersection([.command, .control, .option]).isEmpty:
            stop()
            return true
        default:
            break
        }
        let answer = Core.call([
            "cmd": "check_shortcut", "mac_key": key,
            "command": flags.contains(.command), "control": flags.contains(.control),
            "option": flags.contains(.option), "shift": flags.contains(.shift),
        ])
        guard answer["valid"] as? Bool == true, let data = try? JSONSerialization.data(withJSONObject: answer["shortcut"] ?? [:])
        else {
            model.shortcutError = answer["error"] as? String ?? model.t("overlay.invalid")
            return false
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        if Self.take(try? decoder.decode(Shortcut.self, from: data)) {
            // The field gives up recording, so its key caps show what was just taken
            stop()
        }
        return false
    }

    /// Takes a shortcut if it can be registered (checked even while the overlay is off, then released again); says
    /// why not otherwise. True when it was taken.
    @MainActor
    @discardableResult
    static func take(_ shortcut: Shortcut?) -> Bool {
        let model = AppModel.shared
        guard let shortcut, let overlay = AppDelegate.shared?.overlay else { return false }
        guard overlay.binding.tryChange(shortcut) else {
            model.shortcutError = model.t("overlay.conflict")
            return false
        }
        model.shortcutError = ""
        model.send(["cmd": "set_shortcut", "key": shortcut.key, "modifiers": shortcut.modifiers])
        return true
    }
}
