import Foundation
import Observation
import UserNotifications
import WidgetKit

/// Everything the screens read from one Fusebox: loaded over HTTP, kept current over
/// `/api/live`, polled while the socket is down.
@MainActor
@Observable
final class AppStore {
    enum LiveState: Equatable { case connecting, live, offline }

    private(set) var api: FuseboxAPI?
    var overview: Overview?
    var accounts: [Account] = []
    var faults: [Fault] = []
    var requests: [RequestLog] = []
    var requestsFull = false
    var load: [String: AccountLoad] = [:]
    var activity: [String: AccountActivity] = [:]
    var live: LiveState = .connecting
    var loadError: String?
    var lastLoaded: Date?
    /// Settings: show meters as used or remaining (the dashboard's toggle).
    var quotaDisplay: QuotaDisplay {
        didSet { AppGroup.defaults.set(quotaDisplay.rawValue, forKey: "quota-display") }
    }
    var liveActivitiesEnabled: Bool {
        didSet { AppGroup.defaults.set(liveActivitiesEnabled, forKey: "live-activities"); syncOutages() }
    }

    @ObservationIgnored private var socket: URLSessionWebSocketTask?
    @ObservationIgnored private var socketTask: Task<Void, Never>?
    @ObservationIgnored private var pollTask: Task<Void, Never>?
    @ObservationIgnored private var retryDelay: Double = 1
    @ObservationIgnored private var accountsRefresh: Task<Void, Never>?
    @ObservationIgnored private var widgetReloadedAt = Date.distantPast
    @ObservationIgnored private var lastFaultKeys: [String] = []
    @ObservationIgnored let outages = OutageActivities()
    #if DEBUG
    @ObservationIgnored private var debugOutageUntil: Date?
    #endif

    init() {
        quotaDisplay = QuotaDisplay(rawValue: AppGroup.defaults.string(forKey: "quota-display") ?? "") ?? .used
        liveActivitiesEnabled = AppGroup.defaults.object(forKey: "live-activities") as? Bool ?? true
        api = Connection.api
    }

    var isConnected: Bool { api != nil }

    func account(_ id: String?) -> Account? { id.flatMap { id in accounts.first { $0.id == id } } }

    func inFlight(_ id: String) -> Int { load[id]?.inFlight ?? 0 }

    var badgeCount: Int { faults.filter(\.countsOnBadge).count }

    var weeklyPools: [WeeklyPool] { WeeklyPool.weekly(overview: overview, accounts: accounts) }

    var subscriptions: [Account] { accounts.filter(\.metered) }

    /// Accounts by provider, in the server's order.
    var groups: [(provider: String, name: String, accounts: [Account])] {
        var order: [String] = []
        var map: [String: [Account]] = [:]
        for a in accounts {
            let key = a.provider == "openai-compat" ? "compat:\(a.group ?? "")" : a.provider
            if map[key] == nil { order.append(key) }
            map[key, default: []].append(a)
        }
        return order.map { ($0, map[$0]![0].providerName, map[$0]!) }
    }

    // MARK: Connection

    func connect(url: URL, key: String) {
        Connection.serverURL = url
        Connection.setKey(key)
        api = FuseboxAPI(baseURL: url, key: key)
        start()
    }

    func signOut() {
        stop()
        Connection.setKey(nil)
        api = nil
        overview = nil; accounts = []; faults = []; requests = []; load = [:]; activity = [:]
        WidgetSnapshot(overview: nil, accounts: [], faults: []).save()
        WidgetCenter.shared.reloadAllTimelines()
        UNUserNotificationCenter.current().setBadgeCount(0)
    }

    func start() {
        guard api != nil else { return }
        Task { await refreshAll() }
        connectLive()
    }

    func stop() {
        socketTask?.cancel(); socketTask = nil
        socket?.cancel(with: .goingAway, reason: nil); socket = nil
        pollTask?.cancel(); pollTask = nil
    }

    /// Back from the background: the socket may look open but be dead, so start again.
    func resume() {
        guard api != nil else { return }
        retryDelay = 1
        stop()
        start()
    }

    func refreshAll() async {
        guard let api else { return }
        do {
            async let o = api.overview()
            async let a = api.accounts()
            async let f = api.faults()
            async let r = api.requests(limit: requestsFull ? nil : 60)
            let (ov, ac, fa, rq) = try await (o, a, f, r)
            overview = ov; accounts = ac; requests = rq
            setFaults(fa)
            loadError = nil
            lastLoaded = .now
            dataChanged()
        } catch {
            loadError = error.localizedDescription
        }
    }

