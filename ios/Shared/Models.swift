import Foundation

// The management API's JSON, as the dashboard (ui/app.js, ui/usage.js) reads it.
// Fields the app doesn't need are left out; everything the server may omit is optional,
// so an older or newer Fusebox still decodes. Keys are spelled out (no snake-case
// conversion) because some dictionaries are keyed by model names and account ids.

// MARK: - Counters and series

public struct Counters: Codable, Sendable, Equatable {
    public var requests: Int?
    public var ok: Int?
    public var failed: Int?
    public var failures: Int?
    public var cancelled: Int?
    public var usageMissing: Int?
    public var usagePartial: Int?
    public var inputTokens: Int?
    public var outputTokens: Int?
    public var cacheTokens: Int?

    enum CodingKeys: String, CodingKey {
        case requests, ok, failed, failures, cancelled
        case usageMissing = "usage_missing", usagePartial = "usage_partial"
        case inputTokens = "input_tokens", outputTokens = "output_tokens", cacheTokens = "cache_tokens"
    }

    public init(requests: Int? = nil, ok: Int? = nil, failed: Int? = nil, failures: Int? = nil, cancelled: Int? = nil,
                usageMissing: Int? = nil, usagePartial: Int? = nil, inputTokens: Int? = nil, outputTokens: Int? = nil, cacheTokens: Int? = nil) {
        self.requests = requests; self.ok = ok; self.failed = failed; self.failures = failures; self.cancelled = cancelled
        self.usageMissing = usageMissing; self.usagePartial = usagePartial
        self.inputTokens = inputTokens; self.outputTokens = outputTokens; self.cacheTokens = cacheTokens
    }
}

/// One minute of load: `minute` is minutes since the Unix epoch.
public struct MinuteBucket: Codable, Sendable, Equatable, Identifiable {
    public var minute: Int
    public var requests: Int
    public var failed: Int?
    public var cancelled: Int?
    public var tokens: Int?
    public var inputTokens: Int?
    public var outputTokens: Int?
    public var cacheTokens: Int?
    public var id: Int { minute }

    enum CodingKeys: String, CodingKey {
        case minute, requests, failed, cancelled, tokens
        case inputTokens = "input_tokens", outputTokens = "output_tokens", cacheTokens = "cache_tokens"
    }

    public init(minute: Int, requests: Int = 0, failed: Int? = 0, cancelled: Int? = 0, tokens: Int? = 0) {
        self.minute = minute; self.requests = requests; self.failed = failed; self.cancelled = cancelled; self.tokens = tokens
    }

    public var date: Date { Date(timeIntervalSince1970: TimeInterval(minute) * 60) }
}

// MARK: - Overview

public struct Overview: Codable, Sendable, Equatable {
    public var version: String?
    public var startedAt: Date?
    public var routing: String?
    public var bankedResets: Bool?
    public var sessionAffinity: Bool?
    public var managementKey: Bool?
    public var totals: Counters?
    public var active: Int?
    public var series: [MinuteBucket]?
    public var accounts: AccountSummary?
    public var pools: [Pool]?
    public var models: Int?

    enum CodingKeys: String, CodingKey {
        case version, routing, totals, active, series, accounts, pools, models
        case startedAt = "started_at", bankedResets = "banked_resets", sessionAffinity = "session_affinity"
        case managementKey = "management_key"
    }

    public struct AccountSummary: Codable, Sendable, Equatable {
        public var total: Int?
        public var active: Int?
        public var cooling: Int?
        public var disabled: Int?
    }
}

/// A provider's subscriptions together, per window: the dashboard's combined limits.
public struct Pool: Codable, Sendable, Equatable, Identifiable {
    public var provider: String
    public var window: String
    public var model: String?
    public var accounts: Int
    public var accountIDs: [String]?
    public var reported: Int
    public var used: Double?
    public var pace: Double?
    public var priced: Bool?
    public var runsOutEarly: Bool?
    public var until: Date?
    public var outlook: Outlook?

