package com.digitalpals.fusebox.data

import java.time.Duration
import java.time.Instant
import java.time.ZoneId
import java.time.temporal.ChronoUnit
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** Provider display names and drawable logo identifiers (matching ui/app.js & ios/Shared/QuotaRules.swift). */
object ProviderInfo {
    val names: Map<String, String> = mapOf(
        "claude" to "Claude",
        "codex" to "Codex",
        "gemini" to "Gemini",
        "vertex" to "Vertex AI",
        "antigravity" to "Antigravity",
        "kimi" to "Kimi",
        "xai" to "Grok",
        "meta" to "Meta",
        "devin" to "Devin",
        "openai-compat" to "Compatible",
    )

    val deviceCode: Set<String> = setOf("kimi", "xai", "meta")

    private val knownLogos = setOf(
        "claude", "codex", "gemini", "vertex", "antigravity", "xai", "kimi", "meta", "devin",
    )

    private val compatLogos = listOf(
        "openrouter" to "openrouter",
        "ollama" to "ollama",
        "lmstudio" to "lmstudio",
        "deepseek" to "deepseek",
        "groq" to "groq",
        "mistral" to "mistral",
        "qwen" to "qwen",
        "dashscope" to "qwen",
        "moonshot" to "kimi",
        "kimi" to "kimi",
        "grok" to "xai",
        "xai" to "xai",
        "gemini" to "gemini",
        "anthropic" to "claude",
        "claude" to "claude",
        "openai" to "codex",
    )

    fun name(provider: String?): String =
        if (provider.isNullOrEmpty()) "—" else names[provider] ?: provider

    /** Returns a logo key such as `"logo-claude"`, `"logo-xai-api"`, `"logo-compat"`. */
    fun logo(provider: String?, group: String? = null, kind: String? = null): String {
        val p = provider ?: ""
        if (p == "xai" && kind == "api-key") return "logo-xai-api"
        if (p in knownLogos) return "logo-$p"
        val g = (group ?: "").lowercase()
        val hit = compatLogos.firstOrNull { g.contains(it.first) }
        return if (hit != null) "logo-${hit.second}" else "logo-compat"
    }

    fun usageKey(id: String?): String = when (id) {
        "anthropic" -> "claude"
        "openai" -> "codex"
        "google" -> "gemini"
        else -> id ?: ""
    }
}

enum class QuotaDisplay(val raw: String, val word: String) {
    USED("used", "used"),
    REMAINING("remaining", "left"),
    ;

    companion object {
        val standard: QuotaDisplay = REMAINING
        fun fromRaw(raw: String?): QuotaDisplay? = entries.firstOrNull { it.raw == raw }
    }
}

data class QuotaReading(val used: Double) {
    enum class Level { NORMAL, WARN, ERR }

    val clampedUsed: Double = max(0.0, min(100.0, used))
    val remaining: Double get() = 100.0 - clampedUsed
    val level: Level
        get() = when {
            clampedUsed >= 95.0 -> Level.ERR
            clampedUsed >= 75.0 -> Level.WARN
            else -> Level.NORMAL
        }
    val exhausted: Boolean get() = clampedUsed >= 100.0

    val status: String
        get() = when {
            exhausted -> "Used up"
            level == Level.ERR -> "Almost used up"
            level == Level.WARN -> "Running low"
            else -> "Healthy"
        }

    fun value(mode: QuotaDisplay): Double = if (mode == QuotaDisplay.USED) clampedUsed else remaining

    companion object {
        fun of(w: QuotaWindow?, now: Instant = Instant.now()): QuotaReading? {
            if (w == null) return null
            val u = w.used ?: return null
            if (!u.isFinite()) return null
            if (w.resetsAt != null && !w.resetsAt.isAfter(now)) return null
            return QuotaReading(max(0.0, min(100.0, u)))
        }
    }
}

/**
 * 20 segments of 5%, lit for the shown share (used or left), coloured by how much is *used*
 * in either mode, plus an even-pace tick mirrored when showing what's left.
 */
