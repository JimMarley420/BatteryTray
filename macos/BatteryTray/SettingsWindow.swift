import AppKit
import SwiftUI

/// The settings, as a Mac app has them: a window of their own (⌘,) with the tabs in its toolbar - General, Reminders,
/// Overlay - each one as tall as its contents.
@MainActor
final class SettingsWindowController: NSObject, NSWindowDelegate {
    private let window: NSWindow
    private let tabs = NSTabViewController()
    private let model: AppModel
    private let closed: () -> Void

    init(model: AppModel, closed: @escaping () -> Void) {
        self.model = model
        self.closed = closed
        tabs.tabStyle = .toolbar
        let pages: [(AnyView, String)] = [
            (AnyView(GeneralSettings()), "gearshape"),
            (AnyView(ReminderSettings()), "bell"),
            (AnyView(OverlaySettings()), "rectangle.inset.topright.filled"),
        ]
        for (page, symbol) in pages {
            let host = NSHostingController(rootView: page.environmentObject(model).frame(width: 500))
            host.sizingOptions = [.preferredContentSize]
            let item = NSTabViewItem(viewController: host)
            item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            tabs.addTabViewItem(item)
        }
        window = NSWindow(contentViewController: tabs)
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        // Comes up where the user is, over an app in full screen too, also when it was opened before in another space
        window.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        super.init()
        window.delegate = self
        languageChanged()
        window.center()
    }

    func show() {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    func languageChanged() {
        // The window's title is the open tab's, which the tab view takes from its page
        let labels = ["general.header", "alerts.header", "overlay.header"].map(model.t)
        for (item, label) in zip(tabs.tabViewItems, labels) {
            item.label = label
            item.viewController?.title = label
        }
        if let selected = tabs.tabViewItems[safe: tabs.selectedTabViewItemIndex] {
            window.title = selected.label
        }
    }

    func windowWillClose(_ notification: Notification) {
        // A value being typed is kept, as when the field is left
        window.makeFirstResponder(nil)
        closed()
    }
}

extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

/// An ⓘ after a label. Its hint opens next to it a moment after the pointer comes (and goes when the pointer leaves),
/// or at once on a click, and then stays until a click on the ⓘ or anywhere else. macOS' own tooltip comes after a
/// second or two and never on a click.
struct Hint: View {
    let text: String
    @StateObject private var state = HintState()

    var body: some View {
        Button { state.click() } label: {
            Image(systemName: "info.circle")
                .foregroundStyle(state.hovering || state.shown ? Color.accentColor : Color.secondary)
                .padding(2)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { state.hover($0) }
        .popover(isPresented: $state.shown, arrowEdge: .bottom) {
            Text(text)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
                .frame(width: 260, alignment: .leading)
                .padding(12)
        }
        .accessibilityLabel(text)
    }
}

/// Whether a hint is open, and whether a click opened it (then the pointer leaving doesn't close it).
@MainActor
final class HintState: ObservableObject {
    private static let delay: TimeInterval = 0.3
    @Published var shown = false {
        didSet {
            if !shown { pinned = false }
        }
    }
    @Published private(set) var hovering = false
    private var pinned = false
    private var pending: DispatchWorkItem?

    func hover(_ inside: Bool) {
        hovering = inside
        pending?.cancel()
        if inside {
            let open = DispatchWorkItem { [weak self] in
                guard let self, self.hovering else { return }
                self.shown = true
            }
            pending = open
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.delay, execute: open)
        } else if !pinned {
            shown = false
        }
    }

    func click() {
        pending?.cancel()
        if shown && pinned {
            shown = false
        } else {
            shown = true
            pinned = true
        }
    }
}

/// A button that is a whole row of a settings list, clicked anywhere on it: its text and symbol in the accent color,
/// as a link's.
struct RowButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(Color.accentColor)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .opacity(configuration.isPressed ? 0.6 : 1)
    }
}

/// General: the login item, the Input Monitoring permission when a device waits for it, the language, the updates, the
/// folder of the log and settings, a suggestion.
struct GeneralSettings: View {
    @EnvironmentObject var model: AppModel

    /// "Checked at 15:41" after a "·" reads "checked at 15:41".
    static func lowerFirst(_ text: String) -> String {
        guard let first = text.first else { return text }
        return first.lowercased() + text.dropFirst()
    }