    public var id: String { "\(provider):\(window):\(model ?? "")" }

    enum CodingKeys: String, CodingKey {
        case provider, window, model, accounts, reported, used, pace, priced, until, outlook
        case accountIDs = "account_ids", runsOutEarly = "runs_out_early"
    }

    public struct Outlook: Codable, Sendable, Equatable {
        public var state: String?
        public var reason: String?
        public var nextResetAt: Date?
        public var exhaustsAt: Date?
        public var recoversAt: Date?
        public var learningUntil: Date?
        public var ratePerDay: Double?
        public var observedHours: Double?

        enum CodingKeys: String, CodingKey {
            case state, reason
            case nextResetAt = "next_reset_at", exhaustsAt = "exhausts_at", recoversAt = "recovers_at"
            case learningUntil = "learning_until", ratePerDay = "rate_per_day", observedHours = "observed_hours"
        }
    }
}

// MARK: - Accounts

public struct QuotaWindow: Codable, Sendable, Equatable, Hashable {
    public var name: String
    public var used: Double?
    public var resetsAt: Date?
    public var model: String?

    enum CodingKeys: String, CodingKey { case name, used, model, resetsAt = "resets_at" }

    public init(name: String, used: Double?, resetsAt: Date? = nil, model: String? = nil) {
        self.name = name; self.used = used; self.resetsAt = resetsAt; self.model = model
    }
}

public struct QuotaLimit: Codable, Sendable, Equatable, Identifiable {
    public var id: String?
    public var name: String
    public var scope: Scope?
    public var usedPercent: Double?
    public var used: Double?
    public var limit: Double?
    public var remaining: Double?
    public var unit: String?
    public var unlimited: Bool?
    public var enabled: Bool?
    public var resetsAt: Date?

    enum CodingKeys: String, CodingKey {
        case id, name, scope, used, limit, remaining, unit, unlimited, enabled
        case usedPercent = "used_percent", resetsAt = "resets_at"
    }

    public struct Scope: Codable, Sendable, Equatable {
        public var kind: String?
    }
}

public struct Quota: Codable, Sendable, Equatable {
    public var windows: [QuotaWindow]?
    public var limits: [QuotaLimit]?
    public var updatedAt: Date?
    public var plan: String?
    public var planName: String?
    public var noFiveHourLimit: Bool?
    public var subscriptionPlan: String?

    enum CodingKeys: String, CodingKey {
        case windows, limits, plan
        case updatedAt = "updated_at", planName = "plan_name", noFiveHourLimit = "no_five_hour_limit"
        case subscriptionPlan = "subscription_plan"
    }

    public init(windows: [QuotaWindow]? = nil, limits: [QuotaLimit]? = nil, plan: String? = nil, planName: String? = nil,
                noFiveHourLimit: Bool? = nil, subscriptionPlan: String? = nil) {
        self.windows = windows; self.limits = limits; self.plan = plan; self.planName = planName
        self.noFiveHourLimit = noFiveHourLimit; self.subscriptionPlan = subscriptionPlan
    }
}

public struct BankedResets: Codable, Sendable, Equatable {
    public var checkedAt: Date?
    public var inventory: Inventory?
    public var error: String?
    public var quote: String?
    public var operation: Operation?
    public var retryable: Bool?

    enum CodingKeys: String, CodingKey { case inventory, error, quote, operation, retryable, checkedAt = "checked_at" }

    public struct Inventory: Codable, Sendable, Equatable {
        public var available: Int?
        public var applicable: Int?
        public var eligible: Bool?
        public var grants: [Grant]?
        public var reason: String?
        public var selectedGrant: String?

        enum CodingKeys: String, CodingKey {
            case available, applicable, eligible, grants, reason, selectedGrant = "selected_grant"
        }
    }

    public struct Grant: Codable, Sendable, Equatable, Identifiable {
        public var id: String
        public var label: String?
        public var remaining: Int?
        public var total: Int?
        public var expiresAt: Date?
        public var startsAt: Date?
        public var clears: [String]?
        public var usable: Bool?
        public var reason: String?

