package com.digitalpals.fusebox

import android.app.Application
import androidx.test.core.app.ApplicationProvider
import com.digitalpals.fusebox.data.AppUpdater
import com.digitalpals.fusebox.data.AppVersion
import com.digitalpals.fusebox.data.AutoUpdateOutcome
import com.digitalpals.fusebox.data.ReleasePicker
import com.digitalpals.fusebox.data.SecureStorage
import kotlinx.coroutines.test.runTest
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import java.security.MessageDigest

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = Application::class)
class AppUpdaterTest {

    @Test
    fun versionParsingAndOrdering() {
        val v100 = AppVersion.parse("1.0.0")!!
        val v101 = AppVersion.parse("android-v1.0.1")!!
        val v102 = AppVersion.parse("v1.0.2")!!
        val v1010 = AppVersion.parse("1.0.10")!!
        val v110 = AppVersion.parse("1.1.0")!!
        val v110rc = AppVersion.parse("android-v1.1.0-rc.1")!!

        assertTrue(v100 < v101)
        assertTrue(v101 < v102)
        assertTrue(v102 < v1010)
        assertTrue(v1010 < v110rc)
        assertTrue(v110rc < v110)
        assertNull(AppVersion.parse("not-a-version"))
    }

    @Test
    fun releasePickerIgnoresServerTagsDraftsAndOlderReleases() {
        val sha = "aa525cd941b8d6c7ca39a6f24ea02e5f883dcfe74b7b0741e79cf6cae97a407f"
        val json = """
            [
              {
                "tag_name": "v0.18.0",
                "draft": false,
                "assets": [
                  { "name": "fusebox-linux.tar.gz", "size": 9000000, "browser_download_url": "https://example.test/linux.tar.gz" }
                ]
              },
              {
                "tag_name": "android-v9.9.9",
                "draft": true,
                "assets": [
                  { "name": "fusebox-9.9.9-sideload.apk", "size": 5000000, "browser_download_url": "https://example.test/draft.apk" }
                ]
              },
              {
                "tag_name": "android-v1.0.1",
                "draft": false,
                "prerelease": true,
                "body": "Old release",
                "assets": [
                  { "name": "fusebox-1.0.1-sideload.apk", "size": 5080324, "browser_download_url": "https://example.test/1.0.1.apk" }
                ]
              },
              {
                "tag_name": "android-v1.0.3",
                "draft": false,
                "prerelease": true,
                "body": "New update\n- **SHA-256:** `$sha`",
                "html_url": "https://github.com/DigitalPals/Fusebox/releases/tag/android-v1.0.3",
                "assets": [
                  { "name": "fusebox-1.0.3-debug.apk", "size": 70000000, "browser_download_url": "https://example.test/debug.apk" },
                  { "name": "fusebox-1.0.3-sideload.apk", "size": 5100000, "browser_download_url": "https://example.test/1.0.3-sideload.apk" }
                ]
              }
            ]
        """.trimIndent()

        val found = ReleasePicker.pickUpdate(json, "1.0.2")
        assertNotNull(found)
        assertEquals("1.0.3", found!!.version)
        assertEquals("android-v1.0.3", found.tagName)
        assertEquals("https://example.test/1.0.3-sideload.apk", found.downloadUrl)
        assertEquals(5100000L, found.sizeBytes)
        assertEquals(sha, found.expectedSha256)

        // Already on 1.0.3 -> no update
        assertNull(ReleasePicker.pickUpdate(json, "1.0.3"))
    }

    @Test
    fun updaterChecksDownloadsAndVerifiesSha256() = runTest {
        val apkBytes = "fake-apk-payload-for-checksum-verification".toByteArray()
        val goodSha = MessageDigest.getInstance("SHA-256")
            .digest(apkBytes)
            .joinToString("") { "%02x".format(it) }
        val badSha = "0".repeat(64)

        val server = MockWebServer()
        server.start()
        try {
            val apkUrl = server.url("/downloads/fusebox-1.0.5-sideload.apk").toString()
            val releasesJson = """
                [
                  {
                    "tag_name": "android-v1.0.5",
                    "draft": false,
                    "prerelease": true,
                    "body": "- **SHA-256:** `$goodSha`",
                    "assets": [
                      { "name": "fusebox-1.0.5-sideload.apk", "size": ${apkBytes.size}, "browser_download_url": "$apkUrl" }
                    ]
                  }
                ]
            """.trimIndent()

            server.enqueue(MockResponse(code = 200, body = releasesJson))
            server.enqueue(MockResponse(code = 200, body = String(apkBytes)))

            val context = ApplicationProvider.getApplicationContext<Application>()
            val storage = SecureStorage(context)
            val updater = AppUpdater(
                context = context,
                storage = storage,
                releasesUrl = server.url("/releases").toString(),
                currentVersion = "1.0.2",
                selfUpdateEnabled = true,
            )

            val info = updater.checkForUpdates(manual = true)
            assertNotNull(info)
            assertEquals("1.0.5", info!!.version)
            assertEquals(goodSha, info.expectedSha256)

            val downloaded = updater.downloadApkForTest(info)
            assertTrue(downloaded.isFile)
            assertEquals(apkBytes.size.toLong(), downloaded.length())

            // Corrupt checksum must be rejected and file deleted
            server.enqueue(MockResponse(code = 200, body = String(apkBytes)))
            try {
                updater.downloadApkForTest(info.copy(version = "1.0.6", expectedSha256 = badSha))
                fail("Expected checksum mismatch failure")
            } catch (e: IllegalStateException) {
                assertTrue(e.message.orEmpty().contains("checksum mismatch", ignoreCase = true))
            }
        } finally {
            server.close()
        }
    }

