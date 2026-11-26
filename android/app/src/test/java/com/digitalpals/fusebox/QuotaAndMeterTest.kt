package com.digitalpals.fusebox

import com.digitalpals.fusebox.data.Account
import com.digitalpals.fusebox.data.AccountState
import com.digitalpals.fusebox.data.DeepLink
import com.digitalpals.fusebox.data.Fmt
import com.digitalpals.fusebox.data.FuseboxDate
import com.digitalpals.fusebox.data.FuseboxJson
import com.digitalpals.fusebox.data.MeterModel
import com.digitalpals.fusebox.data.Quota
import com.digitalpals.fusebox.data.QuotaDisplay
import com.digitalpals.fusebox.data.QuotaReading
import com.digitalpals.fusebox.data.QuotaWindow
import com.digitalpals.fusebox.data.Quotas
import com.digitalpals.fusebox.data.Renewal
import com.digitalpals.fusebox.data.cooldownReason
import com.digitalpals.fusebox.data.metered
import com.digitalpals.fusebox.data.noShortLimit
import com.digitalpals.fusebox.data.planName
import com.digitalpals.fusebox.data.renewalRow
import com.digitalpals.fusebox.data.scopedWindows
import com.digitalpals.fusebox.data.state
import com.digitalpals.fusebox.data.statusWord
import com.digitalpals.fusebox.data.subtitle
import com.digitalpals.fusebox.data.window
import com.digitalpals.fusebox.data.windowSpent
import com.digitalpals.fusebox.ui.adaptive.ColumnPlan
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import kotlin.math.abs

class QuotaAndMeterTest {
    private val now = FuseboxDate.parse("2026-11-01T12:00:00Z")!!

    private fun meter(used: Double, pace: Double? = null, mode: QuotaDisplay = QuotaDisplay.USED) =
        MeterModel.build(QuotaReading(used), pace, mode)

    @Test
    fun segmentedMeterLitAndLevelsMatchDashboard() {
        val cases = listOf(0.0 to 0, 2.4 to 0, 2.5 to 1, 5.0 to 1, 47.0 to 9, 47.5 to 10, 50.0 to 10, 97.4 to 19, 97.5 to 20, 100.0 to 20)
        for ((used, lit) in cases) {
            assertEquals("used=$used", lit, meter(used).lit)
        }
        assertEquals(14, meter(30.0, mode = QuotaDisplay.REMAINING).lit)
        assertEquals(0, meter(100.0, mode = QuotaDisplay.REMAINING).lit)
        assertEquals(20, meter(0.0, mode = QuotaDisplay.REMAINING).lit)
        assertEquals(4, meter(82.0, mode = QuotaDisplay.REMAINING).lit)

        for (mode in QuotaDisplay.entries) {
            assertEquals(QuotaReading.Level.NORMAL, meter(74.9, mode = mode).level)
            assertEquals(QuotaReading.Level.WARN, meter(75.0, mode = mode).level)
            assertEquals(QuotaReading.Level.WARN, meter(94.9, mode = mode).level)
            assertEquals(QuotaReading.Level.ERR, meter(95.0, mode = mode).level)
        }
        assertNull(MeterModel.childLevel(QuotaReading(74.9)))
        assertEquals(QuotaReading.Level.WARN, MeterModel.childLevel(QuotaReading(83.0)))
        assertEquals(QuotaReading.Level.ERR, MeterModel.childLevel(QuotaReading(95.0)))
    }

    @Test
    fun paceMirrorsInRemainingModeAndRunsOutForecast() {
        assertEquals(0.4, meter(10.0, pace = 0.4, mode = QuotaDisplay.USED).pace!!, 1e-9)
        assertEquals(0.6, meter(10.0, pace = 0.4, mode = QuotaDisplay.REMAINING).pace!!, 1e-9)
        assertNull(meter(10.0, pace = Double.NaN).pace)

        val w = QuotaWindow("week", used = 80.0, resetsAt = now.plusSeconds((3.5 * 86_400).toLong()))
        val out = Quotas.runsOut(w, now)
        assertNotNull(out)
        assertTrue(abs((out!!.epochSecond - now.epochSecond) - 0.875 * 86_400) < 2.0)

        val early = QuotaWindow("5h", used = 60.0, resetsAt = now.plusSeconds(4 * 3600))
        assertNull(Quotas.runsOut(early, now))
        val light = QuotaWindow("5h", used = 19.0, resetsAt = now.plusSeconds((3.75 * 3600).toLong()))
        assertNull(Quotas.runsOut(light, now))
        val fast = QuotaWindow("5h", used = 26.0, resetsAt = now.plusSeconds((3.75 * 3600).toLong()))
        assertNotNull(Quotas.runsOut(fast, now))
    }

