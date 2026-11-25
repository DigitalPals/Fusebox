package com.digitalpals.fusebox.push

import com.digitalpals.fusebox.FuseboxApplication
import com.digitalpals.fusebox.data.SealedPush
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Receives FCM data messages (both direct and end-to-end encrypted via the Fusebox push relay).
 * Decrypts `sealed` payloads locally with the AES-256-GCM key in Keystore-backed storage.
 */
class FuseboxMessagingService : FirebaseMessagingService() {
    private val serviceScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    override fun onNewToken(token: String) {
        val app = application as? FuseboxApplication ?: return
        serviceScope.launch {
            app.pushManager.onNewFcmToken(token, app.store.currentApi())
        }
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val app = application as? FuseboxApplication ?: return
        val data = message.data
        if (data.isEmpty() && message.notification == null) return
        val key = app.storage.relayDevice?.keyBytes
        val contents = if (data.isNotEmpty()) {
            SealedPush.reveal(data, key)
        } else {
            SealedPush.Contents(
                title = message.notification?.title ?: "Fusebox",
                body = message.notification?.body.orEmpty(),
                thread = "fusebox",
            )
        }
        val badge = data["badge"]?.toIntOrNull()
        FuseboxNotifications.showPushNotification(
            context = applicationContext,
            title = contents.title ?: "Fusebox",
            body = contents.body.orEmpty(),
            path = contents.path,
            thread = contents.thread,
            level = contents.level,
            badgeCount = badge,
        )
    }
}
