package com.digitalpals.fusebox.data

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.time.Instant
import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter

/** Shared lenient JSON instance matching Fusebox's API (unknown keys ignored, nulls safe). */
val FuseboxJson = Json {
    ignoreUnknownKeys = true
    isLenient = true
    explicitNulls = false
    coerceInputValues = true
}

/**
 * Parses RFC 3339 timestamps with up to 9 fractional digits, `Z` or `±HH:MM` offsets,
 * or numeric epoch milliseconds (matching ios/Shared/Formatting.swift `FuseboxDate`).
 */
object FuseboxDate {
    fun parse(input: String?): Instant? {
        if (input.isNullOrBlank()) return null
        val s = input.trim()
        // Fast path via java.time.OffsetDateTime / Instant (handles nanos + Z + offsets)
        runCatching { return OffsetDateTime.parse(s).toInstant() }
        // Handle space separator ("2026-11-01 12:00:00Z") or compact offset (+0200)
        val normalized = buildString {
            for (i in s.indices) {
                val c = s[i]
                append(if (i == 10 && (c == ' ' || c == 't')) 'T' else c)
            }
        }
        val fixedOffset = Regex("([+-]\\d{2})(\\d{2})$").replace(normalized) { m ->
            "${m.groupValues[1]}:${m.groupValues[2]}"
        }
        runCatching { return OffsetDateTime.parse(fixedOffset).toInstant() }
        runCatching { return Instant.parse(fixedOffset) }
        return null
    }

    fun format(instant: Instant): String = DateTimeFormatter.ISO_INSTANT.format(instant)
}

object InstantAsFuseboxSerializer : KSerializer<Instant> {
    override val descriptor: SerialDescriptor =
        PrimitiveSerialDescriptor("FuseboxInstant", PrimitiveKind.STRING)

    override fun serialize(encoder: Encoder, value: Instant) {
        encoder.encodeString(FuseboxDate.format(value))
    }

    override fun deserialize(decoder: Decoder): Instant {
        val jsonDecoder = decoder as? JsonDecoder
        if (jsonDecoder != null) {
            val element = jsonDecoder.decodeJsonElement()
            val prim = element.jsonPrimitive
            if (!prim.isString) {
                val ms = prim.doubleOrNull ?: error("Expected timestamp string or epoch ms: $element")
                return Instant.ofEpochMilli(ms.toLong())
            }
            val text = prim.content
            return FuseboxDate.parse(text) ?: error("Not an RFC 3339 time: $text")
        }
        val text = decoder.decodeString()
        return FuseboxDate.parse(text) ?: error("Not an RFC 3339 time: $text")
    }
}

typealias FbInstant = @Serializable(with = InstantAsFuseboxSerializer::class) Instant

/** Deserializes `"value": "x"` OR `"value": ["x", "y"]` as `List<String>?`. */
@OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)
object StringOrListSerializer : KSerializer<List<String>?> {
    private val listSerializer = ListSerializer(String.serializer())
    override val descriptor: SerialDescriptor = listSerializer.descriptor

    override fun serialize(encoder: Encoder, value: List<String>?) {
        if (value == null) encoder.encodeNull() else listSerializer.serialize(encoder, value)
    }

    override fun deserialize(decoder: Decoder): List<String>? {
        val jsonDecoder = decoder as? JsonDecoder ?: return listSerializer.deserialize(decoder)
        return when (val el = jsonDecoder.decodeJsonElement()) {
            is JsonNull -> null
            is JsonArray -> el.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
            is JsonPrimitive -> el.contentOrNull?.let { listOf(it) }
            else -> null
        }
    }
}

// MARK: - Counters and series

@Serializable
data class Counters(
    val requests: Int? = null,
    val ok: Int? = null,
    val failed: Int? = null,
    val failures: Int? = null,
    val cancelled: Int? = null,
    @SerialName("usage_missing") val usageMissing: Int? = null,
    @SerialName("usage_partial") val usagePartial: Int? = null,
    @SerialName("input_tokens") val inputTokens: Long? = null,
    @SerialName("output_tokens") val outputTokens: Long? = null,
    @SerialName("cache_tokens") val cacheTokens: Long? = null,
)

