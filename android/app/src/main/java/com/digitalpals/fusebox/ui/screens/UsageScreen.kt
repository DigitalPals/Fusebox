package com.digitalpals.fusebox.ui.screens

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.data.AppStore
import com.digitalpals.fusebox.data.AppUiState
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.ProviderInfo
import com.digitalpals.fusebox.data.SpeedLine
import com.digitalpals.fusebox.data.SpeedSummary
import com.digitalpals.fusebox.data.SpeedText
import com.digitalpals.fusebox.data.SpeedTier
import com.digitalpals.fusebox.data.UsageAggregate
import com.digitalpals.fusebox.data.UsageChartModel
import com.digitalpals.fusebox.data.UsageDashboard
import com.digitalpals.fusebox.data.UsageSpeed
import com.digitalpals.fusebox.data.authName
import com.digitalpals.fusebox.data.planName
import com.digitalpals.fusebox.data.providerName
import com.digitalpals.fusebox.ui.adaptive.FoldAwareColumns
import com.digitalpals.fusebox.ui.components.CapsLabel
import com.digitalpals.fusebox.ui.components.CardPanel
import com.digitalpals.fusebox.ui.components.Figure
import com.digitalpals.fusebox.ui.components.FuseButton
import com.digitalpals.fusebox.ui.components.FuseSegmented
import com.digitalpals.fusebox.ui.components.Hairline
import com.digitalpals.fusebox.ui.components.PanelHead
import com.digitalpals.fusebox.ui.components.ProviderLogo
import com.digitalpals.fusebox.ui.components.StatusDot
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.net.URLEncoder
import java.nio.charset.StandardCharsets
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.roundToInt

private val isoDateFmt = DateTimeFormatter.ISO_LOCAL_DATE
private val shortDayFmt = DateTimeFormatter.ofPattern("EEE", Locale.UK)
private val shortMonthDayFmt = DateTimeFormatter.ofPattern("MMM d", Locale.UK)
private val fullDayFmt = DateTimeFormatter.ofPattern("EEE, MMM d", Locale.UK)

private data class DateRange(val start: String, val end: String, val dates: List<String>)

