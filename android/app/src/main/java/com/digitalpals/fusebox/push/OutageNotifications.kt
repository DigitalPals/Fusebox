package com.digitalpals.fusebox.push

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.digitalpals.fusebox.MainActivity
import com.digitalpals.fusebox.R
import com.digitalpals.fusebox.data.Fault
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.ProviderInfo
import java.time.Instant

/**
 * Android notification channels, fault push display, and Android Live-Update / ongoing countdown
 * notifications when every account of a provider is used up (Android counterpart of iOS Live Activities).
 */
object FuseboxNotifications {
    const val CHANNEL_FAULTS = "fusebox_faults"
    const val CHANNEL_OUTAGES = "fusebox_outages"
    private const val OUTAGE_ID_BASE = 20_000
    private const val PUSH_ID_BASE = 10_000

    private val activeOutages = linkedMapOf<String, String>() // provider -> providerName

    fun ensureChannels(context: Context) {
        val mgr = context.getSystemService(NotificationManager::class.java) ?: return
        val faults = NotificationChannel(
            CHANNEL_FAULTS,
            "Faults & alerts",
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = "Sign-in expirations, exhausted providers, banked resets and renewal reminders."
            setShowBadge(true)
        }
        val outages = NotificationChannel(
            CHANNEL_OUTAGES,
            "Provider outages",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Live countdown while every account of a provider is used up."
            setShowBadge(false)
        }
        mgr.createNotificationChannels(listOf(faults, outages))
    }

    fun hasPermission(context: Context): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
                PackageManager.PERMISSION_GRANTED
        } else {
            NotificationManagerCompat.from(context).areNotificationsEnabled()
        }

    fun showPushNotification(
        context: Context,
        title: String,
        body: String,
        path: String?,
        thread: String?,
        level: String?,
        badgeCount: Int?,
    ) {
        if (!hasPermission(context)) return
        ensureChannels(context)
        val groupKey = "fusebox.group.${thread ?: "fusebox"}"
        val intent = Intent(context, MainActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            putExtra(MainActivity.EXTRA_NOTIFICATION_PATH, path)
        }
        val id = PUSH_ID_BASE + ((title + body + (path ?: "")).hashCode() and 0x1FFF)
        val pending = PendingIntent.getActivity(
            context,
            id,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val builder = NotificationCompat.Builder(context, CHANNEL_FAULTS)
            .setSmallIcon(R.drawable.ic_stat_fusebox)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setGroup(groupKey)
            .setAutoCancel(true)
            .setContentIntent(pending)
            .setPriority(if (level == "err") NotificationCompat.PRIORITY_HIGH else NotificationCompat.PRIORITY_DEFAULT)
        if (badgeCount != null && badgeCount >= 0) {
            builder.setNumber(badgeCount)
        }
        runCatching { NotificationManagerCompat.from(context).notify(id, builder.build()) }
    }

    /**
     * Syncs ongoing outage countdown notifications (and Android 16+ promoted Live Updates when
     * permitted) for every current `kind == "provider"` fault.
     */
    fun syncOutages(context: Context, faults: List<Fault>, enabled: Boolean) {
        if (!enabled || !hasPermission(context)) {
            endAllOutages(context)
            return
        }
        ensureChannels(context)
        val mgr = context.getSystemService(NotificationManager::class.java) ?: return
        val current = faults
            .filter { it.kind == "provider" && !it.provider.isNullOrEmpty() }
            .associateBy { it.provider!! }

        // Recoveries: providers that were out and are now back
        val recovered = activeOutages.keys.filter { it !in current.keys }.toList()
        for (provider in recovered) {
            val name = activeOutages.remove(provider) ?: ProviderInfo.name(provider)
            val id = outageNotificationId(provider)
            val intent = Intent(context, MainActivity::class.java).apply {
                action = Intent.ACTION_VIEW
                data = Uri.parse("fusebox://accounts")
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            }
            val pending = PendingIntent.getActivity(
                context,
                id,
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val note = NotificationCompat.Builder(context, CHANNEL_OUTAGES)
                .setSmallIcon(R.drawable.ic_stat_fusebox)
                .setContentTitle("$name is back")
                .setContentText("An account can take requests again.")
                .setOngoing(false)
                .setAutoCancel(true)
                .setTimeoutAfter(15 * 60 * 1000L)
                .setContentIntent(pending)
                .build()
            runCatching { mgr.notify(id, note) }
        }

        // Active outages
        for ((provider, fault) in current) {
            val name = fault.providerName ?: ProviderInfo.name(provider)
            activeOutages[provider] = name
            val id = outageNotificationId(provider)
            val intent = Intent(context, MainActivity::class.java).apply {
                action = Intent.ACTION_VIEW
                data = Uri.parse("fusebox://accounts")
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            }
            val pending = PendingIntent.getActivity(
                context,
                id,
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val now = Instant.now()
            val until = fault.until?.takeIf { it.isAfter(now) }
            val detail = when {
                until != null -> "Back ${Fmt.whenText(until, now)} · ${fault.detail ?: "All accounts used up"}"
                else -> fault.detail ?: "Sign in again to restore capacity."
            }
            val builder = Notification.Builder(context, CHANNEL_OUTAGES)
                .setSmallIcon(R.drawable.ic_stat_fusebox)
                .setContentTitle(fault.title)
                .setContentText(detail)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setContentIntent(pending)
            if (until != null) {
                builder.setWhen(until.toEpochMilli())
                    .setShowWhen(true)
                    .setUsesChronometer(true)
                    .setChronometerCountDown(true)
            }
            // Android 16 (API 36) Live Updates / Promoted Ongoing notification support
            if (Build.VERSION.SDK_INT >= 36) {
                runCatching {
                    val m = Notification.Builder::class.java.getMethod(
                        "setRequestPromotedOngoing",
                        Boolean::class.javaPrimitiveType,
                    )
                    m.invoke(builder, true)
                }
            }
            runCatching { mgr.notify(id, builder.build()) }
        }
    }

    fun endAllOutages(context: Context) {
        val mgr = context.getSystemService(NotificationManager::class.java) ?: return
        for (provider in activeOutages.keys.toList()) {
            mgr.cancel(outageNotificationId(provider))
        }
        activeOutages.clear()
    }

    private fun outageNotificationId(provider: String): Int =
        OUTAGE_ID_BASE + (provider.hashCode() and 0x0FFF)
}
