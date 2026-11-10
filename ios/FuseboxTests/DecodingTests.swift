import Foundation
import Testing
@testable import Fusebox

/// Responses captured from a real Fusebox (scripts/capture-fixtures.mjs), anonymised.
private final class FixtureToken {}

func fixture(_ name: String) throws -> Data {
    let bundle = Bundle(for: FixtureToken.self)
    let url = try #require(bundle.url(forResource: name, withExtension: "json", subdirectory: "Fixtures")
        ?? bundle.url(forResource: name, withExtension: "json"))
    return try Data(contentsOf: url)
}

func decode<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
    try JSONDecoder.fusebox.decode(T.self, from: fixture(name))
}

@Suite("Decoding the management API")
struct DecodingTests {
    @Test func overview() throws {
        let o = try decode(Overview.self, "overview")
        #expect(o.version?.isEmpty == false)
        #expect((o.series ?? []).count >= 1)
        let pools = try #require(o.pools)
        #expect(!pools.isEmpty)
        let week = try #require(pools.first { $0.window == "week" && $0.model == nil })
        #expect(week.accounts >= 1)
        #expect(week.accountIDs?.isEmpty == false)
        #expect(o.totals?.requests != nil)
    }

    @Test func accounts() throws {
        let accounts = try decode([Account].self, "accounts")
        #expect(!accounts.isEmpty)
        for a in accounts {
            #expect(!a.id.isEmpty)
            #expect(a.label.hasSuffix("@example.com"), "labels are anonymised")
        }
        let claude = try #require(accounts.first { $0.provider == "claude" })
        #expect(claude.metered)
        #expect(claude.quota?.windows?.contains { $0.name == "week" } == true)
        #expect(claude.planName != nil)
        // Accounts and pools refer to the same ids.
        let o = try decode(Overview.self, "overview")
        let pools = WeeklyPool.weekly(overview: o, accounts: accounts)
        #expect(!pools.isEmpty)
        #expect(pools[0].accounts.count == pools[0].pool.accountIDs?.count)
        // The capture has Claude's Fable week as its own pool: it rides under the week.
        let fable = try #require(pools[0].scoped.first { $0.pool.model == "fable" })
        #expect(fable.name == "Fable")
        #expect(fable.pool.window == "week fable")
        #expect(fable.pool.accountIDs == pools[0].pool.accountIDs)
    }

    @Test func bankedResets() throws {
        let accounts = try decode([Account].self, "accounts")
        let withInventory = accounts.filter { $0.bankedResets?.inventory != nil }
        #expect(!withInventory.isEmpty)
        for a in withInventory {
            #expect(a.bankedResets?.checkedAt != nil)
            _ = a.bankedLabel
        }
    }

    @Test func faults() throws {
        let faults = try decode([Fault].self, "faults")
        for f in faults {
            #expect(!f.key.isEmpty)
            #expect(["err", "warn", "ok"].contains(f.level))
            if let path = f.path, path.hasPrefix("#/accounts/"), let id = f.accountID {
                #expect(DeepLink.route(path: path) == .account(id))
            }
        }
    }

    @Test func requests() throws {
        let log = try decode([RequestLog].self, "requests")
        #expect(!log.isEmpty)
        let r = log[0]
        #expect(r.id > 0)
        #expect(r.ts.timeIntervalSince1970 > 1_700_000_000)
        #expect(!r.whySentence.isEmpty)
        #expect(log.allSatisfy { $0.source?.ip == nil || $0.source?.ip == "100.64.0.1" }, "addresses are anonymised")
    }

    @Test func activity() throws {
        let a = try decode(AccountActivity.self, "activity")
        #expect(a.series?.count == 60)
    }

