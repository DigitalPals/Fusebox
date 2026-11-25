package com.digitalpals.fusebox.data

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Stores sensitive values (management key, server URL, push relay encryption key) encrypted
 * under an Android Keystore AES-256-GCM key, and non-sensitive preferences in private prefs.
 */
class SecureStorage(context: Context) {
    private val appContext = context.applicationContext
    private val secretPrefs: SharedPreferences =
        appContext.getSharedPreferences("fusebox_secrets", Context.MODE_PRIVATE)
    private val prefs: SharedPreferences =
        appContext.getSharedPreferences("fusebox_prefs", Context.MODE_PRIVATE)

    // MARK: - Connection credentials (Keystore-encrypted)

    var serverUrl: String?
        get() = getSecret("server_url")
        set(value) = putSecret("server_url", value)

    var managementKey: String?
        get() = getSecret("management_key")
        set(value) = putSecret("management_key", value)

    var relayDevice: RelayDevice?
        get() = getSecret("push_relay_device")?.let {
            runCatching { FuseboxJson.decodeFromString(RelayDevice.serializer(), it) }.getOrNull()
        }
        set(value) = putSecret(
            "push_relay_device",
            value?.let { FuseboxJson.encodeToString(RelayDevice.serializer(), it) },
        )

    // MARK: - App preferences

    var quotaDisplay: QuotaDisplay
        get() = QuotaDisplay.fromRaw(prefs.getString("quota_display", null)) ?: QuotaDisplay.standard
        set(value) { prefs.edit().putString("quota_display", value.raw).apply() }

    var outageAlertsEnabled: Boolean
        get() = prefs.getBoolean("outage_alerts_enabled", true)
        set(value) { prefs.edit().putBoolean("outage_alerts_enabled", value).apply() }

    var pushSubscriptionId: String?
        get() = prefs.getString("push_subscription_id", null)
        set(value) {
            prefs.edit().apply {
                if (value == null) remove("push_subscription_id") else putString("push_subscription_id", value)
            }.apply()
        }

    var pushVia: String?
        get() = prefs.getString("push_via", null)
        set(value) {
            prefs.edit().apply {
                if (value == null) remove("push_via") else putString("push_via", value)
            }.apply()
        }

    var pushWanted: Boolean
        get() = prefs.getBoolean("push_wanted", false)
        set(value) { prefs.edit().putBoolean("push_wanted", value).apply() }

    var pushRelayTouchedMs: Long
        get() = prefs.getLong("push_relay_touched_ms", 0L)
        set(value) { prefs.edit().putLong("push_relay_touched_ms", value).apply() }

    var lastUpdateCheckMs: Long
        get() = prefs.getLong("last_update_check_ms", 0L)
        set(value) { prefs.edit().putLong("last_update_check_ms", value).apply() }

    var dismissedUpdateVersion: String?
        get() = prefs.getString("dismissed_update_version", null)
        set(value) {
            prefs.edit().apply {
                if (value == null) remove("dismissed_update_version")
                else putString("dismissed_update_version", value)
            }.apply()
        }

    // MARK: - Widget snapshot & per-widget account choice

    fun saveWidgetSnapshot(snapshot: WidgetSnapshot) {
        val raw = runCatching { FuseboxJson.encodeToString(WidgetSnapshot.serializer(), snapshot) }.getOrNull()
        if (raw != null) prefs.edit().putString("widget_snapshot", raw).apply()
    }

    fun loadWidgetSnapshot(): WidgetSnapshot? =
        prefs.getString("widget_snapshot", null)?.let {
            runCatching { FuseboxJson.decodeFromString(WidgetSnapshot.serializer(), it) }.getOrNull()
        }

    fun widgetAccountId(appWidgetId: Int): String? =
        prefs.getString("widget_account_$appWidgetId", null)

    fun setWidgetAccountId(appWidgetId: Int, accountId: String?) {
        prefs.edit().apply {
            if (accountId == null) remove("widget_account_$appWidgetId")
            else putString("widget_account_$appWidgetId", accountId)
        }.apply()
    }

    // MARK: - Keystore AES-256-GCM encryption

    private fun getSecret(key: String): String? {
        val encoded = secretPrefs.getString(key, null) ?: return null
        return runCatching { decrypt(encoded) }.getOrNull()
    }

    private fun putSecret(key: String, value: String?) {
        val editor = secretPrefs.edit()
        if (value == null) {
            editor.remove(key)
        } else {
            editor.putString(key, encrypt(value))
        }
        editor.apply()
    }

    private fun masterKey(): SecretKey {
        val ks = KeyStore.getInstance(KEYSTORE_PROVIDER).apply { load(null) }
        val existing = ks.getKey(KEY_ALIAS, null) as? SecretKey
        if (existing != null) return existing
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE_PROVIDER)
        gen.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return gen.generateKey()
    }

    private fun encrypt(plaintext: String): String {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, masterKey())
        val iv = cipher.iv
        val ciphertext = cipher.doFinal(plaintext.toByteArray(StandardCharsets.UTF_8))
        val combined = ByteArray(1 + iv.size + ciphertext.size)
        combined[0] = iv.size.toByte()
        System.arraycopy(iv, 0, combined, 1, iv.size)
        System.arraycopy(ciphertext, 0, combined, 1 + iv.size, ciphertext.size)
        return Base64.getEncoder().encodeToString(combined)
    }

    private fun decrypt(encoded: String): String {
        val combined = Base64.getDecoder().decode(encoded)
        require(combined.size > 14)
        val ivLen = combined[0].toInt() and 0xFF
        val iv = combined.copyOfRange(1, 1 + ivLen)
        val ciphertext = combined.copyOfRange(1 + ivLen, combined.size)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, masterKey(), GCMParameterSpec(128, iv))
        val plain = cipher.doFinal(ciphertext)
        return String(plain, StandardCharsets.UTF_8)
    }

    companion object {
        private const val KEYSTORE_PROVIDER = "AndroidKeyStore"
        private const val KEY_ALIAS = "com.digitalpals.fusebox.master"
    }
}
