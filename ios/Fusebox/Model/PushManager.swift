import Foundation
import Observation
import UIKit
import UserNotifications

/// Native push: permission, the APNs token, and this device's subscription on the server
/// (`POST /api/push/apns`). Which events are sent is the server-wide notifications config.
///
/// A Fusebox holding the app's push key (`apns` in `GET /api/push`) gets the device token and
/// sends directly. Any other Fusebox sends through the push relay it names: the app registers
/// its token with the relay for a handle, makes a key, and gives the Fusebox only the handle and
/// the key. Those notifications arrive sealed, and the notification service extension opens them.
@MainActor
@Observable
final class PushManager {
    static let shared = PushManager()

    /// Debug builds get sandbox tokens; Release (TestFlight, App Store) production ones.
    #if DEBUG
    static let environment = "development"
    #else
    static let environment = "production"
    #endif

    /// The relay's handle is touched at most this often, so it isn't forgotten while unused.
    static let touchInterval: TimeInterval = 6 * 3600

    static let unavailableText = "This Fusebox can’t send notifications to the app. Update Fusebox, or set apns in its config.yaml."

    var authorization: UNAuthorizationStatus = .notDetermined
    var token: String?
    var busy = false
    /// What happened last, in the server's words when it answered with an error.
    var message: String?
    var messageIsError = false
    var devices: [PushStatus.Device] = []
    /// How the server can reach the app, from `GET /api/push`; nil until it has answered.
    var path: PushPath?
    var statusError: String?

    private(set) var subscriptionID: String? {
        get { AppGroup.defaults.string(forKey: "push-subscription-id") }
        set { AppGroup.defaults.set(newValue, forKey: "push-subscription-id") }
    }
    /// How this device's subscription is delivered: "direct" or "relay".
    private(set) var via: String? {
        get { AppGroup.defaults.string(forKey: "push-via") }
        set { AppGroup.defaults.set(newValue, forKey: "push-via") }
    }
    /// The user turned notifications on for this device; kept while the server is unreachable.
    private var wanted: Bool {
        get { AppGroup.defaults.bool(forKey: "push-wanted") }
        set { AppGroup.defaults.set(newValue, forKey: "push-wanted") }
    }
    private var touchedAt: Date? {
        get { AppGroup.defaults.object(forKey: "push-relay-touched") as? Date }
        set { AppGroup.defaults.set(newValue, forKey: "push-relay-touched") }
    }
    private var touchDue: Bool { touchedAt.map { Date().timeIntervalSince($0) >= Self.touchInterval } ?? true }

    @ObservationIgnored private var tokenWaiters: [CheckedContinuation<String, Error>] = []
    /// One registration at a time: launch, foreground and the switch can all start one.
    @ObservationIgnored private var registering: Task<Void, Error>?

    var isOn: Bool { subscriptionID != nil }

    var label: String {
        let name = UIDevice.current.name
        return "Fusebox for iOS on \(name)" + (Self.environment == "development" ? " (development)" : "")
    }

