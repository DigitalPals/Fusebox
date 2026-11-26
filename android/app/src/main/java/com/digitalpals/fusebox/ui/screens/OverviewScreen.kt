package com.digitalpals.fusebox.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.data.Account
import com.digitalpals.fusebox.data.AccountState
import com.digitalpals.fusebox.data.AppUiState
import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.Fault
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.LiveState
import com.digitalpals.fusebox.data.QuotaDisplay
import com.digitalpals.fusebox.data.Routing
import com.digitalpals.fusebox.data.bankedCount
import com.digitalpals.fusebox.data.bankedLabel
import com.digitalpals.fusebox.data.bankedNeedsReview
import com.digitalpals.fusebox.data.hasBankedResets
import com.digitalpals.fusebox.data.logo
import com.digitalpals.fusebox.data.metered
import com.digitalpals.fusebox.data.providerName
import com.digitalpals.fusebox.data.scopedWindows
import com.digitalpals.fusebox.data.state
import com.digitalpals.fusebox.data.subtitle
import com.digitalpals.fusebox.ui.adaptive.FoldAwareColumns
import com.digitalpals.fusebox.ui.components.AccountMeters
import com.digitalpals.fusebox.ui.components.CardPanel
import com.digitalpals.fusebox.ui.components.Figure
import com.digitalpals.fusebox.ui.components.FuseButton
import com.digitalpals.fusebox.ui.components.FuseSegmented
import com.digitalpals.fusebox.ui.components.Hairline
import com.digitalpals.fusebox.ui.components.LinkButton
import com.digitalpals.fusebox.ui.components.LiveBadge
import com.digitalpals.fusebox.ui.components.LoadChart
import com.digitalpals.fusebox.ui.components.PanelHead
import com.digitalpals.fusebox.ui.components.ProviderLogo
import com.digitalpals.fusebox.ui.components.SectionHead
import com.digitalpals.fusebox.ui.components.StatusDot
import com.digitalpals.fusebox.ui.components.StatusLabel
import com.digitalpals.fusebox.ui.components.rememberTicker
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface
import kotlinx.coroutines.launch
import java.time.Instant

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OverviewScreen(
    state: AppUiState,
    onRefresh: suspend () -> Unit,
    onSetQuotaDisplay: (QuotaDisplay) -> Unit,
    onNavigate: (DeepLink) -> Unit,
    contentPadding: PaddingValues = PaddingValues(16.dp),
) {
    val colors = LocalFuseboxColors.current
    val scope = rememberCoroutineScope()
    val now = rememberTicker()
    val subs = state.subscriptions
    val labelWidth = if (subs.any { it.scopedWindows(now).isNotEmpty() }) 56.dp else 28.dp

    PullToRefreshBox(
        isRefreshing = state.isRefreshing,
        onRefresh = { scope.launch { onRefresh() } },
        modifier = Modifier
            .fillMaxSize()
            .background(colors.bg),
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(contentPadding),
            verticalArrangement = Arrangement.spacedBy(24.dp),
        ) {
            // Header: Live status + version
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                LiveBadge(state.live)
                state.overview?.version?.let { v ->
                    Text(
                        text = "Fusebox ${v.substringBefore('+')}",
                        style = Typeface.data(12.sp),
                        color = colors.fg3,
                    )
                }
            }

            if (state.loadError != null && state.overview == null) {
                CardPanel {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        StatusDot(colors.err)
                        Text("Can’t reach Fusebox", style = Typeface.sans(16.sp, FontWeight.SemiBold), color = colors.fg)
                    }
                    Text(state.loadError, style = Typeface.row, color = colors.fg2)
                    FuseButton("Try again", onClick = { scope.launch { onRefresh() } })
                }
            }

            if (state.faults.isNotEmpty()) {
                FaultsPreviewCard(faults = state.faults, onNavigate = onNavigate, now = now)
            }

            if (subs.isNotEmpty()) {
                val ready = subs.count { it.state(now).kind == AccountState.Kind.READY }
                val servingCount = subs.count { it.state(now).kind == AccountState.Kind.READY && state.inFlight(it.id) > 0 }
                val serving = if (state.live == LiveState.LIVE) "$servingCount serving · " else ""
                val routing = Routing.sentence(state.overview, short = true)
                val meta = "$serving$ready of ${subs.size} ready" + if (routing.isNotEmpty()) " · $routing" else ""

                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    SectionHead(title = "Subscriptions", meta = meta) {
                        QuotaModeToggle(selected = state.quotaDisplay, onSelect = onSetQuotaDisplay)
                    }
                    FoldAwareColumns(items = subs, minColumnWidth = 330.dp) { column ->
                        CardPanel(rivets = true, padding = 0.dp, spacing = 0.dp) {
                            column.forEachIndexed { i, acct ->
                                if (i > 0) Hairline(Modifier.padding(horizontal = 16.dp))
                                AccountRowContent(
                                    account = acct,
                                    inFlight = state.inFlight(acct.id),
                                    mode = state.quotaDisplay,
                                    bankedEnabled = state.overview?.bankedResets == true,
                                    scoped = true,
                                    labelWidth = labelWidth,
                                    now = now,
                                    onClick = { onNavigate(DeepLink.Account(acct.id)) },
                                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                                )
                            }
                        }
                    }
                }
            }

            LoadCard(state = state, now = now)

            val bottomBlocks = buildList {
                if (state.otherCircuits.isNotEmpty()) add("other")
                add("latest")
            }
            FoldAwareColumns(items = bottomBlocks, minColumnWidth = 340.dp) { col ->
                Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    col.forEach { block ->
                        if (block == "other") {
                            OtherCircuitsCard(state = state, now = now, onNavigate = onNavigate)
                        } else {
                            LatestRequestsCard(state = state, onNavigate = onNavigate)
                        }
                    }
                }
            }
        }
    }
}

