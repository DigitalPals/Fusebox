package com.digitalpals.fusebox

import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.FuseboxJson
import com.digitalpals.fusebox.data.PushPath
import com.digitalpals.fusebox.data.PushStatus
import com.digitalpals.fusebox.data.RelayDevice
import com.digitalpals.fusebox.data.SealedPush
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.util.Base64

/**
 * Verifies the exact AES-256-GCM test vector shared with `src/push/relay.rs`
 * (`the_sealed_format_matches_the_apps_test_vector`) and `ios/FuseboxTests/PushRelayTests.swift`.
 */
class PushRelayTest {
    private val sharedKey = Base64.getDecoder().decode("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=")
    private val sharedSealed =
        "ZGVmZ2hpamtsbW5vMzmqDw2FM7wEQByEuxAOmGKxb23lQZoch7TUOJLRwCy2xWKiI3biJsfRDUyYMPuPfiP5ilAmJAaZnsH2geTA7nusqGexJVJr7E+ELTHrVocNFjEWi+f9p6uZFP5ZMGEw7RL9CwU5y1HQlr+wxmiVnVFO8i7vE8pb3Fu/k7HdZqMQpbrbAFUsQqdreifl5Srn+uXHZDU59LAdsw4/j2fMnvVDBWayv42BHjzE1cBWwq31nrjHQ5Fo7RttcD2m5TK7uUp024Z/B24sXUdBMVKAbSEstDilO2Dch3xn9pGSp6gJOzXHramsjmeW4IC5"

    @Test
    fun opensSharedRelayTestVector() {
        val opened = SealedPush.open(sharedSealed, sharedKey)
        assertEquals("Claude sign-in expired", opened.title)
        assertEquals("user1@example.com needs signing in again.", opened.body)
        assertEquals("#/accounts/claude-user1.json", opened.path)
        assertEquals("signin", opened.kind)
        assertEquals("err", opened.level)
        assertEquals("claude", opened.provider)
        assertEquals("claude-user1.json", opened.account)
        assertEquals("claude", opened.thread)
        assertEquals(DeepLink.Account("claude-user1.json"), DeepLink.routePath(opened.path))
    }

    @Test
    fun rejectsWrongKeyOrTamperedCiphertext() {
        assertThrows(Exception::class.java) { SealedPush.open(sharedSealed, ByteArray(32) { 7 }) }
        val bytes = Base64.getDecoder().decode(sharedSealed)
        bytes[20] = (bytes[20].toInt() xor 1).toByte()
        val tampered = Base64.getEncoder().encodeToString(bytes)
        assertThrows(Exception::class.java) { SealedPush.open(tampered, sharedKey) }
        assertThrows(Exception::class.java) { SealedPush.open("not base64!", sharedKey) }
        assertThrows(Exception::class.java) { SealedPush.open("AAAA", sharedKey) }
    }

    @Test
    fun revealHandlesRelayedDirectAndMissingKey() {
        val revealed = SealedPush.reveal(mapOf("sealed" to sharedSealed, "badge" to "2", "v" to "1"), sharedKey)
        assertEquals("Claude sign-in expired", revealed.title)
        assertEquals("claude", revealed.thread)

        val fallback = SealedPush.reveal(mapOf("sealed" to sharedSealed), null)
        assertEquals("Fusebox", fallback.title)
        assertEquals("Open Fusebox to see what happened.", fallback.body)

        val direct = SealedPush.reveal(
            mapOf("title" to "Codex limit reached", "body" to "Every account is used up.", "path" to "#/accounts/codex-1.json"),
            sharedKey,
        )
        assertEquals("Codex limit reached", direct.title)
        assertEquals("#/accounts/codex-1.json", direct.path)
    }

    @Test
    fun generatesRandom32ByteKeys() {
        val a = RelayDevice.makeKey()
        val b = RelayDevice.makeKey()
        assertEquals(32, a.size)
        assertNotEquals(Base64.getEncoder().encodeToString(a), Base64.getEncoder().encodeToString(b))
    }

    @Test
    fun choosesCorrectAndroidPushPath() {
        val direct = FuseboxJson.decodeFromString(
            PushStatus.serializer(),
            """{"public_key":"B","subscriptions":[],"apns":false,"fcm":true,"relay":"https://push.whombat.com"}""",
        )
        assertEquals(PushPath.Direct, PushPath.chooseAndroid(direct))

        val relay = FuseboxJson.decodeFromString(
            PushStatus.serializer(),
            """{"public_key":"B","subscriptions":[],"apns":true,"fcm":false,"relay":"https://push.whombat.com"}""",
        )
        assertEquals(PushPath.Relay("https://push.whombat.com"), PushPath.chooseAndroid(relay))

        val off = FuseboxJson.decodeFromString(
            PushStatus.serializer(),
            """{"public_key":"B","subscriptions":[],"apns":false,"fcm":false,"relay":null}""",
        )
        assertEquals(PushPath.Unavailable, PushPath.chooseAndroid(off))

        val legacy = FuseboxJson.decodeFromString(PushStatus.serializer(), """{"public_key":"B","subscriptions":[]}""")
        assertEquals(PushPath.Unavailable, PushPath.chooseAndroid(legacy))
    }
}
