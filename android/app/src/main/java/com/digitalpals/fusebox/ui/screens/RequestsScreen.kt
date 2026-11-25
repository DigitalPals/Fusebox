package com.digitalpals.fusebox.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.ListAlt
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.data.Account
import com.digitalpals.fusebox.data.AppStore
import com.digitalpals.fusebox.data.AppUiState
import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.ProviderInfo
import com.digitalpals.fusebox.data.RequestLog
import com.digitalpals.fusebox.data.clientDescription
import com.digitalpals.fusebox.data.clientName
import com.digitalpals.fusebox.data.endpoint
import com.digitalpals.fusebox.data.errorLine
import com.digitalpals.fusebox.data.isError
import com.digitalpals.fusebox.data.isRerouted
import com.digitalpals.fusebox.data.logo
import com.digitalpals.fusebox.data.providerName
import com.digitalpals.fusebox.data.reasonText
import com.digitalpals.fusebox.data.sessionWarning
import com.digitalpals.fusebox.data.tokensText
import com.digitalpals.fusebox.data.transportTag
import com.digitalpals.fusebox.data.whySentence
import com.digitalpals.fusebox.ui.adaptive.FoldableListDetail
import com.digitalpals.fusebox.ui.adaptive.LocalFoldableState
import com.digitalpals.fusebox.ui.components.CapsLabel
import com.digitalpals.fusebox.ui.components.CardPanel
import com.digitalpals.fusebox.ui.components.EmptyState
import com.digitalpals.fusebox.ui.components.Figure
import com.digitalpals.fusebox.ui.components.FuseButton
import com.digitalpals.fusebox.ui.components.FuseTextField
import com.digitalpals.fusebox.ui.components.Hairline
import com.digitalpals.fusebox.ui.components.KeyValueRow
import com.digitalpals.fusebox.ui.components.PanelHead
import com.digitalpals.fusebox.ui.components.ProviderLogo
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface
import kotlinx.coroutines.launch

enum class RequestFilter(val title: String) {
    ALL("All"),
    FAILED("Failed"),
    REROUTED("Rerouted"),
    CANCELLED("Cancelled"),
    ;

    fun matches(r: RequestLog): Boolean = when (this) {
        ALL -> true
        FAILED -> r.isError
        REROUTED -> r.isRerouted
        CANCELLED -> r.status == 499
    }
}

/**
 * Requests tab: Option A adaptive List-Detail on unfolded foldables (live request log left of the fold,
 * routing explanation and request metadata right of the fold).
 */
@Composable
fun RequestsScreen(
    state: AppUiState,
    store: AppStore,
    selectedRequestId: Int?,
    onSelectRequest: (Int?) -> Unit,
    onNavigate: (DeepLink) -> Unit,
) {
    val foldable = LocalFoldableState.current
    var filterName by rememberSaveable { mutableStateOf(RequestFilter.ALL.name) }
    var search by rememberSaveable { mutableStateOf("") }
    val filter = RequestFilter.entries.firstOrNull { it.name == filterName } ?: RequestFilter.ALL

    LaunchedEffect(Unit) {
        store.loadFullRequests()
    }

    val shown = state.requests.filter { r ->
        filter.matches(r) && matchesSearch(r, search)
    }

    val effectiveId = when {
        selectedRequestId != null -> selectedRequestId
        foldable.supportsTwoPanes -> shown.firstOrNull()?.id
        else -> null
    }

    FoldableListDetail(
        hasSelection = effectiveId != null,
        listPane = {
            RequestsListPane(
                state = state,
                shown = shown,
                filter = filter,
                onFilter = { filterName = it.name },
                search = search,
                onSearch = { search = it },
                selectedId = if (foldable.supportsTwoPanes) effectiveId else null,
                onSelect = onSelectRequest,
                onRefresh = { store.refreshAll() },
            )
        },
        detailPane = {
            val id = effectiveId ?: return@FoldableListDetail
            RequestDetailPane(
                id = id,
                state = state,
                showBackButton = !foldable.supportsTwoPanes,
                onBack = { onSelectRequest(null) },
                onNavigate = onNavigate,
            )
        },
        emptyDetailPane = {
            Box(Modifier.fillMaxSize().background(LocalFuseboxColors.current.bg), contentAlignment = Alignment.Center) {
                EmptyState(
                    title = "Select a request",
                    icon = Icons.AutoMirrored.Outlined.ListAlt,
                    message = "Pick a request on the left to see why Fusebox routed it there.",
                )
            }
        },
    )
}

