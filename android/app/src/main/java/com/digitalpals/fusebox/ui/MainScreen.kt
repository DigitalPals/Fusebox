package com.digitalpals.fusebox.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ListAlt
import androidx.compose.material.icons.outlined.BarChart
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.Group
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.Speed
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.R
import com.digitalpals.fusebox.data.AppStore
import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.UsageDashboard
import com.digitalpals.fusebox.data.UsageSpeed
import com.digitalpals.fusebox.push.PushManager
import com.digitalpals.fusebox.ui.adaptive.LocalFoldableState
import com.digitalpals.fusebox.ui.components.Hairline
import com.digitalpals.fusebox.ui.screens.AccountsScreen
import com.digitalpals.fusebox.ui.screens.ConnectScreen
import com.digitalpals.fusebox.ui.screens.FaultsSheetContent
import com.digitalpals.fusebox.ui.screens.OverviewScreen
import com.digitalpals.fusebox.ui.screens.RequestsScreen
import com.digitalpals.fusebox.ui.screens.SettingsScreen
import com.digitalpals.fusebox.ui.screens.UsageScreen
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface

enum class AppTab(val title: String, val icon: ImageVector) {
    OVERVIEW("Overview", Icons.Outlined.Speed),
    ACCOUNTS("Accounts", Icons.Outlined.Group),
    REQUESTS("Requests", Icons.AutoMirrored.Outlined.ListAlt),
    USAGE("Usage", Icons.Outlined.BarChart),
    SETTINGS("Settings", Icons.Outlined.Settings),
}

