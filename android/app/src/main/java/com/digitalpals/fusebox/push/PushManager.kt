package com.digitalpals.fusebox.push

import android.content.Context
import android.os.Build
import com.digitalpals.fusebox.data.ApiException
import com.digitalpals.fusebox.data.ConnectionHelper
import com.digitalpals.fusebox.data.FcmDirectRegistration
import com.digitalpals.fusebox.data.FuseboxApi
import com.digitalpals.fusebox.data.FuseboxJson
import com.digitalpals.fusebox.data.IdResponse
import com.digitalpals.fusebox.data.PushPath
import com.digitalpals.fusebox.data.PushStatus
import com.digitalpals.fusebox.data.RelayClient
import com.digitalpals.fusebox.data.RelayDevice
import com.digitalpals.fusebox.data.RelayRegistration
import com.digitalpals.fusebox.data.SecureStorage
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

data class PushUiState(
    val permissionGranted: Boolean = false,
    val isOn: Boolean = false,
    val via: String? = null,
    val subscriptionId: String? = null,
    val busy: Boolean = false,
    val message: String? = null,
    val messageIsError: Boolean = false,
    val devices: List<PushStatus.Device> = emptyList(),
    val path: PushPath? = null,
    val statusError: String? = null,
    val firebaseAvailable: Boolean = false,
)

/**
 * Native Android push manager (FCM direct or encrypted via Fusebox Push Relay).
 * Gracefully handles sideload builds compiled without `google-services.json`.
 */
