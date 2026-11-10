import Foundation
import Testing
@testable import Fusebox

@Suite("Deep links")
struct DeepLinkTests {
    @Test func accountPathOpensTheAccount() {
        #expect(DeepLink.route(path: "#/accounts/file%3Aclaude-user%40example.com.json") == .account("file:claude-user@example.com.json"))
        #expect(DeepLink.route(path: "/accounts/key%3Aclaude%3Aab12") == .account("key:claude:ab12"))
        #expect(DeepLink.route(path: "accounts/plain-id") == .account("plain-id"))
    }

    @Test(arguments: [
        "#/config/notifications", "#/overview", "#/accounts", "#/accounts/", "#/usage", "#/requests", "", "garbage", "#/models",
    ])
    func everythingElseOpensTheOverview(_ path: String) {
        #expect(DeepLink.route(path: path) == .overview)
    }

    @Test func missingPathOpensTheOverview() {
        #expect(DeepLink.route(path: nil) == .overview)
    }

    @Test func appURLs() {
        let id = "file:claude-user@example.com.json"
        #expect(DeepLink.route(url: DeepLink.url(account: id)) == .account(id))
        #expect(DeepLink.route(url: URL(string: "fusebox://accounts")!) == .accounts)
        #expect(DeepLink.route(url: URL(string: "fusebox://faults")!) == .faults)
        #expect(DeepLink.route(url: URL(string: "fusebox://overview")!) == .overview)
        #expect(DeepLink.route(url: URL(string: "https://example.com/accounts/x")!) == .overview)
    }

    @Test func notificationPayload() throws {
        // The server's APNs payload: aps plus top-level path, kind, level, provider, account.
        let payload: [AnyHashable: Any] = [
            "aps": ["alert": ["title": "Weekly limit used up", "body": "user@example.com"], "badge": 2, "sound": "default", "thread-id": "claude"],
            "path": "#/accounts/file%3Aclaude-user%40example.com.json", "kind": "quota", "level": "warn",
            "provider": "claude", "account": "file:claude-user@example.com.json",
        ]
        #expect(DeepLink.route(path: payload["path"] as? String) == .account("file:claude-user@example.com.json"))
    }
}

@Suite("The dashboard's rules")
struct RulesTests {
    let now = FuseboxDate.parse("2026-11-01T12:00:00Z")!

    @Test func percentEnds() {
        #expect(Fmt.percent(0) == "0%")
        #expect(Fmt.percent(0.3) == "<1%")
        #expect(Fmt.percent(99.7) == ">99%")
        #expect(Fmt.percent(100) == "100%")
        #expect(Fmt.percent(47.4) == "47%")
    }

    @Test func countdowns() {
        #expect(Fmt.compactLeft(now.addingTimeInterval(3 * 86400 + 7 * 3600 + 120), now: now) == "3d 7h")
        #expect(Fmt.compactLeft(now.addingTimeInterval(2 * 86400 + 30), now: now) == "2d")
        #expect(Fmt.compactLeft(now.addingTimeInterval(2 * 3600 + 14 * 60), now: now) == "2h 14m")
        #expect(Fmt.compactLeft(now.addingTimeInterval(3600), now: now) == "1h")
        #expect(Fmt.compactLeft(now.addingTimeInterval(30), now: now) == "<1m")
        #expect(Fmt.compactLeft(now.addingTimeInterval(-5), now: now) == "now")
        #expect(Fmt.left(now.addingTimeInterval(3600 + 41 * 60), now: now) == "1h 41m")
        #expect(Fmt.left(now.addingTimeInterval(3600 + 5 * 60), now: now) == "1h 05m")
    }

    @Test func counts() {
        #expect(Fmt.count(950) == "950")
        #expect(Fmt.count(12_300) == "12.3k")
        #expect(Fmt.count(124_000) == "124k")
        #expect(Fmt.count(2_250_000) == "2.25M")
        #expect(Fmt.count(42_753_262) == "42.8M")
        #expect(Fmt.ms(850) == "850ms")
        #expect(Fmt.ms(7514) == "7.5s")
    }

    @Test func money() {
        #expect(Fmt.dollars(nanos: nil) == "Unpriced")
        #expect(Fmt.dollars(nanos: 0) == "$0.00")
        #expect(Fmt.dollars(nanos: 1_234_560_000_000) == "$1,234.56")
        #expect(Fmt.dollars(nanos: 12_345_000) == "$0.0123")
    }

    @Test func meterColoursMeasureUse() {
        #expect(QuotaReading(used: 74).level == .normal)
        #expect(QuotaReading(used: 75).level == .warn)
        #expect(QuotaReading(used: 95).level == .err)
        #expect(QuotaReading(used: 100).status == "Used up")
        #expect(QuotaReading(used: 20).value(.remaining) == 80)
    }