@Composable
fun QuotaModeToggle(selected: QuotaDisplay, onSelect: (QuotaDisplay) -> Unit) {
    FuseSegmented(
        options = listOf(QuotaDisplay.USED to "Used", QuotaDisplay.REMAINING to "Left"),
        selected = selected,
        onSelect = onSelect,
    )
}

@Composable
fun AccountRowContent(
    account: Account,
    inFlight: Int,
    mode: QuotaDisplay,
    bankedEnabled: Boolean,
    scoped: Boolean = false,
    selected: Boolean = false,
    labelWidth: Dp = 28.dp,
    now: Instant = rememberTicker(),
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = LocalFuseboxColors.current
    val st = account.state(now)
    val banked = account.hasBankedResets(bankedEnabled) &&
        (account.bankedNeedsReview || (account.bankedCount ?: 0) > 0)

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(if (selected) colors.segOn else Color.Transparent)
            .clickable(onClick = onClick)
            .then(modifier),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.Top,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            ProviderLogo(account.logo, size = 20.dp, modifier = Modifier.padding(top = 2.dp))
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    text = account.label,
                    style = Typeface.rowMedium,
                    color = colors.fg,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = account.subtitle,
                    style = Typeface.small,
                    color = colors.fg3,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            StatusLabel(account = account, inFlight = inFlight, now = now, modifier = Modifier.padding(top = 2.dp))
        }
        if (account.metered) {
            AccountMeters(account = account, mode = mode, scoped = scoped, labelWidth = labelWidth, now = now)
        }
        Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
            if (st.kind == AccountState.Kind.COOLING) {
                val modelLabel = if (st.model == "*") "All models" else st.model.orEmpty()
                val backLabel = if (st.resetUnknown) "Reset time unknown" else "back ${Fmt.whenText(st.until ?: now, now)}"
                Text("$modelLabel · $backLabel", style = Typeface.small, color = colors.fg2)
            }
            if (banked) {
                val prio = if (account.bankedResetPlan?.prioritized == true) " · prioritized" else ""
                Text(
                    text = "↻ ${if (account.bankedNeedsReview) "Review reset" else account.bankedLabel}$prio",
                    style = Typeface.small,
                    color = if (account.bankedNeedsReview) colors.warn else colors.fg2,
                )
            }
            val useText = if (inFlight > 0) "in use now" else "last used ${Fmt.ago(account.lastUsed, now)}"
            Text(
                text = "${Fmt.count(account.counters?.requests)} requests · $useText",
                style = Typeface.small,
                color = colors.fg3,
            )
        }
    }
}

