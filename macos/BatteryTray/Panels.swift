import AppKit
import SwiftUI

/// A window that floats above other apps' windows and full-screen apps, on every Space, and never takes the focus or
/// activates the app: the reminder and the overlay.
final class FloatingPanel: NSPanel {
    init(clickThrough: Bool) {
        super.init(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        isFloatingPanel = true
        level = .statusBar
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        hidesOnDeactivate = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        ignoresMouseEvents = clickThrough
        isReleasedWhenClosed = false
        becomesKeyOnlyIfNeeded = true
        animationBehavior = .utilityWindow
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    /// Takes the size its contents want.
    func fit(_ host: NSView) {
        let size = host.fittingSize
        setContentSize(size)
        host.frame = NSRect(origin: .zero, size: size)
    }
}

// MARK: - The reminder

/// What the reminder shows.
final class ReminderState: ObservableObject {
    @Published var effect: [String: Any] = [:]
}

/// The low battery reminder: in the top right corner, where macOS shows its notifications, on top of everything and
/// never taking the focus, until the user snoozes it or mutes it. The sample (`test`) only closes.
@MainActor
final class ReminderController {
    private let test: Bool
    private var panel: FloatingPanel?
    private let state = ReminderState()
    private var keepOnTop: Timer?

    init(test: Bool) {
        self.test = test
    }

    /// Where it is, for the overlay to stay clear of it.
    var frame: NSRect? {
        panel.flatMap { $0.isVisible ? $0.frame : nil }
    }

    func show(_ effect: [String: Any]) {
        state.effect = effect
        let panel = self.panel ?? makePanel()
        self.panel = panel
        if let host = panel.contentView {
            panel.fit(host)
        }
        place()
        panel.orderFrontRegardless()
        // Full-screen apps and other floating windows can push it back; reasserted every few seconds
        keepOnTop?.invalidate()
        keepOnTop = Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.panel?.orderFrontRegardless()
                self?.place()
            }
        }
    }

    func close() {
        keepOnTop?.invalidate()
        keepOnTop = nil
        panel?.orderOut(nil)
    }

    private func makePanel() -> FloatingPanel {
        let panel = FloatingPanel(clickThrough: false)
        let view = ReminderView(state: state, test: test) { [weak self] snooze in
            guard let self else { return }
            if self.test {
                self.close()
            } else {
                AppModel.shared.send(["cmd": snooze ? "snooze" : "mute"])
            }
        }
        let host = NSHostingView(rootView: view.environmentObject(AppModel.shared))
        panel.contentView = host
        return panel
    }

    /// The top right corner of the main screen's free area (16 points in), as tall as its contents.
    private func place() {
        guard let panel, let host = panel.contentView, let area = NSScreen.main?.visibleFrame else { return }
        panel.fit(host)
        let size = panel.frame.size
        panel.setFrameOrigin(NSPoint(x: area.maxX - size.width - 16, y: area.maxY - size.height - 16))
    }
}

struct ReminderView: View {
    @ObservedObject var state: ReminderState
    let test: Bool
    let answer: (Bool) -> Void
    /// The levels' colors, as the Windows app has them
    static let warn = Color(red: 0xF7 / 255, green: 0xC0 / 255, blue: 0x4A / 255)
    static let bad = Color(red: 1, green: 0x6B / 255, blue: 0x6B / 255)