@Serializable
data class MinuteBucket(
    val minute: Long,
    val requests: Int = 0,
    val failed: Int? = 0,
    val cancelled: Int? = 0,
    val tokens: Long? = 0,
    @SerialName("input_tokens") val inputTokens: Long? = null,
    @SerialName("output_tokens") val outputTokens: Long? = null,
    @SerialName("cache_tokens") val cacheTokens: Long? = null,
) {
    val instant: Instant get() = Instant.ofEpochSecond(minute * 60)
}

// MARK: - Overview

@Serializable
data class Overview(
    val version: String? = null,
    @SerialName("started_at") val startedAt: FbInstant? = null,
    val routing: String? = null,
    @SerialName("banked_resets") val bankedResets: Boolean? = null,
    @SerialName("session_affinity") val sessionAffinity: Boolean? = null,
    @SerialName("management_key") val managementKey: Boolean? = null,
    val totals: Counters? = null,
    val active: Int? = null,
    val series: List<MinuteBucket>? = null,
    val accounts: AccountSummary? = null,
    val models: Int? = null,
) {
    @Serializable
    data class AccountSummary(
        val total: Int? = null,
        val active: Int? = null,
        val cooling: Int? = null,
        val disabled: Int? = null,
    )
}

// MARK: - Accounts

@Serializable
data class QuotaWindow(
    val name: String,
    val used: Double? = null,
    @SerialName("resets_at") val resetsAt: FbInstant? = null,
    val model: String? = null,
    val group: String? = null,
    val covers: QuotaLimit.Scope? = null,
) {
    /** Whether this window limits `model`: every window without a model group does. */
    fun coversModel(modelId: String): Boolean {
        val scope = covers ?: return true
        if (modelId == "*") return true
        val id = modelId.lowercase()
        val values = scope.value.orEmpty()
        if (scope.kind == "models") return values.any { it.lowercase() == id }
        fun matches(pattern: String): Boolean {
            if (!pattern.contains('*')) return id.startsWith(pattern.lowercase())
            val regex = "^" + pattern.lowercase().split("*").joinToString(".*") { Regex.escape(it) } + "$"
            return Regex(regex).containsMatchIn(id)
        }
        return values.any { !it.startsWith("!") && matches(it) } &&
            values.none { it.startsWith("!") && matches(it.drop(1)) }
    }
}

@Serializable
data class QuotaLimit(
    val id: String? = null,
    val name: String,
    val scope: Scope? = null,
    @SerialName("used_percent") val usedPercent: Double? = null,
    val used: Double? = null,
    val limit: Double? = null,
    val remaining: Double? = null,
    val unit: String? = null,
    val unlimited: Boolean? = null,
    val enabled: Boolean? = null,
    @SerialName("resets_at") val resetsAt: FbInstant? = null,
) {
    @Serializable
    data class Scope(
        val kind: String? = null,
        @Serializable(with = StringOrListSerializer::class)
        val value: List<String>? = null,
    )
}

@Serializable
data class Renewal(
    val at: FbInstant,
    val estimated: Boolean? = null,
    val ends: Boolean? = null,
    val period: String? = null,
    val manual: Boolean? = null,
)

@Serializable
data class Quota(
    val windows: List<QuotaWindow>? = null,
    val limits: List<QuotaLimit>? = null,
    @SerialName("updated_at") val updatedAt: FbInstant? = null,
    val plan: String? = null,
    @SerialName("plan_name") val planName: String? = null,
    @SerialName("no_five_hour_limit") val noFiveHourLimit: Boolean? = null,
    @SerialName("subscription_plan") val subscriptionPlan: String? = null,
    val renewal: Renewal? = null,
) {
    /**
     * Antigravity reports a 5-hour and a weekly limit for each group of models as limits named
     * `<group> · <window>`. Normalizes them into windows just as the web dashboard and iOS app do.
     */
    fun normalized(): Quota {
        if (!windows.isNullOrEmpty() || limits.isNullOrEmpty()) return this
        val nameMap = mapOf("5h" to "5h", "weekly" to "week", "week" to "week", "daily" to "day", "day" to "day")
        val found = mutableListOf<QuotaWindow>()
        val rest = mutableListOf<QuotaLimit>()
        for (l in limits) {
            val parts = l.name.split(" · ")
            val winName = if (parts.size == 2) nameMap[parts[1].lowercase()] else null
            val used = l.usedPercent
            val scopeKind = l.scope?.kind ?: ""
            if (winName == null || used == null || l.enabled == false || scopeKind !in listOf("model_families", "models")) {
                rest.add(l)
                continue
            }
            val group = parts[0]
                .replace(Regex("\\s+models$", RegexOption.IGNORE_CASE), "")
                .replace(Regex("\\s+and\\s+", RegexOption.IGNORE_CASE), " & ")
            found.add(QuotaWindow(name = winName, used = used, resetsAt = l.resetsAt, group = group, covers = l.scope))
        }
        if (found.isEmpty()) return this
        val groups = mutableListOf<String>()
        for (w in found) {
            val g = w.group ?: ""
            if (g !in groups) groups.add(g)
        }
        val main = groups.firstOrNull { it.lowercase() == "gemini" }
            ?: groups.firstOrNull { it.contains("gemini", ignoreCase = true) }
            ?: groups[0]
        val normalizedWindows = found
            .map { w -> if (w.group != main) w.copy(model = w.group) else w }
            .sortedWith(
                compareBy<QuotaWindow>(
                    { if (it.group == main) 0 else 1 },
                    { groups.indexOf(it.group ?: "") },
                    { Quotas.seconds(it) ?: 0.0 },
                ),
            )
        return copy(windows = normalizedWindows, limits = rest)
    }
}

