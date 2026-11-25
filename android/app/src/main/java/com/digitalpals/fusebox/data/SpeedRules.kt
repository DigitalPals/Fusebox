package com.digitalpals.fusebox.data

import java.text.NumberFormat
import java.util.Locale
import kotlin.math.roundToLong

/** Output speed rules (matching ui/usage.js and ios/Fusebox/Views/UsageSpeed.swift). */
data class SpeedSums(
    val calls: Int = 0,
    val timed: Int = 0,
    val output: Long = 0,
    val streamMs: Long = 0,
    val ttftMs: Long = 0,
) {
    fun plus(r: UsageSpeed.Row): SpeedSums = SpeedSums(
        calls = calls + (r.calls ?: 0),
        timed = timed + (r.timed ?: 0),
        output = output + (r.output ?: 0L),
        streamMs = streamMs + (r.streamMs ?: 0L),
        ttftMs = ttftMs + (r.ttftMs ?: 0L),
    )

    val rate: Double? get() = if (streamMs > 0L) output.toDouble() * 1000.0 / streamMs.toDouble() else null
    val first: Double? get() = if (timed > 0) ttftMs.toDouble() / timed.toDouble() / 1000.0 else null

    companion object {
        fun of(rows: Iterable<UsageSpeed.Row>): SpeedSums =
            rows.fold(SpeedSums()) { acc, r -> acc.plus(r) }
    }
}

object SpeedTier {
    fun name(s: String?): String = when (s) {
        "fast" -> "Fast"
        "ultrafast" -> "Ultrafast"
        else -> "Standard"
    }

    fun order(s: String?): Int = when (s) {
        "fast" -> 0
        "ultrafast" -> 1
        else -> 2
    }

    fun isStandard(s: String?): Boolean = s == null || s == "standard"
}

object SpeedText {
    fun rate(v: Double?): String {
        if (v == null) return "—"
        return if (v < 10.0) String.format(Locale.US, "%.1f", v)
        else NumberFormat.getIntegerInstance(Locale.US).format(v.roundToLong())
    }

    fun seconds(v: Double?): String {
        if (v == null) return "—"
        return if (v < 10.0) String.format(Locale.US, "%.1f s", v) else "${v.roundToLong()} s"
    }
}