    @Test
    fun formattingMatchesIosAndWeb() {
        assertEquals("0%", Fmt.percent(0.0))
        assertEquals("<1%", Fmt.percent(0.3))
        assertEquals(">99%", Fmt.percent(99.7))
        assertEquals("100%", Fmt.percent(100.0))
        assertEquals("47%", Fmt.percent(47.4))

        assertEquals("3d 7h", Fmt.compactLeft(now.plusSeconds(3 * 86_400 + 7 * 3600 + 120), now))
        assertEquals("2d", Fmt.compactLeft(now.plusSeconds(2 * 86_400 + 30), now))
        assertEquals("2h 14m", Fmt.compactLeft(now.plusSeconds(2 * 3600 + 14 * 60), now))
        assertEquals("1h", Fmt.compactLeft(now.plusSeconds(3600), now))
        assertEquals("<1m", Fmt.compactLeft(now.plusSeconds(30), now))
        assertEquals("now", Fmt.compactLeft(now.minusSeconds(5), now))
        assertEquals("1h 41m", Fmt.left(now.plusSeconds(3600 + 41 * 60), now))
        assertEquals("1h 05m", Fmt.left(now.plusSeconds(3600 + 5 * 60), now))

        assertEquals("950", Fmt.count(950))
        assertEquals("12.3k", Fmt.count(12_300))
        assertEquals("124k", Fmt.count(124_000))
        assertEquals("2.25M", Fmt.count(2_250_000))
        assertEquals("42.8M", Fmt.count(42_753_262))
        assertEquals("850ms", Fmt.ms(850))
        assertEquals("7.5s", Fmt.ms(7514))

        assertEquals("Unpriced", Fmt.dollars(null))
        assertEquals("$0.00", Fmt.dollars(0L))
        assertEquals("$1,234.56", Fmt.dollars(1_234_560_000_000L))
        assertEquals("$0.0123", Fmt.dollars(12_345_000L))
    }

    @Test
    fun accountStatesRenewalsAndMeta() {
        var a = Account(id = "a", provider = "claude", label = "user@example.com")
        assertEquals(AccountState.Kind.READY, a.state(now).kind)
        a = a.copy(cooldowns = mapOf("*" to "2026-11-01T13:41:00Z"), cooldownKinds = mapOf("*" to "quota"))
        assertEquals("Cooling 1h 41m", a.statusWord(0, now))
        a = a.copy(cooldowns = null, lastError = "invalid_grant: refresh token expired")
        assertTrue(a.state(now).signin)
        assertEquals("Disabled", a.copy(disabled = true).state(now).word)

        val apiKey = Account(
            id = "k",
            provider = "claude",
            label = "key",
            kind = "api-key",
            cooldownKinds = mapOf("*" to "credit", "claude-opus-5-5" to "rate_limit", "claude-sonnet-5-5" to "quota", "x" to "checking"),
        )
        assertEquals("API credit used up", apiKey.cooldownReason("*"))
        assertEquals("Rate limited", apiKey.cooldownReason("claude-opus-5-5"))
        assertEquals("Usage limit used up", apiKey.cooldownReason("claude-sonnet-5-5"))
        assertEquals("Checking quota", apiKey.cooldownReason("x"))

        val meta = Account(id = "m", provider = "meta", label = "u", quota = Quota(windows = emptyList()))
        assertTrue(meta.metered)
        assertEquals("Meta · Device code", meta.subtitle)
        assertFalse(Account(id = "mk", provider = "meta", label = "k", kind = "api-key", quota = Quota(windows = emptyList())).metered)

        val zone = ZoneId.of("UTC")
        val today = LocalDate.of(2026, 11, 1)
        val noon = today.atTime(12, 0).atZone(zone).toInstant()
        val in19 = today.plusDays(19).atTime(9, 0).atZone(zone).toInstant()
        val manual = Account(id = "c", provider = "claude", label = "u", quota = Quota(renewal = Renewal(at = in19, period = "monthly", manual = true)))
        val row = manual.renewalRow(noon, zone)
        assertEquals("Renews", row?.first)
        assertTrue(row!!.second.endsWith(" · in 19 days · monthly"))
    }