@Serializable
data class BankedResets(
    @SerialName("checked_at") val checkedAt: FbInstant? = null,
    val inventory: Inventory? = null,
    val error: String? = null,
    val quote: String? = null,
    val operation: Operation? = null,
    val retryable: Boolean? = null,
) {
    @Serializable
    data class Inventory(
        val available: Int? = null,
        val applicable: Int? = null,
        val eligible: Boolean? = null,
        val grants: List<Grant>? = null,
        val reason: String? = null,
        @SerialName("selected_grant") val selectedGrant: String? = null,
    )

    @Serializable
    data class Grant(
        val id: String,
        val label: String? = null,
        val remaining: Int? = null,
        val total: Int? = null,
        @SerialName("expires_at") val expiresAt: FbInstant? = null,
        @SerialName("starts_at") val startsAt: FbInstant? = null,
        val clears: List<String>? = null,
        val usable: Boolean? = null,
        val reason: String? = null,
    )

    @Serializable
    data class Operation(
        @SerialName("request_id") val requestID: String? = null,
        val status: String? = null,
        val message: String? = null,
        @SerialName("retry_until") val retryUntil: FbInstant? = null,
        val automatic: Boolean? = null,
    )
}

@Serializable
data class BankedResetPlan(
    @SerialName("auto_redeem") val autoRedeem: Boolean? = null,
    val prioritized: Boolean? = null,
    val message: String? = null,
    @SerialName("expiry_redeem_at") val expiryRedeemAt: FbInstant? = null,
)

@Serializable
data class Account(
    val id: String,
    val provider: String,
    val label: String,
    val email: String? = null,
    val kind: String = "oauth",
    val group: String? = null,
    val file: String? = null,
    val disabled: Boolean = false,
    val cooldowns: Map<String, String>? = null,
    @SerialName("cooldown_kinds") val cooldownKinds: Map<String, String>? = null,
    @SerialName("quota_reset_unknown") val quotaResetUnknown: Boolean? = null,
    @SerialName("last_error") val lastError: String? = null,
    @SerialName("last_error_id") val lastErrorID: String? = null,
    @SerialName("last_used") val lastUsed: FbInstant? = null,
    @SerialName("expires_at") val expiresAt: FbInstant? = null,
    val counters: Counters? = null,
    val priority: Int? = null,
    val quota: Quota? = null,
    @SerialName("banked_resets") val bankedResets: BankedResets? = null,
    @SerialName("banked_reset_plan") val bankedResetPlan: BankedResetPlan? = null,
    val models: List<String>? = null,
) {
    /** Account with Antigravity group-window normalization applied. */
    fun normalized(): Account = if (quota == null) this else copy(quota = quota.normalized())
}

@Serializable
data class AccountActivity(
    val series: List<MinuteBucket>? = null,
    val sessions: List<Session>? = null,
) {
    @Serializable
    data class Session(
        val session: String? = null,
        @SerialName("last_seen") val lastSeen: FbInstant? = null,
        val active: Boolean? = null,
        val since: FbInstant? = null,
        val client: String? = null,
        @SerialName("client_app") val clientApp: String? = null,
        @SerialName("key_label") val keyLabel: String? = null,
        val model: String? = null,
        val requests: Int? = null,
    ) {
        val id: String get() = session ?: "${since?.epochSecond ?: 0}"
    }
}