@Composable
private fun LoadCard(state: AppUiState, now: Instant) {
    val colors = LocalFuseboxColors.current
    val series = state.lastHour(now)
    val requests = series.sumOf { it.requests }
    val failed = series.sumOf { it.failed ?: 0 }
    val cancelled = series.sumOf { it.cancelled ?: 0 }
    val completed = requests - cancelled
    val success = if (completed > 0) (completed - failed).toDouble() / completed.toDouble() * 100.0 else null
    val tokens = series.sumOf { it.tokens ?: 0L }

    CardPanel(spacing = 14.dp) {
        PanelHead("Load · last 60 min")
        Row(modifier = Modifier.fillMaxWidth()) {
            Figure(value = Fmt.count(requests), label = "Requests", modifier = Modifier.weight(1f))
            Figure(
                value = success?.let { String.format(java.util.Locale.US, if (it == 100.0) "%.0f%%" else "%.1f%%", it) } ?: "—",
                label = "Success",
                color = if (success != null && success < 95.0) colors.warn else colors.fg,
                modifier = Modifier.weight(1f),
            )
            Figure(value = Fmt.count(tokens), label = "Tokens", modifier = Modifier.weight(1f))
            Figure(value = Fmt.count(state.overview?.active), label = "In flight", modifier = Modifier.weight(1f))
        }
        LoadChart(series = series, height = 64.dp)
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Text("60 min ago", style = Typeface.data(11.sp), color = colors.fg3)
            Text("now", style = Typeface.data(11.sp), color = colors.fg3)
        }
    }
}

@Composable
private fun FaultsPreviewCard(
    faults: List<Fault>,
    onNavigate: (DeepLink) -> Unit,
    now: Instant,
) {
    CardPanel(spacing = 0.dp) {
        PanelHead("Faults · ${faults.size}") {
            LinkButton("All", onClick = { onNavigate(DeepLink.Faults) })
        }
        Spacer(Modifier.height(4.dp))
        faults.take(4).forEachIndexed { i, f ->
            if (i > 0) Hairline()
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable {
                        val id = f.accountID
                        onNavigate(if (id != null) DeepLink.Account(id) else DeepLink.Faults)
                    }
                    .padding(vertical = 12.dp),
            ) {
                FaultRowView(fault = f, now = now)
            }
        }
    }
}

@Composable
private fun OtherCircuitsCard(
    state: AppUiState,
    now: Instant,
    onNavigate: (DeepLink) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    CardPanel(spacing = 0.dp) {
        PanelHead("Other circuits")
        Text(
            text = "API keys and sign-ins without usage limits",
            style = Typeface.small,
            color = colors.fg3,
            modifier = Modifier.padding(top = 2.dp, bottom = 6.dp),
        )
        state.otherCircuits.forEachIndexed { i, a ->
            if (i > 0) Hairline()
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { onNavigate(DeepLink.Account(a.id)) }
                    .padding(vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                ProviderLogo(a.logo, size = 18.dp)
                Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(a.providerName, style = Typeface.row, color = colors.fg)
                    Text(a.label, style = Typeface.small, color = colors.fg3, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    StatusLabel(account = a, inFlight = state.inFlight(a.id), now = now)
                    Text("${Fmt.count(a.counters?.requests)} req", style = Typeface.data(11.5.sp), color = colors.fg3)
                }
            }
        }
    }
}

@Composable
private fun LatestRequestsCard(
    state: AppUiState,
    onNavigate: (DeepLink) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val latest = state.requests.take(6)
    CardPanel(spacing = 0.dp) {
        PanelHead("Latest requests") {
            LinkButton("All", onClick = { onNavigate(DeepLink.Requests) })
        }
        Spacer(Modifier.height(4.dp))
        if (latest.isEmpty()) {
            Text("No requests yet.", style = Typeface.row, color = colors.fg3, modifier = Modifier.padding(vertical = 8.dp))
        }
        latest.forEachIndexed { i, r ->
            if (i > 0) Hairline()
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { onNavigate(DeepLink.Request(r.id)) }
                    .padding(vertical = 12.dp),
            ) {
                RequestRowView(request = r, account = state.account(r.accountID), compact = true)
            }
        }
    }
}