data class MeterModel(
    val lit: Int,
    val level: QuotaReading.Level?,
    val pace: Double?,
) {
    fun isLit(segment: Int): Boolean = segment < lit

    companion object {
        const val SEGMENTS = 20

        fun build(reading: QuotaReading?, pace: Double?, mode: QuotaDisplay): MeterModel {
            if (reading == null) return MeterModel(lit = 0, level = null, pace = null)
            val lit = min(SEGMENTS, max(0, (reading.value(mode) / 5.0).roundToInt()))
            val normalizedPace = pace?.takeIf { it.isFinite() }?.let { min(1.0, max(0.0, it)) }
                ?.let { if (mode == QuotaDisplay.USED) it else 1.0 - it }
            return MeterModel(lit = lit, level = reading.level, pace = normalizedPace)
        }

        /** Child model rows ("↳ Fable") take their state colour as a whole only when running low. */
        fun childLevel(reading: QuotaReading?): QuotaReading.Level? =
            reading?.level?.takeIf { it != QuotaReading.Level.NORMAL }
    }
}

object Quotas {
    private val hoursRe = Regex("^(\\d+)h$")
    private val unitRe = Regex("^(\\d+)([dms])$")

    fun seconds(w: QuotaWindow?): Double? {
        val name = w?.name ?: return null
        hoursRe.matchEntire(name)?.let { return it.groupValues[1].toDouble() * 3600.0 }
        unitRe.matchEntire(name)?.let { m ->
            val n = m.groupValues[1].toDouble()
            return when (m.groupValues[2]) {
                "d" -> n * 86_400.0
                "m" -> n * 60.0
                else -> n
            }
        }
        if (name == "day") return 86_400.0
        if (name == "week" || name.startsWith("week")) return 7.0 * 86_400.0
        return null
    }

    fun elapsed(w: QuotaWindow?, now: Instant = Instant.now()): Double? {
        val len = seconds(w) ?: return null
        val reset = w?.resetsAt ?: return null
        val left = Duration.between(now, reset).toMillis() / 1000.0
        return if (left in 0.0..len) 1.0 - left / len else null
    }

    fun runsOut(w: QuotaWindow?, now: Instant = Instant.now()): Instant? {
        if (w == null) return null
        val len = seconds(w) ?: return null
        val reset = w.resetsAt ?: return null
        val t = elapsed(w, now) ?: return null
        val r = QuotaReading.of(w, now) ?: return null
        if (t < 0.25 || r.clampedUsed < 20.0 || r.clampedUsed >= 100.0) return null
        val perSecond = r.clampedUsed / (t * len)
        val outMs = ((100.0 - r.clampedUsed) / perSecond * 1000.0).toLong()
        val out = now.plusMillis(outMs)
        return if (out.isBefore(reset)) out else null
    }

    fun isShort(w: QuotaWindow): Boolean = hoursRe.matches(w.name)

    fun label(w: QuotaWindow?, short: Boolean): String {
        if (w != null && w.name !in setOf("week", "day") && !(short && isShort(w) && w.name == "5h")) {
            return w.name.uppercase()
        }
        return when {
            short -> "5H"
            w?.name == "day" -> "DAY"
            else -> "WK"
        }
    }

    fun title(w: QuotaWindow): String {
        if (w.group != null && w.model == null) {
            return "${w.group} ${title(w.copy(group = null)).lowercase()}"
        }
        if (isShort(w)) return "${w.name.dropLast(1)}-hour"
        if (w.name == "day") return "Daily"
        if (w.name == "week") return "Weekly"
        if (w.name.startsWith("week") && w.name.length > 5) return "Weekly · ${w.name.drop(5)}"
        return w.name
    }

    fun scopedTitle(w: QuotaWindow): String {
        val base = QuotaWindow(name = w.name.split(" ").firstOrNull() ?: "")
        return "${Fmt.capitalized(w.model ?: "")} ${title(base).lowercase()}"
    }
}

data class AccountState(
    val kind: Kind,
    val word: String,
    val model: String? = null,
    val until: Instant? = null,
    val cooldownKind: String? = null,
    val resetUnknown: Boolean = false,
    val signin: Boolean = false,
) {
    enum class Kind { READY, COOLING, ERROR, DISABLED }
}