    @Test func pushStatusCurrentAndOld() throws {
        let current = try decode(PushStatus.self, "push")
        #expect(current.subscriptions.allSatisfy { !$0.id.isEmpty })
        // A build from before APNs support: no `kind`, no `apns`.
        let old = Data(#"{"public_key":"B","subscriptions":[{"id":"a","label":"Browser","created_at":"2026-10-09T09:19:37.765408690Z"}]}"#.utf8)
        let s = try JSONDecoder.fusebox.decode(PushStatus.self, from: old)
        #expect(s.apnsAvailable == false)
        #expect(s.subscriptions[0].resolvedKind == "web")
        let new = Data(#"{"public_key":"B","apns":true,"subscriptions":[{"id":"b","label":"Phone","kind":"apns","created_at":"2026-11-01T09:00:00Z"}]}"#.utf8)
        let n = try JSONDecoder.fusebox.decode(PushStatus.self, from: new)
        #expect(n.apnsAvailable)
        #expect(n.subscriptions[0].resolvedKind == "apns")
    }

    @Test func usageDashboard() throws {
        let u = try decode(UsageDashboard.self, "usage-dashboard")
        let c = try #require(u.combined)
        #expect(c.totals?.observations != nil)
        #expect((c.trends?.provider?.count ?? 0) > 0)
        #expect((c.breakdowns?.model?.count ?? 0) > 0)
        let dates = Array(Set((c.trend ?? []).compactMap(\.date))).sorted()
        let chart = UsageChart(trend: c.trends?.model ?? [], dates: dates, calls: false)
        #expect(chart.keys.count <= 6)
        #expect(chart.days.count == dates.count)
    }

    @Test func liveMessages() throws {
        let faults = Data(##"{"type":"faults","data":[{"key":"provider:claude","kind":"provider","level":"err","provider":"claude","provider_name":"Claude","account_id":null,"label":null,"title":"Every Claude account is used up","detail":"Requests for Claude models will fail.","until":"2026-11-02T12:59:59.684362+00:00","path":"#/accounts","badge":true}]}"##.utf8)
        guard case .faults(let list, let dismissed) = try LiveMessage.decode(faults) else { Issue.record("not faults"); return }
        #expect(list[0].kind == "provider")
        #expect(list[0].until != nil)
        #expect(dismissed == nil, "a Fusebox that can't dismiss faults sends no dismissed list")
        let withDismissed = Data(##"{"type":"faults","data":[],"dismissed":[{"key":"rate:a","kind":"rate_limit","level":"warn","title":"Rate limited","badge":true}]}"##.utf8)
        guard case .faults(let shown, let hidden) = try LiveMessage.decode(withDismissed) else { Issue.record("not faults"); return }
        #expect(shown.isEmpty)
        #expect(hidden?.map(\.key) == ["rate:a"])

        let load = Data(#"{"type":"load","data":{"file:a.json":{"in_flight":2,"sessions":1,"ongoing_sessions":1}}}"#.utf8)
        #expect(try LiveMessage.decode(load) == .load(["file:a.json": AccountLoad(inFlight: 2, sessions: 1)]))

        let tick = Data(#"{"type":"tick","data":{"active":1,"totals":{"requests":5,"ok":5,"failed":0}}}"#.utf8)
        guard case .tick(let t) = try LiveMessage.decode(tick) else { Issue.record("not tick"); return }
        #expect(t.totals?.requests == 5)

        #expect(try LiveMessage.decode(Data(#"{"type":"accounts","data":null}"#.utf8)) == .accounts)
        let requests = try decode([RequestLog].self, "requests")
        let one = try JSONSerialization.jsonObject(with: fixture("requests")) as! [Any]
        let wrapped = try JSONSerialization.data(withJSONObject: ["type": "request", "data": one[0]])
        #expect(try LiveMessage.decode(wrapped) == .request(requests[0]))
    }
}

@Suite("Dates")
struct DateTests {
    @Test(arguments: [
        ("2026-11-01T08:49:54.326731560Z", 1_793_522_994.326731560),
        ("2026-11-02T12:59:59.684362+00:00", 1_793_624_399.684362),
        ("2026-11-04T16:00:00Z", 1_793_808_000.0),
        ("2026-11-01T09:49:54+01:00", 1_793_522_994.0),
        ("2026-10-31T23:00:00-05:30", 1_793_507_400.0),
    ])
    func parse(_ text: String, _ expected: Double) throws {
        let d = try #require(FuseboxDate.parse(text))
        #expect(abs(d.timeIntervalSince1970 - expected) < 0.000_01)
    }

    @Test func rejectsJunk() {
        #expect(FuseboxDate.parse("") == nil)
        #expect(FuseboxDate.parse("yesterday") == nil)
        #expect(FuseboxDate.parse("2026-13-01T00:00:00Z") == nil)
    }

    @Test func roundTrip() throws {
        let d = Date(timeIntervalSince1970: 1_793_177_394.326)
        let parsed = try #require(FuseboxDate.parse(FuseboxDate.format(d)))
        #expect(abs(parsed.timeIntervalSince(d)) < 0.001)
    }
}
