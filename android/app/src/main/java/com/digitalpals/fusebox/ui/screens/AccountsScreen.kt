package com.digitalpals.fusebox.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.PersonOff
import androidx.compose.material.icons.outlined.PersonOutline
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.data.Account
import com.digitalpals.fusebox.data.AccountState
import com.digitalpals.fusebox.data.AppStore
import com.digitalpals.fusebox.data.AppUiState
import com.digitalpals.fusebox.data.Counters
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.FuseboxDate
import com.digitalpals.fusebox.data.QuotaDisplay
import com.digitalpals.fusebox.data.QuotaLimit
import com.digitalpals.fusebox.data.QuotaReading
import com.digitalpals.fusebox.data.QuotaWindow
import com.digitalpals.fusebox.data.Quotas
import com.digitalpals.fusebox.data.authName
import com.digitalpals.fusebox.data.bankedCount
import com.digitalpals.fusebox.data.bankedLabel
import com.digitalpals.fusebox.data.bankedNeedsReview
import com.digitalpals.fusebox.data.bankedPolicyText
import com.digitalpals.fusebox.data.cooldownReason
import com.digitalpals.fusebox.data.hasBankedResets
import com.digitalpals.fusebox.data.logo
import com.digitalpals.fusebox.data.metered
import com.digitalpals.fusebox.data.noShortLimit
import com.digitalpals.fusebox.data.planName
import com.digitalpals.fusebox.data.providerName
import com.digitalpals.fusebox.data.renewalRow
import com.digitalpals.fusebox.data.state
import com.digitalpals.fusebox.data.windowSpent
import com.digitalpals.fusebox.ui.adaptive.FoldableListDetail
import com.digitalpals.fusebox.ui.adaptive.LocalFoldableState
import com.digitalpals.fusebox.ui.components.BreakerSwitch
import com.digitalpals.fusebox.ui.components.CapsLabel
import com.digitalpals.fusebox.ui.components.CardPanel
import com.digitalpals.fusebox.ui.components.EmptyState
import com.digitalpals.fusebox.ui.components.Figure
import com.digitalpals.fusebox.ui.components.FuseButton
import com.digitalpals.fusebox.ui.components.FuseButtonKind
import com.digitalpals.fusebox.ui.components.FuseSegmented
import com.digitalpals.fusebox.ui.components.Hairline
import com.digitalpals.fusebox.ui.components.KeyValueRow
import com.digitalpals.fusebox.ui.components.LoadChart
import com.digitalpals.fusebox.ui.components.PanelHead
import com.digitalpals.fusebox.ui.components.ProviderLogo
import com.digitalpals.fusebox.ui.components.SectionHead
import com.digitalpals.fusebox.ui.components.SegmentedMeter
import com.digitalpals.fusebox.ui.components.StatusDot
import com.digitalpals.fusebox.ui.components.StatusLabel
import com.digitalpals.fusebox.ui.components.ToastBanner
import com.digitalpals.fusebox.ui.components.rememberTicker
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface
import kotlinx.coroutines.launch
import java.text.DecimalFormat
import java.time.Duration
import java.time.Instant

/**
 * Accounts tab: Option A adaptive List-Detail on unfolded foldables (list left of the fold,
 * selected account detail right of the fold), single-pane push navigation on folded/compact screens.
 */