    var body: some View {
        let critical = state.effect["critical"] as? Bool ?? false
        let accent: Color = critical ? Self.bad : Self.warn
        let items = state.effect["items"] as? [[String: Any]] ?? []
        HStack(spacing: 0) {
            Rectangle().fill(accent).frame(width: 4)
            // Only what is due and the answers: a header with the cross (which snoozes) over the levels, the devices
            // with their levels, the two buttons
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Spacer()
                    Button { answer(true) } label: {
                        // The glyph ends where the levels end
                        Image(systemName: "xmark")
                            .font(.system(size: 11, weight: .semibold))
                            .frame(width: 24, height: 24, alignment: .trailing)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.borderless)
                    .help(AppModel.shared.t("alert.snooze.tip"))
                    .accessibilityLabel(AppModel.shared.t("alert.snooze.tip"))
                }
                VStack(spacing: 6) {
                    ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                        HStack(spacing: 10) {
                            Image(systemName: symbol(item["kind"] as? String ?? ""))
                                .font(.system(size: 15))
                                .frame(width: 24)
                            Text(item["name"] as? String ?? "").lineLimit(1).truncationMode(.tail)
                            Spacer()
                            Text(item["level"] as? String ?? "")
                                .font(.system(size: 18, weight: .semibold))
                                .monospacedDigit()
                                .foregroundStyle(item["critical"] as? Bool == true ? Self.bad : Self.warn)
                        }
                    }
                }
                .padding(.top, 2)
                VStack(spacing: 6) {
                    Button { answer(true) } label: {
                        Text(state.effect["snooze"] as? String ?? "").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    Button { answer(false) } label: {
                        Text(AppModel.shared.t("alert.mute")).frame(maxWidth: .infinity)
                    }
                    .controlSize(.large)
                }
                .padding(.top, 16)
            }
            .padding(EdgeInsets(top: 8, leading: 16, bottom: 16, trailing: 14))
        }
        .frame(width: 340)
        .background(VisualEffect())
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(accent.opacity(0.9), lineWidth: 1))
    }
}

/// The blurred material of macOS' own floating panels.
struct VisualEffect: NSViewRepresentable {
    var material: NSVisualEffectView.Material = .popover

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = material
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {
        view.material = material
    }
}

// MARK: - The overlay

/// Visibility on a monotonic clock, as on Windows: mode 0 shows for 5 seconds (again from the start on each press),
/// mode 1 toggles; a preview always times out.
struct OverlaySession {
    static let duration: TimeInterval = 5
    private(set) var visible = false
    private(set) var timed = false
    private var started: TimeInterval = 0

    var isTimed: Bool { visible && timed }

    mutating func press(mode: Int, now: TimeInterval) {
        if mode == 1 && visible {
            visible = false
        } else {
            show(expire: mode != 1, now: now)
        }
    }

    mutating func preview(now: TimeInterval) {
        show(expire: true, now: now)
    }

    private mutating func show(expire: Bool, now: TimeInterval) {
        visible = true
        timed = expire
        started = now
    }

    mutating func hide() {
        visible = false
    }

    mutating func expire(now: TimeInterval) {
        if visible && timed && now - started >= Self.duration {
            visible = false
        }
    }
}

final class OverlayState: ObservableObject {
    @Published var entries: [OverlayEntry] = []
    @Published var rowLimit: CGFloat = 480
}

/// The battery overlay: a click-through panel on the screen of the app in front, in its corner, shown by the global
/// shortcut for 5 seconds or until pressed again; it stays clear of the reminder.
@MainActor
final class OverlayController {
    let binding = HotkeyBinding()
    private var session = OverlaySession()
    private var panel: FloatingPanel?
    private let state = OverlayState()
    private var expiry: Timer?
    private var placement: Timer?
    private var mode = 0
    private var corner = 1
    private var enabled = false
    /// The reminder's frame, which only the app knows
    var reminderFrame: () -> NSRect? = { nil }

    var isTimedVisible: Bool { session.isTimed }

    /// The settings as the snapshot has them: the shortcut registered or released (and why it doesn't work), a changed
    /// mode or turning it off hides the panel; the contents follow the devices.
    func apply(_ snapshot: Snapshot?) {
        guard let snapshot else { return }
        let s = snapshot.settings
        if mode != s.overlayMode || (enabled && !s.overlayEnabled) {
            hide()
        }
        (mode, corner, enabled) = (s.overlayMode, s.overlayCorner, s.overlayEnabled)
        let model = AppModel.shared
        if enabled {
            if !binding.tryChange(snapshot.overlay.shortcut), model.shortcutError.isEmpty {
                model.shortcutError = model.t("overlay.unavailable")
            }
        } else {
            binding.clear()
            if model.shortcutError == model.t("overlay.unavailable") {
                model.shortcutError = ""
            }
        }
        if state.entries != snapshot.overlay.entries {
            state.entries = snapshot.overlay.entries
        }
        place()
    }

