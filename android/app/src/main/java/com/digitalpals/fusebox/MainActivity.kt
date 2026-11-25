package com.digitalpals.fusebox

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.DemoFixtures
import com.digitalpals.fusebox.data.UsageDashboard
import com.digitalpals.fusebox.data.UsageSpeed
import com.digitalpals.fusebox.ui.MainScreen
import com.digitalpals.fusebox.ui.adaptive.ProvideFoldableState
import com.digitalpals.fusebox.ui.theme.FuseboxTheme

class MainActivity : ComponentActivity() {
    private var pendingLink by mutableStateOf<DeepLink?>(null)
    private var darkOverride by mutableStateOf<Boolean?>(null)
    private var demoUsage by mutableStateOf<UsageDashboard?>(null)
    private var demoSpeed by mutableStateOf<UsageSpeed?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        handleIntent(intent)

        val app = application as FuseboxApplication

        setContent {
            val systemDark = isSystemInDarkTheme()
            val dark = darkOverride ?: systemDark
            LaunchedEffect(dark) {
                val style = if (dark) {
                    SystemBarStyle.dark(android.graphics.Color.TRANSPARENT)
                } else {
                    SystemBarStyle.light(android.graphics.Color.TRANSPARENT, android.graphics.Color.TRANSPARENT)
                }
                enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
            }
            FuseboxTheme(darkTheme = dark) {
                ProvideFoldableState {
                    MainScreen(
                        store = app.store,
                        pushManager = app.pushManager,
                        pendingLink = pendingLink,
                        onConsumeLink = { pendingLink = null },
                        demoUsage = demoUsage,
                        demoSpeed = demoSpeed,
                    )
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIntent(intent)
    }

    private fun handleIntent(intent: Intent?) {
        if (intent == null) return
        val app = application as FuseboxApplication

        // Debug-only demo mode & visual verification helpers (never compiled into Release)
        if (BuildConfig.DEBUG) {
            if (intent.getBooleanExtra("demo", false)) {
                val bundle = DemoFixtures.build()
                demoUsage = bundle.usage
                demoSpeed = bundle.speed
                app.store.loadDemoData(
                    overview = bundle.overview,
                    accounts = bundle.accounts,
                    faults = bundle.faults,
                    requests = bundle.requests,
                    activity = bundle.activity,
                )
            }
            if (intent.hasExtra("dark")) {
                darkOverride = intent.getBooleanExtra("dark", true)
            }
            intent.getStringExtra("tab")?.let { tab ->
                pendingLink = when (tab.lowercase()) {
                    "accounts" -> DeepLink.Accounts
                    "requests" -> DeepLink.Requests
                    "usage" -> DeepLink.Usage
                    "settings" -> DeepLink.Settings
                    "faults" -> DeepLink.Faults
                    else -> DeepLink.Overview
                }
            }
            intent.getStringExtra("account")?.takeIf { it.isNotEmpty() }?.let {
                pendingLink = DeepLink.Account(it)
            }
            val reqId = intent.getIntExtra("request", 0)
            if (reqId > 0) pendingLink = DeepLink.Request(reqId)
        }

        // Notification tap path (`#/accounts/...`)
        intent.getStringExtra(EXTRA_NOTIFICATION_PATH)?.let { path ->
            pendingLink = DeepLink.routePath(path)
            return
        }
        // App deep link (`fusebox://...`)
        intent.dataString?.takeIf { it.startsWith("fusebox://", ignoreCase = true) }?.let { uri ->
            pendingLink = DeepLink.routeUri(uri)
        }
    }

    companion object {
        const val EXTRA_NOTIFICATION_PATH = "fusebox_notification_path"
    }
}
