package com.digitalpals.fusebox.data

import kotlinx.serialization.Serializable
import java.nio.charset.StandardCharsets
import java.security.SecureRandom
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * This device's registration at the push relay: the handle for its FCM token and the 32-byte
 * AES-256-GCM key shared only with this Fusebox server.
 */
@Serializable
data class RelayDevice(
    val relay: String,
    val handle: String,
    /** Standard base64 of the 32-byte key. */
    val keyBase64: String,
    /** The FCM registration token this handle was created for. */
    val token: String,
) {
    val keyBytes: ByteArray get() = Base64.getDecoder().decode(keyBase64)

    companion object {
        private val random = SecureRandom()

        fun makeKey(): ByteArray = ByteArray(32).also { random.nextBytes(it) }

        fun makeKeyBase64(): String = Base64.getEncoder().encodeToString(makeKey())
    }
}

/**
 * Opens notifications sealed by Fusebox (`src/push/relay.rs`):
 * standard base64 of `nonce(12) || ciphertext || tag(16)`, AES-256-GCM with AAD `fusebox-push-v1`.
 */
object SealedPush {
    private val AAD = "fusebox-push-v1".toByteArray(StandardCharsets.UTF_8)
    private const val NONCE_LEN = 12
    private const val TAG_BITS = 128

    @Serializable
    data class Contents(
        val title: String? = null,
        val body: String? = null,
        val path: String? = null,
        val kind: String? = null,
        val level: String? = null,
        val provider: String? = null,
        val account: String? = null,
        val thread: String? = null,
    )

    fun open(sealedBase64: String, key: ByteArray): Contents {
        require(key.size == 32) { "Key must be 32 bytes" }
        val data = Base64.getDecoder().decode(sealedBase64)
        require(data.size > NONCE_LEN + 16) { "Sealed payload too short" }
        val nonce = data.copyOfRange(0, NONCE_LEN)
        val ciphertextAndTag = data.copyOfRange(NONCE_LEN, data.size)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(TAG_BITS, nonce))
        cipher.updateAAD(AAD)
        val plain = cipher.doFinal(ciphertextAndTag)
        return FuseboxJson.decodeFromString(Contents.serializer(), String(plain, StandardCharsets.UTF_8))
    }

    /**
     * Resolves an incoming FCM data map (relayed with `sealed` or direct plaintext fields)
     * into notification display contents, falling back to the generic alert if decryption fails.
     */
    fun reveal(data: Map<String, String>, key: ByteArray?): Contents {
        val sealed = data["sealed"]
        if (!sealed.isNullOrEmpty()) {
            val opened = if (key != null) runCatching { open(sealed, key) }.getOrNull() else null
            return opened ?: Contents(
                title = "Fusebox",
                body = "Open Fusebox to see what happened.",
                thread = "fusebox",
            )
        }
        return Contents(
            title = data["title"]?.takeIf { it.isNotEmpty() } ?: "Fusebox",
            body = data["body"].orEmpty(),
            path = data["path"],
            kind = data["kind"],
            level = data["level"],
            provider = data["provider"],
            account = data["account"],
            thread = data["thread"]?.takeIf { it.isNotEmpty() } ?: "fusebox",
        )
    }
}
