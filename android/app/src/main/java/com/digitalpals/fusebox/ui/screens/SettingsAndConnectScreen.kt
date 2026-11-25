package com.digitalpals.fusebox.ui.screens

import android.Manifest
import android.net.Uri
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.Image
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
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material.icons.outlined.PhoneAndroid
import androidx.compose.material.icons.outlined.PhoneIphone
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitalpals.fusebox.BuildConfig
import com.digitalpals.fusebox.R
import com.digitalpals.fusebox.data.AppStore
import com.digitalpals.fusebox.data.AppUiState
import com.digitalpals.fusebox.data.ConnectionHelper
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.FuseboxApi
import com.digitalpals.fusebox.data.PushPath
import com.digitalpals.fusebox.data.PushStatus
import com.digitalpals.fusebox.data.QuotaDisplay
import com.digitalpals.fusebox.push.PushManager
import com.digitalpals.fusebox.ui.components.BreakerSwitch
import com.digitalpals.fusebox.ui.components.CapsLabel
import com.digitalpals.fusebox.ui.components.CardPanel
import com.digitalpals.fusebox.ui.components.FuseButton
import com.digitalpals.fusebox.ui.components.FuseButtonKind
import com.digitalpals.fusebox.ui.components.FuseSegmented
import com.digitalpals.fusebox.ui.components.FuseTextField
import com.digitalpals.fusebox.ui.components.Hairline
import com.digitalpals.fusebox.ui.components.KeyValueRow
import com.digitalpals.fusebox.ui.components.LiveBadge
import com.digitalpals.fusebox.ui.components.StatusDot
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface
import kotlinx.coroutines.launch

@Composable
fun ConnectScreen(
    initialUrl: String,
    onConnected: (String, String) -> Unit,
) {
    val colors = LocalFuseboxColors.current
    val scope = rememberCoroutineScope()
    var url by rememberSaveable { mutableStateOf(initialUrl) }
    var key by rememberSaveable { mutableStateOf("") }
    var checking by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    fun submit() {
        val normalized = ConnectionHelper.normalize(url)
        if (normalized == null) {
            error = "That isn’t a web address."
            return
        }
        val trimmedKey = key.trim()
        if (trimmedKey.isEmpty()) return
        scope.launch {
            checking = true
            error = null
            try {
                val api = FuseboxApi(normalized, trimmedKey)
                val ov = api.overview()
                if (ov.managementKey == false) {
                    error = "This Fusebox has no management key set, so it only accepts management from its own machine."
                } else {
                    onConnected(normalized, trimmedKey)
                }
            } catch (e: Exception) {
                error = e.message ?: "Can’t reach Fusebox"
            } finally {
                checking = false
            }
        }
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(colors.bg)
            .safeDrawingPadding(),
        contentAlignment = Alignment.TopCenter,
    ) {
        Column(
            modifier = Modifier
                .widthIn(max = 520.dp)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 32.dp),
            verticalArrangement = Arrangement.spacedBy(24.dp),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Image(
                    painter = painterResource(R.drawable.ic_mark),
                    contentDescription = null,
                    colorFilter = ColorFilter.tint(colors.fg),
                    modifier = Modifier.height(40.dp),
                )
                Text("Connect to Fusebox", style = Typeface.sans(26.sp, FontWeight.SemiBold), color = colors.fg)
                Text(
                    text = "Your subscriptions’ limits, faults and traffic, from your own Fusebox server.",
                    style = Typeface.body,
                    color = colors.fg2,
                )
            }

            CardPanel(spacing = 8.dp) {
                CapsLabel("Server", size = 11.sp)
                FuseTextField(
                    value = url,
                    onValueChange = { url = it },
                    placeholder = "https://fusebox.example.ts.net",
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next),
                )
                Text(
                    text = "The dashboard’s address. A tailnet address needs Tailscale connected on this device.",
                    style = Typeface.small,
                    color = colors.fg3,
                )

                Spacer(Modifier.height(6.dp))
                CapsLabel("Management key", size = 11.sp)
                FuseTextField(
                    value = key,
                    onValueChange = { key = it },
                    placeholder = "Management key",
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { submit() }),
                )
                Text(
                    text = "management-key in the server’s config.yaml. It is kept encrypted in Android Keystore.",
                    style = Typeface.small,
                    color = colors.fg3,
                )
            }

            if (error != null) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(colors.surface, RoundedCornerShape(8.dp))
                        .border(1.dp, colors.errLine, RoundedCornerShape(8.dp))
                        .padding(14.dp),
                    verticalAlignment = Alignment.Top,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    StatusDot(colors.err, modifier = Modifier.padding(top = 6.dp))
                    Text(error.orEmpty(), style = Typeface.row, color = colors.fg)
                }
            }

            FuseButton(
                text = if (checking) "Checking…" else "Connect",
                kind = FuseButtonKind.PRIMARY,
                fill = true,
                enabled = !checking && url.isNotBlank() && key.isNotBlank(),
                onClick = ::submit,
            )
        }
    }
}

