import Foundation

// The dashboard's rules for quota, status and names (ui/app.js), so the app reads the
// same: "5-hour", "weekly", the even-pace mark, "Used up", "Cooling 1h 41m".

public enum Provider {
    public static let names: [String: String] = [
        "claude": "Claude", "codex": "Codex", "gemini": "Gemini", "vertex": "Vertex AI", "antigravity": "Antigravity",
        "kimi": "Kimi", "xai": "Grok", "meta": "Meta", "devin": "Devin", "openai-compat": "Compatible",
    ]
    static let deviceCode: Set<String> = ["kimi", "xai", "meta"]

    public static func name(_ provider: String?) -> String {
        guard let provider, !provider.isEmpty else { return "—" }
        return names[provider] ?? provider
    }

    /// The asset name of a provider's logo (ui/logos.svg), as `logoId` picks it.
    public static func logo(provider: String?, group: String? = nil, kind: String? = nil) -> String {
        let p = provider ?? ""
        if p == "xai" && kind == "api-key" { return "logo-xai-api" }
        let known: Set<String> = ["claude", "codex", "gemini", "vertex", "antigravity", "xai", "kimi", "meta", "devin"]
        if known.contains(p) { return "logo-\(p)" }
        let g = (group ?? "").lowercased()
        let compat: [(String, String)] = [
            ("openrouter", "openrouter"), ("ollama", "ollama"), ("lmstudio", "lmstudio"), ("deepseek", "deepseek"),
            ("groq", "groq"), ("mistral", "mistral"), ("qwen", "qwen"), ("dashscope", "qwen"), ("moonshot", "kimi"),
            ("kimi", "kimi"), ("grok", "xai"), ("xai", "xai"), ("gemini", "gemini"), ("anthropic", "claude"),
            ("claude", "claude"), ("openai", "codex"),
        ]
        if let hit = compat.first(where: { g.contains($0.0) }) { return "logo-\(hit.1)" }
        return "logo-compat"
    }

    /// Usage reports name providers by vendor.
    public static func usageKey(_ id: String?) -> String {
        switch id { case "anthropic": "claude"; case "openai": "codex"; case "google": "gemini"; default: id ?? "" }
    }
}

/// A window's quota as the meters show it. Colours always describe how much is used.
public struct QuotaReading: Equatable, Sendable {
    public enum Level: Sendable { case normal, warn, err }
    public var used: Double
    public var remaining: Double { 100 - used }
    public var level: Level { used >= 95 ? .err : used >= 75 ? .warn : .normal }
    public var exhausted: Bool { used >= 100 }

    public init(used: Double) { self.used = max(0, min(100, used)) }

    /// A live reading, or nil when not reported or the window has renewed since.
    public init?(_ w: QuotaWindow?, now: Date = .now) {
        guard let w, let used = w.used, used.isFinite else { return nil }
        if let r = w.resetsAt, r <= now { return nil }
        self.init(used: used)
    }

    public var status: String {
        exhausted ? "Used up" : level == .err ? "Almost used up" : level == .warn ? "Running low" : "Healthy"
    }

    public func value(_ mode: QuotaDisplay) -> Double { mode == .used ? used : remaining }
}

public enum QuotaDisplay: String, Sendable, CaseIterable {
    case used, remaining
    public var word: String { self == .used ? "used" : "left" }

    /// What's left, like the dashboard, until someone picks "Used".
    public static let standard: QuotaDisplay = .remaining

    /// The app's choice, kept in the shared keychain so the widgets follow it; nil before one is made.
    public static var stored: QuotaDisplay? {
        get { SharedKeychain.data("quota-display").flatMap { String(data: $0, encoding: .utf8) }.flatMap(QuotaDisplay.init(rawValue:)) }
        set { SharedKeychain.set("quota-display", newValue.map { Data($0.rawValue.utf8) }) }
    }

    /// What the meters show now.
    public static var current: QuotaDisplay { stored ?? standard }
}