private val signinErrorRegex = Regex(
    "invalid_grant|refresh token|sign in again|re-?authenticat|unauthori[sz]ed|\\b401\\b|token (?:has )?expired|expired token|revoked",
    RegexOption.IGNORE_CASE,
)

private val codexPlans = mapOf("chatgptprolite" to "Pro 100", "chatgptpro" to "Pro 200", "chatgptpromax" to "Pro 500")
private val claudePlans = mapOf("max_5x" to "Max", "max_20x" to "Ultra")

val Account.providerName: String
    get() = when {
        provider == "openai-compat" -> group ?: "Compatible"
        kind == "api-key" && provider == "xai" -> "xAI"
        kind == "api-key" && provider == "codex" -> "OpenAI"
        else -> ProviderInfo.name(provider)
    }

val Account.authName: String
    get() = when {
        kind == "service-account" -> "Service account"
        kind == "api-key" -> "API key"
        provider in ProviderInfo.deviceCode -> "Device code"
        else -> "OAuth"
    }

val Account.planName: String?
    get() {
        if (provider == "codex") {
            quota?.subscriptionPlan?.let { codexPlans[it] }?.let { return it }
        }
        if (provider == "antigravity" && !quota?.planName.isNullOrEmpty()) {
            return quota!!.planName!!.replace(Regex("^Google AI\\s+", RegexOption.IGNORE_CASE), "")
        }
        if (!quota?.planName.isNullOrEmpty()) return quota!!.planName
        if (provider == "claude") {
            quota?.plan?.let { claudePlans[it] }?.let { return it }
        }
        val plan = quota?.plan?.takeIf { it.isNotEmpty() } ?: return null
        return Fmt.capitalized(plan.replace(Regex("[_-]+"), " "))
    }

fun Account.renewalRow(now: Instant = Instant.now(), zone: ZoneId = ZoneId.systemDefault()): Pair<String, String>? {
    val r = quota?.renewal ?: return null
    val day = Fmt.dayMonth(r.at, zone)
    if (r.manual == true) {
        val renewalDate = r.at.atZone(zone).toLocalDate()
        val endOfDay = renewalDate.plusDays(1).atStartOfDay(zone).toInstant()
        if (!endOfDay.isAfter(now)) return null
        val today = now.atZone(zone).toLocalDate()
        val days = ChronoUnit.DAYS.between(today, renewalDate)
        val whenStr = when {
            days <= 0 -> "today"
            days == 1L -> "tomorrow"
            else -> "in $days days"
        }
        if (r.ends == true) return "Ends" to "$day · $whenStr · cancelled"
        val note = if (r.estimated == true) "plan changed, check the date" else null
        return "Renews" to listOfNotNull(day, whenStr, r.period ?: "monthly", note).joinToString(" · ")
    }
    if (!r.at.isAfter(now)) return null
    val left = "in ${Fmt.left(r.at, now)}"
    if (r.ends == true) return "Ends" to "$day · $left · cancelled"
    if (r.estimated == true) return "Renews" to "about $day · $left · estimated"
    return "Renews" to listOfNotNull("$day, ${Fmt.hm(r.at, zone)}", left, r.period).joinToString(" · ")
}

val Account.logo: String get() = ProviderInfo.logo(provider, group, kind)

val Account.metered: Boolean
    get() = (kind == "oauth" && provider in setOf("claude", "codex", "meta")) ||
        !quota?.windows.isNullOrEmpty() ||
        !quota?.limits.isNullOrEmpty()

fun Account.window(short: Boolean, now: Instant = Instant.now()): QuotaWindow? =
    quota?.windows.orEmpty()
        .filter { it.model == null && QuotaReading.of(it, now) != null && Quotas.isShort(it) == short }
        .maxByOrNull { it.used ?: 0.0 }

fun Account.noShortLimit(now: Instant = Instant.now()): Boolean =
    (quota?.noFiveHourLimit == true) && window(short = true, now = now) == null

