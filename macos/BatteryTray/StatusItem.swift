import AppKit

/// The menu bar item: the level of the device that most needs charging as a colored number (the Windows app's tray
/// icon), the devices in a tooltip, and a menu with each device's level and the app's commands.
@MainActor
final class StatusItemController: NSObject, NSMenuDelegate {
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    private let menu = NSMenu()
    private let model: AppModel

    init(model: AppModel) {
        self.model = model
        super.init()
        menu.delegate = self
        menu.autoenablesItems = false
        item.menu = menu
        item.button?.setAccessibilityLabel("BatteryTray")
        item.behavior = []
        item.isVisible = true
        update(model.snapshot?.tray)
    }

    /// Where the item is on the screen: the device window opens under it.
    var buttonFrame: NSRect? {
        guard let button = item.button, let window = button.window else { return nil }
        return window.convertToScreen(button.convert(button.bounds, to: nil))
    }

    func update(_ tray: Tray?) {
        guard let button = item.button else { return }
        let label = tray?.label ?? "…"
        let color = tray?.color.nsColor ?? .secondaryLabelColor
        let font = NSFont.monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .bold)
        button.attributedTitle = NSAttributedString(string: label, attributes: [.foregroundColor: color, .font: font])
        button.toolTip = tray?.tooltip
    }

    /// Opens the menu and hands its window to `take` while it is open, then closes it (`--snapshot`).
    func snapshotMenu(_ take: @escaping (NSWindow?) -> Void) {
        let timer = Timer(timeInterval: 0.8, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                take(NSApp.windows.first { $0.isVisible && String(describing: type(of: $0)).contains("Menu") })
                self?.menu.cancelTracking()
            }
        }
        // Menu tracking runs the event loop in its own mode
        RunLoop.main.add(timer, forMode: .common)
        item.button?.performClick(nil)
    }

    /// Built afresh each time it opens, in the current language and with the current levels.
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let items = model.snapshot?.tray.items ?? []
        if items.isEmpty {
            let none = NSMenuItem(title: model.t("tray.nodata"), action: nil, keyEquivalent: "")
            none.isEnabled = false
            menu.addItem(none)
        }
        for device in items {
            let entry = NSMenuItem(title: device.name, action: #selector(open), keyEquivalent: "")
            entry.target = self
            entry.image = NSImage(systemSymbolName: symbol(device.kind), accessibilityDescription: nil)
            entry.attributedTitle = title(device)
            menu.addItem(entry)
        }
        menu.addItem(.separator())
        menu.addItem(command(model.t("tray.open"), #selector(open), ""))
        menu.addItem(command(model.t("settings") + "…", #selector(settings), ","))
        menu.addItem(command(model.t("refresh"), #selector(refresh), "r"))
        menu.addItem(.separator())
        menu.addItem(command(model.t("tray.exit"), #selector(quit), "q"))
    }

    /// "Razer Barracuda X   85% ⚡": the level in its color, after a tab so the levels line up.
    private func title(_ device: TrayItem) -> NSAttributedString {
        let text = NSMutableAttributedString(string: device.name + "\t")
        let level = [device.level, device.charging ? "⚡" : ""].filter { !$0.isEmpty }.joined(separator: " ")
        let bold = NSFont.monospacedDigitSystemFont(ofSize: NSFont.menuFont(ofSize: 0).pointSize, weight: .semibold)
        text.append(NSAttributedString(string: level, attributes: [.foregroundColor: device.color.nsColor, .font: bold]))
        let style = NSMutableParagraphStyle()
        style.tabStops = [NSTextTab(textAlignment: .right, location: 260)]
        text.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: text.length))
        return text
    }

    private func command(_ title: String, _ action: Selector, _ key: String) -> NSMenuItem {
        let entry = NSMenuItem(title: title, action: action, keyEquivalent: key)
        entry.target = self
        return entry
    }

    @objc private func open() {
        AppDelegate.shared?.openMain()
    }

    @objc private func settings() {
        AppDelegate.shared?.openSettings()
    }

    @objc private func refresh() {
        model.send(["cmd": "refresh"])
    }

    @objc private func quit() {
        AppDelegate.shared?.quit()
    }
}

/// The menus at the top of the screen while a window of the app is open.
@MainActor
enum MainMenu {
    static func install(_ model: AppModel) {
        let t = model.t
        let main = NSMenu()

        let app = NSMenu()
        app.addItem(withTitle: t("menu.about"), action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
                    keyEquivalent: "")
        app.addItem(.separator())
        let settings = app.addItem(withTitle: t("settings") + "…", action: #selector(MenuActions.settings),
                                   keyEquivalent: ",")
        settings.target = MenuActions.shared
        app.addItem(.separator())
        app.addItem(withTitle: t("menu.hide"), action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        let others = app.addItem(withTitle: t("menu.hideOthers"),
                                 action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h")
        others.keyEquivalentModifierMask = [.command, .option]
        app.addItem(withTitle: t("menu.showAll"), action: #selector(NSApplication.unhideAllApplications(_:)),
                    keyEquivalent: "")
        app.addItem(.separator())
        let quit = app.addItem(withTitle: t("tray.exit"), action: #selector(MenuActions.quit), keyEquivalent: "q")
        quit.target = MenuActions.shared
        main.addItem(submenu(app, title: "BatteryTray"))

        let edit = NSMenu(title: t("menu.edit"))
        edit.addItem(withTitle: t("menu.undo"), action: Selector(("undo:")), keyEquivalent: "z")
        let redo = edit.addItem(withTitle: t("menu.redo"), action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        edit.addItem(.separator())
        edit.addItem(withTitle: t("menu.cut"), action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: t("menu.copy"), action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: t("menu.paste"), action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: t("menu.selectAll"), action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        main.addItem(submenu(edit, title: t("menu.edit")))

        let window = NSMenu(title: t("menu.window"))
        window.addItem(withTitle: t("menu.minimize"), action: #selector(NSWindow.performMiniaturize(_:)),
                       keyEquivalent: "m")
        window.addItem(withTitle: t("menu.close"), action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        main.addItem(submenu(window, title: t("menu.window")))
        NSApp.windowsMenu = window

        NSApp.mainMenu = main
    }

    private static func submenu(_ menu: NSMenu, title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.submenu = menu
        return item
    }
}

/// The targets of the main menu's own commands.
@MainActor
final class MenuActions: NSObject {
    static let shared = MenuActions()

    @objc func settings() {
        AppDelegate.shared?.openSettings()
    }

    @objc func quit() {
        AppDelegate.shared?.quit()
    }
}
