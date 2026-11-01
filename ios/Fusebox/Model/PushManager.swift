import Foundation
import Observation
import UIKit
import UserNotifications

/// Native push: permission, the APNs token, and this device's subscription on the server
/// (`POST /api/push/apns`). Which events are sent is the server-wide notifications config.
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

    var authorization: UNAuthorizationStatus = .notDetermined
    var token: String?
    var busy = false
    /// What happened last, in the server's words when it answered with an error.
    var message: String?
    var messageIsError = false
    var devices: [PushStatus.Device] = []
    var apnsAvailable: Bool?
    var statusError: String?

    private(set) var subscriptionID: String? {
        get { AppGroup.defaults.string(forKey: "push-subscription-id") }
        set { AppGroup.defaults.set(newValue, forKey: "push-subscription-id") }
    }
    /// The user turned notifications on for this device; kept while the server is unreachable.
    private var wanted: Bool {
        get { AppGroup.defaults.bool(forKey: "push-wanted") }
        set { AppGroup.defaults.set(newValue, forKey: "push-wanted") }
    }

    @ObservationIgnored private var tokenWaiters: [CheckedContinuation<String, Error>] = []

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
        guard let api, let id = subscriptionID else { subscriptionID = nil; return }
        busy = true; defer { busy = false }
        do {
            try await api.send("DELETE", "/push/subscriptions/\(FuseboxAPI.encode(id))")
            subscriptionID = nil
            show("Notifications are off for this device.")
        } catch let e as APIError where e.status == 404 {
            subscriptionID = nil
            show("Notifications are off for this device.")
        } catch {
            show(error.localizedDescription, error: true)
        }
        await loadDevices(api: api)
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
            if device.id == subscriptionID { subscriptionID = nil }
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
            apnsAvailable = s.apnsAvailable
            statusError = nil
            // Removed elsewhere (the dashboard, another phone): this device is off.
            if let id = subscriptionID, !s.subscriptions.contains(where: { $0.id == id }) {
                subscriptionID = nil
            }
        } catch {
            statusError = error.localizedDescription
        }
    }

    private func show(_ text: String, error: Bool = false) { message = text; messageIsError = error }

    private func register(token: String, api: FuseboxAPI) async throws {
        let body = APNsRegistration(token: token, environment: Self.environment, label: label)
        let data = try await api.send("POST", "/push/apns", json: body)
        let id = try JSONDecoder.fusebox.decode(IDResponse.self, from: data).id
        subscriptionID = id
    }

    private func deviceToken() async throws -> String {
        if let token { return token }
        return try await withCheckedThrowingContinuation { c in
            tokenWaiters.append(c)
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    // MARK: From the app delegate

    func didRegister(token data: Data) {
        let hex = data.map { String(format: "%02x", $0) }.joined()
        token = hex
        let waiters = tokenWaiters; tokenWaiters = []
        waiters.forEach { $0.resume(returning: hex) }
        // A launch-time registration: keep the server's copy current.
        if waiters.isEmpty, wanted, let api = Connection.api {
            Task { try? await register(token: hex, api: api) }
        }
    }

    func didFail(_ error: Error) {
        let waiters = tokenWaiters; tokenWaiters = []
        waiters.forEach { $0.resume(throwing: APIError(status: 0, message: "This iPhone couldn’t register for notifications: \(error.localizedDescription)")) }
    }
}
