package com.digitalpals.fusebox.data

/** Stacked by-day usage chart model (top five groups keep their own shade, rest merge into Other). */
data class UsageChartModel(
    val keys: List<String>,
    val days: List<Day>,
    val peak: Int,
) {
    data class Day(
        val date: String,
        val total: Double = 0.0,
        val calls: Int = 0,
        val costNanos: Long = 0L,
        val priced: Boolean = false,
        val groupValues: List<Double>,
        val groupCalls: List<Int>,
        val groupCost: List<Long>,
    )

    companion object {
        const val OTHER = "\u0000other"

        fun build(trend: List<UsageAggregate>, dates: List<String>, calls: Boolean): UsageChartModel {
            fun value(r: UsageAggregate): Double =
                if (calls) (r.observations ?: 0).toDouble() else (r.estimatedCostNanos ?: 0L) / 1e9

            val totals = mutableMapOf<String, Double>()
            val volumes = mutableMapOf<String, Int>()
            for (r in trend) {
                val g = r.group ?: ""
                totals[g] = (totals[g] ?: 0.0) + value(r)
                volumes[g] = (volumes[g] ?: 0) + (r.observations ?: 0)
            }
            val ranked = totals.keys
                .filter { (totals[it] ?: 0.0) > 0.0 || (volumes[it] ?: 0) > 0 }
                .sortedWith { a, b ->
                    val ta = totals[a] ?: 0.0
                    val tb = totals[b] ?: 0.0
                    if (ta != tb) return@sortedWith tb.compareTo(ta)
                    val va = volumes[a] ?: 0
                    val vb = volumes[b] ?: 0
                    if (va != vb) return@sortedWith vb.compareTo(va)
                    a.compareTo(b)
                }
            val shown = if (ranked.size > 6) ranked.take(5) else ranked
            val shownSet = shown.toSet()
            val keys = shown + if (ranked.size > shown.size) listOf(OTHER) else emptyList()

            val index = dates.withIndex().associate { it.value to it.index }
            val dayTotals = DoubleArray(dates.size)
            val dayCalls = IntArray(dates.size)
            val dayCost = LongArray(dates.size)
            val dayPriced = BooleanArray(dates.size)
            val gValues = Array(dates.size) { DoubleArray(keys.size) }
            val gCalls = Array(dates.size) { IntArray(keys.size) }
            val gCost = Array(dates.size) { LongArray(keys.size) }

            for (r in trend) {
                val i = index[r.date ?: ""] ?: continue
                val g = r.group ?: ""
                val k = if (g in shownSet) g else OTHER
                val ki = keys.indexOf(k)
                if (ki < 0) continue
                val v = value(r)
                gValues[i][ki] += v
                dayTotals[i] += v
                dayCalls[i] += r.observations ?: 0
                gCalls[i][ki] += r.observations ?: 0
                r.estimatedCostNanos?.let { c ->
                    dayCost[i] += c
                    dayPriced[i] = true
                    gCost[i][ki] += c
                }
            }

            val days = dates.mapIndexed { i, iso ->
                Day(
                    date = iso,
                    total = dayTotals[i],
                    calls = dayCalls[i],
                    costNanos = dayCost[i],
                    priced = dayPriced[i],
                    groupValues = gValues[i].toList(),
                    groupCalls = gCalls[i].toList(),
                    groupCost = gCost[i].toList(),
                )
            }
            var peak = (days.size - 1).coerceAtLeast(0)
            days.forEachIndexed { i, d ->
                val cur = days[peak]
                if (d.total > cur.total || (d.total == cur.total && d.calls > cur.calls)) peak = i
            }
            return UsageChartModel(keys = keys, days = days, peak = peak)
        }
    }
}
