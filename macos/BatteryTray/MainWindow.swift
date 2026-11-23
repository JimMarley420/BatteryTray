import AppKit
import SwiftUI

/// The device window: the list, with refresh and settings in its toolbar. Made when it opens and let go when it closes,
/// as the Windows app's window is; it first stands under the menu bar item.
@MainActor
final class MainWindowController: NSObject, NSWindowDelegate, NSToolbarDelegate {
    private let window: NSWindow
    private let model: AppModel
    private let closed: () -> Void
    private static let refreshItem = NSToolbarItem.Identifier("refresh")
    private static let settingsItem = NSToolbarItem.Identifier("settings")

    init(model: AppModel, anchor: NSRect?, closed: @escaping () -> Void) {
        self.model = model
        self.closed = closed
        let size = NSSize(width: 440, height: 640)
        window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        super.init()
        window.isReleasedWhenClosed = false
        // Opens where the user is, over an app in full screen too (as the settings do)
        window.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        window.contentMinSize = NSSize(width: 380, height: 360)
        window.contentView = NSHostingView(rootView: DevicesView().environmentObject(model))
        window.delegate = self
        let toolbar = NSToolbar(identifier: "devices")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        window.toolbar = toolbar
        window.toolbarStyle = .unified
        place(near: anchor, size: size)
        languageChanged()
    }

    /// Under the menu bar item, kept inside the screen it is on; in the top right corner while the item has no place
    /// yet (right after the app starts).
    private func place(near item: NSRect?, size: NSSize) {
        let anchor = item.flatMap { $0.minX > 0 || $0.minY > 0 ? $0 : nil }
        let screen = NSScreen.screens.first { anchor.map($0.frame.intersects) ?? false } ?? NSScreen.main
        guard let area = screen?.visibleFrame else { return window.center() }
        let frame = window.frameRect(forContentRect: NSRect(origin: .zero, size: size))
        let right = min(anchor.map { $0.midX + frame.width / 2 } ?? area.maxX - 12, area.maxX - 12)
        let x = max(area.minX + 12, right - frame.width)
        window.setFrame(NSRect(x: x, y: area.maxY - frame.height - 8, width: frame.width, height: frame.height),
                        display: false)
    }

    func show() {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    func languageChanged() {
        window.title = model.t("app.title")
        for item in window.toolbar?.items ?? [] {
            configure(item)
        }
    }

    func windowWillClose(_ notification: Notification) {
        // A name being typed is kept, as when the window loses the focus
        window.makeFirstResponder(nil)
        model.renamingID = nil
        closed()
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.flexibleSpace, Self.refreshItem, Self.settingsItem]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar)
    }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier id: NSToolbarItem.Identifier,
                 willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: id)
        item.isBordered = true
        item.target = self
        item.action = id == Self.refreshItem ? #selector(refresh) : #selector(settings)
        configure(item)
        return item
    }

    private func configure(_ item: NSToolbarItem) {
        let refresh = item.itemIdentifier == Self.refreshItem
        let label = model.t(refresh ? "refresh" : "settings")
        item.label = label
        item.toolTip = label
        item.image = NSImage(systemSymbolName: refresh ? "arrow.clockwise" : "gearshape", accessibilityDescription: label)
    }

    @objc private func refresh() {
        model.send(["cmd": "refresh"])
    }

    @objc private func settings() {
        AppDelegate.shared?.openSettings()
    }
}