/// What the segmented meter draws, as the dashboard draws it (ui/app.js `meterHTML`):
/// 20 segments of 5%, lit for the shown share (used, or left), coloured by how much is
/// *used* in either mode (`meter` below 75%, `warn` from 75%, `err` from 95%), and the
/// even-pace mark where usage would be if spread evenly, mirrored when showing what's left.
public struct MeterModel: Equatable, Sendable {
    public static let segments = 20

    /// How many segments are lit, from the leading edge.
    public var lit: Int
    /// The lit segments' level; nil when nothing is reported.
    public var level: QuotaReading.Level?
    /// The pace mark's position along the bar, 0 (leading edge) to 1; nil without one.
    public var pace: Double?

    public init(_ reading: QuotaReading?, pace elapsed: Double?, mode: QuotaDisplay) {
        guard let reading else { lit = 0; level = nil; pace = nil; return }
        lit = min(Self.segments, max(0, Int((reading.value(mode) / 5).rounded())))
        level = reading.level
        pace = elapsed.flatMap { $0.isFinite ? min(1, max(0, $0)) : nil }.map { mode == .used ? $0 : 1 - $0 }
    }

    public func isLit(_ segment: Int) -> Bool { segment < lit }
}

public enum Quotas {
    /// A window's length from its name ("5h", "day", "week", "week fable"), in seconds.
    public static func seconds(_ w: QuotaWindow?) -> TimeInterval? {
        guard let name = w?.name else { return nil }
        if let h = match(name, #"^(\d+)h$"#) { return Double(h)! * 3600 }
        if let r = name.range(of: #"^(\d+)([dms])$"#, options: .regularExpression) {
            let s = String(name[r])
            let n = Double(s.dropLast())!
            switch s.last { case "d": return n * 86400; case "m": return n * 60; default: return n }
        }
        if name == "day" { return 86400 }
        if name == "week" || name.hasPrefix("week") { return 7 * 86400 }
        return nil
    }

    private static func match(_ s: String, _ pattern: String) -> String? {
        guard let r = s.range(of: pattern, options: .regularExpression) else { return nil }
        return String(s[r].filter(\.isNumber))
    }

    /// How far through its window we are, 0 to 1: where the even-pace mark goes.
    public static func elapsed(_ w: QuotaWindow?, now: Date = .now) -> Double? {
        guard let len = seconds(w), let reset = w?.resetsAt else { return nil }
        let left = reset.timeIntervalSince(now)
        return left >= 0 && left <= len ? 1 - left / len : nil
    }

    /// When the window's pace so far would use it up before it resets, the time it runs out
    /// (DESIGN.md "Pace mark": at least a quarter of the window gone and 20% used; the list's
    /// reset cell then reads "out 1h 40m" in `warn`). Nil when it lasts, or it's used up already.
    public static func runsOut(_ w: QuotaWindow?, now: Date = .now) -> Date? {
        guard let w, let len = seconds(w), let reset = w.resetsAt, let t = elapsed(w, now: now),
              let r = QuotaReading(w, now: now), t >= 0.25, r.used >= 20, r.used < 100 else { return nil }
        let perSecond = r.used / (t * len)
        let out = now.addingTimeInterval((100 - r.used) / perSecond)
        return out < reset ? out : nil
    }

    public static func isShort(_ w: QuotaWindow) -> Bool { w.name.range(of: #"^\d+h$"#, options: .regularExpression) != nil }

    /// "5H", "WK", "DAY", or the window's own name.
    public static func label(_ w: QuotaWindow?, short: Bool) -> String {
        if let w, !["week", "day"].contains(w.name), !(short && isShort(w) && w.name == "5h") {
            return w.name.uppercased()
        }
        return short ? "5H" : w?.name == "day" ? "DAY" : "WK"
    }

    /// "5-hour", "Weekly", "Daily", "Weekly · fable".
    public static func title(_ w: QuotaWindow) -> String {
        if isShort(w) { return "\(w.name.dropLast())-hour" }
        if w.name == "day" { return "Daily" }
        if w.name == "week" { return "Weekly" }
        if w.name.hasPrefix("week") { return "Weekly · \(w.name.dropFirst(5))" }
        return w.name
    }

    /// "Fable weekly" for a window limited to one model.
    public static func scopedTitle(_ w: QuotaWindow) -> String {
        let base = QuotaWindow(name: String(w.name.split(separator: " ").first ?? ""), used: nil)
        return "\(Fmt.capitalized(w.model ?? "")) \(title(base).lowercased())"
    }
}

public struct AccountState: Equatable, Sendable {
    public enum Kind: Sendable { case ready, cooling, error, disabled }
    public var kind: Kind
    public var word: String
    public var model: String?
    public var until: Date?
    public var cooldownKind: String?
    public var resetUnknown = false
    public var signin = false
}

private let signinError = try! NSRegularExpression(
    pattern: #"invalid_grant|refresh token|sign in again|re-?authenticat|unauthori[sz]ed|\b401\b|token (?:has )?expired|expired token|revoked"#,
    options: [.caseInsensitive])

extension Account {
    public var providerName: String {
        if provider == "openai-compat" { return group ?? "Compatible" }
        if kind == "api-key" && provider == "xai" { return "xAI" }
        if kind == "api-key" && provider == "codex" { return "OpenAI" }
        return Provider.name(provider)
    }

    public var authName: String {
        if kind == "service-account" { return "Service account" }
        if kind == "api-key" { return "API key" }
        return Provider.deviceCode.contains(provider) ? "Device code" : "OAuth"
    }

    public var planName: String? {
        let codex = ["chatgptprolite": "Pro 100", "chatgptpro": "Pro 200", "chatgptpromax": "Pro 500"]
        let claude = ["max_5x": "Max", "max_20x": "Ultra"]
        if provider == "codex", let n = quota?.subscriptionPlan.flatMap({ codex[$0] }) { return n }
        if provider == "claude", let n = quota?.planName ?? quota?.plan.flatMap({ claude[$0] }) { return n }
        guard let plan = quota?.plan, !plan.isEmpty else { return nil }
        return Fmt.capitalized(plan.replacingOccurrences(of: #"[_-]+"#, with: " ", options: .regularExpression))
    }

    /// The sign-in card's row for the next charge: ("Renews", "Fri 6 Nov, 10:53 · in 3d 21h ·
    /// monthly"), ("Renews", "about Fri 6 Dec · in 1d 17h · estimated": passed before the next read) or
    /// ("Ends", "Fri 6 Nov · in 3d 21h · cancelled"), as the dashboard writes it.
    public func renewalRow(now: Date = .now) -> (key: String, value: String)? {
        guard let r = quota?.renewal, r.at > now else { return nil }
        let day = r.at.formatted(Date.FormatStyle(locale: Locale(identifier: "en_GB")).weekday(.abbreviated).day().month(.abbreviated))
        let left = "in \(Fmt.left(r.at, now: now))"
        if r.ends == true { return ("Ends", "\(day) · \(left) · cancelled") }
        if r.estimated == true { return ("Renews", "about \(day) · \(left) · estimated") }
        return ("Renews", ["\(day), \(Fmt.hm(r.at))", left, r.period].compactMap { $0 }.joined(separator: " · "))
    }

    /// "Claude · Ultra" or "Claude · OAuth", plus "· no 5h limit".
    public var subtitle: String {
        "\(providerName) · \(planName ?? authName)" + (noShortLimit ? " · no 5h limit" : "")
    }

    public var logo: String { Provider.logo(provider: provider, group: group, kind: kind) }
    public var logoProvider: String { provider }

    /// Subscriptions that can report limits get meters; API keys have none.
    public var metered: Bool {
        (kind == "oauth" && ["claude", "codex"].contains(provider))
            || !(quota?.windows ?? []).isEmpty || !(quota?.limits ?? []).isEmpty
    }

    /// The tightest live window of a kind: short (5-hour) or long (weekly), not model-scoped.
    public func window(short: Bool, now: Date = .now) -> QuotaWindow? {
        (quota?.windows ?? [])
            .filter { $0.model == nil && QuotaReading($0, now: now) != nil && Quotas.isShort($0) == short }
            .max { ($0.used ?? 0) < ($1.used ?? 0) }
    }

    /// Verified Claude Ultra and ChatGPT Pro accounts have no 5-hour limit.
    public var noShortLimit: Bool { (quota?.noFiveHourLimit ?? false) && window(short: true) == nil }

    /// Windows limited to one model ("week fable"), shown on the account page.
    public func scopedWindows(now: Date = .now) -> [QuotaWindow] {
        (quota?.windows ?? []).filter { $0.model != nil && QuotaReading($0, now: now) != nil }.sorted { $0.name < $1.name }
    }

    public func state(now: Date = .now) -> AccountState {
        if disabled { return AccountState(kind: .disabled, word: "Disabled") }
        let cds = (cooldowns ?? [:]).compactMap { k, v in FuseboxDate.parse(v).map { (k, $0) } }
            .filter { $0.1 > now }
            .sorted { $0.1 > $1.1 }
        if let (model, until) = cds.first {
            let kind = cooldownKinds?[model] ?? "rate_limit"
            if kind == "quota" && model == "*" && (quotaResetUnknown ?? false) {
                return AccountState(kind: .cooling, word: "Quota used up", model: model, until: until, cooldownKind: kind, resetUnknown: true)
            }
            return AccountState(kind: .cooling, word: "Cooling", model: model, until: until, cooldownKind: kind)
        }
        if let e = lastError, !e.isEmpty {
            let range = NSRange(e.startIndex..., in: e)
            let signin = kind != "api-key" && signinError.firstMatch(in: e, range: range) != nil
            return AccountState(kind: .error, word: "Error", signin: signin)
        }
        return AccountState(kind: .ready, word: "Ready")
    }

    /// "Ready", "Cooling 1h 41m", "Serving 2".
    public func statusWord(inFlight: Int = 0, now: Date = .now) -> String {
        let st = state(now: now)
        if st.kind == .cooling { return st.resetUnknown ? st.word : "Cooling \(Fmt.left(st.until ?? now, now: now))" }
        if st.kind == .ready && inFlight > 0 { return "Serving \(Fmt.count(inFlight))" }
        return st.word
    }

    /// A usage-limit cooldown lasting until the window renews means the window is used up.
    public func windowSpent(_ w: QuotaWindow?) -> Bool {
        guard let renews = w?.resetsAt?.addingTimeInterval(-60) else { return false }
        return (cooldowns ?? [:]).contains { model, until in
            cooldownKinds?[model] == "quota" && (FuseboxDate.parse(until).map { $0 >= renews } ?? false)
        }
    }

    /// "Claude · OAuth · token valid 5h 11m".
    public func signinLine(now: Date = .now) -> String {
        var parts = [authName]
        if kind == "oauth", let exp = expiresAt {
            let secs = exp.timeIntervalSince(now)
            parts.append(secs > 0 ? "token valid \(Fmt.span(secs))" : "token expired")
            parts.append("refreshes on its own")
        }
        return parts.joined(separator: " · ")
    }

    // MARK: Banked resets

    public func hasBankedResets(enabled: Bool) -> Bool {
        enabled && kind == "oauth" && ["claude", "codex"].contains(provider)
    }

    public var bankedNeedsReview: Bool { ["pending", "unknown"].contains(bankedResets?.operation?.status ?? "") }

    public var bankedCount: Int? { bankedResets?.error == nil ? bankedResets?.inventory?.available : nil }

    /// "2 resets banked", "Reset needs review", "Resets unavailable".
    public var bankedLabel: String {
        if bankedNeedsReview { return "Reset needs review" }
        guard let n = bankedCount else { return "Resets unavailable" }
        return "\(n) reset\(n == 1 ? "" : "s") banked"
    }

    public func bankedPolicyText(now: Date = .now) -> String {
        if let plan = bankedResetPlan, plan.autoRedeem == true {
            var s = (plan.prioritized == true ? "Prioritized for new work. " : "") + (plan.message ?? "")
            if let at = plan.expiryRedeemAt { s += " Expiry redemption starts \(Fmt.when(at, now: now))." }
            return s
        }
        return "Manual use asks for confirmation. Automatic redemption is off for this account."
    }

    /// Whether "Use 1 reset" can be offered now, as the dashboard decides it.
    public func bankedUsable(now: Date = .now) -> Bool {
        guard let r = bankedResets, let inv = r.inventory, !disabled, r.error == nil, r.quote != nil,
              inv.eligible == true, !bankedNeedsReview,
              let checked = r.checkedAt, checked > now.addingTimeInterval(-300)
        else { return false }
        let expiryChanged = (inv.grants ?? []).contains { g in
            g.usable == true && ((g.expiresAt.map { $0 <= now } ?? false) || (g.startsAt.map { $0 > now } ?? false))
        }
        return !expiryChanged
    }
}

// MARK: - Combined weekly limits

public struct WeeklyPool: Identifiable, Sendable, Equatable {
    public var pool: Pool
    public var accounts: [Account]
    public var id: String { pool.id }

    public var providerName: String { accounts.first?.providerName ?? Provider.name(pool.provider) }
    public var logo: String { accounts.first?.logo ?? Provider.logo(provider: pool.provider) }

    /// Current combined quota, once every account reports its weekly window.
    public var reading: QuotaReading? {
        guard pool.accounts > 0, pool.reported == pool.accounts, let used = pool.used else { return nil }
        return QuotaReading(used: used)
    }

    public var status: (text: String, level: String) {
        switch pool.outlook?.state {
        case "run_out": ("Quota gap expected", "err")
        case "tight": ("Low reserve", "warn")
        case "on_track": ("Expected to last", "ok")
        case "learning": ("Learning usage", "")
        default: ("Forecast unavailable", "")
        }
    }

    public func detail(now: Date = .now) -> String {
        let o = pool.outlook
        switch o?.state {
        case "run_out":
            if let e = o?.exhaustsAt, let r = o?.recoversAt { return "Runs out \(Fmt.when(e, now: now)) · Back \(Fmt.when(r, now: now))" }
            return "Expected to exhaust weekly quota"
        case "learning":
            if let l = o?.learningUntil { return "Estimate after \(Fmt.when(l, now: now))" }
            return "Needs 6h of recent quota readings"
        case "on_track", "tight":
            return "Next 7 days at recent usage"
        default:
            let reasons = [
                "stale_readings": "Quota readings are stale", "missing_quota": "Waiting for every account’s weekly quota",
                "missing_reset": "Waiting for every account’s reset time", "awaiting_reset": "Waiting for reset confirmation",
                "unavailable_account": "An account needs to sign in again",
            ]
            return reasons[o?.reason ?? ""] ?? "Waiting for quota history"
        }
    }

    /// "4 accounts · Ultra", "2 Ultra, 1 Pro", "3 of 4 accounts reporting".
    public var countText: String {
        let p = pool
        let count = p.reported < p.accounts
            ? "\(Fmt.count(p.reported)) of \(Fmt.plural(p.accounts, "account")) reporting"
            : Fmt.plural(p.accounts, "account")
        var counts: [String: Int] = [:]
        for a in accounts { if let n = a.planName { counts[n, default: 0] += 1 } }
        let plans = counts.sorted { $0.value != $1.value ? $0.value > $1.value : $0.key < $1.key }
        let known = plans.reduce(0) { $0 + $1.value }
        if plans.count == 1 && known == p.accounts { return "\(count) · \(plans[0].key)" }
        let mix = plans.map { "\(Fmt.count($0.value)) \($0.key)" }.joined(separator: ", ")
        if !mix.isEmpty && known == p.accounts && p.reported == p.accounts { return mix }
        return [count, mix].filter { !$0.isEmpty }.joined(separator: " · ")
    }

    /// Each provider's whole-account weekly pool, matched to its accounts.
    public static func weekly(overview: Overview?, accounts: [Account]) -> [WeeklyPool] {
        (overview?.pools ?? []).filter { $0.window == "week" && $0.model == nil }.compactMap { p in
            let list = (p.accountIDs ?? []).compactMap { id in accounts.first { $0.id == id } }
            return list.isEmpty ? nil : WeeklyPool(pool: p, accounts: list)
        }
    }
}

// MARK: - Requests

public enum Routing {
    public static let reasons: [String: String] = [
        "expiring_reset": "Expiring reset", "new_session": "New session", "session_reused": "Same session",
        "quota_exhausted": "Moved: quota used up", "account_disabled": "Moved: account disabled",
        "account_removed": "Moved: account removed", "model_unavailable": "Moved: model not served",
        "temporary_detour": "Detour: account busy", "missing_session": "No session id", "affinity_disabled": "Affinity off",
        "retry_same": "Retried same account",
    ]
    static let strategyPick: [String: String] = [
        "least-used": "the account with the most quota left",
        "smart-quota": "the account with an expiring banked reset, or the earliest weekly renewal",
        "round-robin": "the next account in turn",
        "fill-first": "the first available account",
    ]
    public static let clients = ["openai": "OpenAI", "responses": "Responses", "claude": "Anthropic", "gemini": "Gemini"]
    static let endpoints = ["openai": "/v1/chat/completions", "responses": "/v1/responses", "claude": "/v1/messages", "gemini": "/v1beta/models"]
    static let warnings: [String: (String, String)] = [
        "missing_session_id": ("No session ID", "The client supplied no stable session identifier. Later requests may use another account and lose cache reuse."),
        "connection_only": ("Connection only", "This assignment lasts for the WebSocket connection. A reconnect without a stable session identifier may use another account."),
        "response_id_only": ("Response ID only", "This assignment relies on a previous response ID held in memory. A stable session identifier is needed to preserve it across server restarts."),
        "affinity_disabled": ("Affinity off", "Session affinity is disabled. Requests from this session may use different accounts."),
        "inferred_session": ("Inferred session", "The client sent no session ID, so this conversation was recognised by its first message. Conversations that start the same way share an account; send x-fusebox-session-id to keep them apart."),
    ]
    public static let routingText: [String: (String, String)] = [
        "least-used": ("go to the account with the most quota left", "go to the most quota left"),
        "smart-quota": ("use expiring banked resets first, then the earliest weekly renewal, reserving quota for active sessions", "prefer expiring resets, then weekly renewals"),
        "round-robin": ("take turns across accounts", "take turns across accounts"),
        "fill-first": ("go to the first available account", "go to the first available account"),
    ]

    /// "new sessions prefer expiring resets, then weekly renewals".
    public static func sentence(_ overview: Overview?, short: Bool) -> String {
        guard let r = overview?.routing, let t = routingText[r] else { return "" }
        return "\(overview?.sessionAffinity == false ? "requests" : "new sessions") \(short ? t.1 : t.0)"
    }
}

extension RequestLog {
    public var isError: Bool { (status ?? 0) >= 400 && status != 499 }
    public var isRerouted: Bool {
        guard let r = routingReason else { return false }
        return r.range(of: #"^(quota_exhausted|account_|model_unavailable|temporary_detour)"#, options: .regularExpression) != nil
    }
    public var reasonText: String? {
        routingReason.map { Routing.reasons[$0] ?? $0.replacingOccurrences(of: "_", with: " ") }
    }
    public var clientName: String { Routing.clients[client ?? ""] ?? client ?? "—" }
    public var clientDescription: String { clientApp ?? "\(clientName) format" }
    public var transportTag: String? {
        switch transport { case "ws": "ws"; case "images": "image"; case "video": "video"; default: nil }
    }
    public var endpoint: String {
        let base = transport == "images" ? "/v1/images" : transport == "video" ? "/v1/videos" : Routing.endpoints[client ?? ""] ?? ""
        return base + (transport == "ws" ? " (websocket)" : "")
    }
    public var usageState: String {
        if let u = usageCompleteness, ["complete", "partial", "missing"].contains(u) { return u }
        return (inputTokens ?? 0) + (outputTokens ?? 0) + (cacheTokens ?? 0) > 0 ? "partial" : "missing"
    }

    public var cancellationText: String {
        [
            "downstream_disconnect": "Connection closed before completion.",
            "downstream_write_timeout": "Timed out sending the response to the client.",
            "downstream_write_error": "Connection error while sending the response to the client.",
            "downstream_read_error": "Connection error while reading from the client.",
            "downstream_queue_limit": "Connection ended because the pending request limit was reached.",
        ][failureKind ?? ""] ?? "Connection ended before completion."
    }

    public var errorLine: String? {
        if status == 499 { return cancellationText }
        guard let error, !error.isEmpty, (status ?? 0) >= 400 else { return nil }
        return error
    }

    public var tokensText: String {
        switch usageState {
        case "missing": return "Unknown · provider usage not reported"
        default:
            let p = usageState == "partial" ? "≥" : ""
            return "\(p)\(Fmt.count(inputTokens)) in · \(p)\(Fmt.count(outputTokens)) out · \(p)\(Fmt.count(cacheTokens)) cached"
                + (usageState == "partial" ? " · partial usage" : "")
        }
    }

    public var sessionWarning: (String, String)? {
        guard sessionID == nil, let w = routingWarning else { return nil }
        return Routing.warnings[w]
    }

    /// One sentence on why a request went where it did (ui/app.js `whySentence`).
    public var whySentence: String {
        let pick = Routing.strategyPick[routingStrategy ?? ""] ?? "the routing strategy’s pick"
        let bases: [String: String] = [
            "expiring_reset": "This new session went to an account with a banked reset nearing expiry, to use its allowance before restoring it. The session stays on this account.",
            "new_session": "First request of this session. It went to \(pick), and the session stays there from now on.",
            "session_reused": "Stayed on the account this session is pinned to, so its prompt cache keeps working.",
            "temporary_detour": "The session’s account was busy, so this one request used another. The session stays pinned to its account.",
            "quota_exhausted": "The session’s account ran out of quota, so the session moved here and stays here. Its prompt cache starts cold.",
            "account_disabled": "The session’s account was disabled, so the session moved here and stays here. Its prompt cache starts cold.",
            "account_removed": "The session’s account was removed, so the session moved here and stays here. Its prompt cache starts cold.",
            "model_unavailable": "The session’s account doesn’t serve this model, so the session moved here and stays here.",
            "missing_session": "The client sent no session id, so this request went to \(pick) on its own.",
            "affinity_disabled": "Session affinity is off, so each request goes to \(pick).",
            "retry_same": "Retried on the same account.",
        ]
        let base = bases[routingReason ?? ""] ?? ((account?.isEmpty == false) ? "It went to \(pick)." : "No account could take this request.")
        let words = ["zero", "one", "two", "three", "four", "five"]
        let n = attempts ?? 1
        let tries = n > 1 ? " It took \(n < words.count ? words[n] : String(n)) tries." : ""
        var end = ""
        if status == 499 { end = " \(cancellationText)" }
        else if isError { end = n > 1 ? " It still failed upstream." : " The request failed upstream; nothing was retried." }
        let chunkText = (chunks ?? 0) > 0 ? " It used \(chunks!) embedding chunks." : ""
        return base + tries + chunkText + end
    }
}
