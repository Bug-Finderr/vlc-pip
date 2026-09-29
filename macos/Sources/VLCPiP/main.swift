import AppKit

// `VLCPiP toggle` pokes the running menu-bar app (for scripts, or a VLC Lua menu entry).
if CommandLine.arguments.dropFirst().first == "toggle" {
    DistributedNotificationCenter.default().postNotificationName(toggleNotification, object: nil, userInfo: nil,
                                                                 deliverImmediately: true)
    exit(0)
}

// Opening the app again (Spotlight, Finder) while it runs toggles PiP instead of adding a
// second menu-bar icon that can't get the hotkey.
if let id = Bundle.main.bundleIdentifier,
   NSRunningApplication.runningApplications(withBundleIdentifier: id).contains(where: { $0.processIdentifier != getpid() }) {
    DistributedNotificationCenter.default().postNotificationName(toggleNotification, object: nil, userInfo: nil,
                                                                 deliverImmediately: true)
    exit(0)
}

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    app.setActivationPolicy(.accessory)
    app.run()
}