        enum CodingKeys: String, CodingKey {
            case id, label, remaining, total, clears, usable, reason
            case expiresAt = "expires_at", startsAt = "starts_at"
        }
    }

    public struct Operation: Codable, Sendable, Equatable {
        public var requestID: String?
        public var status: String?
        public var message: String?
        public var retryUntil: Date?
        public var automatic: Bool?

        enum CodingKeys: String, CodingKey {
            case status, message, automatic, requestID = "request_id", retryUntil = "retry_until"
        }
    }
}

public struct BankedResetPlan: Codable, Sendable, Equatable {
    public var autoRedeem: Bool?
    public var prioritized: Bool?
    public var message: String?
    public var expiryRedeemAt: Date?

    enum CodingKeys: String, CodingKey {
        case prioritized, message, autoRedeem = "auto_redeem", expiryRedeemAt = "expiry_redeem_at"
    }
}

public struct Account: Codable, Sendable, Equatable, Identifiable, Hashable {
    public var id: String
    public var provider: String
    public var label: String
    public var email: String?
    public var kind: String
    public var group: String?
    public var file: String?
    public var disabled: Bool
    public var cooldowns: [String: String]?
    public var cooldownKinds: [String: String]?
    public var quotaResetUnknown: Bool?
    public var lastError: String?
    public var lastErrorID: String?
    public var lastUsed: Date?
    public var expiresAt: Date?
    public var counters: Counters?
    public var priority: Int?
    public var quota: Quota?
    public var bankedResets: BankedResets?
    public var bankedResetPlan: BankedResetPlan?
    public var models: [String]?

    enum CodingKeys: String, CodingKey {
        case id, provider, label, email, kind, group, file, disabled, cooldowns, counters, priority, quota, models
        case cooldownKinds = "cooldown_kinds", quotaResetUnknown = "quota_reset_unknown"
        case lastError = "last_error", lastErrorID = "last_error_id", lastUsed = "last_used", expiresAt = "expires_at"
        case bankedResets = "banked_resets", bankedResetPlan = "banked_reset_plan"
    }

    public init(id: String, provider: String, label: String, kind: String = "oauth", disabled: Bool = false,
                cooldowns: [String: String]? = nil, cooldownKinds: [String: String]? = nil, quota: Quota? = nil,
                lastError: String? = nil, counters: Counters? = nil) {
        self.id = id; self.provider = provider; self.label = label; self.kind = kind; self.disabled = disabled
        self.cooldowns = cooldowns; self.cooldownKinds = cooldownKinds; self.quota = quota; self.lastError = lastError
        self.counters = counters
    }

    public static func == (a: Account, b: Account) -> Bool {
        a.id == b.id && a.disabled == b.disabled && a.cooldowns == b.cooldowns && a.quota == b.quota
            && a.lastError == b.lastError && a.counters == b.counters && a.priority == b.priority
            && a.bankedResets == b.bankedResets && a.lastUsed == b.lastUsed && a.label == b.label
    }
    public func hash(into hasher: inout Hasher) { hasher.combine(id) }
}

/// One account's last hour, and the coding sessions pinned to it.
public struct AccountActivity: Codable, Sendable, Equatable {
    public var series: [MinuteBucket]?
    public var sessions: [Session]?

    public struct Session: Codable, Sendable, Equatable, Identifiable {
        public var session: String?
        public var lastSeen: Date?
        public var active: Bool?
        public var since: Date?
        public var client: String?
        public var clientApp: String?
        public var keyLabel: String?
        public var model: String?
        public var requests: Int?
        public var id: String { session ?? "\(since?.timeIntervalSince1970 ?? 0)" }

        enum CodingKeys: String, CodingKey {
            case session, active, since, client, model, requests
            case lastSeen = "last_seen", clientApp = "client_app", keyLabel = "key_label"
        }
    }
}

