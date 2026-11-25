package com.digitalpals.fusebox.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.data.AppStore
import com.digitalpals.fusebox.data.AppUiState
import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.Fault
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.ui.components.CardPanel
import com.digitalpals.fusebox.ui.components.EmptyState
import com.digitalpals.fusebox.ui.components.FuseButton
import com.digitalpals.fusebox.ui.components.FuseButtonKind
import com.digitalpals.fusebox.ui.components.Hairline
import com.digitalpals.fusebox.ui.components.LinkButton
import com.digitalpals.fusebox.ui.components.StatusDot
import com.digitalpals.fusebox.ui.components.ToastBanner
import com.digitalpals.fusebox.ui.components.rememberTicker
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface
import kotlinx.coroutines.launch
import java.time.Instant

@Composable
fun FaultsSheetContent(
    state: AppUiState,
    store: AppStore,
    onNavigate: (DeepLink) -> Unit,
    onClose: () -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val scope = rememberCoroutineScope()
    val now = rememberTicker()
    var toast by remember { mutableStateOf<String?>(null) }

    Box(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 260.dp, max = 640.dp)
            .background(colors.bg),
    ) {
        Column(modifier = Modifier.fillMaxWidth()) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("FAULTS", style = Typeface.pageTitle, color = colors.fg, modifier = Modifier.weight(1f))
                FuseButton("Done", compact = true, kind = FuseButtonKind.QUIET, onClick = onClose)
            }
            Hairline()

            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                if (state.faults.isEmpty()) {
                    EmptyState(
                        title = if (state.dismissedFaults.isEmpty()) "Nothing has tripped" else "Nothing else has tripped",
                        icon = Icons.Outlined.CheckCircle,
                        message = "Expired sign-ins, used-up limits, rate limits and account errors show here.",
                    )
                } else {
                    Text(
                        text = "Expired sign-ins, used-up limits, rate limits and account errors, with the time each comes back.",
                        style = Typeface.meta,
                        color = colors.fg3,
                    )
                    CardPanel(padding = 0.dp, spacing = 0.dp) {
                        state.faults.forEachIndexed { idx, f ->
                            if (idx > 0) Hairline(Modifier.padding(horizontal = 16.dp))
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                verticalAlignment = Alignment.Top,
                            ) {
                                Box(
                                    modifier = Modifier
                                        .weight(1f)
                                        .clickable {
                                            onClose()
                                            val acctId = f.accountID
                                            if (acctId != null) {
                                                onNavigate(DeepLink.Account(acctId))
                                            } else if (f.kind == "provider") {
                                                onNavigate(DeepLink.Accounts)
                                            } else {
                                                onNavigate(DeepLink.routePath(f.path))
                                            }
                                        }
                                        .padding(start = 16.dp, top = 14.dp, bottom = 14.dp),
                                ) {
                                    FaultRowView(fault = f, now = now)
                                }
                                IconButton(
                                    onClick = {
                                        scope.launch {
                                            try {
                                                store.dismiss(f)
                                            } catch (e: Exception) {
                                                toast = e.message ?: "Could not dismiss"
                                            }
                                        }
                                    },
                                    modifier = Modifier.padding(top = 4.dp, end = 4.dp),
                                ) {
                                    Icon(
                                        imageVector = Icons.Default.Close,
                                        contentDescription = "Dismiss ${f.title}",
                                        tint = colors.fg3,
                                        modifier = Modifier.size(16.dp),
                                    )
                                }
                            }
                        }
                    }
                }

                if (state.dismissedFaults.isNotEmpty()) {
                    val n = state.dismissedFaults.size
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            text = "$n dismissed until ${if (n == 1) "it clears" else "they clear"}",
                            style = Typeface.meta,
                            color = colors.fg3,
                            modifier = Modifier.weight(1f),
                        )
                        LinkButton("Show again", onClick = {
                            scope.launch {
                                try {
                                    store.restoreDismissedFaults()
                                } catch (e: Exception) {
                                    toast = e.message ?: "Could not restore faults"
                                }
                            }
                        })
                    }
                }
            }
        }
        ToastBanner(
            text = toast,
            onDismiss = { toast = null },
            modifier = Modifier.align(Alignment.BottomCenter),
        )
    }
}

@Composable
fun FaultRowView(fault: Fault, now: Instant) {
    val colors = LocalFuseboxColors.current
    val dotColor = if (fault.kind == "pace") colors.fg3 else colors.faultColor(fault)
    val who = listOfNotNull(fault.providerName, fault.label).filter { it.isNotEmpty() }.joinToString(" · ")
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.Top,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        StatusDot(dotColor, modifier = Modifier.padding(top = 6.dp))
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(fault.title, style = Typeface.rowMedium, color = colors.fg)
            if (who.isNotEmpty()) Text(who, style = Typeface.small, color = colors.fg2)
            if (!fault.detail.isNullOrEmpty()) {
                Text(fault.detail, style = Typeface.small, color = colors.fg3, maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
            fault.until?.let { until ->
                Text(
                    text = if (until.isAfter(now)) "back ${Fmt.whenText(until, now)} · ${Fmt.left(until, now)}" else "Clearing",
                    style = Typeface.meta,
                    color = if (fault.kind == "pace") colors.fg2 else colors.faultColor(fault),
                )
            }
        }
        if (fault.accountID != null) {
            Icon(
                imageVector = Icons.AutoMirrored.Filled.KeyboardArrowRight,
                contentDescription = null,
                tint = colors.fg3,
                modifier = Modifier.padding(top = 2.dp).size(18.dp),
            )
        }
    }
}