@Composable
fun AccountsScreen(
    state: AppUiState,
    store: AppStore,
    selectedAccountId: String?,
    onSelectAccount: (String?) -> Unit,
    onSetQuotaDisplay: (QuotaDisplay) -> Unit,
) {
    val foldable = LocalFoldableState.current
    val effectiveSelectedId = when {
        selectedAccountId != null -> selectedAccountId
        foldable.supportsTwoPanes -> state.accounts.firstOrNull()?.id
        else -> null
    }

    FoldableListDetail(
        hasSelection = effectiveSelectedId != null,
        listPane = {
            AccountsListPane(
                state = state,
                selectedId = if (foldable.supportsTwoPanes) effectiveSelectedId else null,
                onSelect = onSelectAccount,
                onRefresh = { store.refreshAll() },
                onSetQuotaDisplay = onSetQuotaDisplay,
            )
        },
        detailPane = {
            val id = effectiveSelectedId ?: return@FoldableListDetail
            AccountDetailPane(
                id = id,
                state = state,
                store = store,
                showBackButton = !foldable.supportsTwoPanes,
                onBack = { onSelectAccount(null) },
                onSetQuotaDisplay = onSetQuotaDisplay,
            )
        },
        emptyDetailPane = {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(LocalFuseboxColors.current.bg),
                contentAlignment = Alignment.Center,
            ) {
                EmptyState(
                    title = "Select an account",
                    icon = Icons.Outlined.PersonOutline,
                    message = "Pick an account on the left to inspect its limits, cooldowns and sessions.",
                )
            }
        },
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AccountsListPane(
    state: AppUiState,
    selectedId: String?,
    onSelect: (String) -> Unit,
    onRefresh: suspend () -> Unit,
    onSetQuotaDisplay: (QuotaDisplay) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val scope = rememberCoroutineScope()
    val now = rememberTicker()

    PullToRefreshBox(
        isRefreshing = state.isRefreshing,
        onRefresh = { scope.launch { onRefresh() } },
        modifier = Modifier
            .fillMaxSize()
            .background(colors.bg),
    ) {
        if (state.accounts.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                EmptyState(
                    title = "No accounts connected",
                    icon = Icons.Outlined.PersonOutline,
                    message = "Sign in with a subscription or add an API key from the web dashboard.",
                )
            }
        } else {
            val ready = state.accounts.count { it.state(now).kind == AccountState.Kind.READY }
            val cooling = state.accounts.count { it.state(now).kind == AccountState.Kind.COOLING }
            val meta = "$ready ready" + if (cooling > 0) " · $cooling cooling" else ""

            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(22.dp),
            ) {
                SectionHead(title = "${state.accounts.size} accounts", meta = meta) {
                    QuotaModeToggle(selected = state.quotaDisplay, onSelect = onSetQuotaDisplay)
                }
                state.groups.forEach { group ->
                    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            ProviderLogo(group.accounts[0].logo, size = 16.dp)
                            CapsLabel(group.name, size = 12.sp, color = colors.fg2, isHeader = true)
                            Text("${group.accounts.size}", style = Typeface.data(12.sp), color = colors.fg3)
                        }
                        CardPanel(rivets = true, padding = 0.dp, spacing = 0.dp) {
                            group.accounts.forEachIndexed { idx, acct ->
                                if (idx > 0) Hairline(Modifier.padding(horizontal = 16.dp))
                                AccountRowContent(
                                    account = acct,
                                    inFlight = state.inFlight(acct.id),
                                    mode = state.quotaDisplay,
                                    bankedEnabled = state.overview?.bankedResets == true,
                                    scoped = false,
                                    selected = acct.id == selectedId,
                                    now = now,
                                    onClick = { onSelect(acct.id) },
                                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

private sealed interface PendingAccountAction {
    data object Disable : PendingAccountAction
    data object Enable : PendingAccountAction
    data class ClearError(val errorId: String) : PendingAccountAction
    data object ClearCooldowns : PendingAccountAction
    data class BankedReset(val quote: String, val grant: String, val label: String, val detail: String) : PendingAccountAction
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AccountDetailPane(
    id: String,
    state: AppUiState,
    store: AppStore,
    showBackButton: Boolean,
    onBack: () -> Unit,
    onSetQuotaDisplay: (QuotaDisplay) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val account = state.account(id)
    if (account == null) {
        Box(Modifier.fillMaxSize().background(colors.bg), contentAlignment = Alignment.Center) {
            EmptyState(
                title = "This account is gone",
                icon = Icons.Outlined.PersonOff,
                message = "It was removed or its credentials changed.",
            )
        }
        return
    }

    val scope = rememberCoroutineScope()
    val now = rememberTicker()
    var busy by remember { mutableStateOf(false) }
    var toastText by remember { mutableStateOf<String?>(null) }
    var errorText by remember { mutableStateOf<String?>(null) }
    var pending by remember { mutableStateOf<PendingAccountAction?>(null) }

    LaunchedEffect(account.id) {
        store.loadActivity(account.id)
    }

    fun runAction(successMsg: String, block: suspend () -> Unit) {
        scope.launch {
            busy = true
            errorText = null
            try {
                block()
                toastText = successMsg
            } catch (e: Exception) {
                errorText = e.message ?: "Action failed"
            } finally {
                busy = false
            }
        }
    }

    Box(modifier = Modifier.fillMaxSize().background(colors.bg)) {
        Column(modifier = Modifier.fillMaxSize()) {
            // Sub-bar on compact screens or provider banner
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(colors.bg)
                    .padding(horizontal = 8.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (showBackButton) {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back", tint = colors.fg)
                    }
                } else {
                    Spacer(Modifier.width(8.dp))
                }
                CapsLabel(account.providerName, size = 12.sp, color = colors.fg2, modifier = Modifier.weight(1f))
                if (busy) {
                    CircularProgressIndicator(
                        color = colors.fg2,
                        strokeWidth = 2.dp,
                        modifier = Modifier.padding(end = 12.dp).height(18.dp).width(18.dp),
                    )
                }
            }
            Hairline()

            PullToRefreshBox(
                isRefreshing = state.isRefreshing,
                onRefresh = {
                    scope.launch {
                        store.loadActivity(account.id)
                        store.refreshAccounts()
                    }
                },
                modifier = Modifier.weight(1f),
            ) {
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    // Head
                    Row(
                        verticalAlignment = Alignment.Top,
                        horizontalArrangement = Arrangement.spacedBy(14.dp),
                    ) {
                        ProviderLogo(account.logo, size = 32.dp, modifier = Modifier.padding(top = 2.dp))
                        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                            Text(account.label, style = Typeface.title, color = colors.fg, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            Text(
                                text = listOfNotNull(account.providerName, account.planName, account.authName).joinToString(" · "),
                                style = Typeface.meta,
                                color = colors.fg3,
                            )
                        }
                    }

                    // Status + Refresh + Breaker switch
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        StatusLabel(account = account, inFlight = state.inFlight(account.id), now = now, modifier = Modifier.weight(1f))
                        if (account.kind == "oauth" && account.provider in setOf("claude", "codex", "meta")) {
                            FuseButton(
                                text = "Refresh",
                                compact = true,
                                enabled = !busy,
                                onClick = { runAction("Quota refreshed") { store.refreshQuota(account.id) } },
                            )
                        }
                        Row(
                            modifier = Modifier
                                .border(1.dp, colors.lineStrong, RoundedCornerShape(8.dp))
                                .padding(start = 6.dp, end = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            BreakerSwitch(
                                checked = !account.disabled,
                                enabled = !busy,
                                contentDesc = if (account.disabled) "Enable account" else "Disable account",
                                onCheckedChange = { on -> pending = if (on) PendingAccountAction.Enable else PendingAccountAction.Disable },
                            )
                            Text(
                                text = if (account.disabled) "Disabled" else "Enabled",
                                style = Typeface.meta,
                                color = colors.fg,
                            )
                        }
                    }

                    if (errorText != null) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            StatusDot(colors.err)
                            Text(errorText.orEmpty(), style = Typeface.meta, color = colors.err)
                        }
                    }

                    // Last error box
                    if (!account.lastError.isNullOrEmpty()) {
                        CardPanel(borderColor = colors.errLine) {
                            PanelHead("Last error") {
                                FuseButton(
                                    text = "Clear error",
                                    kind = FuseButtonKind.DESTRUCTIVE,
                                    compact = true,
                                    enabled = account.lastErrorID != null && !busy,
                                    onClick = { account.lastErrorID?.let { pending = PendingAccountAction.ClearError(it) } },
                                )
                            }
                            Text(account.lastError, style = Typeface.data(12.5.sp), color = colors.err)
                            Text("Clears the saved message. A new failure will appear again.", style = Typeface.small, color = colors.fg3)
                        }
                    }

                    // Limits card
                    if (account.metered) {
                        LimitsCard(account = account, mode = state.quotaDisplay, onSetMode = onSetQuotaDisplay, now = now)
                    }

                    // Banked resets
                    if (account.hasBankedResets(state.overview?.bankedResets == true)) {
                        BankedResetsCard(
                            account = account,
                            busy = busy,
                            now = now,
                            onRefresh = { runAction("Checked") { store.refreshQuota(account.id) } },
                            onUse = {
                                scope.launch {
                                    busy = true
                                    errorText = null
                                    try {
                                        val r = store.bankedQuote(account.id)
                                        val quote = r.quote
                                        if (quote == null || r.inventory?.eligible != true || r.error != null) {
                                            errorText = r.error ?: r.inventory?.reason ?: "No reset can be used right now."
                                        } else {
                                            val grant = r.inventory.grants.orEmpty().firstOrNull { it.id == r.inventory.selectedGrant }
                                                ?: r.inventory.grants.orEmpty().firstOrNull { it.usable == true }
                                            val detail = if (grant != null) {
                                                val exp = grant.expiresAt?.let { "Expires ${Fmt.shortDateTime(it)}" } ?: "No expiry reported"
                                                val clears = grant.clears.orEmpty().joinToString(", ").ifEmpty { "provider limits" }
                                                "$exp · Clears $clears"
                                            } else if (account.provider == "codex") {
                                                "Codex chooses the reset and restores its subscription limits."
                                            } else ""
                                            pending = PendingAccountAction.BankedReset(
                                                quote = quote,
                                                grant = grant?.id ?: r.inventory.selectedGrant.orEmpty(),
                                                label = grant?.label ?: "Subscription reset",
                                                detail = detail,
                                            )
                                        }
                                    } catch (e: Exception) {
                                        errorText = e.message ?: "Failed to check resets"
                                    } finally {
                                        busy = false
                                    }
                                }
                            },
                        )
                    }

                    // Cooldowns
                    CooldownsCard(
                        account = account,
                        busy = busy,
                        now = now,
                        onClearAll = { pending = PendingAccountAction.ClearCooldowns },
                    )

                    // Activity
                    ActivityCard(account = account, state = state, now = now)

                    // Priority (New sessions)
                    PriorityCard(
                        account = account,
                        busy = busy,
                        onSetPriority = { p ->
                            val msg = when {
                                p > 0 -> "Takes new sessions first"
                                p < 0 -> "Takes new sessions last"
                                else -> "Back to normal priority"
                            }
                            runAction(msg) { store.setPriority(account.id, p) }
                        },
                    )

                    // Sign-in card
                    SignInCard(account = account, now = now)
                }
            }
        }

        ToastBanner(
            text = toastText,
            onDismiss = { toastText = null },
            modifier = Modifier.align(Alignment.BottomCenter),
        )
    }

    pending?.let { action ->
        val (title, message, confirmLabel, destructive) = when (action) {
            PendingAccountAction.Disable -> listOf(
                "Disable ${account.label}?",
                "It stops taking requests until you enable it again. Its sessions move to another account.",
                "Disable",
                true,
            )
            PendingAccountAction.Enable -> listOf(
                "Enable ${account.label}?",
                "It goes back into rotation.",
                "Enable",
                false,
            )
            is PendingAccountAction.ClearError -> listOf(
                "Clear the saved error?",
                "Clears the saved message. A new failure will appear again.",
                "Clear error",
                true,
            )
            PendingAccountAction.ClearCooldowns -> listOf(
                "Clear all cooldowns?",
                "Every model on this account takes requests again straight away, even if the provider still limits it.",
                "Clear cooldowns",
                true,
            )
            is PendingAccountAction.BankedReset -> listOf(
                "Use 1 reset?",
                "This uses one saved reset: ${action.label}.\n${action.detail}",
                "Apply reset",
                true,
            )
        }
        AlertDialog(
            onDismissRequest = { pending = null },
            containerColor = colors.surfacePop,
            titleContentColor = colors.fg,
            textContentColor = colors.fg2,
            title = { Text(title as String, style = Typeface.sans(17.sp, FontWeight.SemiBold)) },
            text = { Text(message as String, style = Typeface.row) },
            confirmButton = {
                TextButton(
                    onClick = {
                        pending = null
                        when (action) {
                            PendingAccountAction.Disable -> runAction("Disabled") { store.setDisabled(account.id, true) }
                            PendingAccountAction.Enable -> runAction("Enabled") { store.setDisabled(account.id, false) }
                            is PendingAccountAction.ClearError -> runAction("Error cleared") { store.clearError(account.id, action.errorId) }
                            PendingAccountAction.ClearCooldowns -> runAction("Cooldowns cleared") { store.clearCooldowns(account.id) }
                            is PendingAccountAction.BankedReset -> runAction("Reset applied") {
                                store.applyBankedReset(account.id, action.quote, action.grant)
                            }
                        }
                    },
                ) {
                    Text(
                        text = confirmLabel as String,
                        style = Typeface.rowMedium,
                        color = if (destructive as Boolean) colors.err else colors.fg,
                    )
                }
            },
            dismissButton = {
                TextButton(onClick = { pending = null }) {
                    Text("Cancel", style = Typeface.row, color = colors.fg2)
                }
            },
        )
    }
}

@Composable
private fun LimitsCard(
    account: Account,
    mode: QuotaDisplay,
    onSetMode: (QuotaDisplay) -> Unit,
    now: Instant,
) {
    val colors = LocalFuseboxColors.current
    val windows = account.quota?.windows.orEmpty().filter { QuotaReading.of(it, now) != null }
    val limits = account.quota?.limits.orEmpty().filter { !((it.id ?: "").endsWith(":gate") && it.usedPercent == 0.0) }

    CardPanel(rivets = true, spacing = 18.dp) {
        PanelHead("Limits · ${mode.word}") {
            QuotaModeToggle(selected = mode, onSelect = onSetMode)
        }
        if (account.noShortLimit(now)) {
            WindowBlock(
                title = "5-hour window",
                reading = null,
                pace = null,
                note = "This account has no five-hour limit; only the weekly limits apply.",
                mode = mode,
                noLimit = true,
                runsOut = null,
                now = now,
            )
        }
        if (windows.isEmpty() && limits.isEmpty()) {
            listOf(true, false).forEach { short ->
                if (!(short && account.noShortLimit(now))) {
                    WindowBlock(
                        title = if (short) "5-hour window" else "Weekly window",
                        reading = null,
                        pace = null,
                        note = "Not reported yet",
                        mode = mode,
                        noLimit = false,
                        runsOut = null,
                        now = now,
                    )
                }
            }
        }
        windows.forEach { w ->
            val r = QuotaReading.of(w, now)
            val back = (r?.exhausted == true) || account.windowSpent(w)
            val note = w.resetsAt?.let {
                "${if (back) "Back" else "Resets"} ${Fmt.whenText(it, now)} · in ${Fmt.left(it, now)}"
            } ?: "No reset time reported"
            WindowBlock(
                title = (if (w.model != null) Quotas.scopedTitle(w) else Quotas.title(w)) + " window",
                reading = r,
                pace = Quotas.elapsed(w, now),
                note = note,
                mode = mode,
                noLimit = false,
                runsOut = Quotas.runsOut(w, now),
                now = now,
            )
        }
        limits.forEach { l ->
            Hairline()
            LimitBlock(limit = l, mode = mode, now = now)
        }
    }
}

@Composable
private fun WindowBlock(
    title: String,
    reading: QuotaReading?,
    pace: Double?,
    note: String,
    mode: QuotaDisplay,
    noLimit: Boolean,
    runsOut: Instant?,
    now: Instant,
) {
    val colors = LocalFuseboxColors.current
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text(title, style = Typeface.row, color = colors.fg2)
            if (noLimit) {
                Text("No limit", style = Typeface.row, color = colors.fg3)
            } else {
                Text(
                    text = reading?.let { Fmt.percent(it.value(mode)) } ?: "–",
                    style = Typeface.bigFigure,
                    color = when {
                        reading == null -> colors.fg3
                        reading.level == QuotaReading.Level.NORMAL -> colors.fg
                        else -> colors.level(reading.level)
                    },
                )
            }
        }
        if (!noLimit) {
            SegmentedMeter(reading = reading, pace = pace, mode = mode, barHeight = 20.dp, label = title)
        }
        Text(note, style = Typeface.meta, color = colors.fg3)
        if (runsOut != null) {
            Text(
                text = "At this pace it runs out ${Fmt.whenText(runsOut, now)}, in ${Fmt.left(runsOut, now)}",
                style = Typeface.meta,
                color = colors.warn,
            )
        }
    }
}

@Composable
private fun LimitBlock(limit: QuotaLimit, mode: QuotaDisplay, now: Instant) {
    val colors = LocalFuseboxColors.current
    val reading = if (limit.enabled == false) null else QuotaReading.of(QuotaWindow(limit.name, limit.usedPercent, limit.resetsAt), now)
    val divisor = if (limit.unit == "USD cents") 100.0 else 1.0
    val unit = if (divisor == 100.0) "USD" else limit.unit
    val dec = remember { DecimalFormat("#.####") }
    fun amount(v: Double): String = dec.format(v / divisor)
    val values = when {
        limit.used != null && limit.limit != null -> "${amount(limit.used)} / ${amount(limit.limit)}"
        limit.remaining != null -> "${amount(limit.remaining)} left"
        else -> ""
    }
    val detail = when {
        limit.unlimited == true -> "Unlimited"
        values.isNotEmpty() -> values + (unit?.let { " $it" }.orEmpty())
        reading == null -> "Not reported"
        else -> ""
    }
    val scope = when (limit.scope?.kind) {
        "account" -> "Applies to all models"
        "credits" -> "Extra credits; separate from subscription allowance"
        "overage" -> "Optional paid usage; separate from subscription allowance"
        else -> "Separate feature allowance"
    }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Text(limit.name, style = Typeface.row, color = colors.fg2)
            if (reading != null) {
                Text(Fmt.percent(reading.value(mode)), style = Typeface.data(14.sp), color = colors.fg)
            } else {
                Text(if (limit.enabled == false) "Disabled" else detail, style = Typeface.meta, color = colors.fg3)
            }
        }
        if (reading != null) {
            SegmentedMeter(reading = reading, mode = mode, barHeight = 9.dp, label = limit.name)
            if (detail.isNotEmpty()) Text(detail, style = Typeface.data(12.sp), color = colors.fg2)
        }
        Text(scope, style = Typeface.small, color = colors.fg3)
    }
}