    @Test
    fun antigravityModelFamiliesAndModelWeeks() {
        val json = """
        {"id":"g","provider":"antigravity","label":"g","kind":"oauth","disabled":false,
         "cooldowns":{"claude-opus-5-5-high":"2026-11-08T12:00:00Z"},"cooldown_kinds":{"claude-opus-5-5-high":"quota"},
         "quota":{"windows":[],"plan":"g1-ultra-tier","plan_name":"Google AI Ultra","limits":[
          {"id":"antigravity:Gemini Models:weekly","name":"Gemini Models · weekly","scope":{"kind":"model_families","value":["gemini-","!gemini-*carbon*"]},"used_percent":21,"resets_at":"2026-11-08T12:00:00Z"},
          {"id":"antigravity:Gemini Carbon:weekly","name":"Gemini Carbon · weekly","scope":{"kind":"model_families","value":["gemini-*carbon*"]},"used_percent":100,"resets_at":"2026-11-08T12:00:00Z"},
          {"id":"antigravity:Claude and GPT models:weekly","name":"Claude and GPT models · weekly","scope":{"kind":"model_families","value":["claude-","gpt-"]},"used_percent":81,"resets_at":"2026-11-08T12:00:00Z"},
          {"id":"antigravity:disabled","name":"Disabled bucket · 5h","scope":{"kind":"model_families","value":["claude-"]},"used_percent":0,"enabled":false},
          {"id":"x","name":"Agent credits","scope":{"kind":"credits"},"remaining":0}
         ]}}
        """.trimIndent()
        val a = FuseboxJson.decodeFromString(Account.serializer(), json).normalized()
        assertEquals("Ultra", a.planName)
        assertEquals("Antigravity · Ultra", a.subtitle)
        val mainWeek = a.window(short = false, now = now)!!
        assertEquals(21.0, mainWeek.used!!, 1e-9)
        assertTrue(mainWeek.coversModel("gemini-4-neon-high"))
        assertFalse(mainWeek.coversModel("gemini-4-carbon-high"))
        val scoped = a.scopedWindows(now)
        assertEquals(listOf("Claude & GPT", "Gemini Carbon"), scoped.map { it.model })
        assertTrue(a.windowSpent(scoped[0]))
        assertFalse(a.windowSpent(mainWeek))
        assertEquals(listOf("antigravity:disabled", "x"), a.quota?.limits?.map { it.id })

        // A plan without a 5-hour limit, with a Fable model week
        val ultra = Account(
            id = "a",
            provider = "claude",
            label = "a",
            quota = Quota(
                plan = "max_20x",
                noFiveHourLimit = true,
                windows = listOf(
                    QuotaWindow("week", 20.0, now.plusSeconds(60 * 3600)),
                    QuotaWindow("week fable", 30.0, now.minusSeconds(3600), model = "fable"),
                ),
            ),
        )
        assertTrue(ultra.noShortLimit(now))
    }

    @Test
    fun deepLinksAndFoldableColumnPlans() {
        assertEquals(
            DeepLink.Account("file:claude-user@example.com.json"),
            DeepLink.routePath("#/accounts/file%3Aclaude-user%40example.com.json"),
        )
        assertEquals(DeepLink.Overview, DeepLink.routePath("#/config/notifications"))
        assertEquals(DeepLink.Account("file:claude-user@example.com.json"), DeepLink.routeUri(DeepLink.accountUri("file:claude-user@example.com.json")))
        assertEquals(DeepLink.Faults, DeepLink.routeUri("fusebox://faults"))

        // Compact screen -> 1 column
        assertEquals(1, ColumnPlan.compute(380f, 330f, 16f).columns)
        // Unfolded book foldable with vertical hinge at 380..420dp inside 800dp width -> 2 columns split on hinge
        val folded = ColumnPlan.compute(800f, 330f, 16f, verticalHingeLeftDp = 380f, verticalHingeRightDp = 420f, hingeSeparating = true)
        assertEquals(2, folded.columns)
        assertNotNull(folded.leftFraction)
        assertEquals(40f, folded.hingeGapDp, 0.1f)
        // Narrow screen ignores stale hinge
        val narrow = ColumnPlan.compute(400f, 330f, 16f, verticalHingeLeftDp = 380f, verticalHingeRightDp = 420f, hingeSeparating = true)
        assertEquals(1, narrow.columns)
    }
}