    @Test func evenPace() {
        let w = QuotaWindow(name: "week", used: 30, resetsAt: now.addingTimeInterval(3.5 * 86400))
        #expect(abs((Quotas.elapsed(w, now: now) ?? 0) - 0.5) < 0.0001)
        let five = QuotaWindow(name: "5h", used: 10, resetsAt: now.addingTimeInterval(3600))
        #expect(abs((Quotas.elapsed(five, now: now) ?? 0) - 0.8) < 0.0001)
        #expect(Quotas.label(five, short: true) == "5H")
        #expect(Quotas.label(w, short: false) == "WK")
        #expect(Quotas.title(five) == "5-hour")
        #expect(Quotas.title(w) == "Weekly")
        // A renewed window isn't a reading any more.
        #expect(QuotaReading(QuotaWindow(name: "week", used: 90, resetsAt: now.addingTimeInterval(-1)), now: now) == nil)
    }

    @Test func accountStates() {
        var a = Account(id: "a", provider: "claude", label: "user@example.com")
        #expect(a.state(now: now).kind == .ready)
        a.cooldowns = ["*": "2026-11-01T13:41:00Z"]
        a.cooldownKinds = ["*": "quota"]
        let st = a.state(now: now)
        #expect(st.kind == .cooling)
        #expect(a.statusWord(now: now) == "Cooling 1h 41m")
        a.cooldowns = nil
        a.lastError = "invalid_grant: refresh token expired"
        #expect(a.state(now: now).signin)
        a.disabled = true
        #expect(a.state(now: now).word == "Disabled")
    }

    @Test func cooldownReasons() {
        var a = Account(id: "k", provider: "claude", label: "key", kind: "api-key")
        a.cooldownKinds = ["*": "credit", "claude-opus-5-5": "rate_limit", "claude-sonnet-5-5": "quota", "x": "checking"]
        #expect(a.cooldownReason("*") == "API credit used up")
        #expect(a.cooldownReason("claude-opus-5-5") == "Rate limited")
        #expect(a.cooldownReason("claude-sonnet-5-5") == "Usage limit used up")
        #expect(a.cooldownReason("x") == "Checking quota")
        #expect(a.cooldownReason("unlisted") == "Rate limited")
    }

    @Test func noFiveHourLimit() {
        let a = Account(id: "a", provider: "claude", label: "u", quota: Quota(windows: [QuotaWindow(name: "week", used: 26, resetsAt: now.addingTimeInterval(86400))], plan: "max_20x", noFiveHourLimit: true))
        #expect(a.noShortLimit)
        #expect(a.planName == "Ultra")
        #expect(a.subtitle == "Claude · Ultra · no 5h limit")
    }

    @Test func metaSubscriptionsAreMeteredLikeClaudeAndCodex() {
        // Metered from sign-in, before the first reading, like Claude and Codex.
        #expect(Account(id: "m", provider: "meta", label: "u", quota: Quota(windows: [])).metered)
        #expect(!Account(id: "k", provider: "meta", label: "key", kind: "api-key", quota: Quota(windows: [])).metered)
        let live = Account(id: "m", provider: "meta", label: "u", quota: Quota(
            windows: [
                QuotaWindow(name: "5h", used: 3, resetsAt: now.addingTimeInterval(3600)),
                QuotaWindow(name: "week", used: 1, resetsAt: now.addingTimeInterval(86400)),
            ], planName: "Muse Code Max Usage"))
        #expect(live.metered)
        #expect(live.planName == "Muse Code Max Usage", "the mint's tier name")
        #expect(live.subtitle == "Meta · Muse Code Max Usage")
        #expect(live.window(short: true, now: now)?.name == "5h")
        #expect(live.window(short: false, now: now)?.name == "week")
        let unnamed = Account(id: "m", provider: "meta", label: "u", quota: Quota(windows: []))
        #expect(unnamed.planName == nil, "no tier name, no plan")
        #expect(unnamed.subtitle == "Meta · Device code")
    }