private fun computeRange(days: Int): DateRange {
    val today = LocalDate.now()
    val startDate = today.minusDays((days - 1).toLong())
    val endDate = today.plusDays(1)
    val dates = (0 until days).map { startDate.plusDays(it.toLong()).format(isoDateFmt) }
    return DateRange(startDate.format(isoDateFmt), endDate.format(isoDateFmt), dates)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UsageScreen(
    state: AppUiState,
    store: AppStore,
    demoUsage: UsageDashboard? = null,
    demoSpeed: UsageSpeed? = null,
    contentPadding: PaddingValues = PaddingValues(16.dp),
) {
    val colors = LocalFuseboxColors.current
    val scope = rememberCoroutineScope()
    var days by rememberSaveable { mutableIntStateOf(7) }
    var stack by rememberSaveable { mutableStateOf("provider") }
    var metric by rememberSaveable { mutableStateOf("cost") }
    var dim by rememberSaveable { mutableStateOf("model") }
    var filterDim by rememberSaveable { mutableStateOf<String?>(null) }
    var filterId by rememberSaveable { mutableStateOf<String?>(null) }
    var data by remember { mutableStateOf(demoUsage) }
    var speed by remember { mutableStateOf(demoSpeed) }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(false) }
    var selectedDay by remember { mutableStateOf<String?>(null) }
    var showAllBreakdown by remember { mutableStateOf(false) }

    val range = remember(days) { computeRange(days) }

    suspend fun loadUsage() {
        if (state.demoMode && demoUsage != null) {
            data = demoUsage
            speed = demoSpeed
            error = null
            return
        }
        val api = store.currentApi() ?: return
        loading = true
        try {
            val tz = ZoneId.systemDefault().id
            fun enc(v: String) = URLEncoder.encode(v, StandardCharsets.UTF_8.name())
            val params = mutableListOf(
                "start=${enc(range.start)}",
                "end=${enc(range.end)}",
                "timezone=${enc(tz)}",
            )
            if (filterDim != null && filterId != null) {
                params.add("${enc(filterDim!!)}=${enc(filterId!!)}")
            }
            val q = params.joinToString("&")
            coroutineScope {
                val speedDef = async { runCatching { api.usageSpeed(q) }.getOrNull() }
                data = api.usageDashboard(q)
                error = null
                speed = speedDef.await()
            }
        } catch (e: Exception) {
            error = e.message ?: "Could not load usage"
        } finally {
            loading = false
        }
    }

    LaunchedEffect(days, filterDim, filterId, state.demoMode) {
        selectedDay = null
        loadUsage()
    }

    LaunchedEffect(days, filterDim, filterId) {
        while (true) {
            delay(60_000L)
            loadUsage()
        }
    }

    fun dimTitle(d: String) = when (d) {
        "model" -> "Model"
        "account" -> "Account"
        "provider" -> "Provider"
        "client" -> "Client"
        else -> d
    }

    fun rowName(d: String, id: String?): String = when (d) {
        "model" -> id ?: "Unknown model"
        "provider" -> ProviderInfo.name(ProviderInfo.usageKey(id))
        "account" -> if (id == null) "No account reported"
        else state.account(id)?.label ?: data?.facets?.accounts?.firstOrNull { it.id == id }?.label ?: id
        else -> if (id == null) "No client reported"
        else data?.facets?.clients?.firstOrNull { it.id == id }?.label ?: id
    }

    PullToRefreshBox(
        isRefreshing = loading,
        onRefresh = { scope.launch { loadUsage() } },
        modifier = Modifier
            .fillMaxSize()
            .background(colors.bg),
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(contentPadding),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            FuseSegmented(
                options = listOf(1 to "Today", 7 to "7 days", 30 to "30 days"),
                selected = days,
                onSelect = { days = it },
                fill = true,
            )

            if (filterDim != null && filterId != null) {
                Row(
                    modifier = Modifier
                        .clip(RoundedCornerShape(6.dp))
                        .border(1.dp, colors.lineStrong, RoundedCornerShape(6.dp))
                        .clickable {
                            filterDim = null
                            filterId = null
                        }
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text(
                        text = "${dimTitle(filterDim!!)}: ${rowName(filterDim!!, filterId)}",
                        style = Typeface.meta,
                        color = colors.fg,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Icon(Icons.Default.Close, contentDescription = "Remove filter", tint = colors.fg3, modifier = Modifier.size(14.dp))
                }
            }

            if (error != null) {
                CardPanel {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        StatusDot(colors.err)
                        Text(error.orEmpty(), style = Typeface.meta, color = colors.err)
                    }
                    FuseButton("Retry", onClick = { scope.launch { loadUsage() } })
                }
            }

            val combined = data?.combined
            if (combined != null) {
                val parts = buildList {
                    add("cost")
                    add("chart")
                    if (speed != null) add("speed")
                    add("breakdown")
                }
                FoldAwareColumns(items = parts, minColumnWidth = 360.dp) { col ->
                    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                        col.forEach { p ->
                            when (p) {
                                "cost" -> CostCard(combined, days)
                                "chart" -> UsageChartCard(
                                    combined = combined,
                                    days = days,
                                    dates = range.dates,
                                    stack = stack,
                                    onStack = { stack = it },
                                    metric = metric,
                                    onMetric = { metric = it },
                                    selectedDay = selectedDay,
                                    onSelectDay = { selectedDay = it },
                                )
                                "speed" -> speed?.let { SpeedCard(speed = it, days = days, dates = range.dates) }
                                "breakdown" -> BreakdownCard(
                                    combined = combined,
                                    state = state,
                                    days = days,
                                    dim = dim,
                                    onDim = { dim = it; showAllBreakdown = false },
                                    showAll = showAllBreakdown,
                                    onToggleShowAll = { showAllBreakdown = !showAllBreakdown },
                                    rowName = ::rowName,
                                    onPickFilter = { d, id -> filterDim = d; filterId = id },
                                )
                            }
                        }
                    }
                }
            } else if (loading) {
                Box(Modifier.fillMaxWidth().padding(40.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(color = colors.fg2, strokeWidth = 2.dp)
                }
            }

            Text(
                text = "What your traffic would cost at published API prices, in USD. An estimate, not a subscription bill.",
                style = Typeface.small,
                color = colors.fg3,
            )
        }
    }
}