@Composable
private fun BankedResetsCard(
    account: Account,
    busy: Boolean,
    now: Instant,
    onRefresh: () -> Unit,
    onUse: () -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val r = account.bankedResets
    CardPanel(spacing = 10.dp) {
        PanelHead("Banked resets") {
            r?.checkedAt?.let { Text("Checked ${Fmt.ago(it, now)}", style = Typeface.small, color = colors.fg3) }
        }
        Text(
            text = "↻ ${account.bankedLabel}",
            style = Typeface.sans(17.sp, FontWeight.Medium),
            color = if (account.bankedNeedsReview) colors.warn else colors.fg,
        )
        r?.inventory?.grants.orEmpty().forEach { g ->
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(g.label ?: "Subscription reset", style = Typeface.rowMedium, color = colors.fg)
                    Text("${g.remaining ?: 0} left", style = Typeface.data(12.sp), color = colors.fg2)
                }
                val exp = g.expiresAt?.let { "Expires ${Fmt.shortDateTime(it)}" } ?: "No expiry reported"
                val clears = g.clears.orEmpty().joinToString(", ").ifEmpty { "provider limits" }
                Text("$exp · Clears $clears", style = Typeface.small, color = colors.fg3)
            }
        }
        Text(account.bankedPolicyText(now), style = Typeface.meta, color = colors.fg2)
        if (r?.error == null && !r?.inventory?.reason.isNullOrEmpty()) {
            Text(r!!.inventory!!.reason!!, style = Typeface.meta, color = colors.fg3)
        }
        r?.operation?.message?.let { msg ->
            val ok = r.operation.status in setOf("applied", "reconciled_used")
            Text(msg, style = Typeface.meta, color = if (ok) colors.ok else colors.fg2)
        }
        if (account.bankedNeedsReview) {
            Text("A reset may have been used. Resolve it in the web dashboard before using another.", style = Typeface.meta, color = colors.warn)
        }
        r?.error?.let { Text(it, style = Typeface.meta, color = colors.err) }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.padding(top = 4.dp)) {
            FuseButton("Refresh", enabled = !busy, onClick = onRefresh)
            FuseButton(
                text = "Use 1 reset",
                enabled = !busy && !account.disabled && r?.inventory?.eligible == true &&
                    (account.bankedCount ?: 0) > 0 && !account.bankedNeedsReview,
                onClick = onUse,
            )
        }
    }
}