    @Test func renewalRow() throws {
        let json = #"{"renewal":{"at":"2026-11-06T09:53:56Z","estimated":false,"ends":false,"period":"monthly"}}"#
        var a = Account(id: "a", provider: "codex", label: "u", quota: try JSONDecoder.fusebox.decode(Quota.self, from: Data(json.utf8)))
        let at = try #require(a.quota?.renewal?.at)
        let before = at.addingTimeInterval(-(3 * 86400 + 21 * 3600))
        let row = try #require(a.renewalRow(now: before))
        #expect(row.key == "Renews")
        #expect(row.value.hasSuffix(", \(Fmt.hm(at)) · in 3d 21h · monthly"), "\(row.value)")
        a.quota?.renewal?.estimated = true
        #expect(a.renewalRow(now: before)?.value.hasPrefix("about ") == true)
        #expect(a.renewalRow(now: before)?.value.hasSuffix(" · in 3d 21h · estimated") == true)
        a.quota?.renewal?.ends = true
        #expect(a.renewalRow(now: before)?.key == "Ends")
        #expect(a.renewalRow(now: before)?.value.hasSuffix(" · in 3d 21h · cancelled") == true)
        #expect(a.renewalRow(now: at) == nil, "a date that has passed is no row")
        let ended = try JSONDecoder.fusebox.decode(Quota.self, from: Data(#"{"renewal":null}"#.utf8))
        #expect(Account(id: "a", provider: "codex", label: "u", quota: ended).renewalRow(now: before) == nil)
    }

    @Test func renewalRowSetByHand() throws {
        let cal = Calendar.current
        let today = cal.startOfDay(for: .now)
        let morning = { (days: Int) in cal.date(byAdding: .hour, value: 9, to: cal.date(byAdding: .day, value: days, to: today)!)! }
        func account(_ at: Date, estimated: Bool = false, ends: Bool = false) -> Account {
            let renewal = Renewal(at: at, estimated: estimated, ends: ends, period: "monthly", manual: true)
            return Account(id: "c", provider: "claude", label: "u", quota: Quota(windows: [], renewal: renewal))
        }
        let noon = cal.date(byAdding: .hour, value: 12, to: today)!
        let soon = try #require(account(morning(19)).renewalRow(now: noon))
        #expect(soon.key == "Renews")
        #expect(soon.value.hasSuffix(" · in 19 days · monthly"), "\(soon.value)")
        #expect(account(morning(0)).renewalRow(now: noon)?.value.contains(" · today · ") == true, "its morning passed, the day did not")
        #expect(account(morning(1), estimated: true).renewalRow(now: noon)?.value.hasSuffix(" · tomorrow · monthly · plan changed, check the date") == true)
        #expect(account(morning(2), ends: true).renewalRow(now: noon)?.value.hasSuffix(" · in 2 days · cancelled") == true)
        #expect(account(morning(-1)).renewalRow(now: noon) == nil, "a day that is over")
    }

    /// The overview's Fable rows: Claude's model weeks ride under its combined week, each
    /// with the first member renewal still to come (ui/app.js `weeklyPools`, `poolNextReset`).
    @Test func modelWeeksRideUnderTheProvidersCombinedWeek() throws {
        let hour: TimeInterval = 3600
        func member(_ id: String, fable: TimeInterval?) -> Account {
            var windows = [QuotaWindow(name: "week", used: 20, resetsAt: now.addingTimeInterval(60 * hour))]
            if let fable { windows.append(QuotaWindow(name: "week fable", used: 30, resetsAt: now.addingTimeInterval(fable * hour), model: "fable")) }
            return Account(id: id, provider: "claude", label: id, quota: Quota(windows: windows, plan: "max_20x", noFiveHourLimit: true))
        }
        let accounts = [member("a", fable: -1), member("b", fable: 30), member("c", fable: 42),
                        Account(id: "x", provider: "codex", label: "x"), Account(id: "y", provider: "codex", label: "y")]
        let ids = ["a", "b", "c"]
        let overview = Overview(pools: [
            Pool(provider: "claude", window: "week", accounts: 3, accountIDs: ids, reported: 3, used: 22, pace: 0.5, priced: true),
            Pool(provider: "claude", window: "week fable", model: "fable", accounts: 3, accountIDs: ids, reported: 3, used: 61.25, pace: 0.25, priced: true),
            Pool(provider: "codex", window: "week", accounts: 2, accountIDs: ["x", "y"], reported: 2, used: 10),
        ])
        let pools = WeeklyPool.weekly(overview: overview, accounts: accounts, now: now)
        #expect(pools.map(\.pool.provider) == ["claude", "codex"], "model weeks get no row of their own")
        let fable = try #require(pools[0].scoped.first)
        #expect(pools[0].scoped.count == 1)
        #expect(pools[1].scoped.isEmpty, "only the provider's own model weeks")
        #expect(fable.name == "Fable")
        #expect(fable.reading?.used == 61.25)
        #expect(fable.nextReset == now.addingTimeInterval(30 * hour), "a's Fable week has renewed; b's comes first")
        // Not every member reporting it: no combined figure rather than a partial one.
        var partial = overview
        partial.pools?[1].reported = 2
        #expect(WeeklyPool.weekly(overview: partial, accounts: accounts, now: now)[0].scoped.first?.reading == nil)
        // The account rows draw the same windows: only live ones, in name order.
        #expect(accounts[1].scopedWindows(now: now).map(\.name) == ["week fable"])
        #expect(accounts[0].scopedWindows(now: now).isEmpty, "a renewed window waits for the next reading")
        #expect(Quotas.scopedTitle(accounts[1].scopedWindows(now: now)[0]) == "Fable weekly")
        let pace = try #require(Quotas.elapsed(accounts[2].scopedWindows(now: now)[0], now: now))
        #expect(abs(pace - (1 - 42.0 / 168)) < 1e-9, "a model's week is a week long, so it has an even-pace mark")
    }

    @Test func routingSentence() throws {
        let r = try decode([RequestLog].self, "requests")
        let reused = try #require(r.first { $0.routingReason == "session_reused" })
        #expect(reused.whySentence.hasPrefix("Stayed on the account this session is pinned to"))
        #expect(reused.reasonText == "Same session")
    }
}