// MARK: - Faults

/// What has tripped (src/push/faults.rs `Fault::json`).
public struct Fault: Codable, Sendable, Equatable, Identifiable, Hashable {
    public var key: String
    public var kind: String
    public var level: String
    public var provider: String?
    public var providerName: String?
    public var accountID: String?
    public var label: String?
    public var title: String
    public var detail: String?
    public var until: Date?
    public var path: String?
    public var badge: Bool?

    public var id: String { key }

    enum CodingKeys: String, CodingKey {
        case key, kind, level, provider, label, title, detail, until, path, badge
        case providerName = "provider_name", accountID = "account_id"
    }

    public init(key: String, kind: String, level: String, provider: String? = nil, providerName: String? = nil,
                accountID: String? = nil, label: String? = nil, title: String, detail: String? = nil,
                until: Date? = nil, path: String? = nil, badge: Bool? = true) {
        self.key = key; self.kind = kind; self.level = level; self.provider = provider; self.providerName = providerName
        self.accountID = accountID; self.label = label; self.title = title; self.detail = detail; self.until = until
        self.path = path; self.badge = badge
    }

    /// Pace warnings don't count on the badge: they ask for attention, not action.
    public var countsOnBadge: Bool { badge ?? (kind != "pace") }
}

// MARK: - Requests

public struct RequestLog: Codable, Sendable, Equatable, Identifiable, Hashable {
    public var id: Int
    public var ts: Date
    public var client: String?
    public var clientApp: String?
    public var source: Source?
    public var keyLabel: String?
    public var provider: String?
    public var model: String?
    public var account: String?
    public var accountID: String?
    public var sessionID: String?
    public var sessionSource: String?
    public var routingStrategy: String?
    public var routingReason: String?
    public var routingWarning: String?
    public var cachePolicy: CachePolicy?
    public var status: Int?
    public var latencyMs: Int?
    public var ttftMs: Int?
    public var inputTokens: Int?
    public var outputTokens: Int?
    public var cacheTokens: Int?
    public var usageCompleteness: String?
    public var failureKind: String?
    public var stream: Bool?
    public var transport: String?
    public var attempts: Int?
    public var chunks: Int?
    public var error: String?

    enum CodingKeys: String, CodingKey {
        case id, ts, client, source, provider, model, account, status, stream, transport, attempts, chunks, error
        case clientApp = "client_app", keyLabel = "key_label", accountID = "account_id", sessionID = "session_id"
        case sessionSource = "session_source", routingStrategy = "routing_strategy", routingReason = "routing_reason"
        case routingWarning = "routing_warning", cachePolicy = "cache_policy", latencyMs = "latency_ms", ttftMs = "ttft_ms"
        case inputTokens = "input_tokens", outputTokens = "output_tokens", cacheTokens = "cache_tokens"
        case usageCompleteness = "usage_completeness", failureKind = "failure_kind"
    }

    public struct Source: Codable, Sendable, Equatable, Hashable {
        public var ip: String?
        public var via: String?
        public var login: String?
    }

    public struct CachePolicy: Codable, Sendable, Equatable, Hashable {
        public var requested: String?
        public var applied: String?
        public var keyScope: String?
        enum CodingKeys: String, CodingKey { case requested, applied, keyScope = "key_scope" }
    }

    public init(id: Int, ts: Date, provider: String? = nil, model: String? = nil, account: String? = nil,
                accountID: String? = nil, status: Int? = 200, routingReason: String? = nil) {
        self.id = id; self.ts = ts; self.provider = provider; self.model = model; self.account = account
        self.accountID = accountID; self.status = status; self.routingReason = routingReason
    }

    public static func == (a: RequestLog, b: RequestLog) -> Bool { a.id == b.id && a.ts == b.ts }
    public func hash(into hasher: inout Hasher) { hasher.combine(id) }
}

// MARK: - Live socket

