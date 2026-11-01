import Foundation
import Security

/// What the app and its extensions share lives in a keychain access group they are all entitled
/// to (`<team>.com.digitalpals.fusebox.shared`): the management key, the server URL, the widgets'
/// snapshot, and the push relay's handle and key, which the notification service decrypts with.
/// No App Group: App Store Connect API keys can't register one, and the publish pipeline signs
/// unattended with such a key (README, “Publishing”).
public enum SharedKeychain {
    private static let service = "com.digitalpals.fusebox"

    /// `$(AppIdentifierPrefix)com.digitalpals.fusebox.shared`, from Info.plist.
    static var accessGroup: String? {
        Bundle.main.object(forInfoDictionaryKey: "FuseboxKeychainGroup") as? String
    }

    private static func query(_ account: String) -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: account == "management-key" ? "com.digitalpals.fusebox.management-key" : service,
            kSecAttrAccount as String: account,
        ]
        if let g = accessGroup, !g.hasPrefix("$(") { q[kSecAttrAccessGroup as String] = g }
        return q
    }

    public static func data(_ account: String) -> Data? {
        var q = query(account)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess else { return nil }
        return out as? Data
    }

    @discardableResult
    public static func set(_ account: String, _ data: Data?) -> Bool {
        SecItemDelete(query(account) as CFDictionary)
        guard let data, !data.isEmpty else { return true }
        var q = query(account)
        q[kSecValueData as String] = data
        // The widgets refresh while the phone is locked, after its first unlock.
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(q as CFDictionary, nil) == errSecSuccess
    }
}
