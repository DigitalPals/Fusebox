import CryptoKit
import Foundation
import Security
import UserNotifications

/// This device at the push relay: the handle the relay gave out for its APNs token, and the key
/// that only this device and its Fusebox hold. In the shared keychain group, so the notification
/// service can read the key.
public struct RelayDevice: Codable, Equatable, Sendable {
    public var relay: URL
    public var handle: String
    /// AES-256-GCM, 32 bytes.
    public var key: Data
    /// The APNs token the handle was registered for: a new token needs a new handle.
    public var token: String

    public init(relay: URL, handle: String, key: Data, token: String) {
        self.relay = relay; self.handle = handle; self.key = key; self.token = token
    }

    static let account = "push-relay-device"

    public static var stored: RelayDevice? {
        SharedKeychain.data(account).flatMap { try? JSONDecoder().decode(RelayDevice.self, from: $0) }
    }

    @discardableResult
    public static func store(_ device: RelayDevice?) -> Bool {
        SharedKeychain.set(account, device.flatMap { try? JSONEncoder().encode($0) })
    }

    public static func makeKey() -> Data? {
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else { return nil }
        return Data(bytes)
    }

    /// Standard base64, as Fusebox takes it.
    public var keyBase64: String { key.base64EncodedString() }
}

/// A notification that came through the relay: `sealed` is standard base64 of
/// `nonce(12) ‖ ciphertext ‖ tag(16)`, AES-256-GCM with the AAD `fusebox-push-v1`
/// (`src/push/relay.rs`). The plaintext holds what a direct notification carries in the clear.
public enum SealedPush {
    static let aad = Data("fusebox-push-v1".utf8)

    public struct Contents: Codable, Equatable, Sendable {
        public var title: String?
        public var body: String?
        public var path: String?
        public var kind: String?
        public var level: String?
        public var provider: String?
        public var account: String?
        public var thread: String?
    }

    public enum Failure: Error { case notBase64 }

    public static func open(_ sealed: String, key: Data) throws -> Contents {
        guard let data = Data(base64Encoded: sealed) else { throw Failure.notBase64 }
        let box = try AES.GCM.SealedBox(combined: data)
        let plain = try AES.GCM.open(box, using: SymmetricKey(data: key), authenticating: aad)
        return try JSONDecoder().decode(Contents.self, from: plain)
    }

    /// What the notification service shows: the decrypted title and body, with `path`, `kind`,
    /// `level`, `provider` and `account` in userInfo, so a tap routes as a direct notification's
    /// does. Without the key, or when it doesn't open, the relay's generic alert stays; a direct
    /// notification (no `sealed`) passes through unchanged.
    public static func reveal(_ content: UNNotificationContent, key: Data?) -> UNNotificationContent {
        guard let sealed = content.userInfo["sealed"] as? String, let key,
              let c = try? open(sealed, key: key),
              let out = content.mutableCopy() as? UNMutableNotificationContent else { return content }
        if let t = c.title, !t.isEmpty { out.title = t }
        if let b = c.body, !b.isEmpty { out.body = b }
        if let t = c.thread, !t.isEmpty { out.threadIdentifier = t }
        var info = content.userInfo
        info.removeValue(forKey: "sealed")
        let fields: [(String, String?)] = [
            ("path", c.path), ("kind", c.kind), ("level", c.level), ("provider", c.provider), ("account", c.account),
        ]
        for (name, value) in fields { if let value { info[name] = value } }
        out.userInfo = info
        return out
    }
}
