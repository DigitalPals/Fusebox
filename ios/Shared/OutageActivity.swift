import ActivityKit
import Foundation

/// The Live Activity for a provider whose every account is out (the `provider` fault):
/// a countdown to the first account coming back. Updated only from the app.
public struct OutageAttributes: ActivityAttributes, Sendable {
    public struct ContentState: Codable, Hashable, Sendable {
        /// When the first account is back; nil when none will come back by itself (sign-in needed).
        public var backAt: Date?
        public var title: String
        public var detail: String
        public var recovered: Bool

        public init(backAt: Date?, title: String, detail: String, recovered: Bool = false) {
            self.backAt = backAt; self.title = title; self.detail = detail; self.recovered = recovered
        }
    }

    public var provider: String
    public var providerName: String
    public var startedAt: Date

    public init(provider: String, providerName: String, startedAt: Date = .now) {
        self.provider = provider; self.providerName = providerName; self.startedAt = startedAt
    }
}

/// What the widgets show when they can't reach the server: the app's last good data.
public struct WidgetSnapshot: Codable, Sendable {
    public var savedAt: Date
    public var overview: Overview?
    public var accounts: [Account]
    public var faults: [Fault]

    public init(savedAt: Date = .now, overview: Overview?, accounts: [Account], faults: [Fault]) {
        self.savedAt = savedAt; self.overview = overview; self.accounts = accounts; self.faults = faults
    }

    /// Kept in the shared keychain group, which the widget extension can read.
    public static func load() -> WidgetSnapshot? {
        guard let data = SharedKeychain.data("widget-snapshot") else { return nil }
        return try? JSONDecoder.fusebox.decode(WidgetSnapshot.self, from: data)
    }

    public func save() {
        guard let data = try? JSONEncoder.fusebox.encode(self) else { return }
        SharedKeychain.set("widget-snapshot", data)
    }
}