/// Busy accounts: in-flight requests and pinned coding sessions.
public struct AccountLoad: Codable, Sendable, Equatable {
    public var inFlight: Int?
    public var sessions: Int?
    enum CodingKeys: String, CodingKey { case sessions, inFlight = "in_flight" }
}

public struct LiveTick: Codable, Sendable, Equatable {
    public var active: Int?
    public var totals: Counters?
}

/// One `/api/live` message.
public enum LiveMessage: Sendable, Equatable {
    case request(RequestLog)
    case load([String: AccountLoad])
    case faults([Fault])
    case tick(LiveTick)
    case accounts
    case models
    case other(String)

    public static func decode(_ data: Data) throws -> LiveMessage {
        struct Envelope: Decodable { var type: String }
        let decoder = JSONDecoder.fusebox
        let type = try decoder.decode(Envelope.self, from: data).type
        func payload<T: Decodable>(_: T.Type) throws -> T {
            try decoder.decode(LivePayload<T>.self, from: data).data
        }
        switch type {
        case "request": return .request(try payload(RequestLog.self))
        case "load": return .load(try payload([String: AccountLoad].self))
        case "faults": return .faults(try payload([Fault].self))
        case "tick": return .tick(try payload(LiveTick.self))
        case "accounts": return .accounts
        case "models": return .models
        default: return .other(type)
        }
    }
}

private struct LivePayload<U: Decodable>: Decodable { var data: U }

// MARK: - Push

public struct PushStatus: Codable, Sendable, Equatable {
    public var publicKey: String?
    public var subscriptions: [Device]
    /// Missing on builds before APNs support: treat as false.
    public var apns: Bool?
    /// The push relay this Fusebox sends through, or null when it's turned off; missing on
    /// builds before the relay. `PushPath.choose` decides from both.
    public var relay: String?

    enum CodingKeys: String, CodingKey { case subscriptions, apns, relay, publicKey = "public_key" }

    public var apnsAvailable: Bool { apns ?? false }

    public struct Device: Codable, Sendable, Equatable, Identifiable {
        public var id: String
        public var label: String?
        /// "web" or "apns"; missing on builds before APNs support, where every device is a browser.
        public var kind: String?
        /// For app devices: "direct" or "relay".
        public var via: String?
        public var createdAt: Date?

        enum CodingKeys: String, CodingKey { case id, label, kind, via, createdAt = "created_at" }

        public var resolvedKind: String { kind ?? "web" }
    }
}

public struct APNsRegistration: Codable, Sendable, Equatable {
    public var token: String
    public var environment: String
    public var label: String
}

/// How a Fusebox reaches this app (docs/notifications.md, “The iPhone app”), from `GET /api/push`:
/// directly when it holds the app's push key (`apns`), else through the push relay it names.
public enum PushPath: Equatable, Sendable {
    case direct
    case relay(URL)
    /// An older Fusebox, or one with `apns` unset and the relay turned off.
    case unavailable

    public static func choose(_ status: PushStatus) -> PushPath {
        if status.apns == true { return .direct }
        if let text = status.relay?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty,
           let url = URL(string: text), let scheme = url.scheme?.lowercased(), ["https", "http"].contains(scheme),
           url.host() != nil {
            return .relay(url)
        }
        return .unavailable
    }
}

/// `POST /api/push/apns` for a device reached through the push relay.
public struct RelayRegistration: Codable, Sendable, Equatable {
    public struct Relay: Codable, Sendable, Equatable {
        public var handle: String
        /// The 32-byte key, standard base64.
        public var key: String
    }
    public var relay: Relay
    public var label: String
}

public struct IDResponse: Codable, Sendable { public var id: String }
public struct ErrorResponse: Codable, Sendable { public var error: String? }

// MARK: - Usage (docs/usage-ui-contract.md)

public struct UsageTokens: Codable, Sendable, Equatable {
    public var input: Int?
    public var output: Int?
    public var cacheRead: Int?
    public var cacheWrite: Int?
    public var reasoning: Int?