@Composable
private fun CostCard(c: UsageDashboard.Combined, days: Int) {
    val colors = LocalFuseboxColors.current
    val t = c.totals ?: UsageAggregate()
    val tokens = t.tokens
    val word = if (days == 1) "today" else "last $days days"
    val avg = t.estimatedCostNanos?.let { Fmt.dollars(it / max(1, days).toLong()) } ?: "Unpriced"
    val totalNanos = t.estimatedCostNanos ?: if ((t.observations ?: 0) == 0) 0L else null

    CardPanel(spacing = 14.dp) {
        PanelHead("Estimated cost · $word")
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                text = Fmt.dollars(totalNanos),
                style = Typeface.sans(34.sp, FontWeight.Medium).copy(letterSpacing = (-0.7).sp),
                color = colors.fg,
            )
            Text(
                text = if (days == 1) "So far today · priced calls only" else "Avg $avg a day · priced calls only",
                style = Typeface.meta,
                color = colors.fg3,
            )
        }
        Hairline()
        Row(modifier = Modifier.fillMaxWidth()) {
            Figure(Fmt.count(t.observations), "Calls", modifier = Modifier.weight(1f))
            Figure(Fmt.compact(tokens?.input), "Tokens in", modifier = Modifier.weight(1f))
            Figure(Fmt.compact(tokens?.output), "Tokens out", modifier = Modifier.weight(1f))
        }
        Row(modifier = Modifier.fillMaxWidth()) {
            Figure(Fmt.compact(tokens?.cacheRead), "Cache read", modifier = Modifier.weight(1f))
            Figure(if (tokens?.cacheWrite == null) "Unknown" else Fmt.compact(tokens.cacheWrite), "Cache write", modifier = Modifier.weight(1f))
            Figure(Fmt.count(t.historyOnly ?: 0), "History only", modifier = Modifier.weight(1f))
        }
        Text(coverageText(t), style = Typeface.small, color = colors.fg2)
    }
}