data class SpeedSummary(
    val models: List<Model>,
    val calls: Int,
    val timed: Int,
    val fastProviders: List<String>,
    val fastShare: Double?,
    val gain: Pair<String, Double>?,
    val backfilling: Boolean,
) {
    data class Tier(
        val speed: String,
        val sums: SpeedSums,
        val before: Double?,
    )

    data class Model(
        val key: String,
        val provider: String?,
        val model: String?,
        val tiers: List<Tier>,
        val sums: SpeedSums,
        val before: Double?,
        val fastCalls: Int,
    ) {
        val rate: Double? get() = sums.rate
        val first: Double? get() = sums.first
    }

    fun picks(chosen: Map<String, Int>?): Map<String, Int> {
        val keys = models.map { it.key }.toSet()
        val kept = chosen.orEmpty().filterKeys { it in keys }
        if (kept.isNotEmpty()) return kept
        return models
            .filter { it.sums.timed > 0 }
            .sortedByDescending { it.sums.timed }
            .take(4)
            .mapIndexed { i, m -> m.key to i }
            .toMap()
    }

    fun sorted(by: String, provider: String): List<Model> =
        models.filter { provider.isEmpty() || (it.provider ?: "") == provider }
            .sortedWith { a, b ->
                if (by == "calls") return@sortedWith b.sums.calls.compareTo(a.sums.calls)
                val x = if (by == "first") a.first else a.rate
                val y = if (by == "first") b.first else b.rate
                if (x == null || y == null) {
                    if ((x == null) != (y == null)) return@sortedWith if (x != null) -1 else 1
                    return@sortedWith b.sums.calls.compareTo(a.sums.calls)
                }
                if (x == y) return@sortedWith b.sums.calls.compareTo(a.sums.calls)
                if (by == "first") x.compareTo(y) else y.compareTo(x)
            }

    val maxRate: Double
        get() = models.flatMap { listOf(it.rate) + it.tiers.map { t -> t.sums.rate } }
            .filterNotNull()
            .maxOrNull() ?: 0.0

    companion object {
        const val MAX_PICKS = 5

        fun key(r: UsageSpeed.Row): String = "${r.provider ?: ""}\u0000${r.model ?: ""}"

        fun from(s: UsageSpeed): SpeedSummary {
            val order = mutableListOf<String>()
            val rows = linkedMapOf<String, MutableList<UsageSpeed.Row>>()
            for (r in s.models.orEmpty()) {
                val k = key(r)
                if (k !in rows) order.add(k)
                rows.getOrPut(k) { mutableListOf() }.add(r)
            }
            val beforeMap = s.previous.orEmpty().groupBy { key(it) }
            val models = order.map { k ->
                val tiers = rows[k]!!.sortedBy { SpeedTier.order(it.speed) }
                val prev = beforeMap[k].orEmpty()
                Model(
                    key = k,
                    provider = tiers[0].provider,
                    model = tiers[0].model,
                    tiers = tiers.map { r ->
                        Tier(
                            speed = r.speed ?: "standard",
                            sums = SpeedSums.of(listOf(r)),
                            before = SpeedSums.of(prev.filter { it.speed == r.speed }).rate,
                        )
                    },
                    sums = SpeedSums.of(tiers),
                    before = SpeedSums.of(prev).rate,
                    fastCalls = tiers.filter { !SpeedTier.isStandard(it.speed) }.sumOf { it.calls ?: 0 },
                )
            }
            val calls = models.sumOf { it.sums.calls }
            val timed = models.sumOf { it.sums.timed }
            val providers = mutableListOf<String>()
            for (m in models) {
                val p = m.provider ?: ""
                if (m.fastCalls > 0 && p !in providers) providers.add(p)
            }
            val withFast = models.filter { (it.provider ?: "") in providers }
            val totalOf = withFast.sumOf { it.sums.calls }
            val fastShare = if (totalOf > 0) withFast.sumOf { it.fastCalls }.toDouble() / totalOf.toDouble() else null
            val both = models.sortedByDescending { it.sums.calls }.firstOrNull { m ->
                m.tiers.any { it.speed == "fast" && it.sums.rate != null } &&
                    m.tiers.any { it.speed == "standard" && it.sums.rate != null }
            }
            val gain = both?.let { m ->
                val fast = m.tiers.first { it.speed == "fast" }.sums.rate!!
                val std = m.tiers.first { it.speed == "standard" }.sums.rate!!
                (m.model ?: "Unknown model") to (fast / std)
            }
            return SpeedSummary(
                models = models,
                calls = calls,
                timed = timed,
                fastProviders = providers,
                fastShare = fastShare,
                gain = gain,
                backfilling = s.backfilling ?: false,
            )
        }
    }
}

data class SpeedLine(
    val key: String,
    val model: String?,
    val speed: String,
    val slot: Int,
    val values: List<Double?>,
) {
    val id: String get() = "$key\u0000$speed"
    val name: String get() = (model ?: "Unknown model") + if (SpeedTier.isStandard(speed)) "" else " ${SpeedTier.name(speed)}"

    val runs: List<List<Int>>
        get() {
            val out = mutableListOf<List<Int>>()
            var run = mutableListOf<Int>()
            values.forEachIndexed { i, v ->
                if (v != null) {
                    run.add(i)
                } else if (run.isNotEmpty()) {
                    out.add(run)
                    run = mutableListOf()
                }
            }
            if (run.isNotEmpty()) out.add(run)
            return out
        }

    companion object {
        fun lines(
            trend: List<UsageSpeed.Row>,
            dates: List<String>,
            picks: Map<String, Int>,
            first: Boolean,
        ): List<SpeedLine> {
            val index = dates.withIndex().associate { it.value to it.index }
            data class Acc(val row: UsageSpeed.Row, val slot: Int, val days: Array<SpeedSums>)
            val map = linkedMapOf<String, Acc>()
            for (r in trend) {
                val k = SpeedSummary.key(r)
                val slot = picks[k] ?: continue
                val i = index[r.date ?: ""] ?: continue
                val id = "$k\u0000${r.speed ?: "standard"}"
                val acc = map.getOrPut(id) { Acc(r, slot, Array(dates.size) { SpeedSums() }) }
                acc.days[i] = acc.days[i].plus(r)
            }
            return map.values.map { acc ->
                SpeedLine(
                    key = SpeedSummary.key(acc.row),
                    model = acc.row.model,
                    speed = acc.row.speed ?: "standard",
                    slot = acc.slot,
                    values = acc.days.map { if (first) it.first else it.rate },
                )
            }.sortedWith(compareBy<SpeedLine> { it.slot }.thenBy { SpeedTier.order(it.speed) })
        }
    }
}