@Composable
private fun CooldownsCard(
    account: Account,
    busy: Boolean,
    now: Instant,
    onClearAll: () -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val cds = account.cooldowns.orEmpty()
        .mapNotNull { (m, v) -> FuseboxDate.parse(v)?.let { m to it } }
        .filter { it.second.isAfter(now) }
        .sortedBy { it.second }

    CardPanel(spacing = 10.dp) {
        PanelHead("Cooldowns") {
            if (cds.isNotEmpty()) {
                FuseButton("Clear all", compact = true, enabled = !busy, onClick = onClearAll)
            }
        }
        if (cds.isEmpty()) {
            Text("No models cooling down. A rate limit pauses only that model on this account.", style = Typeface.meta, color = colors.fg3)
        }
        cds.forEach { (model, until) ->
            Hairline()
            Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(if (model == "*") "All models" else model, style = Typeface.data(13.sp), color = colors.fg)
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(account.cooldownReason(model), style = Typeface.meta, color = colors.fg2)
                    val text = if (model == "*" && account.quotaResetUnknown == true) {
                        "Reset time unknown"
                    } else {
                        "back ${Fmt.whenText(until, now)} · ${Fmt.left(until, now)}"
                    }
                    Text(text, style = Typeface.meta, color = colors.warn)
                }
            }
        }
    }
}

