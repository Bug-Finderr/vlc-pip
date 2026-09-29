import AppKit

/// Talks to VLC through raw Apple Events built from its scripting dictionary
/// (VLC.app/Contents/Resources/vlc.scriptSuite). Events go to VLC's pid, so nothing here
/// can launch VLC, and they run on a private queue, so a busy VLC never stalls the UI.
final class VLC: @unchecked Sendable {
    static let bundleID = "org.videolan.vlc"

    struct Status: Equatable {
        var playing = false
        var time = 0.0        // seconds
        var duration = 0.0    // seconds; <= 0 for streams or when unknown
        var name = ""         // empty when nothing is loaded
        var path = ""         // POSIX path of the item, or its URL for streams
        var fullscreen = false
    }

    enum Failure: Error {
        case notRunning
        case notPermitted
        case failed
    }

    private let queue = DispatchQueue(label: "vlc-pip.vlc", qos: .userInitiated)

    static var app: NSRunningApplication? {
        NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first { !$0.isTerminated }
    }

    // MARK: Async API (safe from any thread)

    /// Asks for Automation consent if macOS hasn't recorded an answer yet (blocks only this
    /// queue while the prompt is up). False when the user has denied control of VLC.
    func ensurePermission() async -> Bool {
        await onQueue {
            guard let pid = Self.app?.processIdentifier else { return true }
            var target = AEAddressDesc()
            var p = pid
            guard AECreateDesc(typeKernelProcessID, &p, MemoryLayout<pid_t>.size, &target) == noErr else { return true }
            defer { AEDisposeDesc(&target) }
            let status = AEDeterminePermissionToAutomateTarget(&target, typeWildCard, typeWildCard, true)
            return status != OSStatus(errAEEventNotPermitted)
        }
    }

    func status() async -> Result<Status, Failure> {
        await onQueue {
            do {
                var s = Status()
                s.playing = try self.get("AAPL")?.booleanValue ?? false
                s.fullscreen = try self.get("pFsC")?.booleanValue ?? false
                // The item properties error out when nothing is loaded.
                s.name = (try? self.get("AANA"))??.stringValue ?? ""
                if !s.name.isEmpty {
                    s.path = (try? self.get("AAPA"))??.stringValue ?? ""
                    s.time = Double((try? self.get("AACT"))??.int32Value ?? 0)
                    s.duration = Double((try? self.get("AADU"))??.int32Value ?? 0)
                }
                return .success(s)
            } catch let f as Failure {
                return .failure(f)
            } catch {
                return .failure(.failed)
            }
        }
    }

    /// VLC's `play` command toggles play/pause.
    func togglePlay() {
        queue.async { _ = try? self.send("VLC#", "VLC1") }
    }

    func seek(to seconds: Double) {
        queue.async {
            _ = try? self.send("core", "setd") { event in
                event.setParam(Self.property("AACT"), forKeyword: keyDirectObject)
                event.setParam(NSAppleEventDescriptor(int32: Int32(max(0, seconds.rounded()))), forKeyword: Self.code("data"))
            }
        }
    }

    // MARK: Apple Events

    private func onQueue<T>(_ body: @escaping () -> T) async -> T {
        await withCheckedContinuation { cont in queue.async { cont.resume(returning: body()) } }
    }

    private func get(_ property: String) throws -> NSAppleEventDescriptor? {
        let reply = try send("core", "getd") { $0.setParam(Self.property(property), forKeyword: keyDirectObject) }
        return reply.paramDescriptor(forKeyword: keyDirectObject)
    }

    private func send(_ eventClass: String, _ eventID: String,
                      configure: (NSAppleEventDescriptor) -> Void = { _ in }) throws -> NSAppleEventDescriptor {
        guard let pid = Self.app?.processIdentifier else { throw Failure.notRunning }
        let event = NSAppleEventDescriptor.appleEvent(
            withEventClass: Self.code(eventClass), eventID: Self.code(eventID),
            targetDescriptor: NSAppleEventDescriptor(processIdentifier: pid),
            returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
        configure(event)
        let reply: NSAppleEventDescriptor
        do {
            reply = try event.sendEvent(options: [.waitForReply], timeout: 2)
        } catch let error as NSError {
            throw Self.failure(for: error.code)
        }
        if let err = reply.paramDescriptor(forKeyword: keyErrorNumber)?.int32Value, err != 0 {
            throw Self.failure(for: Int(err))
        }
        return reply
    }

    private static func failure(for code: Int) -> Failure {
        switch code {
        case Int(errAEEventNotPermitted), Int(errAEEventWouldRequireUserConsent): .notPermitted
        case Int(procNotFound), Int(connectionInvalid): .notRunning
        default: .failed
        }
    }

    /// Object specifier for `property <code> of application`.
    private static func property(_ property: String) -> NSAppleEventDescriptor {
        let spec = NSAppleEventDescriptor.record()
        spec.setDescriptor(NSAppleEventDescriptor(typeCode: code("prop")), forKeyword: code("want"))
        spec.setDescriptor(NSAppleEventDescriptor(enumCode: code("prop")), forKeyword: code("form"))
        spec.setDescriptor(NSAppleEventDescriptor(typeCode: code(property)), forKeyword: code("seld"))
        spec.setDescriptor(NSAppleEventDescriptor.null(), forKeyword: code("from"))
        return spec.coerce(toDescriptorType: code("obj ")) ?? spec
    }

    private static func code(_ s: String) -> FourCharCode {
        s.utf8.reduce(0) { $0 << 8 | FourCharCode($1) }
    }
}
