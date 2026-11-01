import AppIntents
import Foundation
import WidgetKit

/// The widgets' data: read from the server with the shared key, or the app's last snapshot
/// when the server can't be reached (no tailnet, locked keychain).
struct WidgetData: Sendable {
    var overview: Overview?
    var accounts: [Account]
    var faults: [Fault]
    var date: Date
    var stale: Bool
    var connected: Bool
    /// Used or left, as chosen in the app.
    var mode: QuotaDisplay = .current

    static func load() async -> WidgetData {
        guard let api = Connection.api else {
            return WidgetData(overview: nil, accounts: [], faults: [], date: .now, stale: false, connected: false)
        }
        do {
            async let o = api.overview()
            async let a = api.accounts()
            async let f = api.faults()
            let (ov, ac, fa) = try await (o, a, f)
            return WidgetData(overview: ov, accounts: ac, faults: fa, date: .now, stale: false, connected: true)
        } catch {
            let s = WidgetSnapshot.load()
            return WidgetData(overview: s?.overview, accounts: s?.accounts ?? [], faults: s?.faults ?? [],
                              date: s?.savedAt ?? .now, stale: true, connected: true)
        }
    }

    var pools: [WeeklyPool] { WeeklyPool.weekly(overview: overview, accounts: accounts) }

    /// Next refresh: about every 20 minutes, sooner when a reset or a cooldown ends first.
    func nextRefresh(now: Date = .now) -> Date {
        var next = now.addingTimeInterval(20 * 60)
        let soon = accounts.flatMap { a in (a.quota?.windows ?? []).compactMap(\.resetsAt) }
            + faults.compactMap(\.until)
        if let first = soon.filter({ $0 > now.addingTimeInterval(60) }).min(), first < next {
            next = max(first.addingTimeInterval(30), now.addingTimeInterval(5 * 60))
        }
        return next
    }

    static let preview: WidgetData = {
        let reset = Date.now.addingTimeInterval(3 * 86400 + 4 * 3600)
        let a = Account(id: "preview", provider: "claude", label: "you@example.com",
                        quota: Quota(windows: [QuotaWindow(name: "5h", used: 42, resetsAt: .now.addingTimeInterval(7200)),
                                               QuotaWindow(name: "week", used: 31, resetsAt: reset)], plan: "max_20x"))
        var pool = Pool(provider: "claude", window: "week", model: nil, accounts: 1, accountIDs: ["preview"], reported: 1, used: 31, pace: 0.5)
        pool.outlook = Pool.Outlook(state: "on_track", nextResetAt: reset)
        var o = Overview()
        o.pools = [pool]
        return WidgetData(overview: o, accounts: [a], faults: [], date: .now, stale: false, connected: true)
    }()
}

// MARK: - Choosing a subscription

struct AccountEntity: AppEntity {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Subscription"
    static let defaultQuery = AccountQuery()

    var id: String
    var label: String
    var provider: String

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(label)", subtitle: "\(Provider.name(provider))")
    }
}

struct AccountQuery: EntityQuery {
    func entities(for identifiers: [AccountEntity.ID]) async throws -> [AccountEntity] {
        try await suggestedEntities().filter { identifiers.contains($0.id) }
    }

    func suggestedEntities() async throws -> [AccountEntity] {
        let data = await WidgetData.load()
        return data.accounts.filter(\.metered).map { AccountEntity(id: $0.id, label: $0.label, provider: $0.provider) }
    }
}

struct SelectAccountIntent: WidgetConfigurationIntent {
    static let title: LocalizedStringResource = "Subscription"
    static let description = IntentDescription("Shows one subscription’s 5-hour and weekly meters.")

    @Parameter(title: "Subscription")
    var account: AccountEntity?
}