private fun coverageText(t: UsageAggregate): String {
    val n = t.observations ?: 0
    val unpriced = t.unpriced ?: 0
    val parts = mutableListOf<String>()
    parts.add(
        if (n > 0) String.format(Locale.US, "%.1f%% of calls priced", floor((1.0 - unpriced.toDouble() / n.toDouble()) * 1000.0) / 10.0)
        else "No matching calls",
    )
    t.matched?.takeIf { it > 0 }?.let { parts.add("${Fmt.count(it)} matched across sources, counted once") }
    if (unpriced > 0) parts.add("${Fmt.count(unpriced)} unpriced")
    t.partial?.takeIf { it > 0 }?.let { parts.add("${Fmt.count(it)} partial") }
    t.pricingPartial?.takeIf { it > 0 }?.let { parts.add("${Fmt.count(it)} with assumptions") }
    return parts.joinToString(" · ")
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun UsageChartCard(
    combined: UsageDashboard.Combined,
    days: Int,
    dates: List<String>,
    stack: String,
    onStack: (String) -> Unit,
    metric: String,
    onMetric: (String) -> Unit,
    selectedDay: String?,
    onSelectDay: (String?) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val trend = (if (stack == "model") combined.trends?.model else combined.trends?.provider) ?: combined.trend.orEmpty()
    val model = remember(trend, dates, metric) { UsageChartModel.build(trend, dates, calls = metric == "calls") }

    fun groupName(k: String): String = when {
        k == UsageChartModel.OTHER -> "Other"
        stack == "model" -> k.ifEmpty { "Unknown model" }
        else -> if (k.isEmpty()) "Unknown provider" else ProviderInfo.name(ProviderInfo.usageKey(k))
    }

    val groupColors = model.keys.mapIndexed { i, k ->
        if (k == UsageChartModel.OTHER) colors.chartOther else colors.chart[i % colors.chart.size]
    }
    val readout = model.days.firstOrNull { it.date == selectedDay } ?: model.days.getOrNull(model.peak)

    CardPanel(spacing = 14.dp) {
        PanelHead("By day")
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            FuseSegmented(listOf("provider" to "Provider", "model" to "Model"), selected = stack, onSelect = onStack)
            FuseSegmented(listOf("cost" to "Cost", "calls" to "Calls"), selected = metric, onSelect = onMetric)
        }
        if (days > 1 && model.days.isNotEmpty()) {
            val maxTotal = max(0.0001, model.days.maxOfOrNull { it.total } ?: 0.0)
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(modifier = Modifier.fillMaxWidth().height(168.dp)) {
                    Canvas(
                        modifier = Modifier
                            .fillMaxSize()
                            .pointerInput(model.days) {
                                detectTapGestures { offset ->
                                    val idx = ((offset.x / size.width) * model.days.size).toInt().coerceIn(0, model.days.lastIndex)
                                    val tapped = model.days[idx].date
                                    onSelectDay(if (selectedDay == tapped) null else tapped)
                                }
                            },
                    ) {
                        val n = model.days.size
                        val slot = size.width / n
                        val barW = (slot * 0.62f).coerceAtLeast(3f)
                        val pad = (slot - barW) / 2f
                        val chartH = size.height - 2.dp.toPx()

                        // Horizontal guide lines (3 steps)
                        for (g in 0..2) {
                            val y = chartH * (g / 3f)
                            drawLine(colors.line, Offset(0f, y), Offset(size.width, y), strokeWidth = 1.dp.toPx())
                        }

                        model.days.forEachIndexed { di, day ->
                            val alpha = if (selectedDay == null || selectedDay == day.date) 1f else 0.35f
                            var accH = 0f
                            day.groupValues.forEachIndexed { ki, v ->
                                if (v <= 0.0) return@forEachIndexed
                                val segH = ((v / maxTotal) * chartH).toFloat().coerceAtLeast(1.5f)
                                drawRect(
                                    color = groupColors[ki].copy(alpha = alpha),
                                    topLeft = Offset(di * slot + pad, chartH - accH - segH),
                                    size = Size(barW, segH),
                                )
                                accH += segH
                            }
                        }
                        drawLine(colors.line, Offset(0f, chartH), Offset(size.width, chartH), strokeWidth = 1.dp.toPx())
                    }
                }
                // X-axis day labels
                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    val step = if (days > 10) 7 else 1
                    model.days.forEachIndexed { idx, d ->
                        if (idx % step == 0 || idx == model.days.lastIndex) {
                            val parsed = runCatching { LocalDate.parse(d.date) }.getOrNull()
                            val label = parsed?.format(if (days > 10) shortMonthDayFmt else shortDayFmt) ?: d.date
                            Text(label, style = Typeface.label(10.5.sp), color = colors.fg3)
                        }
                    }
                }
            }

            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                model.keys.forEachIndexed { i, k ->
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Box(
                            modifier = Modifier
                                .size(9.dp)
                                .clip(RoundedCornerShape(2.dp))
                                .background(groupColors[i])
                                .border(0.5.dp, colors.lineStrong, RoundedCornerShape(2.dp)),
                        )
                        Text(groupName(k), style = Typeface.small, color = colors.fg2, maxLines = 1)
                    }
                }
            }
        }

        if (readout != null) {
            Hairline()
            val parsed = runCatching { LocalDate.parse(readout.date) }.getOrNull()
            val dayStr = parsed?.format(fullDayFmt) ?: readout.date
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                CapsLabel("${if (selectedDay == null) "Peak day" else "Day"} · $dayStr", size = 10.5.sp)
                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.Bottom) {
                    Text(
                        text = if (readout.priced) Fmt.dollars(readout.costNanos) else if (readout.calls > 0) "Unpriced" else "$0.00",
                        style = Typeface.figure,
                        color = colors.fg,
                    )
                    Text(Fmt.plural(readout.calls, "call"), style = Typeface.data(12.5.sp), color = colors.fg2)
                }
                model.keys.forEachIndexed { i, k ->
                    if (readout.groupCalls[i] > 0) {
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Box(
                                modifier = Modifier
                                    .size(9.dp)
                                    .clip(RoundedCornerShape(2.dp))
                                    .background(groupColors[i])
                                    .border(0.5.dp, colors.lineStrong, RoundedCornerShape(2.dp)),
                            )
                            Text(groupName(k), style = Typeface.meta, color = colors.fg2, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                            Text(
                                text = if (metric == "calls") "${Fmt.count(readout.groupCalls[i])} calls" else Fmt.dollars(readout.groupCost[i]),
                                style = Typeface.data(12.sp),
                                color = colors.fg2,
                            )
                        }
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun SpeedCard(speed: UsageSpeed, days: Int, dates: List<String>) {
    val colors = LocalFuseboxColors.current
    val summary = remember(speed) { SpeedSummary.from(speed) }
    val word = if (days == 1) "today" else "last $days days"

    var sort by rememberSaveable { mutableStateOf("rate") }
    var metric by rememberSaveable { mutableStateOf("rate") }
    var provider by remember { mutableStateOf("") }
    var chosen by remember { mutableStateOf<Map<String, Int>?>(null) }
    var showAll by remember { mutableStateOf(false) }
    var fullWarning by remember { mutableStateOf(false) }
    var selectedDayIdx by remember { mutableStateOf<Int?>(null) }

    CardPanel(spacing = 12.dp) {
        PanelHead("Output speed by model · $word")
        if (summary.models.isEmpty()) {
            Text(
                text = if (summary.backfilling) "Adding timings to earlier calls; this fills in shortly."
                else "No timed calls in this range. Fusebox times the calls it serves; imported history has no timings.",
                style = Typeface.meta,
                color = colors.fg3,
            )
            return@CardPanel
        }

        val picks = summary.picks(chosen)
        val providers = summary.models.mapNotNull { it.provider }.distinct()
        val activeProvider = if (provider in providers) provider else ""
        val rows = summary.sorted(by = sort, provider = activeProvider)
        val shown = if (showAll) rows else rows.take(12)
        val topRate = summary.maxRate

        // Figures
        Row(modifier = Modifier.fillMaxWidth()) {
            Figure(Fmt.count(summary.models.size), "Models", modifier = Modifier.weight(1f))
            Figure(Fmt.count(summary.timed), "Timed calls", modifier = Modifier.weight(1f))
            Figure(Fmt.count(summary.calls - summary.timed), "Not timed", modifier = Modifier.weight(1f))
        }
        if (summary.fastShare != null || summary.gain != null) {
            Row(modifier = Modifier.fillMaxWidth()) {
                if (summary.fastShare != null) {
                    val lbl = if (summary.fastProviders.size == 1) "${ProviderInfo.name(ProviderInfo.usageKey(summary.fastProviders[0]))} on Fast" else "On Fast"
                    Figure("${(summary.fastShare * 100.0).roundToInt()}%", lbl, modifier = Modifier.weight(1f))
                } else {
                    Spacer(Modifier.weight(1f))
                }
                if (summary.gain != null) {
                    Figure(
                        value = String.format(Locale.US, "×%.1f", summary.gain.second),
                        label = "Fast vs Standard · ${summary.gain.first}",
                        modifier = Modifier.weight(2f),
                    )
                }
            }
        }

        Hairline()
        if (providers.size > 1) {
            FuseSegmented(
                options = listOf("" to "All") + providers.map { it to ProviderInfo.name(ProviderInfo.usageKey(it)) },
                selected = activeProvider,
                onSelect = { provider = it },
                fill = true,
            )
        }
        FuseSegmented(
            options = listOf("rate" to "Speed", "first" to "First token", "calls" to "Calls"),
            selected = sort,
            onSelect = { sort = it },
            fill = true,
        )
        Text(
            text = if (fullWarning) "Up to ${SpeedSummary.MAX_PICKS} models at a time; tap a charted one to remove it." else "Tap a model to chart it.",
            style = Typeface.small,
            color = if (fullWarning) colors.fg2 else colors.fg3,
        )

        Column(verticalArrangement = Arrangement.spacedBy(0.dp)) {
            shown.forEachIndexed { idx, m ->
                if (idx > 0) Hairline()
                val slot = picks[m.key]
                val only = if (m.tiers.size == 1 && !SpeedTier.isStandard(m.tiers[0].speed)) " · ${SpeedTier.name(m.tiers[0].speed)} only" else ""
                val fast = if (m.fastCalls > 0 && m.tiers.size > 1) {
                    " · ${(m.fastCalls.toDouble() / max(1, m.sums.calls) * 100.0).roundToInt()}% Fast"
                } else ""
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable {
                            val next = picks.toMutableMap()
                            if (m.key in next) {
                                next.remove(m.key)
                                fullWarning = false
                                chosen = next
                            } else {
                                val free = (0 until SpeedSummary.MAX_PICKS).firstOrNull { s -> s !in next.values }
                                if (free != null) {
                                    next[m.key] = free
                                    fullWarning = false
                                    chosen = next
                                } else {
                                    fullWarning = true
                                }
                            }
                        }
                        .padding(vertical = 10.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Box(
                            modifier = Modifier
                                .size(9.dp)
                                .clip(RoundedCornerShape(2.dp))
                                .background(if (slot != null) colors.speedColor(slot) else Color.Transparent)
                                .border(if (slot == null) 1.dp else 0.5.dp, colors.lineStrong, RoundedCornerShape(2.dp)),
                        )
                        ProviderLogo(ProviderInfo.logo(ProviderInfo.usageKey(m.provider)), size = 15.dp)
                        Text(m.model ?: "Unknown model", style = Typeface.data(13.sp), color = colors.fg, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Row(horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.Bottom) {
                            Text(SpeedText.rate(m.rate), style = Typeface.data(13.sp), color = if (m.rate == null) colors.fg3 else colors.fg)
                            Text("tok/s", style = Typeface.data(11.sp), color = colors.fg3)
                        }
                    }
                    // Speed bar
                    val frac = if (m.rate != null && topRate > 0.0) (m.rate!! / topRate).toFloat().coerceIn(0f, 1f) else 0f
                    Box(modifier = Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(1.dp)).background(colors.segOff)) {
                        if (frac > 0f) {
                            Box(modifier = Modifier.fillMaxWidth(frac).height(4.dp).clip(RoundedCornerShape(1.dp)).background(colors.meter))
                        }
                    }
                    Text(
                        text = "${SpeedText.seconds(m.first)} to first token · ${Fmt.plural(m.sums.calls, "call")}$fast$only",
                        style = Typeface.small,
                        color = colors.fg3,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (m.tiers.size > 1) {
                        m.tiers.forEach { t ->
                            Row(
                                modifier = Modifier.fillMaxWidth().padding(start = 17.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                Text("↳ ${SpeedTier.name(t.speed)}", style = Typeface.small, color = colors.fg2)
                                Text("${SpeedText.seconds(t.sums.first)} · ${Fmt.plural(t.sums.calls, "call")}", style = Typeface.small, color = colors.fg3, maxLines = 1, modifier = Modifier.weight(1f))
                                Text("${SpeedText.rate(t.sums.rate)} tok/s", style = Typeface.data(12.sp), color = if (t.sums.rate == null) colors.fg3 else colors.fg)
                            }
                        }
                    }
                }
            }
        }

        if (rows.size > 12) {
            FuseButton(if (showAll) "Show top 12" else "Show all ${rows.size}", compact = true, onClick = { showAll = !showAll })
        }

        Hairline()
        val firstMode = metric == "first"
        val lines = SpeedLine.lines(speed.trend.orEmpty(), dates, picks, firstMode)
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            CapsLabel("By day", size = 10.5.sp)
            FuseSegmented(listOf("rate" to "Speed", "first" to "First token"), selected = metric, onSelect = { metric = it })
        }
        Text(
            text = if (firstMode) "Seconds to first token · charted models" else "Output tokens a second · charted models",
            style = Typeface.small,
            color = colors.fg3,
        )

        if (days <= 1) {
            Text("Pick a longer range to see speed by day.", style = Typeface.meta, color = colors.fg3)
        } else if (lines.isEmpty()) {
            Text("Tap a model above to chart it.", style = Typeface.meta, color = colors.fg3)
        } else {
            val maxY = max(1.0, lines.flatMap { it.values }.filterNotNull().maxOrNull() ?: 1.0)
            Canvas(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(160.dp)
                    .pointerInput(dates) {
                        detectTapGestures { offset ->
                            val i = ((offset.x / size.width) * (dates.size - 1)).roundToInt().coerceIn(0, dates.lastIndex)
                            selectedDayIdx = if (selectedDayIdx == i) null else i
                        }
                    },
            ) {
                val padY = 8.dp.toPx()
                val chartH = size.height - padY * 2
                val count = dates.size.coerceAtLeast(2)
                for (g in 0..3) {
                    val y = padY + chartH * (g / 3f)
                    drawLine(colors.line, Offset(0f, y), Offset(size.width, y), strokeWidth = 1.dp.toPx())
                }
                selectedDayIdx?.let { i ->
                    val x = (i.toFloat() / (count - 1)) * size.width
                    drawLine(colors.lineStrong, Offset(x, padY), Offset(x, size.height - padY), strokeWidth = 1.dp.toPx())
                }
                lines.forEach { line ->
                    val col = colors.speedColor(line.slot)
                    val effect = when (line.speed) {
                        "fast" -> PathEffect.dashPathEffect(floatArrayOf(12f, 8f), 0f)
                        "ultrafast" -> PathEffect.dashPathEffect(floatArrayOf(4f, 6f), 0f)
                        else -> null
                    }
                    val lastIdx = line.values.indexOfLast { it != null }
                    line.runs.forEach { run ->
                        if (run.size >= 2) {
                            val path = Path()
                            run.forEachIndexed { ri, dayIdx ->
                                val v = line.values[dayIdx] ?: return@forEachIndexed
                                val x = (dayIdx.toFloat() / (count - 1)) * size.width
                                val y = padY + chartH * (1f - (v / maxY).toFloat().coerceIn(0f, 1f))
                                if (ri == 0) path.moveTo(x, y) else path.lineTo(x, y)
                            }
                            drawPath(
                                path = path,
                                color = col,
                                style = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round, pathEffect = effect),
                            )
                        }
                        run.forEach { dayIdx ->
                            if (run.size == 1 || dayIdx == lastIdx) {
                                val v = line.values[dayIdx] ?: return@forEach
                                val x = (dayIdx.toFloat() / (count - 1)) * size.width
                                val y = padY + chartH * (1f - (v / maxY).toFloat().coerceIn(0f, 1f))
                                drawCircle(col, radius = 3.5.dp.toPx(), center = Offset(x, y))
                            }
                        }
                    }
                }
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                lines.forEach { l ->
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Box(modifier = Modifier.size(9.dp).clip(RoundedCornerShape(2.dp)).background(colors.speedColor(l.slot)))
                        Text(l.model ?: "Unknown model", style = Typeface.data(11.5.sp), color = colors.fg2, maxLines = 1)
                        if (!SpeedTier.isStandard(l.speed)) Text(SpeedTier.name(l.speed), style = Typeface.small, color = colors.fg3)
                    }
                }
            }
            val latest = (dates.indices).lastOrNull { i -> lines.any { it.values.getOrNull(i) != null } }
            val showIdx = selectedDayIdx ?: latest
            if (showIdx != null && showIdx in dates.indices) {
                Hairline()
                val parsed = runCatching { LocalDate.parse(dates[showIdx]) }.getOrNull()
                CapsLabel("${if (selectedDayIdx == null) "Latest day" else "Day"} · ${parsed?.format(fullDayFmt) ?: dates[showIdx]}", size = 10.5.sp)
                lines.forEach { l ->
                    val v = l.values.getOrNull(showIdx)
                    Row(modifier = Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Box(modifier = Modifier.size(9.dp).clip(RoundedCornerShape(2.dp)).background(colors.speedColor(l.slot)))
                        Text(l.name, style = Typeface.meta, color = colors.fg2, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Text(
                            text = v?.let { if (firstMode) SpeedText.seconds(it) else "${SpeedText.rate(it)} tok/s" } ?: "—",
                            style = Typeface.data(12.sp),
                            color = if (v == null) colors.fg3 else colors.fg2,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun BreakdownCard(
    combined: UsageDashboard.Combined,
    state: AppUiState,
    days: Int,
    dim: String,
    onDim: (String) -> Unit,
    showAll: Boolean,
    onToggleShowAll: () -> Unit,
    rowName: (String, String?) -> String,
    onPickFilter: (String, String) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val b = combined.breakdowns
    val rows = when (dim) {
        "account" -> b?.account.orEmpty()
        "provider" -> b?.provider.orEmpty()
        "client" -> b?.client.orEmpty()
        else -> b?.model.orEmpty()
    }
    val limit = if (dim == "model") 20 else 8
    val shown = if (showAll) rows else rows.take(limit)
    val total = (combined.totals?.estimatedCostNanos ?: 0L).toDouble()

    fun rowSub(r: UsageAggregate): String? = when (dim) {
        "model" -> ProviderInfo.name(ProviderInfo.usageKey(r.provider))
        "provider" -> if ((r.accounts ?: 0) > 0) Fmt.plural(r.accounts!!, "account") else "No account reported"
        "account" -> state.account(r.id)?.let { "${it.providerName} · ${it.planName ?: it.authName}" }
            ?: ProviderInfo.name(ProviderInfo.usageKey(r.provider))
        else -> if ((r.id ?: "").startsWith("collector:")) "Collector" else null
    }

    CardPanel(spacing = 12.dp) {
        PanelHead("Breakdown · ${if (days == 1) "today" else "last $days days"}")
        FuseSegmented(
            options = listOf("model" to "Model", "account" to "Account", "provider" to "Provider", "client" to "Client"),
            selected = dim,
            onSelect = onDim,
            fill = true,
        )
        if (rows.isEmpty()) {
            Text("No records match these filters.", style = Typeface.meta, color = colors.fg3)
        }
        Column {
            shown.forEachIndexed { i, r ->
                if (i > 0) Hairline()
                val share = if (total > 0.0) ((r.estimatedCostNanos ?: 0L).toDouble() / total).coerceIn(0.0, 1.0) else 0.0
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(enabled = r.id != null) { r.id?.let { onPickFilter(dim, it) } }
                        .padding(vertical = 10.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        if (dim != "client") {
                            ProviderLogo(
                                ProviderInfo.logo(ProviderInfo.usageKey(if (dim == "provider") r.id else r.provider)),
                                size = 15.dp,
                                modifier = Modifier.padding(top = 2.dp),
                            )
                        }
                        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text(
                                text = rowName(dim, r.id),
                                style = if (dim == "model") Typeface.data(13.sp) else Typeface.row,
                                color = colors.fg,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            rowSub(r)?.let { Text(it, style = Typeface.small, color = colors.fg3, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                        }
                        Text(
                            text = Fmt.dollars(r.estimatedCostNanos),
                            style = Typeface.data(13.sp),
                            color = if (r.estimatedCostNanos == null) colors.fg3 else colors.fg,
                        )
                    }
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Box(
                            modifier = Modifier
                                .weight(1f)
                                .height(4.dp)
                                .clip(RoundedCornerShape(1.dp))
                                .background(colors.segOff),
                        ) {
                            if (share > 0.0) {
                                Box(
                                    modifier = Modifier
                                        .fillMaxWidth(share.toFloat().coerceAtLeast(0.02f))
                                        .height(4.dp)
                                        .clip(RoundedCornerShape(1.dp))
                                        .background(colors.meter),
                                )
                            }
                        }
                        val pctText = when {
                            share <= 0.0 -> "—"
                            share < 0.01 -> "<1%"
                            else -> "${(share * 100.0).roundToInt()}%"
                        }
                        Text(
                            text = "$pctText · ${Fmt.count(r.observations)} calls",
                            style = Typeface.data(11.5.sp),
                            color = colors.fg3,
                            textAlign = TextAlign.End,
                            maxLines = 1,
                            modifier = Modifier.width(116.dp),
                        )
                    }
                }
            }
        }
        if (rows.size > limit) {
            FuseButton(if (showAll) "Show top $limit" else "Show all ${rows.size}", compact = true, onClick = onToggleShowAll)
        }
    }
}