    func hotkeyPressed(_ id: UInt32) {
        // While the shortcut is being recorded, pressing it does nothing
        guard !AppModel.shared.recordingShortcut, binding.isCurrent(id) else { return }
        session.press(mode: mode, now: ProcessInfo.processInfo.systemUptime)
        present()
    }

    func preview() {
        session.preview(now: ProcessInfo.processInfo.systemUptime)
        present()
    }

    private func present() {
        guard session.visible else { return hide() }
        let panel = self.panel ?? makePanel()
        self.panel = panel
        if !panel.isVisible {
            // On the screen of the app in front, where the user is looking
            let screen = activeScreen()
            state.rowLimit = rowLimit(screen.visibleFrame.width)
            place(on: screen)
            panel.alphaValue = 0.93
            panel.orderFrontRegardless()
        }
        expiry?.invalidate()
        if session.isTimed {
            expiry = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.tick() }
            }
        }
        // Device lists change size and displays come and go: placed again every few seconds while shown
        placement?.invalidate()
        placement = Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.panel?.orderFrontRegardless()
                self?.place()
            }
        }
        AppDelegate.shared?.reportWindows()
    }

    private func tick() {
        session.expire(now: ProcessInfo.processInfo.systemUptime)
        if !session.visible {
            hide()
        }
    }

    func hide() {
        let was = session.isTimed
        session.hide()
        expiry?.invalidate()
        placement?.invalidate()
        panel?.orderOut(nil)
        if was {
            AppDelegate.shared?.reportWindows()
        }
    }

    /// Lets the shortcut go (the app is quitting).
    func release() {
        binding.clear()
    }

    private func makePanel() -> FloatingPanel {
        let panel = FloatingPanel(clickThrough: true)
        panel.hasShadow = false
        panel.contentView = NSHostingView(rootView: OverlayView(state: state).environmentObject(AppModel.shared))
        return panel
    }

    /// Into its corner of the screen it is on, off the reminder.
    func place() {
        guard let panel, panel.isVisible else { return }
        let screen = NSScreen.screens.first { $0.frame.contains(NSPoint(x: panel.frame.midX, y: panel.frame.midY)) }
        place(on: screen ?? activeScreen())
    }

    private func place(on screen: NSScreen) {
        guard let panel, let host = panel.contentView else { return }
        panel.fit(host)
        let origin = OverlayPlacement.place(area: screen.visibleFrame, size: panel.frame.size, corner: corner,
                                            margin: 12, obstruction: reminderFrame())
        panel.setFrameOrigin(origin)
    }

    /// The screen of the frontmost app's frontmost window; the one with the pointer when that can't be told.
    private func activeScreen() -> NSScreen {
        let pointer = NSScreen.screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? NSScreen.main ?? NSScreen.screens[0]
        guard let app = NSWorkspace.shared.frontmostApplication,
              let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
                as? [[String: Any]],
              let window = windows.first(where: {
                  ($0[kCGWindowOwnerPID as String] as? pid_t) == app.processIdentifier
                      && ($0[kCGWindowLayer as String] as? Int) == 0
              }),
              let bounds = window[kCGWindowBounds as String] as? [String: CGFloat],
              let primary = NSScreen.screens.first
        else { return pointer }
        // Window bounds count down from the top of the main display; screens count up from its bottom
        let center = NSPoint(x: (bounds["X"] ?? 0) + (bounds["Width"] ?? 0) / 2,
                             y: primary.frame.maxY - ((bounds["Y"] ?? 0) + (bounds["Height"] ?? 0) / 2))
        return NSScreen.screens.first { $0.frame.contains(center) } ?? pointer
    }

    /// The widest a row of entries gets: half the screen, as on Windows (never under 240, never past the screen), less
    /// the panel's padding.
    private func rowLimit(_ width: CGFloat) -> CGFloat {
        min(width - 24, max(width * 0.5, 240)) - 12
    }
}