// MARK: - Faults

@Serializable
data class Fault(
    val key: String,
    val kind: String,
    val level: String,
    val provider: String? = null,
    @SerialName("provider_name") val providerName: String? = null,
    @SerialName("account_id") val accountID: String? = null,
    val label: String? = null,
    val title: String,
    val detail: String? = null,
    val until: FbInstant? = null,
    val path: String? = null,
    val badge: Boolean? = null,
) {
    val countsOnBadge: Boolean get() = badge ?: (kind != "pace")
}

// MARK: - Requests

@Serializable
data class RequestLog(
    val id: Int,
    val ts: FbInstant,
    val client: String? = null,
    @SerialName("client_app") val clientApp: String? = null,
    val source: Source? = null,
    @SerialName("key_label") val keyLabel: String? = null,
    val provider: String? = null,
    val model: String? = null,
    val account: String? = null,
    @SerialName("account_id") val accountID: String? = null,
    @SerialName("session_id") val sessionID: String? = null,
    @SerialName("session_source") val sessionSource: String? = null,
    @SerialName("routing_strategy") val routingStrategy: String? = null,
    @SerialName("routing_reason") val routingReason: String? = null,
    @SerialName("routing_warning") val routingWarning: String? = null,
    @SerialName("cache_policy") val cachePolicy: CachePolicy? = null,
    val status: Int? = null,
    @SerialName("latency_ms") val latencyMs: Int? = null,
    @SerialName("ttft_ms") val ttftMs: Int? = null,
    @SerialName("input_tokens") val inputTokens: Long? = null,
    @SerialName("output_tokens") val outputTokens: Long? = null,
    @SerialName("cache_tokens") val cacheTokens: Long? = null,
    @SerialName("usage_completeness") val usageCompleteness: String? = null,
    @SerialName("failure_kind") val failureKind: String? = null,
    val stream: Boolean? = null,
    val transport: String? = null,
    val attempts: Int? = null,
    val chunks: Int? = null,
    val error: String? = null,
) {
    @Serializable
    data class Source(
        val ip: String? = null,
        val via: String? = null,
        val login: String? = null,
    )

    @Serializable
    data class CachePolicy(
        val requested: String? = null,
        val applied: String? = null,
        @SerialName("key_scope") val keyScope: String? = null,
    )
}

// MARK: - Live socket

@Serializable
data class AccountLoad(
    @SerialName("in_flight") val inFlight: Int? = null,
    val sessions: Int? = null,
)

@Serializable
data class LiveTick(
    val active: Int? = null,
    val totals: Counters? = null,
)

sealed interface LiveMessage {
    data class Request(val log: RequestLog) : LiveMessage
    data class Load(val load: Map<String, AccountLoad>) : LiveMessage
    data class Faults(val faults: List<Fault>, val dismissed: List<Fault>?) : LiveMessage
    data class Tick(val tick: LiveTick) : LiveMessage
    data object Accounts : LiveMessage
    data object Models : LiveMessage
    data class Other(val type: String) : LiveMessage

    companion object {
        fun decode(raw: String): LiveMessage {
            val obj = FuseboxJson.parseToJsonElement(raw).jsonObject
            val type = obj["type"]?.jsonPrimitive?.content ?: return Other("")
            val data: JsonElement? = obj["data"]
            return when (type) {
                "request" -> Request(FuseboxJson.decodeFromJsonElement(data!!))
                "load" -> Load(FuseboxJson.decodeFromJsonElement(data!!))
                "faults" -> {
                    val list = FuseboxJson.decodeFromJsonElement<List<Fault>>(data!!)
                    val dismissed = obj["dismissed"]?.takeIf { it !is JsonNull }?.let {
                        FuseboxJson.decodeFromJsonElement<List<Fault>>(it)
                    }
                    Faults(list, dismissed)
                }
                "tick" -> Tick(FuseboxJson.decodeFromJsonElement(data!!))
                "accounts" -> Accounts
                "models" -> Models
                else -> Other(type)
            }
        }
    }
}

// MARK: - Push

@Serializable
data class PushStatus(
    @SerialName("public_key") val publicKey: String? = null,
    val subscriptions: List<Device> = emptyList(),
    val apns: Boolean? = null,
    val fcm: Boolean? = null,
    val relay: String? = null,
) {
    @Serializable
    data class Device(
        val id: String,
        val label: String? = null,
        val kind: String? = null,
        val via: String? = null,
        @SerialName("created_at") val createdAt: FbInstant? = null,
    ) {
        val resolvedKind: String get() = kind ?: "web"
    }
}

