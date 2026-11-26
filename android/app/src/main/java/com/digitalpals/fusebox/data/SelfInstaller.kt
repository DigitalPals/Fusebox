package com.digitalpals.fusebox.data

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ProcessLifecycleOwner
import com.digitalpals.fusebox.BuildConfig
import com.digitalpals.fusebox.FuseboxApplication
import com.digitalpals.fusebox.MainActivity
import com.digitalpals.fusebox.R
import com.digitalpals.fusebox.push.FuseboxNotifications
import java.io.File

/**
 * Installs a downloaded Fusebox APK over this app through a [PackageInstaller] session.
 *
 * Android 12+ lets an app update itself without asking when it is the installer of record (on
 * Android 14+, the update owner) of the installed copy. The first update after a manual install
 * therefore still needs one confirmation; installing through a session makes Fusebox the
 * installer of record, so the updates after it go through without one.
 */
object SelfInstaller {
    private const val TAG = "FuseboxUpdate"
    const val CHANNEL_UPDATES = "fusebox_updates"
    private const val NOTIFY_READY = 30_001
    private const val NOTIFY_UPDATED = 30_002
    private const val NOTIFY_PERMISSION = 30_003
    internal const val EXTRA_VERSION = "com.digitalpals.fusebox.extra.UPDATE_VERSION"

    /**
     * Why [apk] must not be installed over this app, or null when it may: it must be a Fusebox
     * APK with a higher version code, signed with exactly the installed app's certificates.
     */
    fun problemWith(context: Context, apk: File): String? {
        val pm = context.packageManager
        val flags = PackageManager.GET_SIGNING_CERTIFICATES
        val archive: PackageInfo = runCatching { pm.getPackageArchiveInfo(apk.path, flags) }.getOrNull()
            ?: return "The download is not an Android app"
        if (archive.packageName != context.packageName) return "The download is a different app (${archive.packageName})"
        val installed = pm.getPackageInfo(context.packageName, flags)
        if (archive.longVersionCode <= installed.longVersionCode) {
            return "The download is not newer than the installed version"
        }
        val signers = { info: PackageInfo -> info.signingInfo?.apkContentsSigners?.map { it.toCharsString() }?.toSet().orEmpty() }
        val theirs = signers(archive)
        if (theirs.isEmpty() || theirs != signers(installed)) return "The download is signed with a different key"
        return null
    }

    /** Can this app install packages at all ("Install unknown apps" allowed for Fusebox)? */
    fun canInstall(context: Context): Boolean = context.packageManager.canRequestPackageInstalls()

    /**
     * Starts installing [apk] ([version]). The result arrives at [StatusReceiver]; when Android
     * installs without asking, this process is replaced and [UpdatedReceiver] runs in the new one.
     */
    fun install(context: Context, apk: File, version: String, storage: SecureStorage) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(apk.length())
            setInstallReason(PackageManager.INSTALL_REASON_USER)
            setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) setRequestUpdateOwnership(true)
        }
        val sessionId = installer.createSession(params)
        try {
            installer.openSession(sessionId).use { session ->
                apk.inputStream().use { input ->
                    session.openWrite("base.apk", 0, apk.length()).use { output ->
                        input.copyTo(output)
                        session.fsync(output)
                    }
                }
                storage.pendingSelfUpdateVersion = version
                val status = Intent(context, StatusReceiver::class.java)
                    .setPackage(context.packageName)
                    .putExtra(EXTRA_VERSION, version)
                val pending = PendingIntent.getBroadcast(
                    context,
                    sessionId,
                    status,
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
                )
                session.commit(pending.intentSender)
            }
        } catch (e: Exception) {
            runCatching { installer.abandonSession(sessionId) }
            throw e
        }
    }

    internal fun ensureChannel(context: Context) {
        val mgr = context.getSystemService(NotificationManager::class.java) ?: return
        mgr.createNotificationChannel(
            NotificationChannel(CHANNEL_UPDATES, "App updates", NotificationManager.IMPORTANCE_LOW).apply {
                description = "New versions of Fusebox: ready to install, or installed."
                setShowBadge(false)
            },
        )
    }

    private fun notify(context: Context, id: Int, title: String, text: String, tap: PendingIntent) {
        if (!FuseboxNotifications.hasPermission(context)) return
        ensureChannel(context)
        val notification = NotificationCompat.Builder(context, CHANNEL_UPDATES)
            .setSmallIcon(R.drawable.ic_stat_fusebox)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .build()
        try {
            NotificationManagerCompat.from(context).notify(id, notification)
        } catch (_: SecurityException) {
            // Notifications turned off between the check and here.
        }
    }

    private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        NOTIFY_UPDATED,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )

    /** "Install unknown apps" is off for Fusebox, so updates can't install: say so once per version. */
    internal fun notifyPermissionNeeded(context: Context, version: String) {
        val settings = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, android.net.Uri.parse("package:${context.packageName}"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val tap = PendingIntent.getActivity(context, NOTIFY_PERMISSION, settings, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        notify(context, NOTIFY_PERMISSION, "Fusebox $version can't install yet", "Allow Fusebox to install unknown apps so updates install by themselves.", tap)
    }

    private fun inForeground(): Boolean =
        ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)

    /** The session's result. */
    class StatusReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val version = intent.getStringExtra(EXTRA_VERSION).orEmpty()
            val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
            val updater = (context.applicationContext as? FuseboxApplication)?.updater
            when (status) {
                PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                    // Android wants one confirmation (the first update after a manual install).
                    @Suppress("DEPRECATION")
                    val confirm = (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                    } else {
                        intent.getParcelableExtra(Intent.EXTRA_INTENT)
                    }) ?: return
                    confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    if (inForeground() && runCatching { context.startActivity(confirm) }.isSuccess) return
                    val tap = PendingIntent.getActivity(context, NOTIFY_READY, confirm, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
                    notify(
                        context,
                        NOTIFY_READY,
                        "Fusebox $version is ready to install",
                        "Tap to install. After this one, updates install by themselves.",
                        tap,
                    )
                }
                PackageInstaller.STATUS_SUCCESS -> Unit // UpdatedReceiver says so from the new version.
                else -> {
                    val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
                    Log.w(TAG, "Installing $version failed: status $status, $message")
                    // Failed or declined: not tried again by itself for a day; the banner still offers it.
                    SecureStorage(context).apply {
                        pendingSelfUpdateVersion = null
                        if (version.isNotEmpty()) {
                            autoUpdateFailedVersion = version
                            autoUpdateFailedAtMs = System.currentTimeMillis()
                        }
                    }
                    updater?.onInstallFailed(
                        if (status == PackageInstaller.STATUS_FAILURE_ABORTED) null else "Could not install $version${message?.let { ": $it" }.orEmpty()}",
                    )
                }
            }
        }
    }

    /** Runs in the new version after any update of this app; says so after a self-update. */
    class UpdatedReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
            val storage = SecureStorage(context)
            val expected = storage.pendingSelfUpdateVersion ?: return
            storage.pendingSelfUpdateVersion = null
            NotificationManagerCompat.from(context).cancel(NOTIFY_READY)
            if (expected != BuildConfig.VERSION_NAME) return
            notify(context, NOTIFY_UPDATED, "Fusebox updated to ${BuildConfig.VERSION_NAME}", "Installed automatically from GitHub.", openApp(context))
        }
    }
}
