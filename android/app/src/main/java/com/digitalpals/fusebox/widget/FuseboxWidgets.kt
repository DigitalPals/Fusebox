package com.digitalpals.fusebox.widget

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.Image
import androidx.glance.ImageProvider
import androidx.glance.LocalContext
import androidx.glance.LocalSize
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.appwidget.updateAll
import androidx.glance.background
import androidx.glance.color.ColorProvider as DayNightColorProvider
import androidx.glance.unit.ColorProvider
import androidx.glance.layout.Alignment
import androidx.glance.layout.Box
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxHeight
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.height
import androidx.glance.layout.padding
import androidx.glance.layout.size
import androidx.glance.layout.width
import androidx.glance.text.FontFamily
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.digitalpals.fusebox.MainActivity
import com.digitalpals.fusebox.R
import com.digitalpals.fusebox.data.Account
import com.digitalpals.fusebox.data.AccountState
import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.FuseboxApi
import com.digitalpals.fusebox.data.MeterModel
import com.digitalpals.fusebox.data.QuotaDisplay
import com.digitalpals.fusebox.data.QuotaReading
import com.digitalpals.fusebox.data.Quotas
import com.digitalpals.fusebox.data.SecureStorage
import com.digitalpals.fusebox.data.WidgetSnapshot
import com.digitalpals.fusebox.data.metered
import com.digitalpals.fusebox.data.noShortLimit
import com.digitalpals.fusebox.data.planName
import com.digitalpals.fusebox.data.providerName
import com.digitalpals.fusebox.data.state
import com.digitalpals.fusebox.data.window
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import java.time.Instant
import java.util.concurrent.TimeUnit

private object WidgetColors {
    val bg = DayNightColorProvider(day = Color(0xFFF6F4EF), night = Color(0xFF0B0B0A))
    val surface = DayNightColorProvider(day = Color(0xFFFCFBF8), night = Color(0xFF121210))
    val line = DayNightColorProvider(day = Color(0xFFE3DFD6), night = Color(0xFF24231F))
    val fg = DayNightColorProvider(day = Color(0xFF1B1A17), night = Color(0xFFF2EFE8))
    val fg2 = DayNightColorProvider(day = Color(0xFF4A4741), night = Color(0xFFB4AFA4))
    val fg3 = DayNightColorProvider(day = Color(0xFF6B665C), night = Color(0xFF857F74))
    val meter = DayNightColorProvider(day = Color(0xFF3A3833), night = Color(0xFFD9D4C8))
    val ok = DayNightColorProvider(day = Color(0xFF15803D), night = Color(0xFF4ADE80))
    val warn = DayNightColorProvider(day = Color(0xFFA16207), night = Color(0xFFFBBF24))
    val err = DayNightColorProvider(day = Color(0xFFBE123C), night = Color(0xFFFB7185))
    val idle = DayNightColorProvider(day = Color(0xFFA39E92), night = Color(0xFF55524B))

    fun level(l: QuotaReading.Level?): ColorProvider = when (l) {
        QuotaReading.Level.WARN -> warn
        QuotaReading.Level.ERR -> err
        else -> meter
    }

    fun accountState(kind: AccountState.Kind): ColorProvider = when (kind) {
        AccountState.Kind.READY -> ok
        AccountState.Kind.COOLING -> warn
        AccountState.Kind.ERROR -> err
        AccountState.Kind.DISABLED -> idle
    }
}

private data class LoadedWidgetData(
    val connected: Boolean,
    val snapshot: WidgetSnapshot,
    val stale: Boolean,
    val mode: QuotaDisplay,
)