val Account.subtitle: String
    get() = "$providerName · ${planName ?: authName}" + if (noShortLimit()) " · no 5h limit" else ""

fun Account.scopedWindows(now: Instant = Instant.now()): List<QuotaWindow> =
    quota?.windows.orEmpty()
        .filter { it.model != null && QuotaReading.of(it, now) != null }
        .sortedWith(compareBy({ it.model ?: "" }, { Quotas.seconds(it) ?: 0.0 }, { it.name }))

fun Account.state(now: Instant = Instant.now()): AccountState {
    if (disabled) return AccountState(AccountState.Kind.DISABLED, "Disabled")
    val cds = cooldowns.orEmpty()
        .mapNotNull { (k, v) -> FuseboxDate.parse(v)?.let { k to it } }
        .filter { it.second.isAfter(now) }
        .sortedByDescending { it.second }
    cds.firstOrNull()?.let { (model, until) ->
        val ck = cooldownKinds?.get(model) ?: "rate_limit"
        if (ck == "quota" && model == "*" && quotaResetUnknown == true) {
            return AccountState(
                kind = AccountState.Kind.COOLING,
                word = "Quota used up",
                model = model,
                until = until,
                cooldownKind = ck,
                resetUnknown = true,
            )
        }
        return AccountState(
            kind = AccountState.Kind.COOLING,
            word = "Cooling",
            model = model,
            until = until,
            cooldownKind = ck,
        )
    }
    val err = lastError
    if (!err.isNullOrEmpty()) {
        val signin = kind != "api-key" && signinErrorRegex.containsMatchIn(err)
        return AccountState(AccountState.Kind.ERROR, "Error", signin = signin)
    }
    return AccountState(AccountState.Kind.READY, "Ready")
}

fun Account.statusWord(inFlight: Int = 0, now: Instant = Instant.now()): String {
    val st = state(now)
    if (st.kind == AccountState.Kind.COOLING) {
        return if (st.resetUnknown) st.word else "Cooling ${Fmt.left(st.until ?: now, now)}"
    }
    if (st.kind == AccountState.Kind.READY && inFlight > 0) return "Serving ${Fmt.count(inFlight)}"
    return st.word
}

fun Account.cooldownReason(model: String): String = when (cooldownKinds?.get(model)) {
    "quota" -> "Usage limit used up"
    "checking" -> "Checking quota"
    "credit" -> "API credit used up"
    else -> "Rate limited"
}

fun Account.windowSpent(w: QuotaWindow?): Boolean {
    val renews = w?.resetsAt?.minusSeconds(60) ?: return false
    return cooldowns.orEmpty().any { (model, until) ->
        cooldownKinds?.get(model) == "quota" &&
            w.coversModel(model) &&
            (FuseboxDate.parse(until)?.let { !it.isBefore(renews) } == true)
    }
}

fun Account.hasBankedResets(enabled: Boolean): Boolean =
    enabled && kind == "oauth" && provider in setOf("claude", "codex")

val Account.bankedNeedsReview: Boolean
    get() = bankedResets?.operation?.status in setOf("pending", "unknown")

val Account.bankedCount: Int?
    get() = if (bankedResets?.error == null) bankedResets?.inventory?.available else null

val Account.bankedLabel: String
    get() {
        if (bankedNeedsReview) return "Reset needs review"
        val n = bankedCount ?: return "Resets unavailable"
        return "$n reset${if (n == 1) "" else "s"} banked"
    }

fun Account.bankedPolicyText(now: Instant = Instant.now()): String {
    val plan = bankedResetPlan
    if (plan != null && plan.autoRedeem == true) {
        var s = (if (plan.prioritized == true) "Prioritized for new work. " else "") + (plan.message ?: "")
        plan.expiryRedeemAt?.let { s += " Expiry redemption starts ${Fmt.whenText(it, now)}." }
        return s
    }
    return "Manual use asks for confirmation. Automatic redemption is off for this account."
}

// MARK: - Combined weekly limits

data class ScopedPool(
    val pool: Pool,
    val nextReset: Instant?,
) {
    val id: String get() = pool.id
    val name: String get() = Fmt.capitalized(pool.model ?: "")
    val reading: QuotaReading? get() = WeeklyPool.reading(pool)
}