@Composable
private fun ActivityCard(account: Account, state: AppUiState, now: Instant) {
    val colors = LocalFuseboxColors.current
    val act = state.activity[account.id]
    val series = act?.series.orEmpty()
    val c = account.counters ?: Counters()

    CardPanel(spacing = 12.dp) {
        PanelHead("Activity") {
            Text(
                text = if (state.inFlight(account.id) > 0) "In use now" else "Last used ${Fmt.ago(account.lastUsed, now)}",
                style = Typeface.small,
                color = colors.fg3,
            )
        }
        CapsLabel("Load · last 60 min", size = 10.5.sp)
        if (series.isEmpty()) {
            Text(if (act == null) "Loading…" else "No requests in the last hour.", style = Typeface.meta, color = colors.fg3)
        } else {
            LoadChart(series = series, height = 56.dp)
            Text("${Fmt.plural(series.sumOf { it.requests }, "request")} in the last hour", style = Typeface.small, color = colors.fg3)
        }
        Hairline()
        CapsLabel("Since start", size = 10.5.sp)
        Row(modifier = Modifier.fillMaxWidth()) {
            Figure(Fmt.count(c.requests), "Requests", modifier = Modifier.weight(1f))
            Figure(Fmt.count(c.failures), "Failed", color = if ((c.failures ?: 0) > 0) colors.err else colors.fg, modifier = Modifier.weight(1f))
            Figure(Fmt.count(c.cancelled), "Cancelled", modifier = Modifier.weight(1f))
        }
        Row(modifier = Modifier.fillMaxWidth()) {
            Figure(Fmt.compact(c.inputTokens), "Tokens in", modifier = Modifier.weight(1f))
            Figure(Fmt.compact(c.outputTokens), "Tokens out", modifier = Modifier.weight(1f))
            Figure(Fmt.compact(c.cacheTokens), "Cached", modifier = Modifier.weight(1f))
        }
        Hairline()
        val sessions = act?.sessions.orEmpty()
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            CapsLabel("Pinned sessions", size = 10.5.sp)
            Text("${sessions.size}", style = Typeface.data(12.sp), color = colors.fg3)
        }
        if (sessions.isEmpty()) {
            Text("No coding sessions are pinned to this account.", style = Typeface.meta, color = colors.fg3)
        } else {
            Text("Coding sessions stay on this account so their prompt cache keeps working.", style = Typeface.small, color = colors.fg3)
        }
        sessions.take(8).forEachIndexed { idx, s ->
            if (idx > 0) Hairline(color = colors.lineSoft)
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.Top,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                StatusDot(if (s.active == true) colors.ok else colors.idle, size = 7.dp, modifier = Modifier.padding(top = 6.dp))
                Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    val title = listOfNotNull(s.clientApp ?: s.client, s.keyLabel).joinToString(" · ").ifEmpty { "Session" }
                    Text(title, style = Typeface.row, color = colors.fg, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val sub = listOfNotNull(s.model, s.session?.take(8)).joinToString(" · ")
                    Text(sub, style = Typeface.data(12.sp), color = colors.fg3, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text("${Fmt.count(s.requests)} req", style = Typeface.data(12.sp), color = colors.fg2)
                    Text(Fmt.ago(s.lastSeen, now), style = Typeface.small, color = colors.fg3)
                }
            }
        }
    }
}