private suspend fun loadWidgetData(context: Context): LoadedWidgetData {
    val storage = SecureStorage(context)
    val url = storage.serverUrl
    val key = storage.managementKey
    val mode = storage.quotaDisplay
    val cached = storage.loadWidgetSnapshot() ?: WidgetSnapshot()
    if (url.isNullOrBlank() || key.isNullOrBlank()) {
        return LoadedWidgetData(connected = false, snapshot = cached, stale = false, mode = mode)
    }
    val fresh = runCatching {
        val api = FuseboxApi(url, key)
        coroutineScope {
            val ov = async { api.overview() }
            val ac = async { api.accounts() }
            val fa = async { api.faults() }
            WidgetSnapshot(ov.await(), ac.await(), fa.await(), System.currentTimeMillis())
        }
    }.getOrNull()
    return if (fresh != null) {
        storage.saveWidgetSnapshot(fresh)
        LoadedWidgetData(connected = true, snapshot = fresh, stale = false, mode = mode)
    } else {
        LoadedWidgetData(connected = true, snapshot = cached, stale = true, mode = mode)
    }
}

private fun deepLinkIntent(context: Context, uri: String): Intent =
    Intent(context, MainActivity::class.java).apply {
        action = Intent.ACTION_VIEW
        data = Uri.parse(uri)
        flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
    }

@Composable
private fun GlanceSegmentedBar(reading: QuotaReading?, mode: QuotaDisplay) {
    val model = MeterModel.build(reading, pace = null, mode = mode)
    val litColor = WidgetColors.level(model.level)
    Row(modifier = GlanceModifier.fillMaxWidth().height(7.dp)) {
        for (i in 0 until 10) {
            // 10 crisp blocks (each 10%) keeps Glance RemoteViews fast and well under view limits.
            val lit = (i * 2) < model.lit
            Box(
                modifier = GlanceModifier
                    .defaultWeight()
                    .fillMaxHeight()
                    .background(if (lit) litColor else WidgetColors.line)
                    .cornerRadius(1.dp),
            ) {}
            if (i < 9) Spacer(GlanceModifier.width(2.dp))
        }
    }
}

@Composable
private fun WidgetNotConnected(context: Context) {
    Column(
        modifier = GlanceModifier
            .fillMaxSize()
            .background(WidgetColors.bg)
            .padding(14.dp)
            .clickable(actionStartActivity(deepLinkIntent(context, "fusebox://overview"))),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Image(
            provider = ImageProvider(R.drawable.ic_stat_fusebox),
            contentDescription = "Fusebox",
            modifier = GlanceModifier.size(22.dp),
        )
        Spacer(GlanceModifier.height(6.dp))
        Text(
            text = "Open Fusebox to connect",
            style = TextStyle(color = WidgetColors.fg2, fontSize = 12.sp),
        )
    }
}

// MARK: - Single Subscription Widget

class SubscriptionWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Responsive(setOf(DpSize(140.dp, 110.dp), DpSize(240.dp, 120.dp)))

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val manager = GlanceAppWidgetManager(context)
        val appWidgetId = runCatching { manager.getAppWidgetId(id) }.getOrDefault(-1)
        val storage = SecureStorage(context)
        val selectedId = if (appWidgetId >= 0) storage.widgetAccountId(appWidgetId) else null
        val data = loadWidgetData(context)
        val account = selectedId?.let { sid -> data.snapshot.accounts.firstOrNull { it.id == sid } }
            ?: data.snapshot.accounts.firstOrNull { it.metered }
        provideContent { SubscriptionWidgetContent(data, account) }
    }
}

