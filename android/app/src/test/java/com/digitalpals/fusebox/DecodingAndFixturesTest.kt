package com.digitalpals.fusebox

import com.digitalpals.fusebox.data.Account
import com.digitalpals.fusebox.data.AccountActivity
import com.digitalpals.fusebox.data.ConnectionHelper
import com.digitalpals.fusebox.data.Fault
import com.digitalpals.fusebox.data.FuseboxDate
import com.digitalpals.fusebox.data.FuseboxJson
import com.digitalpals.fusebox.data.LiveMessage
import com.digitalpals.fusebox.data.Overview
import com.digitalpals.fusebox.data.PushStatus
import com.digitalpals.fusebox.data.RequestLog
import com.digitalpals.fusebox.data.UsageDashboard
import com.digitalpals.fusebox.data.UsageSpeed
import com.digitalpals.fusebox.data.WeeklyPool
import kotlinx.serialization.builtins.ListSerializer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class DecodingAndFixturesTest {
    private fun readFixture(name: String): String {
        val candidates = listOf(
            File("../ios/FuseboxTests/Fixtures/$name.json"),
            File("../../ios/FuseboxTests/Fixtures/$name.json"),
        )
        val f = candidates.firstOrNull { it.isFile }
            ?: error("Fixture $name.json not found from ${File(".").absolutePath}")
        return f.readText()
    }

    @Test
    fun decodesAllSharedProductionFixtures() {
        val overview = FuseboxJson.decodeFromString(Overview.serializer(), readFixture("overview"))
        assertNotNull(overview.version)

        val accounts = FuseboxJson.decodeFromString(ListSerializer(Account.serializer()), readFixture("accounts"))
            .map { it.normalized() }
        assertTrue(accounts.isNotEmpty())

        val faults = FuseboxJson.decodeFromString(ListSerializer(Fault.serializer()), readFixture("faults"))
        assertNotNull(faults)

        val requests = FuseboxJson.decodeFromString(ListSerializer(RequestLog.serializer()), readFixture("requests"))
        assertTrue(requests.isNotEmpty())

        val activity = FuseboxJson.decodeFromString(AccountActivity.serializer(), readFixture("activity"))
        assertNotNull(activity)

        val push = FuseboxJson.decodeFromString(PushStatus.serializer(), readFixture("push"))
        assertNotNull(push)

        val usage = FuseboxJson.decodeFromString(UsageDashboard.serializer(), readFixture("usage-dashboard"))
        assertNotNull(usage.combined)

        val speed = FuseboxJson.decodeFromString(UsageSpeed.serializer(), readFixture("usage-speed"))
        assertNotNull(speed)

        // Ensure weekly pools resolve cleanly against fixture accounts
        val pools = WeeklyPool.weekly(overview, accounts)
        assertNotNull(pools)
    }

    @Test
    fun parsesRfc3339WithNanosecondsAndOffsets() {
        val a = FuseboxDate.parse("2026-11-01T12:00:54.326731560Z")
        val b = FuseboxDate.parse("2026-11-01T14:00:54.326731560+02:00")
        assertNotNull(a)
        assertEquals(a, b)
        assertNotNull(FuseboxDate.parse("2026-11-01 12:00:00Z"))
        assertNull(FuseboxDate.parse("not-a-date"))
    }

    @Test
    fun decodesLiveMessages() {
        val faultsRaw = """{"type":"faults","data":[{"key":"k1","kind":"signin","level":"err","title":"Expired"}],"dismissed":[{"key":"k2","kind":"quota","level":"warn","title":"Used"}]}"""
        val msg = LiveMessage.decode(faultsRaw) as LiveMessage.Faults
        assertEquals(1, msg.faults.size)
        assertEquals(1, msg.dismissed?.size)
        assertEquals("k2", msg.dismissed?.first()?.key)

        val accountsMsg = LiveMessage.decode("""{"type":"accounts"}""")
        assertEquals(LiveMessage.Accounts, accountsMsg)

        val loadMsg = LiveMessage.decode("""{"type":"load","data":{"a1":{"in_flight":2,"sessions":3}}}""") as LiveMessage.Load
        assertEquals(2, loadMsg.load["a1"]?.inFlight)
        assertEquals(3, loadMsg.load["a1"]?.sessions)
    }

    @Test
    fun normalizesServerUrls() {
        assertEquals("https://fusebox.example.ts.net", ConnectionHelper.normalize("fusebox.example.ts.net/"))
        assertEquals("http://127.0.0.1:8317", ConnectionHelper.normalize(" http://127.0.0.1:8317/// "))
        assertNull(ConnectionHelper.normalize(""))
        assertNull(ConnectionHelper.normalize("ftp://example.com"))
    }
}
