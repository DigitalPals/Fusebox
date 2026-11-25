package com.digitalpals.fusebox

import com.digitalpals.fusebox.data.SpeedLine
import com.digitalpals.fusebox.data.SpeedSummary
import com.digitalpals.fusebox.data.SpeedText
import com.digitalpals.fusebox.data.UsageAggregate
import com.digitalpals.fusebox.data.UsageChartModel
import com.digitalpals.fusebox.data.UsageSpeed
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Test

class SpeedAndChartTest {
    @Test
    fun computesSpeedSummaryAndMultiTierGain() {
        val s = UsageSpeed(
            models = listOf(
                UsageSpeed.Row("openai", "gpt-6-astra", "fast", calls = 10, timed = 10, output = 10_000, streamMs = 50_000, ttftMs = 5_000),
                UsageSpeed.Row("openai", "gpt-6-astra", "standard", calls = 10, timed = 10, output = 10_000, streamMs = 100_000, ttftMs = 10_000),
            ),
            trend = listOf(
                UsageSpeed.Row("openai", "gpt-6-astra", "fast", calls = 5, timed = 5, output = 5_000, streamMs = 25_000, date = "2026-11-01"),
                UsageSpeed.Row("openai", "gpt-6-astra", "fast", calls = 5, timed = 5, output = 5_000, streamMs = 25_000, date = "2026-11-03"),
            ),
        )
        val summary = SpeedSummary.from(s)
        assertEquals(1, summary.models.size)
        assertEquals(0.5, summary.fastShare!!, 1e-9)
        assertNotNull(summary.gain)
        assertEquals(2.0, summary.gain!!.second, 1e-9)

        val picks = summary.picks(null)
        val lines = SpeedLine.lines(s.trend!!, listOf("2026-11-01", "2026-11-02", "2026-11-03"), picks, first = false)
        assertEquals(1, lines.size)
        assertEquals(listOf(listOf(0), listOf(2)), lines[0].runs)
        assertEquals("8.4", SpeedText.rate(8.42))
        assertEquals("1,204", SpeedText.rate(1204.2))
        assertEquals("1.2 s", SpeedText.seconds(1.23))
    }

    @Test
    fun usageChartMergesTailGroupsIntoOther() {
        val trend = (1..7).map { i ->
            UsageAggregate(date = "2026-11-01", group = "g$i", observations = i, estimatedCostNanos = i * 1_000_000_000L)
        }
        val chart = UsageChartModel.build(trend, listOf("2026-11-01", "2026-11-02"), calls = false)
        assertEquals(6, chart.keys.size)
        assertEquals(UsageChartModel.OTHER, chart.keys.last())
        assertEquals(0, chart.peak)
    }
}