data class WeeklyPool(
    val pool: Pool,
    val accounts: List<Account>,
    val scoped: List<ScopedPool> = emptyList(),
) {
    val id: String get() = pool.id
    val providerName: String get() = accounts.firstOrNull()?.providerName ?: ProviderInfo.name(pool.provider)
    val logo: String get() = accounts.firstOrNull()?.logo ?: ProviderInfo.logo(pool.provider)
    val reading: QuotaReading? get() = reading(pool)

    val status: Pair<String, String>
        get() = when (pool.outlook?.state) {
            "run_out" -> "Quota gap expected" to "err"
            "tight" -> "Low reserve" to "warn"
            "on_track" -> "Expected to last" to "ok"
            "learning" -> "Learning usage" to ""
            else -> "Forecast unavailable" to ""
        }

    fun detail(now: Instant = Instant.now()): String {
        val o = pool.outlook
        return when (o?.state) {
            "run_out" -> {
                if (o.exhaustsAt != null && o.recoversAt != null) {
                    "Runs out ${Fmt.whenText(o.exhaustsAt, now)} · Back ${Fmt.whenText(o.recoversAt, now)}"
                } else {
                    "Expected to exhaust weekly quota"
                }
            }
            "learning" -> {
                if (o.learningUntil != null) "Estimate after ${Fmt.whenText(o.learningUntil, now)}"
                else "Needs 6h of recent quota readings"
            }
            "on_track", "tight" -> "Next 7 days at recent usage"
            else -> when (o?.reason) {
                "stale_readings" -> "Quota readings are stale"
                "missing_quota" -> "Waiting for every account’s weekly quota"
                "missing_reset" -> "Waiting for every account’s reset time"
                "awaiting_reset" -> "Waiting for reset confirmation"
                "unavailable_account" -> "An account needs to sign in again"
                else -> "Waiting for quota history"
            }
        }
    }

    val countText: String
        get() {
            val count = if (pool.reported < pool.accounts) {
                "${Fmt.count(pool.reported)} of ${Fmt.plural(pool.accounts, "account")} reporting"
            } else {
                Fmt.plural(pool.accounts, "account")
            }
            val counts = mutableMapOf<String, Int>()
            for (a in accounts) {
                a.planName?.let { counts[it] = (counts[it] ?: 0) + 1 }
            }
            val plans = counts.entries.sortedWith(compareByDescending<Map.Entry<String, Int>> { it.value }.thenBy { it.key })
            val known = plans.sumOf { it.value }
            if (plans.size == 1 && known == pool.accounts) return "$count · ${plans[0].key}"
            val mix = plans.joinToString(", ") { "${Fmt.count(it.value)} ${it.key}" }
            if (mix.isNotEmpty() && known == pool.accounts && pool.reported == pool.accounts) return mix
            return listOf(count, mix).filter { it.isNotEmpty() }.joinToString(" · ")
        }

    companion object {
        fun reading(p: Pool): QuotaReading? {
            if (p.accounts <= 0 || p.reported != p.accounts) return null
            val used = p.used ?: return null
            return QuotaReading(max(0.0, min(100.0, used)))
        }

        fun weekly(overview: Overview?, accounts: List<Account>, now: Instant = Instant.now()): List<WeeklyPool> {
            val all = overview?.pools.orEmpty()
            return all.filter { it.window == "week" && it.model == null }.mapNotNull { p ->
                val list = p.accountIDs.orEmpty().mapNotNull { id -> accounts.firstOrNull { it.id == id } }
                if (list.isEmpty()) return@mapNotNull null
                val scoped = all.filter {
                    it.provider == p.provider && it.model != null && it.window.split(" ").firstOrNull() == "week"
                }.sortedBy { it.model ?: "" }.map {
                    ScopedPool(pool = it, nextReset = nextReset(it, accounts, now))
                }
                WeeklyPool(pool = p, accounts = list, scoped = scoped)
            }
        }

        fun nextReset(p: Pool, accounts: List<Account>, now: Instant = Instant.now()): Instant? =
            p.accountIDs.orEmpty()
                .mapNotNull { id -> accounts.firstOrNull { it.id == id } }
                .mapNotNull { a ->
                    a.quota?.windows.orEmpty().firstOrNull { it.name == p.window && it.model == p.model }?.resetsAt
                }
                .filter { it.isAfter(now) }
                .minOrNull()

        fun basisText(pools: List<WeeklyPool>): String {
            val priced = pools.filter { it.pool.priced == true }.map { it.providerName }
            val equal = pools.filter { it.pool.priced != true }.map { it.providerName }
            return when {
                equal.isEmpty() -> "Each provider’s subscriptions together, weighted by plan price"
                priced.isEmpty() -> "Each provider’s subscriptions together, weighted equally"
                else -> "${priced.joinToString(", ")} weighted by plan price, ${equal.joinToString(", ")} equally"
            }
        }
    }
}

