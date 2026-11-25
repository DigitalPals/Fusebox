package com.digitalpals.fusebox

import android.app.Application
import android.graphics.Bitmap
import android.graphics.Canvas
import androidx.activity.ComponentActivity
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import androidx.test.core.app.ApplicationProvider
import com.digitalpals.fusebox.data.AppStore
import com.digitalpals.fusebox.data.AppUpdateInfo
import com.digitalpals.fusebox.data.AppUpdater
import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.DemoFixtures
import com.digitalpals.fusebox.data.SecureStorage
import com.digitalpals.fusebox.push.PushManager
import com.digitalpals.fusebox.ui.MainScreen
import com.digitalpals.fusebox.ui.adaptive.FoldableState
import com.digitalpals.fusebox.ui.adaptive.HingeBoundsDp
import com.digitalpals.fusebox.ui.adaptive.LocalFoldableState
import com.digitalpals.fusebox.ui.theme.FuseboxTheme
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File
import java.io.FileOutputStream

/**
 * Adaptive foldable UI tests rendered with Robolectric native Skia graphics.
 * Verifies both cover-screen (folded, < 600dp) and inner-screen (unfolded Pixel 9 Pro Fold,
 * 840x840dp with active vertical hinge) layouts, dark and light themes, and interactive state.
 *
 * Pass `-Dfusebox.screenshots.dir=/abs/path` to save PNG screenshots of every rendered screen.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], application = Application::class)
class FoldableUiTest {

    @get:Rule
    val composeRule = createAndroidComposeRule<ComponentActivity>()

    private fun newDemoHarness(connected: Boolean = true): Triple<AppStore, PushManager, DemoFixtures.Bundle> {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val storage = SecureStorage(context)
        val store = AppStore(context, storage)
        val push = PushManager(context, storage)
        val bundle = DemoFixtures.build()
        if (connected) {
            store.loadDemoData(
                overview = bundle.overview,
                accounts = bundle.accounts,
                faults = bundle.faults,
                requests = bundle.requests,
                activity = bundle.activity,
            )
        }
        return Triple(store, push, bundle)
    }

    private fun maybeSaveScreenshot(name: String) {
        val dirPath = System.getProperty("fusebox.screenshots.dir")?.takeIf { it.isNotBlank() } ?: return
        val dir = File(dirPath).apply { mkdirs() }
        composeRule.waitForIdle()
        var saved: Bitmap? = null
        composeRule.runOnUiThread {
            val view = composeRule.activity.window.decorView
            val w = view.width.coerceAtLeast(1)
            val h = view.height.coerceAtLeast(1)
            val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bmp)
            view.draw(canvas)
            saved = bmp
        }
        val bmp = saved ?: return
        FileOutputStream(File(dir, "$name.png")).use { out ->
            bmp.compress(Bitmap.CompressFormat.PNG, 100, out)
        }
    }

    private fun foldedState() = FoldableState(
        widthDp = 412.dp,
        heightDp = 915.dp,
    )

    private fun unfoldedFoldableState() = FoldableState(
        widthDp = 840.dp,
        heightDp = 840.dp,
        hingeBoundsDp = HingeBoundsDp(left = 416.dp, top = 0.dp, right = 424.dp, bottom = 840.dp),
        isHingeVertical = true,
        isSeparating = true,
        isHalfOpened = false,
    )

    @Test
    @Config(qualifiers = "w412dp-h915dp-night-xhdpi")
    fun foldedConnectScreenRendersCleanly() {
        val (store, push, _) = newDemoHarness(connected = false)
        composeRule.setContent {
            FuseboxTheme(darkTheme = true) {
                CompositionLocalProvider(LocalFoldableState provides foldedState()) {
                    MainScreen(
                        store = store,
                        pushManager = push,
                        pendingLink = null,
                        onConsumeLink = {},
                    )
                }
            }
        }
        composeRule.onNodeWithText("Connect to Fusebox").assertIsDisplayed()
        composeRule.onNodeWithText("Connect").assertIsDisplayed()
        maybeSaveScreenshot("01-folded-connect-dark")
    }

    @Test
    @Config(qualifiers = "w412dp-h915dp-night-xhdpi")
    fun foldedCoverScreenUsesBottomBarAndSingleColumn() {
        val (store, push, bundle) = newDemoHarness(connected = true)
        composeRule.setContent {
            FuseboxTheme(darkTheme = true) {
                CompositionLocalProvider(LocalFoldableState provides foldedState()) {
                    MainScreen(
                        store = store,
                        pushManager = push,
                        pendingLink = null,
                        onConsumeLink = {},
                        demoUsage = bundle.usage,
                        demoSpeed = bundle.speed,
                    )
                }
            }
        }
        // Overview screen visible with bottom navigation labels
        composeRule.onNodeWithText("SUBSCRIPTIONS").assertIsDisplayed()
        composeRule.onNodeWithText("Accounts").assertIsDisplayed()
        assertTrue(composeRule.onAllNodesWithText("Requests").fetchSemanticsNodes().isNotEmpty())
        composeRule.onNodeWithText("Usage").assertIsDisplayed()
        maybeSaveScreenshot("02-folded-overview-dark")

        // Switch to Accounts tab -> single-pane list
        composeRule.onNodeWithText("Accounts").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithText("alex@studio.example").assertIsDisplayed()
        maybeSaveScreenshot("03-folded-accounts-dark")

        // Tap an account -> opens full-screen detail on folded screen
        composeRule.onNodeWithText("alex@studio.example").performClick()
        composeRule.waitForIdle()
        composeRule.onAllNodesWithText("Refresh").onFirst().assertIsDisplayed()
        composeRule.onNodeWithText("LIMITS · LEFT").assertIsDisplayed()
        maybeSaveScreenshot("03b-folded-account-detail-dark")
    }

    @Test
    @Config(qualifiers = "w840dp-h840dp-night-xhdpi")
    fun unfoldedFoldableShowsNavigationRailTwoColumnsAndListDetail() {
        val (store, push, bundle) = newDemoHarness(connected = true)
        composeRule.setContent {
            FuseboxTheme(darkTheme = true) {
                CompositionLocalProvider(LocalFoldableState provides unfoldedFoldableState()) {
                    MainScreen(
                        store = store,
                        pushManager = push,
                        pendingLink = null,
                        onConsumeLink = {},
                        demoUsage = bundle.usage,
                        demoSpeed = bundle.speed,
                    )
                }
            }
        }
        // Unfolded Overview (2 columns + Navigation Rail)
        composeRule.onNodeWithText("SUBSCRIPTIONS").assertIsDisplayed()
        composeRule.onNodeWithText("COMBINED WEEKLY LIMITS").assertExists()
        maybeSaveScreenshot("04-unfolded-overview-dark")

        // Switch to Accounts -> 2-pane List-Detail (BOTH list and detail pane visible simultaneously!)
        composeRule.onNodeWithText("Accounts").performClick()
        composeRule.waitForIdle()
        assertTrue(composeRule.onAllNodesWithText("alex@studio.example").fetchSemanticsNodes().size >= 2)
        composeRule.onNodeWithText("work@team.example").assertIsDisplayed()
        composeRule.onNodeWithText("LIMITS · LEFT").assertIsDisplayed()
        composeRule.onAllNodesWithText("Refresh").onFirst().assertIsDisplayed()
        maybeSaveScreenshot("05-unfolded-accounts-dark")

        // Switch to Requests -> 2-pane List-Detail (request log left, routing explanation right)
        composeRule.onAllNodesWithText("Requests").onFirst().performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithText("WHY THIS ACCOUNT").assertIsDisplayed()
        maybeSaveScreenshot("06-unfolded-requests-dark")

        // Switch to Usage -> 2-column Cost + Charts + Speed
        composeRule.onNodeWithText("Usage").performClick()
        composeRule.waitForIdle()
        composeRule.onAllNodesWithText("ESTIMATED COST · LAST 7 DAYS").onFirst().assertIsDisplayed()
        maybeSaveScreenshot("07-unfolded-usage-dark")

        // Switch to Settings
        composeRule.onNodeWithText("Settings").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithText("NOTIFICATIONS").assertIsDisplayed()
        maybeSaveScreenshot("09-unfolded-settings-dark")
    }

    @Test
    @Config(qualifiers = "w840dp-h840dp-notnight-xhdpi")
    fun unfoldedOverviewLightThemeRendersCleanly() {
        val (store, push, bundle) = newDemoHarness(connected = true)
        composeRule.setContent {
            FuseboxTheme(darkTheme = false) {
                CompositionLocalProvider(LocalFoldableState provides unfoldedFoldableState()) {
                    MainScreen(
                        store = store,
                        pushManager = push,
                        pendingLink = DeepLink.Overview,
                        onConsumeLink = {},
                        demoUsage = bundle.usage,
                        demoSpeed = bundle.speed,
                    )
                }
            }
        }
        composeRule.onNodeWithText("SUBSCRIPTIONS").assertIsDisplayed()
        composeRule.onNodeWithText("COMBINED WEEKLY LIMITS").assertExists()
        maybeSaveScreenshot("08-unfolded-overview-light")
    }

    @Test
    @Config(qualifiers = "w840dp-h840dp-night-xhdpi")
    fun updateBannerAndSettingsUpdatesSectionRenderCleanly() {
        val context = ApplicationProvider.getApplicationContext<Application>()
        val storage = SecureStorage(context)
        val (store, push, bundle) = newDemoHarness(connected = true)
        val updater = AppUpdater(context, storage, selfUpdateEnabled = true).apply {
            setDemoAvailable(
                AppUpdateInfo(
                    version = "1.0.3",
                    tagName = "android-v1.0.3",
                    downloadUrl = "https://github.com/DigitalPals/Fusebox/releases/download/android-v1.0.3/fusebox-1.0.3-sideload.apk",
                    sizeBytes = 5_120_000L,
                ),
            )
        }
        composeRule.setContent {
            FuseboxTheme(darkTheme = true) {
                CompositionLocalProvider(LocalFoldableState provides unfoldedFoldableState()) {
                    MainScreen(
                        store = store,
                        pushManager = push,
                        updater = updater,
                        pendingLink = null,
                        onConsumeLink = {},
                        demoUsage = bundle.usage,
                        demoSpeed = bundle.speed,
                    )
                }
            }
        }
        composeRule.onNodeWithText("Fusebox 1.0.3 available").assertIsDisplayed()
        composeRule.onNodeWithText("Update").assertIsDisplayed()
        maybeSaveScreenshot("10-unfolded-update-banner-dark")

        composeRule.onNodeWithText("Settings").performClick()
        composeRule.waitForIdle()
        composeRule.onNodeWithText("UPDATES").assertExists()
        composeRule.onNodeWithText("Update to 1.0.3").assertExists()
        maybeSaveScreenshot("11-unfolded-settings-update-dark")
    }
}