/// Where the overlay goes: its corner of the free area inside a margin, moved off the reminder when they would overlap
/// (to the nearest place that fits). As overlay::place on Windows, in a Mac screen's coordinates (y counts up).
enum OverlayPlacement {
    static func place(area: NSRect, size: NSSize, corner: Int, margin: CGFloat, obstruction: NSRect?) -> NSPoint {
        let inset = min(margin, min(area.width, area.height) / 2)
        let inner = area.insetBy(dx: inset, dy: inset)
        let (w, h) = (min(size.width, inner.width), min(size.height, inner.height))
        let right = corner == 1 || corner == 3
        let top = corner == 0 || corner == 1
        let point = NSPoint(x: right ? inner.maxX - w : inner.minX, y: top ? inner.maxY - h : inner.minY)
        guard let blocked = obstruction else { return point }
        let at = { (p: NSPoint) in NSRect(x: p.x, y: p.y, width: w, height: h) }
        // Touching edges count, as WPF's IntersectsWith
        let touches = { (a: NSRect, b: NSRect) in a.minX <= b.maxX && b.minX <= a.maxX && a.minY <= b.maxY && b.minY <= a.maxY }
        if !touches(at(point), blocked) {
            return point
        }
        let alternatives = [
            NSPoint(x: point.x, y: blocked.minY - margin - h),
            NSPoint(x: point.x, y: blocked.maxY + margin),
            NSPoint(x: blocked.minX - margin - w, y: point.y),
            NSPoint(x: blocked.maxX + margin, y: point.y),
        ]
        let fitting = alternatives.filter { inner.contains(at($0)) && !touches(at($0), blocked) }
        return fitting.min { a, b in
            hypot(a.x - point.x, a.y - point.y) < hypot(b.x - point.x, b.y - point.y)
        } ?? point
    }
}

/// The overlay's contents: each device's icon, name when shown, and level in its color, in rows no wider than the
/// limit.
struct OverlayView: View {
    @ObservedObject var state: OverlayState
    @EnvironmentObject var model: AppModel

    var body: some View {
        Group {
            if state.entries.isEmpty {
                Text(model.t("none.online")).padding(.horizontal, 8).padding(.vertical, 5)
            } else {
                FlowLayout(limit: state.rowLimit) {
                    ForEach(Array(state.entries.enumerated()), id: \.offset) { _, entry in
                        HStack(spacing: 7) {
                            Image(systemName: symbol(entry.kind)).font(.system(size: 17)).foregroundStyle(entry.color.color)
                            if entry.showName {
                                Text(entry.name).lineLimit(1).truncationMode(.tail).frame(maxWidth: 140, alignment: .leading)
                            }
                            HStack(spacing: 3) {
                                Text(entry.level).font(.system(size: 17, weight: .semibold)).monospacedDigit()
                                if entry.power {
                                    Image(systemName: "bolt.fill").font(.system(size: 15, weight: .semibold))
                                }
                            }
                            .foregroundStyle(entry.color.color)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 5)
                    }
                }
            }
        }
        .padding(6)
        .fixedSize()
        // Dark and nearly opaque whatever is underneath, a game or a document, as on Windows (#202020 at 93%)
        .background(Color(white: 0x20 / 255))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .environment(\.colorScheme, .dark)
    }
}

/// Lays its children out in rows, starting a new row where the next one would pass `limit`.
struct FlowLayout: Layout {
    let limit: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        rows(subviews).reduce(CGSize.zero) { size, row in
            CGSize(width: max(size.width, row.width), height: size.height + row.height)
        }
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in rows(subviews) {
            var x = bounds.minX
            for index in row.indices {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: y + (row.height - size.height) / 2), proposal: .unspecified)
                x += size.width
            }
            y += row.height
        }
    }

    private func rows(_ subviews: Subviews) -> [(indices: [Int], width: CGFloat, height: CGFloat)] {
        var rows: [(indices: [Int], width: CGFloat, height: CGFloat)] = []
        for (index, subview) in subviews.enumerated() {
            let size = subview.sizeThatFits(.unspecified)
            if let last = rows.last, !last.indices.isEmpty, last.width + size.width <= limit {
                rows[rows.count - 1] = (last.indices + [index], last.width + size.width, max(last.height, size.height))
            } else {
                rows.append(([index], size.width, size.height))
            }
        }
        return rows
    }
}