// MARK: - Requests & Routing

object Routing {
    val reasons = mapOf(
        "expiring_reset" to "Expiring reset",
        "new_session" to "New session",
        "session_reused" to "Same session",
        "quota_exhausted" to "Moved: quota used up",
        "account_disabled" to "Moved: account disabled",
        "account_removed" to "Moved: account removed",
        "model_unavailable" to "Moved: model not served",
        "temporary_detour" to "Detour: account busy",
        "missing_session" to "No session id",
        "affinity_disabled" to "Affinity off",
        "retry_same" to "Retried same account",
    )

    val strategyPick = mapOf(
        "least-used" to "the account with the most quota left",
        "smart-quota" to "the account with an expiring banked reset, or the earliest weekly renewal",
        "round-robin" to "the next account in turn",
        "fill-first" to "the first available account",
    )

    val clients = mapOf("openai" to "OpenAI", "responses" to "Responses", "claude" to "Anthropic", "gemini" to "Gemini")
    val endpoints = mapOf(
        "openai" to "/v1/chat/completions",
        "responses" to "/v1/responses",
        "claude" to "/v1/messages",
        "gemini" to "/v1beta/models",
    )

    val warnings = mapOf(
        "missing_session_id" to ("No session ID" to "The client supplied no stable session identifier. Later requests may use another account and lose cache reuse."),
        "connection_only" to ("Connection only" to "This assignment lasts for the WebSocket connection. A reconnect without a stable session identifier may use another account."),
        "response_id_only" to ("Response ID only" to "This assignment relies on a previous response ID held in memory. A stable session identifier is needed to preserve it across server restarts."),
        "affinity_disabled" to ("Affinity off" to "Session affinity is disabled. Requests from this session may use different accounts."),
        "inferred_session" to ("Inferred session" to "The client sent no session ID, so this conversation was recognised by its first message. Conversations that start the same way share an account; send x-fusebox-session-id to keep them apart."),
    )

    val routingText = mapOf(
        "least-used" to ("go to the account with the most quota left" to "go to the most quota left"),
        "smart-quota" to ("use expiring banked resets first, then the earliest weekly renewal, reserving quota for active sessions" to "prefer expiring resets, then weekly renewals"),
        "round-robin" to ("take turns across accounts" to "take turns across accounts"),
        "fill-first" to ("go to the first available account" to "go to the first available account"),
    )

    fun sentence(overview: Overview?, short: Boolean): String {
        val r = overview?.routing ?: return ""
        val t = routingText[r] ?: return ""
        val who = if (overview.sessionAffinity == false) "requests" else "new sessions"
        return "$who ${if (short) t.second else t.first}"
    }
}

private val reroutedRegex = Regex("^(quota_exhausted|account_|model_unavailable|temporary_detour)")

val RequestLog.isError: Boolean get() = (status ?: 0) >= 400 && status != 499
val RequestLog.isRerouted: Boolean get() = routingReason?.let { reroutedRegex.containsMatchIn(it) } == true
val RequestLog.reasonText: String? get() = routingReason?.let { Routing.reasons[it] ?: it.replace("_", " ") }
val RequestLog.clientName: String get() = Routing.clients[client ?: ""] ?: client ?: "—"
val RequestLog.clientDescription: String get() = clientApp ?: "$clientName format"
val RequestLog.transportTag: String?
    get() = when (transport) {
        "ws" -> "ws"
        "images" -> "image"
        "video" -> "video"
        else -> null
    }
