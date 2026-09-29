import AppKit
import Carbon.HIToolbox
import ServiceManagement

/// Global hotkey via Carbon: needs no Accessibility or Input Monitoring permission.
final class HotKey {
    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let action: () -> Void

    init?(keyCode: Int, modifiers: Int, action: @escaping () -> Void) {
        self.action = action
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, _, ctx in
            guard let ctx else { return OSStatus(eventNotHandledErr) }
            Unmanaged<HotKey>.fromOpaque(ctx).takeUnretainedValue().action()
            return noErr
        }, 1, &spec, Unmanaged.passUnretained(self).toOpaque(), &handler)
        let id = EventHotKeyID(signature: OSType(0x5650_4950), id: 1)  // 'VPIP'
        guard RegisterEventHotKey(UInt32(keyCode), UInt32(modifiers), id, GetApplicationEventTarget(), 0, &ref) == noErr
        else { return nil }
    }

    deinit {
        if let ref { UnregisterEventHotKey(ref) }
        if let handler { RemoveEventHandler(handler) }
    }
}

let toggleNotification = Notification.Name("dev.vlc-pip.toggle")

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let vlc = VLC()
    private var session: Session?
    private var statusItem: NSStatusItem!
    private var toggleItem: NSMenuItem!
    private var systemPiPItem: NSMenuItem!
    private var loginItem: NSMenuItem!
    private var hotKey: HotKey?

    private var useSystemPiP: Bool {
        get { UserDefaults.standard.object(forKey: "useSystemPiP") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "useSystemPiP") }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        hotKey = HotKey(keyCode: kVK_ANSI_P, modifiers: controlKey | optionKey) { [weak self] in
            MainActor.assumeIsolated { self?.toggle() }
        }

        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        let menu = NSMenu()
        toggleItem = menu.addItem(withTitle: hotKey == nil ? "Toggle PiP  (⌃⌥P is taken)" : "Toggle PiP  (⌃⌥P)",
                                  action: #selector(toggle), keyEquivalent: "")
        menu.addItem(.separator())
        systemPiPItem = menu.addItem(withTitle: "Use System PiP", action: #selector(toggleSystemPiP), keyEquivalent: "")
        loginItem = menu.addItem(withTitle: "Open at Login", action: #selector(toggleLogin), keyEquivalent: "")
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit VLC PiP", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        statusItem.menu = menu
        refreshUI()

        DistributedNotificationCenter.default().addObserver(self, selector: #selector(toggle), name: toggleNotification, object: nil)

        if !CGPreflightScreenCaptureAccess() { CGRequestScreenCaptureAccess() }
        if CommandLine.arguments.contains("--start") { toggle() }
    }

    func applicationWillTerminate(_ notification: Notification) {
        session?.stop()
    }

    @objc func toggle() {
        if let session {
            session.stop()
            return
        }
        let s = Session(vlc: vlc)
        session = s
        s.onEnd = { [weak self, weak s] in
            guard let self, self.session === s else { return }
            self.session = nil
            self.refreshUI()
        }
        refreshUI()
        Task {
            do {
                try await s.start(systemPiP: useSystemPiP)
            } catch {
                if session === s {
                    s.stop()
                    session = nil
                }
                refreshUI()
                report(error)
            }
        }
    }

    @objc private func toggleSystemPiP() {
        useSystemPiP.toggle()
        refreshUI()
    }

    @objc private func toggleLogin() {
        let service = SMAppService.mainApp
        do {
            if service.status == .enabled {
                try service.unregister()
            } else {
                try service.register()
            }
        } catch {
            report(error)
        }
        refreshUI()
    }

    private func refreshUI() {
        let symbol = session == nil ? "pip.enter" : "pip.exit"
        statusItem.button?.image = NSImage(systemSymbolName: symbol, accessibilityDescription: "VLC PiP")
        systemPiPItem.state = useSystemPiP ? .on : .off
        let inBundle = Bundle.main.bundleIdentifier != nil
        loginItem.isHidden = !inBundle
        loginItem.state = inBundle && SMAppService.mainApp.status == .enabled ? .on : .off
    }

    private func report(_ error: Error) {
        NSLog("vlc-pip: %@", error.localizedDescription)
        let alert = NSAlert()
        alert.messageText = "VLC PiP"
        alert.informativeText = error.localizedDescription
        let settings = (error as? PiPError)?.settingsURL
        if settings != nil {
            alert.addButton(withTitle: "Open System Settings")
            alert.addButton(withTitle: "Cancel")
        }
        NSApp.activate()
        if alert.runModal() == .alertFirstButtonReturn, let settings { NSWorkspace.shared.open(settings) }
    }
}