/**
 * Root adaptive shell (Option A):
 * - Folded / phone (< 600dp): flat top bar + content + bottom navigation bar.
 * - Unfolded foldable / tablet (>= 600dp): left Navigation Rail + top bar + multi-pane / 2-column content.
 * Preserves active tab, selected account, selected request and faults sheet across folding/unfolding.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MainScreen(
    store: AppStore,
    pushManager: PushManager,
    pendingLink: DeepLink?,
    onConsumeLink: () -> Unit,
    demoUsage: UsageDashboard? = null,
    demoSpeed: UsageSpeed? = null,
) {
    val state by store.state.collectAsState()
    val foldable = LocalFoldableState.current
    val colors = LocalFuseboxColors.current

    if (!state.isConnected) {
        ConnectScreen(
            initialUrl = state.serverUrl.orEmpty(),
            onConnected = { url, key -> store.connect(url, key) },
        )
        return
    }

    var currentTabName by rememberSaveable { mutableStateOf(AppTab.OVERVIEW.name) }
    var selectedAccountId by rememberSaveable { mutableStateOf<String?>(null) }
    var selectedRequestId by rememberSaveable { mutableStateOf<Int?>(null) }
    var faultsSheetOpen by rememberSaveable { mutableStateOf(false) }

    val currentTab = AppTab.entries.firstOrNull { it.name == currentTabName } ?: AppTab.OVERVIEW

    fun navigate(link: DeepLink) {
        faultsSheetOpen = false
        when (link) {
            DeepLink.Overview -> currentTabName = AppTab.OVERVIEW.name
            DeepLink.Accounts -> {
                currentTabName = AppTab.ACCOUNTS.name
                selectedAccountId = null
            }
            is DeepLink.Account -> {
                currentTabName = AppTab.ACCOUNTS.name
                selectedAccountId = link.id
            }
            DeepLink.Requests -> {
                currentTabName = AppTab.REQUESTS.name
                selectedRequestId = null
            }
            is DeepLink.Request -> {
                currentTabName = AppTab.REQUESTS.name
                selectedRequestId = link.id
            }
            DeepLink.Usage -> currentTabName = AppTab.USAGE.name
            DeepLink.Faults -> faultsSheetOpen = true
            DeepLink.Settings -> currentTabName = AppTab.SETTINGS.name
        }
    }

    LaunchedEffect(pendingLink) {
        if (pendingLink != null) {
            navigate(pendingLink)
            onConsumeLink()
        }
    }

    // Back gesture handling: faults sheet -> compact detail -> Overview tab
    BackHandler(
        enabled = faultsSheetOpen ||
            (!foldable.supportsTwoPanes && currentTab == AppTab.ACCOUNTS && selectedAccountId != null) ||
            (!foldable.supportsTwoPanes && currentTab == AppTab.REQUESTS && selectedRequestId != null) ||
            currentTab != AppTab.OVERVIEW,
    ) {
        when {
            faultsSheetOpen -> faultsSheetOpen = false
            !foldable.supportsTwoPanes && currentTab == AppTab.ACCOUNTS && selectedAccountId != null -> selectedAccountId = null
            !foldable.supportsTwoPanes && currentTab == AppTab.REQUESTS && selectedRequestId != null -> selectedRequestId = null
            currentTab != AppTab.OVERVIEW -> currentTabName = AppTab.OVERVIEW.name
        }
    }

    val showingCompactDetail = !foldable.supportsTwoPanes && (
        (currentTab == AppTab.ACCOUNTS && selectedAccountId != null) ||
            (currentTab == AppTab.REQUESTS && selectedRequestId != null)
        )

    @Composable
    fun PageContent() {
        when (currentTab) {
            AppTab.OVERVIEW -> OverviewScreen(
                state = state,
                onRefresh = { store.refreshAll() },
                onSetQuotaDisplay = { store.setQuotaDisplay(it) },
                onNavigate = ::navigate,
            )
            AppTab.ACCOUNTS -> AccountsScreen(
                state = state,
                store = store,
                selectedAccountId = selectedAccountId,
                onSelectAccount = { selectedAccountId = it },
                onSetQuotaDisplay = { store.setQuotaDisplay(it) },
            )
            AppTab.REQUESTS -> RequestsScreen(
                state = state,
                store = store,
                selectedRequestId = selectedRequestId,
                onSelectRequest = { selectedRequestId = it },
                onNavigate = ::navigate,
            )
            AppTab.USAGE -> UsageScreen(
                state = state,
                store = store,
                demoUsage = demoUsage,
                demoSpeed = demoSpeed,
            )
            AppTab.SETTINGS -> SettingsScreen(
                state = state,
                store = store,
                pushManager = pushManager,
            )
        }
    }

    if (foldable.useNavigationRail) {
        // Unfolded foldable / tablet layout: Left Navigation Rail + full-height adaptive content
        Row(
            modifier = Modifier
                .fillMaxSize()
                .background(colors.bg)
                .windowInsetsPadding(WindowInsets.safeDrawing),
        ) {
            FuseboxNavRail(
                currentTab = currentTab,
                faultsCount = state.faults.size,
                hasErrFault = state.faults.any { it.level == "err" },
                onSelectTab = { currentTabName = it.name },
                onOpenFaults = { faultsSheetOpen = true },
            )
            Box(modifier = Modifier.width(1.dp).fillMaxHeight().background(colors.line))
            Column(modifier = Modifier.weight(1f).fillMaxHeight()) {
                TopHeaderBar(
                    title = currentTab.title,
                    faultsCount = state.faults.size,
                    hasErrFault = state.faults.any { it.level == "err" },
                    showMark = false,
                    showFaults = false,
                    onOpenFaults = { faultsSheetOpen = true },
                )
                Hairline()
                Box(modifier = Modifier.weight(1f).fillMaxWidth()) {
                    PageContent()
                }
            }
        }
    } else {
        // Compact / folded outer screen layout: Top bar + Content + Flat bottom tab bar
        Column(
            modifier = Modifier
                .fillMaxSize()
                .background(colors.bg),
        ) {
            if (!showingCompactDetail) {
                Box(modifier = Modifier.fillMaxWidth().background(colors.bg).statusBarsPadding()) {
                    TopHeaderBar(
                        title = currentTab.title,
                        faultsCount = state.faults.size,
                        hasErrFault = state.faults.any { it.level == "err" },
                        showMark = true,
                        showFaults = currentTab != AppTab.SETTINGS,
                        onOpenFaults = { faultsSheetOpen = true },
                    )
                }
                Hairline()
            } else {
                Spacer(Modifier.statusBarsPadding())
            }
            Box(modifier = Modifier.weight(1f).fillMaxWidth()) {
                PageContent()
            }
            Hairline()
            FuseboxBottomBar(
                currentTab = currentTab,
                onSelectTab = { tab ->
                    if (tab == currentTab) {
                        // Tapping active tab pops detail back to list
                        if (tab == AppTab.ACCOUNTS) selectedAccountId = null
                        if (tab == AppTab.REQUESTS) selectedRequestId = null
                    }
                    currentTabName = tab.name
                },
            )
        }
    }

    if (faultsSheetOpen) {
        val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        ModalBottomSheet(
            onDismissRequest = { faultsSheetOpen = false },
            sheetState = sheetState,
            containerColor = colors.bg,
            scrimColor = Color.Black.copy(alpha = 0.55f),
            shape = RoundedCornerShape(topStart = 18.dp, topEnd = 18.dp),
            dragHandle = null,
        ) {
            FaultsSheetContent(
                state = state,
                store = store,
                onNavigate = ::navigate,
                onClose = { faultsSheetOpen = false },
            )
        }
    }
}

@Composable
private fun TopHeaderBar(
    title: String,
    faultsCount: Int,
    hasErrFault: Boolean,
    showMark: Boolean,
    showFaults: Boolean,
    onOpenFaults: () -> Unit,
) {
    val colors = LocalFuseboxColors.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(52.dp)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (showMark) {
            Image(
                painter = painterResource(R.drawable.ic_mark),
                contentDescription = null,
                colorFilter = ColorFilter.tint(colors.fg),
                modifier = Modifier.height(18.dp).padding(end = 10.dp),
            )
        }
        Text(
            text = title.uppercase(),
            style = Typeface.pageTitle,
            color = colors.fg,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (showFaults) {
            FaultsHeaderButton(
                count = faultsCount,
                hasErr = hasErrFault,
                onClick = onOpenFaults,
            )
        }
    }
}

@Composable
private fun FaultsHeaderButton(count: Int, hasErr: Boolean, onClick: () -> Unit) {
    val colors = LocalFuseboxColors.current
    val desc = if (count == 0) "No faults" else "Faults, $count"
    Row(
        modifier = Modifier
            .defaultMinSize(minWidth = 44.dp, minHeight = 40.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = desc }
            .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(
            imageVector = if (count == 0) Icons.Outlined.Check else Icons.Outlined.WarningAmber,
            contentDescription = null,
            tint = if (count == 0) colors.fg3 else if (hasErr) colors.err else colors.warn,
            modifier = Modifier.size(18.dp),
        )
        if (count > 0) {
            Box(
                modifier = Modifier
                    .clip(CircleShape)
                    .background(if (hasErr) colors.err else colors.warn)
                    .padding(horizontal = 6.dp, vertical = 1.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("$count", style = Typeface.data(11.sp), color = colors.bg)
            }
        }
    }
}

@Composable
private fun FuseboxBottomBar(
    currentTab: AppTab,
    onSelectTab: (AppTab) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(colors.bg)
            .navigationBarsPadding()
            .height(62.dp)
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AppTab.entries.forEach { tab ->
            val on = tab == currentTab
            Column(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .clickable(role = Role.Tab) { onSelectTab(tab) }
                    .semantics { selected = on },
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Box(
                    modifier = Modifier
                        .width(18.dp)
                        .height(2.dp)
                        .background(if (on) colors.fg else Color.Transparent, RoundedCornerShape(1.dp)),
                )
                Spacer(Modifier.height(6.dp))
                Icon(
                    imageVector = tab.icon,
                    contentDescription = tab.title,
                    tint = if (on) colors.fg else colors.fg3,
                    modifier = Modifier.size(20.dp),
                )
                Spacer(Modifier.height(2.dp))
                Text(
                    text = tab.title,
                    style = Typeface.small.copy(fontSize = 11.sp),
                    color = if (on) colors.fg else colors.fg3,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
private fun FuseboxNavRail(
    currentTab: AppTab,
    faultsCount: Int,
    hasErrFault: Boolean,
    onSelectTab: (AppTab) -> Unit,
    onOpenFaults: () -> Unit,
) {
    val colors = LocalFuseboxColors.current
    Column(
        modifier = Modifier
            .width(80.dp)
            .fillMaxHeight()
            .background(colors.bg)
            .padding(vertical = 14.dp, horizontal = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Image(
            painter = painterResource(R.drawable.ic_mark),
            contentDescription = "Fusebox",
            colorFilter = ColorFilter.tint(colors.fg),
            modifier = Modifier.padding(vertical = 8.dp).height(22.dp),
        )
        Spacer(Modifier.height(6.dp))
        AppTab.entries.forEach { tab ->
            val on = tab == currentTab
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) colors.segOn else Color.Transparent)
                    .clickable(role = Role.Tab) { onSelectTab(tab) }
                    .semantics { selected = on }
                    .padding(vertical = 10.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Icon(
                    imageVector = tab.icon,
                    contentDescription = tab.title,
                    tint = if (on) colors.fg else colors.fg3,
                    modifier = Modifier.size(20.dp),
                )
                Text(
                    text = tab.title,
                    style = Typeface.small.copy(fontSize = 11.sp),
                    color = if (on) colors.fg else colors.fg3,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Spacer(Modifier.weight(1f))
        FaultsHeaderButton(count = faultsCount, hasErr = hasErrFault, onClick = onOpenFaults)
    }
}