@Composable
private fun PriorityCard(
    account: Account,
    busy: Boolean,
    onSetPriority: (Int) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val p = account.priority ?: 0
    val pick = when {
        p > 0 -> 10
        p < 0 -> -10
        else -> 0
    }
    val help = when (pick) {
        10 -> "Takes new sessions before accounts set to Normal or Last."
        -10 -> "Takes new sessions only when no other account can."
        else -> "Takes new sessions by the routing strategy, with the other Normal accounts."
    }
    CardPanel(spacing = 10.dp) {
        PanelHead("New sessions") {
            FuseSegmented(
                options = listOf(10 to "First", 0 to "Normal", -10 to "Last"),
                selected = pick,
                enabled = !busy,
                onSelect = { v -> if (v != pick) onSetPriority(v) },
            )
        }
        Text(help, style = Typeface.meta, color = colors.fg2)
    }
}

@Composable
private fun SignInCard(account: Account, now: Instant) {
    val colors = LocalFuseboxColors.current
    val tokenInfo = if (account.kind == "oauth" && account.expiresAt != null) {
        val secs = Duration.between(now, account.expiresAt).seconds.toDouble()
        if (secs > 0) "valid ${Fmt.span(secs)} · refreshes on its own" else "expired"
    } else null

    CardPanel(spacing = 0.dp) {
        PanelHead("Sign-in")
        Spacer(Modifier.height(4.dp))
        Hairline()
        KeyValueRow("Method", account.authName)
        account.planName?.let { plan ->
            Hairline()
            KeyValueRow("Plan", "${account.providerName} $plan")
        }
        account.renewalRow(now)?.let { (k, v) ->
            Hairline()
            KeyValueRow(k, v)
        }
        if (tokenInfo != null) {
            Hairline()
            KeyValueRow("Token", tokenInfo, valueColor = if (tokenInfo == "expired") colors.err else colors.fg)
        }
        account.file?.let { file ->
            Hairline()
            Column(modifier = Modifier.padding(vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("Stored in", style = Typeface.row, color = colors.fg3)
                Text(file, style = Typeface.data(12.5.sp), color = colors.fg)
            }
        }
    }
}