    func refreshAuthorization() async {
        authorization = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    /// At launch: re-register when this device is subscribed, so a changed token reaches the server.
    func launch() {
        Task {
            await refreshAuthorization()
            if wanted, authorization == .authorized || authorization == .provisional {
                UIApplication.shared.registerForRemoteNotifications()
            }
        }
    }

    /// Back in the foreground: keep the relay's handle, at most every few hours.
    func foreground() {
        guard wanted, via == "relay", touchDue, let token, let api = Connection.api else { return }
        Task { try? await register(token: token, api: api) }
    }

    func turnOn(api: FuseboxAPI?) async {
        guard let api else { return }
        busy = true; message = nil; messageIsError = false
        defer { busy = false }
        do {
            let granted = try await UNUserNotificationCenter.current()
                .requestAuthorization(options: [.alert, .badge, .sound])
            await refreshAuthorization()
            guard granted else {
                show("Notifications are off for Fusebox in Settings. Turn them on there first.", error: true)
                return
            }
            wanted = true
            let token = try await deviceToken()
            try await register(token: token, api: api)
            show("Notifications are on for this device.")
        } catch {
            show(error.localizedDescription, error: true)
        }
        await loadDevices(api: api)
    }

    func turnOff(api: FuseboxAPI?) async {
        wanted = false
        guard let api, let id = subscriptionID else {
            subscriptionID = nil
            await forgetRelayDevice()
            return
        }
        busy = true; defer { busy = false }
        do {
            if !Self.isDebugID(id) {
                try await api.send("DELETE", "/push/subscriptions/\(FuseboxAPI.encode(id))")
            }
            await turnedOff()
        } catch let e as APIError where e.status == 404 {
            await turnedOff()
        } catch {
            show(error.localizedDescription, error: true)
        }
        await loadDevices(api: api)
    }

    private func turnedOff() async {
        subscriptionID = nil
        via = nil
        await forgetRelayDevice()
        show("Notifications are off for this device.")
    }

    func sendTest(api: FuseboxAPI?) async {
        struct Body: Encodable { var id: String }
        guard let api, let id = subscriptionID else { return }
        busy = true; defer { busy = false }
        do {
            try await api.send("POST", "/push/test", json: Body(id: id))
            show("Test sent. It should arrive in a few seconds.")
        } catch let e as APIError where e.status == 404 || e.status == 410 {
            subscriptionID = nil
            show(e.message, error: true)
        } catch {
            show(error.localizedDescription, error: true)
        }
    }

    func remove(device: PushStatus.Device, api: FuseboxAPI?) async {
        guard let api else { return }
        do {
            try await api.send("DELETE", "/push/subscriptions/\(FuseboxAPI.encode(device.id))")
            if device.id == subscriptionID {
                subscriptionID = nil
                via = nil
                await forgetRelayDevice()
            }
        } catch {
            show(error.localizedDescription, error: true)
        }
        await loadDevices(api: api)
    }

    func loadDevices(api: FuseboxAPI?) async {
        guard let api else { return }
        do {
            let s: PushStatus = try await api.get("/push")
            devices = s.subscriptions
            path = Self.path(for: s)
            statusError = nil
            // Removed elsewhere (the dashboard, another phone): this device is off.
            if let id = subscriptionID, !Self.isDebugID(id), !s.subscriptions.contains(where: { $0.id == id }) {
                subscriptionID = nil
                via = nil
                await forgetRelayDevice()
            }
        } catch {
            statusError = error.localizedDescription
        }
    }

    private func show(_ text: String, error: Bool = false) { message = text; messageIsError = error }

    // MARK: Registration

    /// Registers this device on the path the server offers now. Serialised, so a launch and a
    /// tap on the switch never get two relay handles.
    private func register(token: String, api: FuseboxAPI) async throws {
        let previous = registering
        let task = Task { @MainActor in
            _ = await previous?.result
            try await self.registerNow(token: token, api: api)
        }
        registering = task
        try await task.value
    }

    private func registerNow(token: String, api: FuseboxAPI) async throws {
        let status: PushStatus = try await api.get("/push")
        path = Self.path(for: status)
        let previous = subscriptionID
        let id: String
        switch Self.path(for: status) {
        case .unavailable:
            throw APIError(status: 0, message: Self.unavailableText)
        case .direct:
            let body = APNsRegistration(token: token, environment: Self.environment, label: label)
            id = try JSONDecoder.fusebox.decode(IDResponse.self, from: try await api.send("POST", "/push/apns", json: body)).id
            via = "direct"
            await forgetRelayDevice()
        case .relay(let url):
            let device = try await relayDevice(url: url, token: token)
            if Self.forceRelay {
                // Debug: the relay only; the Fusebox side is played from the Mac.
                id = "debug-relay"
                Self.writeDebugFile(device)
            } else {
                let body = RelayRegistration(relay: .init(handle: device.handle, key: device.keyBase64), label: label)
                id = try JSONDecoder.fusebox.decode(IDResponse.self, from: try await api.send("POST", "/push/apns", json: body)).id
            }
            via = "relay"
        }
        subscriptionID = id
        // Another path or a new handle is another device on the server: drop the old one.
        if let previous, previous != id, !Self.isDebugID(previous) {
            try? await api.send("DELETE", "/push/subscriptions/\(FuseboxAPI.encode(previous))")
        }
    }

    /// The relay's handle for this token: the stored one while the relay still knows it,
    /// otherwise a new one with a new key (a new token, another relay, or one it forgot).
    private func relayDevice(url: URL, token: String) async throws -> RelayDevice {
        let client = RelayClient(base: url)
        if let d = RelayDevice.stored {
            if d.relay == url, d.token == token {
                guard touchDue else { return d }
                switch try? await client.touch(d.handle) {
                case true?: touchedAt = Date(); return d
                case nil: return d // The relay can't be reached now: keep the handle, try again later.
                case false?: break // 410: forgotten.
                }
            } else {
                try? await RelayClient(base: d.relay).unregister(d.handle)
            }
            RelayDevice.store(nil)
        }
        let handle = try await client.register(token: token, environment: Self.environment)
        guard let key = RelayDevice.makeKey() else {
            try? await client.unregister(handle)
            throw APIError(status: 0, message: "This iPhone couldn’t make a key for its notifications.")
        }
        let device = RelayDevice(relay: url, handle: handle, key: key, token: token)
        guard RelayDevice.store(device) else {
            try? await client.unregister(handle)
            throw APIError(status: 0, message: "This iPhone couldn’t keep its notification key in the Keychain.")
        }
        touchedAt = Date()
        return device
    }

    /// After the Fusebox has forgotten this device: the relay forgets its handle too.
    private func forgetRelayDevice() async {
        guard let d = RelayDevice.stored else { return }
        RelayDevice.store(nil)
        touchedAt = nil
        try? await RelayClient(base: d.relay).unregister(d.handle)
        #if DEBUG
        try? FileManager.default.removeItem(at: Self.debugFile)
        #endif
    }

    private func deviceToken() async throws -> String {
        if let token { return token }
        return try await withCheckedThrowingContinuation { c in
            tokenWaiters.append(c)
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    static func path(for status: PushStatus) -> PushPath {
        forceRelay ? .relay(debugRelay) : PushPath.choose(status)
    }

    // MARK: Debug: relay registration without the Fusebox side

    #if DEBUG
    /// `FUSEBOX_DEBUG_FORCE_RELAY=1`: register with the real relay (development environment) and
    /// skip `POST /api/push/apns`, so a test can play the Fusebox from the Mac with the handle
    /// and key in `debugFile`.
    static var forceRelay: Bool { ProcessInfo.processInfo.environment["FUSEBOX_DEBUG_FORCE_RELAY"] == "1" }
    static let debugRelay = URL(string: "https://push.whombat.com")!
    static var debugFile: URL {
        FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("relay-debug.json")
    }
    static func isDebugID(_ id: String) -> Bool { id.hasPrefix("debug-") }

    static func writeDebugFile(_ device: RelayDevice) {
        let json: [String: String] = ["relay": device.relay.absoluteString, "handle": device.handle, "key": device.keyBase64]
        if let data = try? JSONSerialization.data(withJSONObject: json) {
            try? data.write(to: debugFile, options: .atomic)
            print("FUSEBOX_DEBUG relay device written to \(debugFile.path)")
        }
    }
    #else
    static let forceRelay = false
    static let debugRelay = URL(string: "https://push.whombat.com")!
    static func isDebugID(_ id: String) -> Bool { false }
    static func writeDebugFile(_ device: RelayDevice) {}
    #endif

    // MARK: From the app delegate

    func didRegister(token data: Data) {
        let hex = data.map { String(format: "%02x", $0) }.joined()
        token = hex
        let waiters = tokenWaiters; tokenWaiters = []
        waiters.forEach { $0.resume(returning: hex) }
        // A launch-time registration: keep the server's copy current, and the relay's handle
        // (a changed token gets a new one).
        if waiters.isEmpty, wanted, let api = Connection.api {
            Task { try? await register(token: hex, api: api) }
        }
    }

    func didFail(_ error: Error) {
        let waiters = tokenWaiters; tokenWaiters = []
        waiters.forEach { $0.resume(throwing: APIError(status: 0, message: "This iPhone couldn’t register for notifications: \(error.localizedDescription)")) }
    }
}
