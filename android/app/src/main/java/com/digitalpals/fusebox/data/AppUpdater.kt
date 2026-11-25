package com.digitalpals.fusebox.data

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import com.digitalpals.fusebox.BuildConfig
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.concurrent.TimeUnit

/** Semantic version parser for `1.0.2`, `v1.0.2`, or `android-v1.0.2`. */
data class AppVersion(
    val major: Int,
    val minor: Int,
    val patch: Int,
    val preRelease: String? = null,
) : Comparable<AppVersion> {
    override fun compareTo(other: AppVersion): Int {
        if (major != other.major) return major.compareTo(other.major)
        if (minor != other.minor) return minor.compareTo(other.minor)
        if (patch != other.patch) return patch.compareTo(other.patch)
        return when {
            preRelease == null && other.preRelease == null -> 0
            preRelease == null -> 1
            other.preRelease == null -> -1
            else -> preRelease.compareTo(other.preRelease)
        }
    }

    override fun toString(): String =
        if (preRelease == null) "$major.$minor.$patch" else "$major.$minor.$patch-$preRelease"

    companion object {
        private val REGEX = Regex("""^(?:android-)?v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$""")

        fun parse(raw: String): AppVersion? {
            val m = REGEX.matchEntire(raw.trim()) ?: return null
            val maj = m.groupValues[1].toIntOrNull() ?: return null
            val min = m.groupValues[2].toIntOrNull() ?: return null
            val pat = m.groupValues[3].toIntOrNull() ?: return null
            val pre = m.groupValues[4].takeIf { it.isNotEmpty() }
            return AppVersion(maj, min, pat, pre)
        }
    }
}

data class AppUpdateInfo(
    val version: String,
    val tagName: String,
    val downloadUrl: String,
    val sizeBytes: Long,
    val expectedSha256: String? = null,
    val releaseNotes: String? = null,
    val htmlUrl: String? = null,
)

data class UpdateUiState(
    val supported: Boolean = true,
    val checking: Boolean = false,
    val available: AppUpdateInfo? = null,
    val dismissedVersion: String? = null,
    val downloading: Boolean = false,
    val downloadProgress: Float = 0f,
    val downloadedVersion: String? = null,
    val needsInstallPermission: Boolean = false,
    val statusMessage: String? = null,
    val error: String? = null,
) {
    /** Banner appears above screens whenever an undismissed update (or active download) exists. */
    val showBanner: Boolean
        get() = supported && available != null &&
            (downloading || needsInstallPermission || available.version != dismissedVersion)
}

@Serializable
internal data class GitHubReleaseDto(
    @SerialName("tag_name") val tagName: String = "",
    val name: String? = null,
    val draft: Boolean = false,
    val prerelease: Boolean = false,
    val body: String? = null,
    @SerialName("html_url") val htmlUrl: String? = null,
    val assets: List<AssetDto> = emptyList(),
) {
    @Serializable
    data class AssetDto(
        val name: String = "",
        val size: Long = 0L,
        @SerialName("browser_download_url") val browserDownloadUrl: String = "",
    )
}

object ReleasePicker {
    private val SHA_REGEX = Regex("""SHA-?256[^0-9a-fA-F]{0,12}([0-9a-fA-F]{64})""", RegexOption.IGNORE_CASE)

    fun extractSha256(body: String?): String? =
        body?.let { SHA_REGEX.find(it)?.groupValues?.getOrNull(1)?.lowercase() }

    /**
     * Parses a GitHub Releases JSON array and returns the highest `android-vX.Y.Z` release
     * strictly newer than [currentVersion] that attaches an `.apk` asset.
     */
    fun pickUpdate(releasesJson: String, currentVersion: String): AppUpdateInfo? {
        val current = AppVersion.parse(currentVersion) ?: return null
        val releases = FuseboxJson.decodeFromString(
            kotlinx.serialization.builtins.ListSerializer(GitHubReleaseDto.serializer()),
            releasesJson,
        )
        var bestVersion: AppVersion? = null
        var bestInfo: AppUpdateInfo? = null

        for (rel in releases) {
            if (rel.draft) continue
            if (!rel.tagName.startsWith("android-v")) continue
            val parsed = AppVersion.parse(rel.tagName) ?: continue
            if (parsed <= current) continue

            val apk = rel.assets.firstOrNull {
                it.name.endsWith("-sideload.apk", ignoreCase = true) && it.browserDownloadUrl.isNotBlank()
            } ?: rel.assets.firstOrNull {
                it.name.endsWith(".apk", ignoreCase = true) && it.browserDownloadUrl.isNotBlank()
            } ?: continue

            if (bestVersion == null || parsed > bestVersion) {
                bestVersion = parsed
                bestInfo = AppUpdateInfo(
                    version = parsed.toString(),
                    tagName = rel.tagName,
                    downloadUrl = apk.browserDownloadUrl,
                    sizeBytes = apk.size,
                    expectedSha256 = extractSha256(rel.body),
                    releaseNotes = rel.body?.trim()?.takeIf { it.isNotEmpty() },
                    htmlUrl = rel.htmlUrl,
                )
            }
        }
        return bestInfo
    }
}