val RequestLog.endpoint: String
    get() {
        val base = when (transport) {
            "images" -> "/v1/images"
            "video" -> "/v1/videos"
            else -> Routing.endpoints[client ?: ""] ?: ""
        }
        return base + if (transport == "ws") " (websocket)" else ""
    }
val RequestLog.usageState: String
    get() = if (usageCompleteness in setOf("complete", "partial", "missing")) {
        usageCompleteness!!
    } else if ((inputTokens ?: 0L) + (outputTokens ?: 0L) + (cacheTokens ?: 0L) > 0L) {
        "partial"
    } else {
        "missing"
    }

val RequestLog.cancellationText: String
    get() = when (failureKind) {
        "downstream_disconnect" -> "Connection closed before completion."
        "downstream_write_timeout" -> "Timed out sending the response to the client."
        "downstream_write_error" -> "Connection error while sending the response to the client."
        "downstream_read_error" -> "Connection error while reading from the client."
        "downstream_queue_limit" -> "Connection ended because the pending request limit was reached."
        "server_shutdown" -> "Fusebox stopped or restarted before the answer was complete."
        else -> "Connection ended before completion."
    }

val RequestLog.errorLine: String?
    get() = when {
        status == 499 -> cancellationText
        !error.isNullOrEmpty() && (status ?: 0) >= 400 -> error
        else -> null
    }

val RequestLog.tokensText: String
    get() = if (usageState == "missing") {
        "Unknown · provider usage not reported"
    } else {
        val p = if (usageState == "partial") "≥" else ""
        "${p}${Fmt.count(inputTokens)} in · ${p}${Fmt.count(outputTokens)} out · ${p}${Fmt.count(cacheTokens)} cached" +
            if (usageState == "partial") " · partial usage" else ""
    }

val RequestLog.sessionWarning: Pair<String, String>?
    get() = if (sessionID == null) routingWarning?.let { Routing.warnings[it] } else null

val RequestLog.whySentence: String
    get() {
        val pick = Routing.strategyPick[routingStrategy ?: ""] ?: "the routing strategy’s pick"
        val base = when (routingReason) {
            "expiring_reset" -> "This new session went to an account with a banked reset nearing expiry, to use its allowance before restoring it. The session stays on this account."
            "new_session" -> "First request of this session. It went to $pick, and the session stays there from now on."
            "session_reused" -> "Stayed on the account this session is pinned to, so its prompt cache keeps working."
            "temporary_detour" -> "The session’s account was busy, so this one request used another. The session stays pinned to its account."
            "quota_exhausted" -> "The session’s account ran out of quota, so the session moved here and stays here. Its prompt cache starts cold."
            "account_disabled" -> "The session’s account was disabled, so the session moved here and stays here. Its prompt cache starts cold."
            "account_removed" -> "The session’s account was removed, so the session moved here and stays here. Its prompt cache starts cold."
            "model_unavailable" -> "The session’s account doesn’t serve this model, so the session moved here and stays here."
            "missing_session" -> "The client sent no session id, so this request went to $pick on its own."
            "affinity_disabled" -> "Session affinity is off, so each request goes to $pick."
            "retry_same" -> "Retried on the same account."
            else -> if (!account.isNullOrEmpty()) "It went to $pick." else "No account could take this request."
        }
        val words = listOf("zero", "one", "two", "three", "four", "five")
        val n = attempts ?: 1
        val tries = if (n > 1) " It took ${if (n < words.size) words[n] else n.toString()} tries." else ""
        val end = when {
            status == 499 -> " $cancellationText"
            isError -> if (n > 1) " It still failed upstream." else " The request failed upstream; nothing was retried."
            else -> ""
        }
        val chunkText = if ((chunks ?: 0) > 0) " It used $chunks embedding chunks." else ""
        return base + tries + chunkText + end
    }
