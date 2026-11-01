import Observation
import UIKit
import UserNotifications

/// Pending navigation from outside a scene: a notification tap or the app's own URLs.
@MainActor
@Observable
final class Router {
    static let shared = Router()
    var pending: DeepLink?
}

final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(_ application: UIApplication,
                     didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        #if DEBUG
        DebugPreload.run()
        #endif
        PushManager.shared.launch()
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        PushManager.shared.didRegister(token: deviceToken)
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        PushManager.shared.didFail(error)
    }

    // A notification while the app is open still shows, like a fault in the dashboard's menu.
    // The completion-handler forms, answered on the caller's (main) thread: the async forms
    // complete on a background thread, where UIKit's state restoration asserts.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .list, .sound, .badge])
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let link = DeepLink.route(path: response.notification.request.content.userInfo["path"] as? String)
        if Thread.isMainThread {
            MainActor.assumeIsolated { Router.shared.pending = link }
        } else {
            Task { @MainActor in Router.shared.pending = link }
        }
        completionHandler()
    }
}

#if DEBUG
/// Debug builds only: preload the server URL and key for simulator runs, once, into the
/// Keychain. `FUSEBOX_DEBUG_KEY_FILE` names a file holding the key (the simulator reads the
/// Mac's files), so the key never appears in a command line or a log. `FUSEBOX_DEBUG_FORCE_RELAY=1`
/// (read in PushManager) registers through the real push relay without telling the server.
enum DebugPreload {
    static func run() {
        let env = ProcessInfo.processInfo.environment
        if env["FUSEBOX_DEBUG_RESET"] == "1" {
            Connection.setKey(nil)
            for key in ["push-subscription-id", "push-wanted", "push-via", "push-relay-touched"] { AppGroup.defaults.removeObject(forKey: key) }
            RelayDevice.store(nil)
        }
        if let url = env["FUSEBOX_DEBUG_URL"].flatMap(Connection.normalize) { Connection.serverURL = url }
        if let path = env["FUSEBOX_DEBUG_KEY_FILE"],
           let key = try? String(contentsOfFile: path, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines),
           !key.isEmpty {
            if Connection.serverURL == nil { Connection.serverURL = URL(string: Connection.defaultURL) }
            if Connection.key != key { Connection.setKey(key) }
        }
    }
}
#endif
