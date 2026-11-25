package com.digitalpals.fusebox.widget

import android.appwidget.AppWidgetManager
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.Text
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.digitalpals.fusebox.FuseboxApplication
import com.digitalpals.fusebox.data.logo
import com.digitalpals.fusebox.data.metered
import com.digitalpals.fusebox.data.subtitle
import com.digitalpals.fusebox.ui.components.CardPanel
import com.digitalpals.fusebox.ui.components.Hairline
import com.digitalpals.fusebox.ui.components.ProviderLogo
import com.digitalpals.fusebox.ui.theme.FuseboxTheme
import com.digitalpals.fusebox.ui.theme.LocalFuseboxColors
import com.digitalpals.fusebox.ui.theme.Typeface

/** Lets the user choose which subscription a Subscription Home Screen widget tracks. */
class SubscriptionWidgetConfigActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setResult(RESULT_CANCELED)
        val appWidgetId = intent?.extras?.getInt(
            AppWidgetManager.EXTRA_APPWIDGET_ID,
            AppWidgetManager.INVALID_APPWIDGET_ID,
        ) ?: AppWidgetManager.INVALID_APPWIDGET_ID
        if (appWidgetId == AppWidgetManager.INVALID_APPWIDGET_ID) {
            finish()
            return
        }
        val app = application as FuseboxApplication
        val accounts = (app.store.state.value.accounts.ifEmpty {
            app.storage.loadWidgetSnapshot()?.accounts.orEmpty()
        }).filter { it.metered }

        setContent {
            FuseboxTheme {
                val colors = LocalFuseboxColors.current
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .background(colors.bg)
                        .safeDrawingPadding()
                        .padding(16.dp),
                ) {
                    Text(
                        text = "CHOOSE A SUBSCRIPTION",
                        style = Typeface.pageTitle,
                        color = colors.fg,
                    )
                    Spacer(Modifier.height(6.dp))
                    Text(
                        text = "Pick which account’s 5-hour and weekly meters this widget shows.",
                        style = Typeface.meta,
                        color = colors.fg3,
                    )
                    Spacer(Modifier.height(16.dp))
                    if (accounts.isEmpty()) {
                        CardPanel {
                            Text(
                                text = "Open Fusebox and connect to your server first.",
                                style = Typeface.row,
                                color = colors.fg2,
                            )
                        }
                    } else {
                        CardPanel(padding = 0.dp) {
                            LazyColumn {
                                itemsIndexed(accounts, key = { _, a -> a.id }) { index, acct ->
                                    if (index > 0) Hairline(Modifier.padding(horizontal = 16.dp))
                                    Row(
                                        modifier = Modifier
                                            .fillMaxWidth()
                                            .clickable {
                                                app.storage.setWidgetAccountId(appWidgetId, acct.id)
                                                FuseboxWidgetUpdater.updateAll(this@SubscriptionWidgetConfigActivity)
                                                setResult(
                                                    RESULT_OK,
                                                    Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, appWidgetId),
                                                )
                                                finish()
                                            }
                                            .padding(horizontal = 16.dp, vertical = 14.dp),
                                        verticalAlignment = Alignment.CenterVertically,
                                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                                    ) {
                                        ProviderLogo(acct.logo, size = 20.dp)
                                        Column(Modifier.weight(1f)) {
                                            Text(acct.label, style = Typeface.rowMedium, color = colors.fg, maxLines = 1)
                                            Text(acct.subtitle, style = Typeface.small, color = colors.fg3, maxLines = 1)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