    var body: some View {
        if let s = model.snapshot {
            Form {
                Section {
                    Toggle(isOn: Binding(get: { s.autostart }, set: { on in
                        if on {
                            Installer.enableAutostart(model)
                        } else {
                            model.send(["cmd": "autostart", "on": false])
                        }
                    })) {
                        Label(model.t("autostart"), systemImage: "power")
                    }
                }
                if s.needsPermission {
                    Section {
                        Label {
                            VStack(alignment: .leading, spacing: 8) {
                                Text(model.t("permission.text")).fixedSize(horizontal: false, vertical: true)
                                HStack {
                                    Button(model.t("permission.allow")) { Permissions.askForInputMonitoring(model) }
                                    Button(model.t("permission.settings")) { Permissions.openInputMonitoringSettings() }
                                }
                            }
                        } icon: {
                            Image(systemName: "hand.raised").foregroundStyle(.orange)
                        }
                    }
                }
                Section {
                    Picker(selection: Binding(get: { s.settings.language }, set: { model.send(["cmd": "language", "value": $0]) })) {
                        Text(model.t("lang.auto")).tag("auto")
                        Text("Русский").tag("ru")
                        Text("English").tag("en")
                        Text("中文").tag("zh")
                    } label: {
                        Label(model.t("lang.header"), systemImage: "globe")
                    }
                }
                Section(model.t("updates.header")) {
                    VStack(alignment: .leading, spacing: 4) {
                        // The version and when it was asked, one line: "You have the latest version 2.2.1 · checked
                        // at 15:41"
                        (Text(s.update.status)
                            + Text(s.update.checked.isEmpty ? "" : " · " + Self.lowerFirst(s.update.checked))
                            .foregroundColor(.secondary))
                            .fixedSize(horizontal: false, vertical: true)
                        if !s.update.notes.isEmpty {
                            Text(s.update.notes)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                                .padding(.top, 4)
                        }
                        HStack {
                            if s.update.installVisible {
                                Button(s.update.installText) { model.send(["cmd": "update_install"]) }
                                    .buttonStyle(.borderedProminent)
                            }
                            Button(model.t("updates.check")) { model.send(["cmd": "update_check"]) }
                                .disabled(!s.update.checkEnabled)
                        }
                        .padding(.top, 4)
                    }
                    Toggle(isOn: Binding(get: { s.settings.autoUpdate }, set: { model.set("auto_update", $0) })) {
                        Text(model.t("updates.auto"))
                    }
                }
                Section {
                    Button {
                        if let path = Core.call(["cmd": "log_folder"])["path"] as? String {
                            try? FileManager.default.createDirectory(atPath: path, withIntermediateDirectories: true)
                            NSWorkspace.shared.open(URL(fileURLWithPath: path))
                        }
                    } label: {
                        Label(model.t("open.folder"), systemImage: "folder")
                    }
                    Button {
                        if let text = Core.call(["cmd": "suggestion_url"])["url"] as? String, let url = URL(string: text) {
                            NSWorkspace.shared.open(url)
                        }
                    } label: {
                        Label {
                            HStack(spacing: 4) {
                                Text(model.t("feedback"))
                                Image(systemName: "arrow.up.forward.square")
                                    .foregroundStyle(.secondary)
                                    .accessibilityLabel(model.t("feedback.browser"))
                            }
                        } icon: {
                            Image(systemName: "lightbulb")
                        }
                    }
                }
                .buttonStyle(RowButtonStyle())
            }
            .formStyle(.grouped)
            .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// Reminders: the low and critical levels and how often a reminder comes back, whether it closes on its own and after
/// how many seconds, its sound, the "charged" notice, a sample.
struct ReminderSettings: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        if let s = model.snapshot?.settings {
            Form {
                Section {
                    LevelSetting(title: model.t("alerts.low"), hint: nil, value: s.low, range: 10...50,
                                 unit: "%", current: { model.snapshot?.settings.low ?? s.low }) { model.set("low", $0) }
                    LevelSetting(title: model.t("alerts.critical"), hint: model.t("alerts.critical.hint"),
                                 value: s.critical, range: 5...45, unit: "%",
                                 current: { model.snapshot?.settings.critical ?? s.critical }) { model.set("critical", $0) }
                    LevelSetting(title: model.t("alerts.repeat"), hint: nil, value: s.repeatMinutes, range: 1...60,
                                 unit: model.t("alerts.repeat.unit"), step: 1,
                                 current: { model.snapshot?.settings.repeatMinutes ?? s.repeatMinutes }) { model.set("repeat", $0) }
                }
                Section {
                    Toggle(model.t("alerts.autoclose"), isOn: Binding(get: { s.autoClose }, set: { model.set("auto_close", $0) }))
                    if s.autoClose {
                        LevelSetting(title: model.t("alerts.autoclose.after"), hint: nil, value: s.autoCloseSeconds,
                                     range: 3...15, unit: model.t("alerts.autoclose.unit"), step: 1,
                                     current: { model.snapshot?.settings.autoCloseSeconds ?? s.autoCloseSeconds }) {
                            model.set("auto_close_seconds", $0)
                        }
                    }
                }
                Section {
                    Toggle(model.t("alerts.sound"), isOn: Binding(get: { s.sound }, set: { model.set("sound", $0) }))
                    Toggle(isOn: Binding(get: { s.notifyCharged }, set: { model.set("notify_charged", $0) })) {
                        HStack(spacing: 6) {
                            Text(model.t("charged.toggle"))
                            Hint(text: model.t("charged.hint"))
                        }
                    }
                }
                Section {
                    Button {
                        model.send(["cmd": "test_alert"])
                    } label: {
                        Label(model.t("alerts.test"), systemImage: "bell.badge")
                    }
                    .buttonStyle(RowButtonStyle())
                }
            }
            .formStyle(.grouped)
            .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// A level: the number as typed (accent colored) and a slider that moves in steps (of 5 unless told otherwise). The
/// field takes more than the slider covers (settings.rs keeps a value in what it accepts: LEVEL_PERCENT,
/// REPEAT_MINUTES, AUTO_CLOSE_SECONDS); the slider then stands at its end.
struct LevelSetting: View {
    let title: String
    let hint: String?
    let value: Int
    let range: ClosedRange<Int>
    let unit: String
    var step = 5
    let current: () -> Int
    let change: (Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(title)
                if let hint {
                    Hint(text: hint)
                }
                Spacer()
                NumberField(value: value, unit: unit, label: title, current: current, commit: change)
                    .foregroundStyle(Color.accentColor)
            }
            let position = Binding(
                get: { Double(min(max(value, range.lowerBound), range.upperBound)) },
                set: { (fresh: Double) in
                    let stepped = Int(fresh.rounded())
                    if stepped != current() { change(stepped) }
                })
            let bounds = Double(range.lowerBound)...Double(range.upperBound)
            Group {
                if step > 1 {
                    Slider(value: position, in: bounds, step: Double(step)) { Text(title) }
                } else {
                    // A step of 1 would draw a tick mark for every value: the slider runs smoothly instead and the
                    // value is the nearest whole number
                    Slider(value: position, in: bounds) { Text(title) }
                }
            }
            .labelsHidden()
        }
    }
}

/// Overlay: the shortcut that shows the battery panel over any app, how long it stays, its corner, the names.
struct OverlaySettings: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        if let snapshot = model.snapshot {
            let s = snapshot.settings
            Form {
                Section {
                    Toggle(isOn: Binding(get: { s.overlayEnabled }, set: { model.set("overlay_enabled", $0) })) {
                        HStack(spacing: 6) {
                            Text(model.t("overlay.enabled"))
                            Hint(text: model.t("overlay.compatibility"))
                        }
                    }
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Text(model.t("overlay.shortcut"))
                            Spacer()
                            ShortcutRecorder(shortcut: snapshot.overlay.shortcut)
                            Button {
                                ShortcutRecorder.take(Core.decode(Shortcut.self, ["cmd": "default_shortcut"]))
                            } label: {
                                Image(systemName: "arrow.uturn.backward")
                            }
                            .help(model.t("overlay.reset"))
                            .accessibilityLabel(model.t("overlay.reset"))
                        }
                        if !model.shortcutError.isEmpty {
                            Text(model.shortcutError)
                                .font(.caption)
                                .foregroundStyle(.red)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    Picker(model.t("overlay.mode"), selection: Binding(get: { s.overlayMode }, set: { model.set("overlay_mode", $0) })) {
                        Text(model.t("overlay.timed")).tag(0)
                        Text(model.t("overlay.toggle")).tag(1)
                    }
                    Picker(model.t("overlay.corner"), selection: Binding(get: { s.overlayCorner }, set: { model.set("overlay_corner", $0) })) {
                        Text(model.t("overlay.top.left")).tag(0)
                        Text(model.t("overlay.top.right")).tag(1)
                        Text(model.t("overlay.bottom.left")).tag(2)
                        Text(model.t("overlay.bottom.right")).tag(3)
                    }
                    Toggle(model.t("overlay.names"), isOn: Binding(get: { s.overlayNames }, set: { model.set("overlay_names", $0) }))
                }
                Section {
                    Button {
                        AppDelegate.shared?.overlay.preview()
                    } label: {
                        Label(model.t("overlay.preview"), systemImage: "eye")
                    }
                    .buttonStyle(RowButtonStyle())
                }
            }
            .formStyle(.grouped)
            .fixedSize(horizontal: false, vertical: true)
        }
    }
}