    enum CodingKeys: String, CodingKey { case input, output, reasoning, cacheRead = "cache_read", cacheWrite = "cache_write" }
}

/// The aggregate shape shared by totals, trend rows and breakdown rows.
public struct UsageAggregate: Codable, Sendable, Equatable {
    public var observations: Int?
    public var estimatedCostNanos: Int64?
    public var unpriced: Int?
    public var partial: Int?
    public var missingUsage: Int?
    public var pricingPartial: Int?
    public var tokens: UsageTokens?
    public var historyOnly: Int?
    public var matched: Int?
    public var weakIdentity: Int?
    public var aggregationOverflow: Bool?
    // Trend rows
    public var date: String?
    public var group: String?
    // Breakdown rows
    public var id: String?
    public var accounts: Int?
    public var provider: String?

    enum CodingKeys: String, CodingKey {
        case observations, unpriced, partial, tokens, matched, date, group, id, accounts, provider
        case estimatedCostNanos = "estimated_cost_nanos", missingUsage = "missing_usage", pricingPartial = "pricing_partial"
        case historyOnly = "history_only", weakIdentity = "weak_identity", aggregationOverflow = "aggregation_overflow"
    }

    public init(observations: Int? = nil, estimatedCostNanos: Int64? = nil, unpriced: Int? = nil, date: String? = nil,
                group: String? = nil, id: String? = nil, provider: String? = nil) {
        self.observations = observations; self.estimatedCostNanos = estimatedCostNanos; self.unpriced = unpriced
        self.date = date; self.group = group; self.id = id; self.provider = provider
    }
}

public struct UsageDashboard: Codable, Sendable, Equatable {
    public var facets: Facets?
    public var pricing: Pricing?
    public var combined: Combined?

    public struct Facets: Codable, Sendable, Equatable {
        public var providers: [String]?
        public var models: [String]?
        public var accounts: [Labelled]?
        public var clients: [Labelled]?
    }

    public struct Labelled: Codable, Sendable, Equatable {
        public var id: String?
        public var label: String?
    }

    public struct Pricing: Codable, Sendable, Equatable {
        public var version: String?
        public var basis: String?
    }

    public struct Combined: Codable, Sendable, Equatable {
        public var basis: String?
        public var totals: UsageAggregate?
        public var proxyFirstEventAtMs: Int64?
        public var stack: String?
        public var trend: [UsageAggregate]?
        public var trends: Trends?
        public var breakdowns: Breakdowns?

        enum CodingKeys: String, CodingKey {
            case basis, totals, stack, trend, trends, breakdowns, proxyFirstEventAtMs = "proxy_first_event_at_ms"
        }
    }

    public struct Trends: Codable, Sendable, Equatable {
        public var provider: [UsageAggregate]?
        public var model: [UsageAggregate]?
    }

    public struct Breakdowns: Codable, Sendable, Equatable {
        public var provider: [UsageAggregate]?
        public var model: [UsageAggregate]?
        public var account: [UsageAggregate]?
        public var client: [UsageAggregate]?
    }
}

// MARK: - Decoder

extension JSONDecoder {
    /// Fusebox writes RFC 3339 times with up to nine fractional digits and either `Z` or an offset.
    public static var fusebox: JSONDecoder {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .custom { decoder in
            let c = try decoder.singleValueContainer()
            if let ms = try? c.decode(Double.self) { return Date(timeIntervalSince1970: ms / 1000) }
            let s = try c.decode(String.self)
            guard let date = FuseboxDate.parse(s) else {
                throw DecodingError.dataCorruptedError(in: c, debugDescription: "Not an RFC 3339 time: \(s)")
            }
            return date
        }
        return d
    }
}

extension JSONEncoder {
    public static var fusebox: JSONEncoder {
        let e = JSONEncoder()
        e.dateEncodingStrategy = .custom { date, encoder in
            var c = encoder.singleValueContainer()
            try c.encode(FuseboxDate.format(date))
        }
        return e
    }
}