    func loadFullRequests() async {
        guard let api, !requestsFull else { return }
        if let all = try? await api.requests() {
            requests = all
            requestsFull = true
        }
    }

    func refreshAccounts() {
        accountsRefresh?.cancel()
        accountsRefresh = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard let api, !Task.isCancelled else { return }
            if let a = try? await api.accounts() { accounts = a }
            if let o = try? await api.overview() { overview = o }
            dataChanged()
        }
    }

    func loadActivity(_ id: String) async {
        guard let api else { return }
        if let a = try? await api.activity(id) { activity[id] = a }
    }

    // MARK: Live socket

    private func connectLive() {
        guard let api else { return }
        live = .connecting
        let task = FuseboxAPI.session.webSocketTask(with: api.liveURL)
        socket = task
        task.resume()
        socketTask = Task { [weak self] in
            var opened = false
            while !Task.isCancelled {
                do {
                    let msg = try await task.receive()
                    guard let self else { return }
                    if !opened { opened = true; self.live = .live; self.retryDelay = 1; self.pollTask?.cancel(); self.pollTask = nil }
                    let data: Data
                    switch msg {
                    case .string(let s): data = Data(s.utf8)
                    case .data(let d): data = d
                    @unknown default: continue
                    }
                    if let m = try? LiveMessage.decode(data) { self.onLive(m) }
                } catch {
                    guard let self, !Task.isCancelled else { return }
                    self.socketClosed()
                    return
                }
            }
        }
    }

    private func socketClosed() {
        live = .offline
        load = [:]
        socket = nil
        startPolling()
        let delay = retryDelay
        retryDelay = min(retryDelay * 2, 15)
        socketTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self, !Task.isCancelled else { return }
            self.connectLive()
        }
    }

    /// While the socket is down, read everything every 30 seconds.
    private func startPolling() {
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(30))
                guard let self, !Task.isCancelled, self.live != .live else { return }
                await self.refreshAll()
            }
        }
    }

    private func onLive(_ m: LiveMessage) {
        switch m {
        case .request(let r): onRequest(r)
        case .load(let l): load = l
        case .faults(let f): setFaults(f)
        case .tick(let t):
            overview?.totals = t.totals ?? overview?.totals
            overview?.active = t.active ?? overview?.active
        case .accounts: refreshAccounts()
        case .models, .other: break
        }
    }

    private func onRequest(_ r: RequestLog) {
        requests.insert(r, at: 0)
        if requests.count > 300 { requests.removeLast(requests.count - 300) }
        guard var o = overview else { return }
        let minute = Int(r.ts.timeIntervalSince1970 / 60)
        let cutoff = Int(Date.now.timeIntervalSince1970 / 60) - 59
        var series = (o.series ?? []).filter { $0.minute >= cutoff }
        if minute >= cutoff {
            if let i = series.firstIndex(where: { $0.minute == minute }) {
                series[i].requests += 1
                if r.isError { series[i].failed = (series[i].failed ?? 0) + 1 }
                series[i].tokens = (series[i].tokens ?? 0) + (r.inputTokens ?? 0) + (r.outputTokens ?? 0) + (r.cacheTokens ?? 0)
            } else {
                var b = MinuteBucket(minute: minute, requests: 1, failed: r.isError ? 1 : 0)
                b.tokens = (r.inputTokens ?? 0) + (r.outputTokens ?? 0) + (r.cacheTokens ?? 0)
                series.append(b)
                series.sort { $0.minute < $1.minute }
            }
        }
        o.series = series
        overview = o
    }

    /// The last 60 minutes with empty minutes filled in, oldest first.
    var lastHour: [MinuteBucket] {
        let now = Int(Date.now.timeIntervalSince1970 / 60)
        let byMinute = Dictionary((overview?.series ?? []).map { ($0.minute, $0) }, uniquingKeysWith: { a, _ in a })
        return (now - 59...now).map { byMinute[$0] ?? MinuteBucket(minute: $0) }
    }

    // MARK: Faults, badge, widgets, Live Activities

    private func setFaults(_ incoming: [Fault]) {
        var f = incoming
        #if DEBUG
        // Debug builds, FUSEBOX_DEBUG_OUTAGE=<provider>: add a provider-exhausted fault, back in 47
        // minutes, to try the Live Activity without a real outage.
        if let p = ProcessInfo.processInfo.environment["FUSEBOX_DEBUG_OUTAGE"], !p.isEmpty, !f.contains(where: { $0.key == "provider:\(p)" }) {
            debugOutageUntil = debugOutageUntil ?? .now.addingTimeInterval(47 * 60)
            let name = Provider.name(p)
            f.append(Fault(key: "provider:\(p)", kind: "provider", level: "err", provider: p, providerName: name,
                           title: "Every \(name) account is used up", detail: "Requests for \(name) models will fail.",
                           until: debugOutageUntil, path: "#/accounts"))
        }
        #endif
        let rank: (Fault) -> Int = { $0.kind == "pace" ? 2 : $0.level == "err" ? 0 : 1 }
        faults = f.enumerated().sorted { rank($0.element) != rank($1.element) ? rank($0.element) < rank($1.element) : $0.offset < $1.offset }.map(\.element)
        UNUserNotificationCenter.current().setBadgeCount(badgeCount)
        let keys = faults.map(\.key)
        if keys != lastFaultKeys {
            lastFaultKeys = keys
            reloadWidgets(force: true)
        }
        syncOutages()
    }

    func syncOutages() {
        outages.sync(faults: faults, enabled: liveActivitiesEnabled)
    }

    private func dataChanged() {
        WidgetSnapshot(overview: overview, accounts: accounts, faults: faults).save()
        reloadWidgets(force: false)
    }

    /// Widgets follow the app's data, at most every five minutes unless something tripped or cleared.
    private func reloadWidgets(force: Bool) {
        guard force || Date.now.timeIntervalSince(widgetReloadedAt) > 300 else { return }
        widgetReloadedAt = .now
        WidgetSnapshot(overview: overview, accounts: accounts, faults: faults).save()
        WidgetCenter.shared.reloadAllTimelines()
    }

    // MARK: Account actions (as the dashboard's buttons)

    func refreshQuota(_ id: String) async throws {
        guard let api else { return }
        let data = try await api.send("POST", "/accounts/\(FuseboxAPI.encode(id))/quota/refresh")
        if let r = try? JSONDecoder.fusebox.decode(BankedResets.self, from: data), r.checkedAt != nil,
           let i = accounts.firstIndex(where: { $0.id == id }) {
            accounts[i].bankedResets = r
        }
        refreshAccounts()
    }

    func setDisabled(_ id: String, _ disabled: Bool) async throws {
        struct Body: Encodable { var disabled: Bool }
        try await api?.send("POST", "/accounts/\(FuseboxAPI.encode(id))/toggle", json: Body(disabled: disabled))
        if let i = accounts.firstIndex(where: { $0.id == id }) { accounts[i].disabled = disabled }
        refreshAccounts()
    }

    func clearCooldowns(_ id: String) async throws {
        try await api?.send("POST", "/accounts/\(FuseboxAPI.encode(id))/reset")
        refreshAccounts()
    }

    func clearError(_ id: String, errorID: String) async throws {
        struct Body: Encodable { var error_id: String }
        try await api?.send("POST", "/accounts/\(FuseboxAPI.encode(id))/error/clear", json: Body(error_id: errorID))
        refreshAccounts()
    }

    func setPriority(_ id: String, _ priority: Int) async throws {
        struct Body: Encodable { var priority: Int }
        try await api?.send("POST", "/accounts/\(FuseboxAPI.encode(id))/priority", json: Body(priority: priority))
        if let i = accounts.firstIndex(where: { $0.id == id }) { accounts[i].priority = priority }
        refreshAccounts()
    }

    /// Fetches a fresh quote for using a banked reset.
    func bankedQuote(_ id: String) async throws -> BankedResets {
        guard let api else { throw APIError(status: 0, message: "Not connected") }
        let r: BankedResets = try await api.get("/accounts/\(FuseboxAPI.encode(id))/banked-resets")
        if let i = accounts.firstIndex(where: { $0.id == id }) { accounts[i].bankedResets = r }
        return r
    }

    func applyBankedReset(_ id: String, request: String, grant: String) async throws {
        struct Body: Encodable { var action = "redeem"; var request_id: String; var grant_id: String; var confirmed = true }
        let data = try await api?.send("POST", "/accounts/\(FuseboxAPI.encode(id))/banked-resets",
                                       json: Body(request_id: request, grant_id: grant))
        if let data, let r = try? JSONDecoder.fusebox.decode(BankedResets.self, from: data),
           let i = accounts.firstIndex(where: { $0.id == id }) {
            accounts[i].bankedResets = r
        }
        refreshAccounts()
    }
}