private fun matchesSearch(r: RequestLog, query: String): Boolean {
    val q = query.trim().lowercase()
    if (q.isEmpty()) return true
    return listOfNotNull(r.model, r.account, r.clientApp, r.keyLabel, r.provider, r.status?.toString())
        .any { it.lowercase().contains(q) }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RequestsListPane(
    state: AppUiState,
    shown: List<RequestLog>,
    filter: RequestFilter,
    onFilter: (RequestFilter) -> Unit,
    search: String,
    onSearch: (String) -> Unit,
    selectedId: Int?,
    onSelect: (Int) -> Unit,
    onRefresh: suspend () -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val scope = rememberCoroutineScope()

    PullToRefreshBox(
        isRefreshing = state.isRefreshing,
        onRefresh = { scope.launch { onRefresh() } },
        modifier = Modifier.fillMaxSize().background(colors.bg),
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            FuseTextField(
                value = search,
                onValueChange = onSearch,
                placeholder = "Model, account or client",
            )
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                RequestFilter.entries.forEach { f ->
                    val count = state.requests.count { f.matches(it) }
                    val on = filter == f
                    val shape = RoundedCornerShape(6.dp)
                    Row(
                        modifier = Modifier
                            .defaultMinSize(minHeight = 32.dp)
                            .clip(shape)
                            .background(if (on) colors.segOn else Color.Transparent, shape)
                            .border(1.dp, if (on) colors.lineHover else colors.line, shape)
                            .clickable { onFilter(f) }
                            .padding(horizontal = 9.dp, vertical = 5.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(f.title, style = Typeface.small, color = if (on) colors.fg else colors.fg2)
                        Text(Fmt.count(count), style = Typeface.data(11.sp), color = colors.fg3)
                    }
                }
            }
            if (shown.isEmpty()) {
                Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                    EmptyState(
                        title = if (state.requests.isEmpty()) "No requests yet" else "No matching requests",
                        icon = Icons.AutoMirrored.Outlined.ListAlt,
                    )
                }
            } else {
                CardPanel(padding = 0.dp, spacing = 0.dp, modifier = Modifier.weight(1f)) {
                    LazyColumn(modifier = Modifier.fillMaxSize()) {
                        itemsIndexed(shown, key = { _, r -> r.id }) { idx, r ->
                            if (idx > 0) Hairline(Modifier.padding(horizontal = 16.dp))
                            Box(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .background(if (r.id == selectedId) colors.segOn else Color.Transparent)
                                    .clickable { onSelect(r.id) }
                                    .padding(horizontal = 16.dp, vertical = 12.dp),
                            ) {
                                RequestRowView(request = r, account = state.account(r.accountID), compact = false)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
fun RequestRowView(
    request: RequestLog,
    account: Account?,
    compact: Boolean = false,
) {
    val colors = LocalFuseboxColors.current
    Column(modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(Fmt.clock(request.ts), style = Typeface.data(12.sp), color = colors.fg3)
            if (request.provider != null) {
                ProviderLogo(account?.logo ?: ProviderInfo.logo(request.provider), size = 13.dp)
            }
            Text(
                text = "${request.clientName} → ${account?.providerName ?: ProviderInfo.name(request.provider)}",
                style = Typeface.meta,
                color = colors.fg2,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            val tag = request.transportTag ?: if ((request.attempts ?: 1) > 1) "${request.attempts} tries" else null
            if (tag != null) {
                CapsLabel(
                    text = tag,
                    size = 9.5.sp,
                    color = colors.fg3,
                    modifier = Modifier
                        .border(1.dp, colors.lineStrong, RoundedCornerShape(3.dp))
                        .padding(horizontal = 4.dp, vertical = 1.dp),
                )
            }
            Text(
                text = request.status?.toString() ?: "—",
                style = Typeface.data(12.5.sp),
                color = when {
                    request.isError -> colors.err
                    request.status == 499 -> colors.fg3
                    else -> colors.fg2
                },
            )
            Text(
                text = Fmt.ms(request.latencyMs),
                style = Typeface.data(12.sp),
                color = colors.fg2,
            )
        }
        Text(
            text = request.model ?: "—",
            style = Typeface.data(13.sp),
            color = colors.fg,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = account?.label ?: request.account ?: "—",
                style = Typeface.small,
                color = colors.fg3,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            request.reasonText?.let { reason ->
                Text("·", style = Typeface.small, color = colors.fg3)
                Text(reason, style = Typeface.small, color = if (request.isRerouted) colors.warn else colors.fg3, maxLines = 1)
            }
            if (!compact) {
                val sid = request.sessionID
                val warn = request.sessionWarning
                if (sid != null) {
                    Text("·", style = Typeface.small, color = colors.fg3)
                    Text(sid.take(8), style = Typeface.data(11.5.sp), color = colors.fg3)
                } else if (warn != null) {
                    Text("·", style = Typeface.small, color = colors.fg3)
                    Text(warn.first, style = Typeface.small, color = colors.warn, maxLines = 1)
                }
            }
        }
        if (!compact && request.errorLine != null) {
            Text(
                text = request.errorLine!!,
                style = Typeface.small,
                color = if (request.status == 499) colors.fg3 else colors.err,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun RequestDetailPane(
    id: Int,
    state: AppUiState,
    showBackButton: Boolean,
    onBack: () -> Unit,
    onNavigate: (DeepLink) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val r = state.requests.firstOrNull { it.id == id }
    if (r == null) {
        Box(Modifier.fillMaxSize().background(colors.bg), contentAlignment = Alignment.Center) {
            EmptyState(
                title = "Request not in the log",
                icon = Icons.AutoMirrored.Outlined.ListAlt,
                message = "The server keeps the most recent 300 requests.",
            )
        }
        return
    }
    val acct = state.account(r.accountID)

    Column(modifier = Modifier.fillMaxSize().background(colors.bg)) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (showBackButton) {
                IconButton(onClick = onBack) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back", tint = colors.fg)
                }
            } else {
                Spacer(Modifier.width(8.dp))
            }
            CapsLabel("Request ${Fmt.clock(r.ts)}", size = 12.sp, color = colors.fg2)
        }
        Hairline()

        Column(
            modifier = Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            // Summary header
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    if (r.provider != null) {
                        ProviderLogo(acct?.logo ?: ProviderInfo.logo(r.provider), size = 22.dp)
                    }
                    Text(r.model ?: "—", style = Typeface.data(17.sp), color = colors.fg, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
                Text(
                    text = "${r.clientName} → ${acct?.providerName ?: ProviderInfo.name(r.provider)} · ${acct?.label ?: r.account ?: "—"}",
                    style = Typeface.meta,
                    color = colors.fg3,
                )
                Row(modifier = Modifier.fillMaxWidth()) {
                    Figure(
                        value = r.status?.toString() ?: "—",
                        label = "Status",
                        color = if (r.isError) colors.err else if (r.status == 499) colors.fg3 else colors.fg,
                        modifier = Modifier.weight(1f),
                    )
                    Figure(Fmt.ms(r.ttftMs), "First token", modifier = Modifier.weight(1f))
                    Figure(Fmt.ms(r.latencyMs), "Total", modifier = Modifier.weight(1f))
                }
            }

            CardPanel(spacing = 8.dp) {
                PanelHead("Why this account")
                Text(r.whySentence, style = Typeface.body, color = colors.fg)
                r.sessionWarning?.let { (wTitle, wBody) ->
                    Text("$wTitle: $wBody", style = Typeface.meta, color = colors.warn)
                }
            }

            r.errorLine?.let { err ->
                CardPanel(
                    spacing = 8.dp,
                    borderColor = if (r.status != 499) colors.errLine else null,
                ) {
                    PanelHead(if (r.status == 499) "Cancelled" else "Error")
                    Text(err, style = Typeface.data(12.5.sp), color = if (r.status == 499) colors.fg2 else colors.err)
                }
            }

            CardPanel(spacing = 0.dp) {
                PanelHead("Request")
                Spacer(Modifier.height(4.dp))
                val items = buildList {
                    add(Triple("Client", r.clientDescription, false))
                    add(Triple("Endpoint", r.endpoint, true))
                    add(Triple("Account", acct?.label ?: r.account ?: "—", false))
                    add(Triple("Session", r.sessionID?.take(8) ?: "none", true))
                    r.source?.let { add(Triple("Source", sourceText(it), true)) }
                    r.keyLabel?.let { add(Triple("Key", it, false)) }
                    add(Triple("Tokens", r.tokensText, true))
                    add(Triple("Time", Fmt.fullTime(r.ts), true))
                }
                items.forEach { (k, v, mono) ->
                    Hairline()
                    KeyValueRow(key = k, value = v, mono = mono)
                }
            }

            r.cachePolicy?.let { p ->
                CardPanel(spacing = 8.dp) {
                    PanelHead("Prompt cache")
                    Text(
                        text = "Requested ${cacheLabel(p.requested)} · applied ${cacheLabel(p.applied)} · ${cacheLabel(p.keyScope)}",
                        style = Typeface.row,
                        color = colors.fg2,
                    )
                }
            }

            if (acct != null) {
                FuseButton(
                    text = "Open ${acct.label}",
                    fill = true,
                    onClick = { onNavigate(DeepLink.Account(acct.id)) },
                )
            }
        }
    }
}

private fun sourceText(s: RequestLog.Source): String {
    val via = when (s.via) {
        "forwarded" -> s.login?.let { "via Tailscale · $it" } ?: "via reverse proxy"
        "proxy_protocol" -> "via TCP forwarder"
        "unknown" -> "proxy gave no readable address"
        else -> if ((s.ip ?: "").startsWith("127.") || s.ip == "::1") "this machine" else "direct"
    }
    return "${s.ip.orEmpty()} · $via"
}

private fun cacheLabel(v: String?): String = when (v) {
    "provider_default" -> "provider default"
    "prewarm" -> "prewarm"
    "automatic" -> "automatic"
    "explicit" -> "explicit boundaries"
    "explicit_no_breakpoints" -> "explicit without boundaries"
    "none" -> "no cache controls"
    "rejected" -> "rejected"
    "caller" -> "caller key"
    "session" -> "session"
    "client" -> "client"
    "workspace" -> "workspace"
    "session_fallback" -> "session fallback"
    "not_sent" -> "no cache key sent"
    else -> v ?: "—"
}