@Composable
private fun SubscriptionWidgetContent(data: LoadedWidgetData, account: Account?) {
    val context = LocalContext.current
    val size = LocalSize.current
    val compact = size.width < 200.dp
    if (!data.connected) {
        WidgetNotConnected(context)
        return
    }
    if (account == null) {
        Column(
            modifier = GlanceModifier
                .fillMaxSize()
                .background(WidgetColors.bg)
                .padding(12.dp)
                .clickable(actionStartActivity(deepLinkIntent(context, "fusebox://accounts"))),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("Choose a subscription", style = TextStyle(color = WidgetColors.fg3, fontSize = 12.sp))
        }
        return
    }
    val now = Instant.now()
    val st = account.state(now)
    val updated = Instant.ofEpochMilli(data.snapshot.savedAtEpochMs)

    Column(
        modifier = GlanceModifier
            .fillMaxSize()
            .background(WidgetColors.bg)
            .padding(12.dp)
            .clickable(actionStartActivity(deepLinkIntent(context, DeepLink.accountUri(account.id)))),
    ) {
        Row(modifier = GlanceModifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Column(modifier = GlanceModifier.defaultWeight()) {
                Text(
                    text = account.label,
                    style = TextStyle(color = WidgetColors.fg, fontSize = 12.sp, fontWeight = FontWeight.Medium),
                    maxLines = 1,
                )
                Text(
                    text = account.planName?.let { "${account.providerName} $it" } ?: account.providerName,
                    style = TextStyle(color = WidgetColors.fg3, fontSize = 10.sp),
                    maxLines = 1,
                )
            }
            Spacer(GlanceModifier.width(6.dp))
            Box(
                modifier = GlanceModifier
                    .size(8.dp)
                    .cornerRadius(4.dp)
                    .background(WidgetColors.accountState(st.kind)),
            ) {}
        }
        Spacer(GlanceModifier.height(6.dp))
        for (short in listOf(true, false)) {
            if (short && account.noShortLimit(now)) continue
            val w = account.window(short = short, now = now)
            val r = QuotaReading.of(w, now)
            Row(modifier = GlanceModifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = Quotas.label(w, short),
                    style = TextStyle(color = WidgetColors.fg3, fontSize = 10.sp, fontWeight = FontWeight.Bold),
                    modifier = GlanceModifier.defaultWeight(),
                )
                Text(
                    text = r?.let { Fmt.percent(it.value(data.mode)) } ?: "–",
                    style = TextStyle(
                        color = if (r == null || r.level == QuotaReading.Level.NORMAL) WidgetColors.fg else WidgetColors.level(r.level),
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                    ),
                )
                if (!compact && w?.resetsAt != null) {
                    Spacer(GlanceModifier.width(6.dp))
                    Text(
                        text = "↺ ${Fmt.compactLeft(w.resetsAt, now)}",
                        style = TextStyle(color = WidgetColors.fg3, fontSize = 10.sp, fontFamily = FontFamily.Monospace),
                    )
                }
            }
            Spacer(GlanceModifier.height(3.dp))
            GlanceSegmentedBar(r, data.mode)
            Spacer(GlanceModifier.height(5.dp))
        }
        Spacer(GlanceModifier.defaultWeight())
        val footer = if (st.kind == AccountState.Kind.COOLING && st.until != null) {
            "${st.word} · back in ${Fmt.compactLeft(st.until, now)}"
        } else {
            "${if (data.stale) "Last seen" else "Updated"} ${Fmt.hm(updated)}"
        }
        Text(
            text = footer,
            style = TextStyle(
                color = if (st.kind == AccountState.Kind.COOLING) WidgetColors.warn else WidgetColors.fg3,
                fontSize = 9.sp,
                fontFamily = FontFamily.Monospace,
            ),
            maxLines = 1,
        )
    }
}

class SubscriptionWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget: GlanceAppWidget = SubscriptionWidget()
}

// MARK: - Periodic refresh & updater

class WidgetRefreshWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        runCatching { SubscriptionWidget().updateAll(applicationContext) }
        return Result.success()
    }
}

object FuseboxWidgetUpdater {
    private val scope = CoroutineScope(Dispatchers.IO)

    fun updateAll(context: Context) {
        val appContext = context.applicationContext
        scope.launch {
                runCatching { SubscriptionWidget().updateAll(appContext) }
        }
    }

    fun schedulePeriodic(context: Context) {
        val constraints = Constraints.Builder()
            .setRequiredNetworkType(NetworkType.CONNECTED)
            .build()
        val request = PeriodicWorkRequestBuilder<WidgetRefreshWorker>(20, TimeUnit.MINUTES)
            .setConstraints(constraints)
            .build()
        WorkManager.getInstance(context.applicationContext).enqueueUniquePeriodicWork(
            "fusebox_widget_refresh",
            ExistingPeriodicWorkPolicy.KEEP,
            request,
        )
    }
}