@Composable
fun SettingsScreen(
    state: AppUiState,
    store: AppStore,
    pushManager: PushManager,
    contentPadding: PaddingValues = PaddingValues(16.dp),
) {
    val colors = LocalFuseboxColors.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val push by pushManager.state.collectAsState()
    var confirmSignOut by remember { mutableStateOf(false) }
    var removingDevice by remember { mutableStateOf<PushStatus.Device?>(null) }

    val notifPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        pushManager.refreshPermission()
        if (granted) {
            scope.launch { pushManager.turnOn(store.currentApi()) }
        }
    }

    LaunchedEffect(state.serverUrl) {
        pushManager.refreshPermission()
        pushManager.loadDevices(store.currentApi())
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(colors.bg),
        contentAlignment = Alignment.TopCenter,
    ) {
        Column(
            modifier = Modifier
                .widthIn(max = 640.dp)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(contentPadding),
            verticalArrangement = Arrangement.spacedBy(18.dp),
        ) {
            // Connection section
            SettingsSection(
                title = "Connection",
                note = "Sign-in flows, API keys, models and the config editor are in the web dashboard.",
            ) {
                CardPanel(spacing = 0.dp) {
                    KeyValueRow("Server", state.serverHost ?: "—", mono = true)
                    state.overview?.version?.let { v ->
                        Hairline()
                        KeyValueRow("Fusebox", v, mono = true)
                    }
                    Hairline()
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = 10.dp),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text("Status", style = Typeface.row, color = colors.fg3)
                        LiveBadge(state.live)
                    }
                    state.serverUrl?.let { url ->
                        Spacer(Modifier.height(6.dp))
                        FuseButton(
                            text = "Open the web dashboard",
                            fill = true,
                            onClick = {
                                runCatching {
                                    CustomTabsIntent.Builder().build().launchUrl(context, Uri.parse(url))
                                }
                            },
                        )
                    }
                }
            }

            // Notifications section
            SettingsSection(
                title = "Notifications",
                note = "Which events are sent is set for the whole server in the dashboard’s Config, Notifications.",
            ) {
                CardPanel(spacing = 0.dp) {
                    val statusLine = when {
                        !push.firebaseAvailable -> "Firebase not configured in this build"
                        !push.permissionGranted && !push.isOn -> "Off · permission required"
                        push.isOn -> "On · sends to this device"
                        else -> "Off"
                    }
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text("Notifications on this device", style = Typeface.row, color = colors.fg)
                            Text(statusLine, style = Typeface.small, color = colors.fg3)
                        }
                        BreakerSwitch(
                            checked = push.isOn,
                            enabled = !push.busy,
                            contentDesc = "Notifications on this device",
                            onCheckedChange = { on ->
                                if (on) {
                                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && !push.permissionGranted) {
                                        notifPermissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                                    } else {
                                        scope.launch { pushManager.turnOn(store.currentApi()) }
                                    }
                                } else {
                                    scope.launch { pushManager.turnOff(store.currentApi()) }
                                }
                            },
                        )
                    }

                    if (push.isOn && push.via != null) {
                        Hairline()
                        KeyValueRow(
                            key = "Delivery",
                            value = if (push.via == "relay") "Through the Fusebox push relay (encrypted)" else "Direct",
                        )
                    }
                    push.message?.let { m ->
                        Text(
                            text = m,
                            style = Typeface.meta,
                            color = if (push.messageIsError) colors.warn else colors.fg2,
                            modifier = Modifier.padding(vertical = 8.dp),
                        )
                    }
                    if (push.path == PushPath.Unavailable && !push.isOn) {
                        Text(PushManager.UNAVAILABLE_TEXT, style = Typeface.meta, color = colors.fg3, modifier = Modifier.padding(vertical = 8.dp))
                    }
                    if (push.isOn) {
                        Spacer(Modifier.height(6.dp))
                        FuseButton(
                            text = "Send test",
                            fill = true,
                            enabled = !push.busy,
                            onClick = { scope.launch { pushManager.sendTest(store.currentApi()) } },
                        )
                    }
                    val others = push.devices.filter { it.id != push.subscriptionId }
                    if (others.isNotEmpty()) {
                        Spacer(Modifier.height(6.dp))
                        Hairline()
                        CapsLabel("Other devices", size = 10.5.sp, modifier = Modifier.padding(top = 12.dp, bottom = 4.dp))
                        others.forEachIndexed { idx, d ->
                            if (idx > 0) Hairline(color = colors.lineSoft)
                            Row(
                                modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(12.dp),
                            ) {
                                val icon = when (d.resolvedKind) {
                                    "apns" -> Icons.Outlined.PhoneIphone
                                    "fcm" -> Icons.Outlined.PhoneAndroid
                                    else -> Icons.Outlined.Language
                                }
                                Icon(icon, contentDescription = null, tint = colors.fg3, modifier = Modifier.size(18.dp))
                                Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                    Text(d.label ?: "Device", style = Typeface.row, color = colors.fg, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    val kindText = when (d.resolvedKind) {
                                        "apns" -> if (d.via == "relay") "iPhone app · relay" else "iPhone app"
                                        "fcm" -> if (d.via == "relay") "Android app · relay" else "Android app"
                                        else -> "Web push"
                                    }
                                    val since = d.createdAt?.let { " · since ${Fmt.shortDate(it)}" }.orEmpty()
                                    Text("$kindText$since", style = Typeface.small, color = colors.fg3)
                                }
                                FuseButton(
                                    text = "Remove",
                                    kind = FuseButtonKind.DESTRUCTIVE,
                                    compact = true,
                                    onClick = { removingDevice = d },
                                )
                            }
                        }
                    }
                    push.statusError?.let { e ->
                        Text(e, style = Typeface.meta, color = colors.fg3, modifier = Modifier.padding(vertical = 8.dp))
                    }
                }
            }

            // Display section
            SettingsSection(
                title = "Display",
                note = "When every account of a provider is used up, a live countdown notification tracks the first one back until the provider recovers.",
            ) {
                CardPanel(spacing = 0.dp) {
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Text("Meters show", style = Typeface.row, color = colors.fg, modifier = Modifier.weight(1f))
                        FuseSegmented(
                            options = listOf(QuotaDisplay.USED to "Used", QuotaDisplay.REMAINING to "Remaining"),
                            selected = state.quotaDisplay,
                            onSelect = { store.setQuotaDisplay(it) },
                        )
                    }
                    Hairline()
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Text("Live countdown when a provider runs out", style = Typeface.row, color = colors.fg, modifier = Modifier.weight(1f))
                        BreakerSwitch(
                            checked = state.outageAlertsEnabled,
                            contentDesc = "Live countdown when a provider runs out",
                            onCheckedChange = { store.setOutageAlertsEnabled(it) },
                        )
                    }
                }
            }

            // Disconnect
            SettingsSection(
                title = null,
                note = "Removes the management key from this device’s Android Keystore.",
            ) {
                FuseButton(
                    text = "Disconnect from this server",
                    kind = FuseButtonKind.DESTRUCTIVE,
                    fill = true,
                    onClick = { confirmSignOut = true },
                )
            }

            Text(
                text = "Fusebox for Android ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})",
                style = Typeface.data(12.sp),
                color = colors.fg3,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }

    if (confirmSignOut) {
        AlertDialog(
            onDismissRequest = { confirmSignOut = false },
            containerColor = colors.surfacePop,
            titleContentColor = colors.fg,
            textContentColor = colors.fg2,
            title = { Text("Disconnect from this server?", style = Typeface.sans(17.sp, FontWeight.SemiBold)) },
            text = { Text("Notifications for this device are turned off too.", style = Typeface.row) },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmSignOut = false
                        scope.launch {
                            pushManager.turnOff(store.currentApi())
                            store.signOut()
                        }
                    },
                ) {
                    Text("Disconnect", style = Typeface.rowMedium, color = colors.err)
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmSignOut = false }) {
                    Text("Cancel", style = Typeface.row, color = colors.fg2)
                }
            },
        )
    }

    removingDevice?.let { d ->
        AlertDialog(
            onDismissRequest = { removingDevice = null },
            containerColor = colors.surfacePop,
            titleContentColor = colors.fg,
            textContentColor = colors.fg2,
            title = { Text("Remove ${d.label ?: "this device"}?", style = Typeface.sans(17.sp, FontWeight.SemiBold)) },
            text = { Text("It stops getting Fusebox notifications.", style = Typeface.row) },
            confirmButton = {
                TextButton(
                    onClick = {
                        val target = d
                        removingDevice = null
                        scope.launch { pushManager.remove(target, store.currentApi()) }
                    },
                ) {
                    Text("Remove", style = Typeface.rowMedium, color = colors.err)
                }
            },
            dismissButton = {
                TextButton(onClick = { removingDevice = null }) {
                    Text("Cancel", style = Typeface.row, color = colors.fg2)
                }
            },
        )
    }
}

@Composable
private fun SettingsSection(
    title: String?,
    note: String?,
    content: @Composable () -> Unit,
) {
    val colors = LocalFuseboxColors.current
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (title != null) CapsLabel(title, size = 12.sp, color = colors.fg2, isHeader = true)
        content()
        if (note != null) Text(note, style = Typeface.small, color = colors.fg3)
    }
}