/**
 * Checks GitHub Releases (`android-v*`) for newer sideload APKs, downloads them with SHA-256
 * verification, and hands them to the Android package installer via FileProvider.
 * Automatically disabled on Google Play installs and Play Store builds.
 */
class AppUpdater(
    context: Context,
    private val storage: SecureStorage,
    private val releasesUrl: String = DEFAULT_RELEASES_URL,
    private val currentVersion: String = BuildConfig.VERSION_NAME,
    selfUpdateEnabled: Boolean = BuildConfig.SELF_UPDATE_ENABLED,
) {
    private val appContext = context.applicationContext
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val http = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .followRedirects(true)
        .build()

    private val supported: Boolean = selfUpdateEnabled && !isPlayStoreInstall(appContext)
    private val _state = MutableStateFlow(
        UpdateUiState(
            supported = supported,
            dismissedVersion = storage.dismissedUpdateVersion,
        ),
    )
    val state: StateFlow<UpdateUiState> = _state.asStateFlow()

    private var downloadJob: Job? = null
    private var downloadedApkFile: File? = null

    fun checkOnForeground() {
        if (!supported) return
        val now = System.currentTimeMillis()
        if (now - storage.lastUpdateCheckMs < CHECK_INTERVAL_MS) return
        scope.launch { checkForUpdates(manual = false) }
    }

    suspend fun checkForUpdates(manual: Boolean = true): AppUpdateInfo? {
        if (!supported) return null
        _state.update {
            it.copy(
                checking = true,
                error = if (manual) null else it.error,
                statusMessage = if (manual) null else it.statusMessage,
            )
        }
        return try {
            val body = withContext(Dispatchers.IO) { fetchReleasesJson() }
            storage.lastUpdateCheckMs = System.currentTimeMillis()
            val update = ReleasePicker.pickUpdate(body, currentVersion)
            _state.update {
                it.copy(
                    checking = false,
                    available = update,
                    statusMessage = when {
                        update != null -> null
                        manual -> "Up to date"
                        else -> it.statusMessage
                    },
                    error = null,
                )
            }
            update
        } catch (ce: CancellationException) {
            throw ce
        } catch (e: Exception) {
            _state.update {
                it.copy(
                    checking = false,
                    error = if (manual) (e.message ?: "Could not check for updates") else it.error,
                )
            }
            null
        }
    }

    fun dismissBanner() {
        val ver = _state.value.available?.version ?: return
        storage.dismissedUpdateVersion = ver
        _state.update { it.copy(dismissedVersion = ver, needsInstallPermission = false) }
    }

    fun downloadAndInstall(activity: Activity) {
        val info = _state.value.available ?: return
        if (_state.value.downloading) return

        // If already downloaded and verified for this version, go straight to installer.
        val cached = downloadedApkFile
        if (cached != null && cached.isFile && _state.value.downloadedVersion == info.version) {
            triggerInstall(activity, cached)
            return
        }

        downloadJob?.cancel()
        downloadJob = scope.launch {
            _state.update {
                it.copy(
                    downloading = true,
                    downloadProgress = 0f,
                    error = null,
                    statusMessage = null,
                    needsInstallPermission = false,
                )
            }
            try {
                val apkFile = withContext(Dispatchers.IO) { downloadApk(info) }
                downloadedApkFile = apkFile
                _state.update {
                    it.copy(
                        downloading = false,
                        downloadProgress = 1f,
                        downloadedVersion = info.version,
                    )
                }
                triggerInstall(activity, apkFile)
            } catch (ce: CancellationException) {
                _state.update { it.copy(downloading = false) }
                throw ce
            } catch (e: Exception) {
                _state.update {
                    it.copy(
                        downloading = false,
                        error = e.message ?: "Download failed",
                    )
                }
            }
        }
    }

    fun onResumeAfterPermission(activity: Activity) {
        if (!_state.value.needsInstallPermission) return
        val apk = downloadedApkFile ?: return
        if (!apk.isFile) return
        if (appContext.packageManager.canRequestPackageInstalls()) {
            _state.update { it.copy(needsInstallPermission = false) }
            launchInstallerIntent(activity, apk)
        }
    }

    internal suspend fun downloadApkForTest(info: AppUpdateInfo): File =
        withContext(Dispatchers.IO) { downloadApk(info) }

    fun setDemoAvailable(info: AppUpdateInfo?) {
        _state.update {
            it.copy(
                supported = true,
                available = info,
                dismissedVersion = null,
                error = null,
                statusMessage = null,
            )
        }
    }

    private fun fetchReleasesJson(): String {
        val req = Request.Builder()
            .url(releasesUrl)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "Fusebox-Android/$currentVersion")
            .get()
            .build()
        http.newCall(req).execute().use { resp ->
            if (!resp.isSuccessful) error("GitHub returned HTTP ${resp.code}")
            return resp.body?.string() ?: error("Empty response from GitHub")
        }
    }

    private fun downloadApk(info: AppUpdateInfo): File {
        val dir = File(appContext.cacheDir, "updates").apply { mkdirs() }
        // Clean older APKs first so cache stays tidy.
        dir.listFiles()?.forEach { f ->
            if (f.name != "fusebox-${info.version}.apk") f.delete()
        }
        val partFile = File(dir, "fusebox-${info.version}.apk.part")
        val finalFile = File(dir, "fusebox-${info.version}.apk")
        partFile.delete()

        val req = Request.Builder()
            .url(info.downloadUrl)
            .header("User-Agent", "Fusebox-Android/$currentVersion")
            .get()
            .build()

        http.newCall(req).execute().use { resp ->
            if (!resp.isSuccessful) error("Download failed (HTTP ${resp.code})")
            val body = resp.body ?: error("Empty APK download")
            val total = body.contentLength().takeIf { it > 0L } ?: info.sizeBytes
            val digest = MessageDigest.getInstance("SHA-256")
            var written = 0L
            val buf = ByteArray(16 * 1024)
            var lastReportedPct = -1

            body.byteStream().use { input ->
                FileOutputStream(partFile).use { output ->
                    while (true) {
                        val read = input.read(buf)
                        if (read <= 0) break
                        output.write(buf, 0, read)
                        digest.update(buf, 0, read)
                        written += read
                        if (total > 0L) {
                            val progress = (written.toFloat() / total.toFloat()).coerceIn(0f, 1f)
                            val pct = (progress * 100).toInt()
                            if (pct != lastReportedPct) {
                                lastReportedPct = pct
                                _state.update { it.copy(downloadProgress = progress) }
                            }
                        } else {
                            _state.update { it.copy(downloadProgress = -1f) }
                        }
                    }
                    output.flush()
                }
            }

            if (written == 0L) {
                partFile.delete()
                error("Downloaded APK is empty")
            }

            val expected = info.expectedSha256?.lowercase()
            if (expected != null) {
                val actual = digest.digest().joinToString("") { "%02x".format(it) }
                if (actual != expected) {
                    partFile.delete()
                    error("APK checksum mismatch — download rejected")
                }
            }
        }

        finalFile.delete()
        if (!partFile.renameTo(finalFile)) {
            partFile.copyTo(finalFile, overwrite = true)
            partFile.delete()
        }
        return finalFile
    }

    private fun triggerInstall(activity: Activity, apkFile: File) {
        if (!appContext.packageManager.canRequestPackageInstalls()) {
            _state.update {
                it.copy(
                    needsInstallPermission = true,
                    statusMessage = "Allow “Install unknown apps” for Fusebox, then return here.",
                )
            }
            val intent = Intent(
                Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                Uri.parse("package:${appContext.packageName}"),
            )
            runCatching { activity.startActivity(intent) }
            return
        }
        launchInstallerIntent(activity, apkFile)
    }

    private fun launchInstallerIntent(activity: Activity, apkFile: File) {
        val uri = FileProvider.getUriForFile(
            appContext,
            "${appContext.packageName}.fileprovider",
            apkFile,
        )
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, APK_MIME_TYPE)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        runCatching { activity.startActivity(intent) }.onFailure { err ->
            _state.update { it.copy(error = err.message ?: "Could not open installer") }
        }
    }

    companion object {
        const val DEFAULT_RELEASES_URL =
            "https://api.github.com/repos/DigitalPals/Fusebox/releases?per_page=20"
        private const val APK_MIME_TYPE = "application/vnd.android.package-archive"
        private const val PLAY_STORE_PACKAGE = "com.android.vending"
        private const val CHECK_INTERVAL_MS = 6L * 60L * 60L * 1000L // 6 hours

        fun isPlayStoreInstall(context: Context): Boolean = runCatching {
            val installer = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                context.packageManager.getInstallSourceInfo(context.packageName).installingPackageName
            } else {
                @Suppress("DEPRECATION")
                context.packageManager.getInstallerPackageName(context.packageName)
            }
            installer == PLAY_STORE_PACKAGE
        }.getOrDefault(false)
    }
}