sealed interface PushPath {
    data object Direct : PushPath
    data class Relay(val url: String) : PushPath
    data object Unavailable : PushPath

    companion object {
        fun chooseAndroid(status: PushStatus): PushPath {
            if (status.fcm == true) return Direct
            val text = status.relay?.trim()
            if (!text.isNullOrEmpty()) {
                val lower = text.lowercase()
                if ((lower.startsWith("https://") || lower.startsWith("http://")) && text.length > 9) {
                    return Relay(text.trimEnd('/'))
                }
            }
            return Unavailable
        }
    }
}

@Serializable
data class FcmDirectRegistration(
    val token: String,
    val label: String,
)

@Serializable
data class RelayRegistration(
    val relay: RelayBody,
    val label: String,
) {
    @Serializable
    data class RelayBody(
        val handle: String,
        val key: String,
    )
}

@Serializable
data class IdResponse(val id: String)

@Serializable
data class ErrorResponse(val error: String? = null)

// MARK: - Usage

@Serializable
data class UsageTokens(
    val input: Long? = null,
    val output: Long? = null,
    @SerialName("cache_read") val cacheRead: Long? = null,
    @SerialName("cache_write") val cacheWrite: Long? = null,
    val reasoning: Long? = null,
)

@Serializable
data class UsageAggregate(
    val observations: Int? = null,
    @SerialName("estimated_cost_nanos") val estimatedCostNanos: Long? = null,
    val unpriced: Int? = null,
    val partial: Int? = null,
    @SerialName("missing_usage") val missingUsage: Int? = null,
    @SerialName("pricing_partial") val pricingPartial: Int? = null,
    val tokens: UsageTokens? = null,
    @SerialName("history_only") val historyOnly: Int? = null,
    val matched: Int? = null,
    @SerialName("weak_identity") val weakIdentity: Int? = null,
    @SerialName("aggregation_overflow") val aggregationOverflow: Boolean? = null,
    val date: String? = null,
    val group: String? = null,
    val id: String? = null,
    val accounts: Int? = null,
    val provider: String? = null,
)

@Serializable
data class UsageDashboard(
    val facets: Facets? = null,
    val pricing: Pricing? = null,
    val combined: Combined? = null,
) {
    @Serializable
    data class Facets(
        val providers: List<String>? = null,
        val models: List<String>? = null,
        val accounts: List<Labelled>? = null,
        val clients: List<Labelled>? = null,
    )

    @Serializable
    data class Labelled(
        val id: String? = null,
        val label: String? = null,
    )

    @Serializable
    data class Pricing(
        val version: String? = null,
        val basis: String? = null,
    )

    @Serializable
    data class Combined(
        val basis: String? = null,
        val totals: UsageAggregate? = null,
        @SerialName("proxy_first_event_at_ms") val proxyFirstEventAtMs: Long? = null,
        val stack: String? = null,
        val trend: List<UsageAggregate>? = null,
        val trends: Trends? = null,
        val breakdowns: Breakdowns? = null,
    )

    @Serializable
    data class Trends(
        val provider: List<UsageAggregate>? = null,
        val model: List<UsageAggregate>? = null,
    )

    @Serializable
    data class Breakdowns(
        val provider: List<UsageAggregate>? = null,
        val model: List<UsageAggregate>? = null,
        val account: List<UsageAggregate>? = null,
        val client: List<UsageAggregate>? = null,
    )
}

@Serializable
data class UsageSpeed(
    val models: List<Row>? = null,
    val trend: List<Row>? = null,
    val previous: List<Row>? = null,
    val backfilling: Boolean? = null,
) {
    @Serializable
    data class Row(
        val provider: String? = null,
        val model: String? = null,
        val speed: String? = null,
        val calls: Int? = null,
        val timed: Int? = null,
        val output: Long? = null,
        @SerialName("stream_ms") val streamMs: Long? = null,
        @SerialName("ttft_ms") val ttftMs: Long? = null,
        val date: String? = null,
    )
}

@Serializable
data class WidgetSnapshot(
    val overview: Overview? = null,
    val accounts: List<Account> = emptyList(),
    val faults: List<Fault> = emptyList(),
    val savedAtEpochMs: Long = System.currentTimeMillis(),
)