class PushManager(
    private val context: Context,
    private val storage: SecureStorage,
) {
    private val appContext = context.applicationContext
    private val registerMutex = Mutex()

    private val _state = MutableStateFlow(
        PushUiState(
            permissionGranted = FuseboxNotifications.hasPermission(appContext),
            isOn = storage.pushSubscriptionId != null,
            via = storage.pushVia,
            subscriptionId = storage.pushSubscriptionId,
            firebaseAvailable = isFirebaseConfigured(),
        ),
    )
    val state: StateFlow<PushUiState> = _state.asStateFlow()

    val deviceLabel: String
        get() {
            val model = Build.MODEL?.trim().orEmpty()
            val manuf = Build.MANUFACTURER?.trim().orEmpty()
            val name = when {
                model.isEmpty() -> "Android"
                manuf.isNotEmpty() && !model.startsWith(manuf, ignoreCase = true) ->
                    "${manuf.replaceFirstChar { it.uppercase() }} $model"
                else -> model
            }
            return "Fusebox for Android on $name"
        }

    fun isFirebaseConfigured(): Boolean = runCatching {
        if (FirebaseApp.getApps(appContext).isNotEmpty()) return true
        FirebaseApp.initializeApp(appContext) != null
    }.getOrDefault(false)

    fun refreshPermission() {
        _state.update {
            it.copy(
                permissionGranted = FuseboxNotifications.hasPermission(appContext),
                firebaseAvailable = isFirebaseConfigured(),
                isOn = storage.pushSubscriptionId != null,
                via = storage.pushVia,
                subscriptionId = storage.pushSubscriptionId,
            )
        }
    }

    suspend fun onLaunchOrForeground(api: FuseboxApi?) {
        refreshPermission()
        if (api == null || !storage.pushWanted || !FuseboxNotifications.hasPermission(appContext)) return
        if (!isFirebaseConfigured()) return
        val now = System.currentTimeMillis()
        val touchDue = (now - storage.pushRelayTouchedMs) >= TOUCH_INTERVAL_MS
        if (storage.pushSubscriptionId == null || (storage.pushVia == "relay" && touchDue)) {
            runCatching {
                val token = fetchFcmToken()
                register(token, api)
            }
        }
    }

    suspend fun onNewFcmToken(token: String, api: FuseboxApi?) {
        if (api == null || !storage.pushWanted) return
        runCatching { register(token, api) }
    }

    suspend fun turnOn(api: FuseboxApi?) {
        if (api == null) return
        refreshPermission()
        if (!FuseboxNotifications.hasPermission(appContext)) {
            show("Notifications are off for Fusebox in Android Settings. Allow them first.", error = true)
            return
        }
        if (!isFirebaseConfigured()) {
            show(NO_FIREBASE_TEXT, error = true)
            return
        }
        _state.update { it.copy(busy = true, message = null, messageIsError = false) }
        try {
            storage.pushWanted = true
            val token = fetchFcmToken()
            register(token, api)
            show("Notifications are on for this device.")
        } catch (e: Exception) {
            show(e.message ?: "Could not turn on notifications.", error = true)
        } finally {
            _state.update { it.copy(busy = false) }
        }
        loadDevices(api)
    }

    suspend fun turnOff(api: FuseboxApi?) {
        storage.pushWanted = false
        val id = storage.pushSubscriptionId
        if (api == null || id == null) {
            clearLocalSubscription()
            forgetRelayDevice()
            return
        }
        _state.update { it.copy(busy = true) }
        try {
            try {
                api.request("DELETE", "/push/subscriptions/${ConnectionHelper.encodePathSegment(id)}")
            } catch (e: ApiException) {
                if (e.status != 404) throw e
            }
            clearLocalSubscription()
            forgetRelayDevice()
            show("Notifications are off for this device.")
        } catch (e: Exception) {
            show(e.message ?: "Could not turn off notifications.", error = true)
        } finally {
            _state.update { it.copy(busy = false) }
        }
        loadDevices(api)
    }

    suspend fun sendTest(api: FuseboxApi?) {
        val id = storage.pushSubscriptionId ?: return
        if (api == null) return
        _state.update { it.copy(busy = true) }
        try {
            api.send("POST", "/push/test", IdResponse.serializer(), IdResponse(id))
            show("Test sent. It should arrive in a few seconds.")
        } catch (e: ApiException) {
            if (e.status == 404 || e.status == 410) clearLocalSubscription()
            show(e.message, error = true)
        } catch (e: Exception) {
            show(e.message ?: "Test failed.", error = true)
        } finally {
            _state.update { it.copy(busy = false) }
        }
    }

    suspend fun remove(device: PushStatus.Device, api: FuseboxApi?) {
        if (api == null) return
        try {
            api.request("DELETE", "/push/subscriptions/${ConnectionHelper.encodePathSegment(device.id)}")
            if (device.id == storage.pushSubscriptionId) {
                clearLocalSubscription()
                forgetRelayDevice()
            }
        } catch (e: Exception) {
            show(e.message ?: "Could not remove device.", error = true)
        }
        loadDevices(api)
    }

    suspend fun loadDevices(api: FuseboxApi?) {
        if (api == null) return
        try {
            val s = api.pushStatus()
            val path = PushPath.chooseAndroid(s)
            val myId = storage.pushSubscriptionId
            if (myId != null && s.subscriptions.none { it.id == myId }) {
                clearLocalSubscription()
                forgetRelayDevice()
            }
            _state.update {
                it.copy(
                    devices = s.subscriptions,
                    path = path,
                    statusError = null,
                    isOn = storage.pushSubscriptionId != null,
                    via = storage.pushVia,
                    subscriptionId = storage.pushSubscriptionId,
                )
            }
        } catch (e: Exception) {
            _state.update { it.copy(statusError = e.message) }
        }
    }

    private suspend fun register(token: String, api: FuseboxApi) = registerMutex.withLock {
        val status = api.pushStatus()
        val chosen = PushPath.chooseAndroid(status)
        _state.update { it.copy(path = chosen) }
        val previous = storage.pushSubscriptionId
        val newId: String
        when (chosen) {
            PushPath.Unavailable -> throw ApiException(0, UNAVAILABLE_TEXT)
            PushPath.Direct -> {
                val body = FcmDirectRegistration(token = token, label = deviceLabel)
                val raw = api.send("POST", "/push/fcm", FcmDirectRegistration.serializer(), body)
                newId = FuseboxJson.decodeFromString(IdResponse.serializer(), raw).id
                storage.pushVia = "direct"
                forgetRelayDevice()
            }
            is PushPath.Relay -> {
                val device = ensureRelayDevice(chosen.url, token)
                val body = RelayRegistration(
                    relay = RelayRegistration.RelayBody(handle = device.handle, key = device.keyBase64),
                    label = deviceLabel,
                )
                val raw = api.send("POST", "/push/fcm", RelayRegistration.serializer(), body)
                newId = FuseboxJson.decodeFromString(IdResponse.serializer(), raw).id
                storage.pushVia = "relay"
            }
        }
        storage.pushSubscriptionId = newId
        _state.update { it.copy(isOn = true, subscriptionId = newId, via = storage.pushVia) }
        if (previous != null && previous != newId) {
            runCatching { api.request("DELETE", "/push/subscriptions/${ConnectionHelper.encodePathSegment(previous)}") }
        }
    }

    private suspend fun ensureRelayDevice(relayUrl: String, token: String): RelayDevice {
        val client = RelayClient(relayUrl)
        val existing = storage.relayDevice
        if (existing != null) {
            if (existing.relay == relayUrl && existing.token == token) {
                val touchDue = (System.currentTimeMillis() - storage.pushRelayTouchedMs) >= TOUCH_INTERVAL_MS
                if (!touchDue) return existing
                when (client.touch(existing.handle)) {
                    true -> {
                        storage.pushRelayTouchedMs = System.currentTimeMillis()
                        return existing
                    }
                    null -> return existing // Relay unreachable right now; keep handle
                    false -> Unit // 410 forgotten -> re-register below
                }
            } else {
                RelayClient(existing.relay).unregister(existing.handle)
            }
            storage.relayDevice = null
        }
        val handle = client.registerFcm(token)
        val keyBase64 = RelayDevice.makeKeyBase64()
        val device = RelayDevice(relay = relayUrl, handle = handle, keyBase64 = keyBase64, token = token)
        storage.relayDevice = device
        storage.pushRelayTouchedMs = System.currentTimeMillis()
        return device
    }

    private suspend fun forgetRelayDevice() {
        val existing = storage.relayDevice ?: return
        storage.relayDevice = null
        storage.pushRelayTouchedMs = 0L
        RelayClient(existing.relay).unregister(existing.handle)
    }

    private fun clearLocalSubscription() {
        storage.pushSubscriptionId = null
        storage.pushVia = null
        _state.update { it.copy(isOn = false, subscriptionId = null, via = null) }
    }

    private fun show(text: String, error: Boolean = false) {
        _state.update { it.copy(message = text, messageIsError = error) }
    }

    private suspend fun fetchFcmToken(): String = suspendCancellableCoroutine { cont ->
        FirebaseMessaging.getInstance().token
            .addOnSuccessListener { tok ->
                if (cont.isActive) {
                    if (tok.isNullOrBlank()) cont.resumeWithException(ApiException(0, "Firebase returned an empty push token."))
                    else cont.resume(tok)
                }
            }
            .addOnFailureListener { err ->
                if (cont.isActive) {
                    cont.resumeWithException(
                        ApiException(0, "Could not get a Firebase push token: ${err.message ?: "unknown error"}"),
                    )
                }
            }
    }

    companion object {
        const val TOUCH_INTERVAL_MS = 6L * 3600L * 1000L
        const val UNAVAILABLE_TEXT =
            "This Fusebox can’t send notifications to the Android app. Update Fusebox, or set fcm in its config.yaml."
        const val NO_FIREBASE_TEXT =
            "This sideload build has no Firebase config (google-services.json). Add it to android/app/ and rebuild to enable push."
    }
}