    /** A release list with one newer APK, served by [server]; [sha] null leaves the checksum out. */
    private fun release(server: MockWebServer, version: String, apk: ByteArray, sha: String?): String {
        val url = server.url("/downloads/fusebox-$version-sideload.apk").toString()
        val body = if (sha == null) "No checksum here." else "- **SHA-256:** `$sha`"
        return """
            [{ "tag_name": "android-v$version", "draft": false, "prerelease": true, "body": "$body",
               "assets": [{ "name": "fusebox-$version-sideload.apk", "size": ${apk.size}, "browser_download_url": "$url" }] }]
        """.trimIndent()
    }

    private fun sha256(bytes: ByteArray) =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    private fun updaterFor(server: MockWebServer, storage: SecureStorage) = AppUpdater(
        context = ApplicationProvider.getApplicationContext<Application>(),
        storage = storage,
        releasesUrl = server.url("/releases").toString(),
        currentVersion = "1.0.3",
        selfUpdateEnabled = true,
    )

    @Test
    fun automaticUpdatesNeverInstallWhatTheyCannotCheck() = runTest {
        val context = ApplicationProvider.getApplicationContext<Application>()
        shadowOf(context.packageManager).setCanRequestPackageInstalls(true)
        val apk = "not-really-an-apk".toByteArray()
        val server = MockWebServer()
        server.start()
        try {
            val storage = SecureStorage(context).apply { autoUpdate = true; autoUpdateFailedVersion = null }
            val updater = updaterFor(server, storage)

            // A release without a SHA-256 line is only offered, never installed by itself.
            server.enqueue(MockResponse(code = 200, body = release(server, "1.0.4", apk, sha = null)))
            assertEquals(AutoUpdateOutcome.NO_CHECKSUM, updater.autoUpdate())
            assertEquals("1.0.4", updater.state.value.available?.version)
            assertEquals(1, server.requestCount)

            // Automatic updates off: the banner still offers it, nothing is downloaded.
            storage.autoUpdate = false
            server.enqueue(MockResponse(code = 200, body = release(server, "1.0.4", apk, sha256(apk))))
            assertEquals(AutoUpdateOutcome.OFF, updater.autoUpdate())
            assertEquals(2, server.requestCount)

            // A download that isn't a Fusebox APK signed like this one is refused and kept off
            // for a day, so a bad release isn't fetched every 6 hours.
            storage.autoUpdate = true
            server.enqueue(MockResponse(code = 200, body = release(server, "1.0.4", apk, sha256(apk))))
            server.enqueue(MockResponse(code = 200, body = String(apk)))
            assertEquals(AutoUpdateOutcome.FAILED, updater.autoUpdate())
            assertEquals("The download is not an Android app", updater.state.value.error)
            assertEquals("1.0.4", storage.autoUpdateFailedVersion)
            assertTrue(!updater.state.value.installing && !updater.state.value.downloading)
            server.enqueue(MockResponse(code = 200, body = release(server, "1.0.4", apk, sha256(apk))))
            assertEquals(AutoUpdateOutcome.FAILED_RECENTLY, updater.autoUpdate())
            assertEquals("no second download", 5, server.requestCount)
        } finally {
            server.close()
        }
    }

    @Test
    fun withoutInstallPermissionItAsksOncePerVersion() = runTest {
        val context = ApplicationProvider.getApplicationContext<Application>()
        shadowOf(context.packageManager).setCanRequestPackageInstalls(false)
        val apk = "payload".toByteArray()
        val server = MockWebServer()
        server.start()
        try {
            val storage = SecureStorage(context).apply { autoUpdate = true; permissionPromptedVersion = null }
            val updater = updaterFor(server, storage)
            server.enqueue(MockResponse(code = 200, body = release(server, "1.0.4", apk, sha256(apk))))
            assertEquals(AutoUpdateOutcome.NEEDS_PERMISSION, updater.autoUpdate())
            assertEquals("1.0.4", storage.permissionPromptedVersion)
            assertEquals("nothing downloaded", 1, server.requestCount)
        } finally {
            server.close()
        }
    }
}
