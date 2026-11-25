package com.digitalpals.fusebox.data

import java.text.NumberFormat
import java.time.Duration
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.roundToInt
import kotlin.math.roundToLong

/**
 * The dashboard's number and time wording (matching ui/app.js and ios/Shared/Formatting.swift).
 * Clock times always use 24-hour formatting (`HH:mm`, `HH:mm:ss`) in the device's time zone.
 */
object Fmt {
    private val enUS = Locale.US
    private val enGB = Locale.UK

    private val hmFmt = DateTimeFormatter.ofPattern("HH:mm", enGB)
    private val clockFmt = DateTimeFormatter.ofPattern("HH:mm:ss", enGB)
    private val weekdayFmt = DateTimeFormatter.ofPattern("EEE", enGB)
    private val dayMonthFmt = DateTimeFormatter.ofPattern("EEE d MMM", enGB)
    private val fullTimeFmt = DateTimeFormatter.ofPattern("EEEE d MMMM yyyy, HH:mm z", enGB)
    private val shortDateFmt = DateTimeFormatter.ofPattern("d MMM yyyy", enGB)
    private val shortDateTimeFmt = DateTimeFormatter.ofPattern("d MMM, HH:mm", enGB)

    /** "950", "12.3k", "124k", "2.25M", "1.5B". */
    fun count(value: Number?): String {
        val n = value?.toDouble() ?: 0.0
        if (n < 1000.0) return NumberFormat.getIntegerInstance(enUS).format(n.toLong())
        if (n < 1e6) {
            val s = String.format(enUS, if (n < 1e5) "%.1f" else "%.0f", n / 1e3)
            return trimPointZero(s) + "k"
        }
        if (n < 1e9) {
            val digits = when {
                n < 1e7 -> 2
                n < 1e8 -> 1
                else -> 0
            }
            return trimZeros(String.format(enUS, "%.${digits}f", n / 1e6)) + "M"
        }
        return trimZeros(String.format(enUS, "%.2f", n / 1e9)) + "B"
    }

    private fun trimPointZero(s: String): String = if (s.endsWith(".0")) s.dropLast(2) else s

    private fun trimZeros(s: String): String {
        if (!s.contains('.')) return s
        var t = s
        while (t.endsWith('0')) t = t.dropLast(1)
        if (t.endsWith('.')) t = t.dropLast(1)
        return t
    }

    /** Latency: "850ms", "1.2s", "14s", "2m 5s". */
    fun ms(v: Int?): String {
        if (v == null) return "—"
        if (v < 1000) return "${v}ms"
        if (v < 60_000) {
            return String.format(enUS, if (v < 10_000) "%.1fs" else "%.0fs", v / 1000.0)
        }
        return "${v / 60_000}m ${((v % 60_000) / 1000.0).roundToInt()}s"
    }

    /** "just now", "45s ago", "12m ago", "3h ago", "2d ago", "never". */
    fun ago(instant: Instant?, now: Instant = Instant.now()): String {
        if (instant == null) return "never"
        val s = max(0L, Duration.between(instant, now).seconds)
        return when {
            s < 10 -> "just now"
            s < 60 -> "${s}s ago"
            s < 3600 -> "${s / 60}m ago"
            s < 86_400 -> "${s / 3600}h ago"
            else -> "${s / 86_400}d ago"
        }
    }

    /** Time left: "2d 22h", "1h 41m", "41m", "45s". */
    fun left(instant: Instant, now: Instant = Instant.now()): String {
        val diffMs = Duration.between(now, instant).toMillis()
        val s = max(0L, (diffMs / 1000.0).roundToLong())
        return when {
            s >= 86_400 -> "${s / 86_400}d ${(s % 86_400) / 3600}h"
            s >= 3600 -> "${s / 3600}h ${String.format(enUS, "%02d", (s % 3600) / 60)}m"
            s >= 60 -> "${s / 60}m"
            else -> "${s}s"
        }
    }

    /** The meters' countdown: at most two units, no zero suffix: "3d 7h", "2h 14m", "41m", "<1m", "now". */
    fun compactLeft(instant: Instant, now: Instant = Instant.now()): String {
        val remainingSec = Duration.between(now, instant).toMillis() / 1000.0
        if (remainingSec <= 0.0) return "now"
        val minutes = (remainingSec / 60.0).toLong()
        if (minutes >= 1440) {
            val hours = (minutes % 1440) / 60
            return "${minutes / 1440}d" + if (hours > 0) " ${hours}h" else ""
        }
        if (minutes >= 60) {
            val rest = minutes % 60
            return "${minutes / 60}h" + if (rest > 0) " ${rest}m" else ""
        }
        return if (minutes > 0) "${minutes}m" else "<1m"
    }

    /** A duration: "45s", "12m", "5h 11m", "2d 3h". */
    fun span(secs: Double): String {
        val s = max(0L, secs.toLong())
        return when {
            s < 60 -> "${s}s"
            s < 3600 -> "${s / 60}m"
            s < 86_400 -> "${s / 3600}h ${(s % 3600) / 60}m"
            else -> "${s / 86_400}d ${(s % 86_400) / 3600}h"
        }
    }

    fun hm(instant: Instant, zone: ZoneId = ZoneId.systemDefault()): String =
        hmFmt.format(instant.atZone(zone))

    fun clock(instant: Instant, zone: ZoneId = ZoneId.systemDefault()): String =
        clockFmt.format(instant.atZone(zone))

    fun weekday(instant: Instant, zone: ZoneId = ZoneId.systemDefault()): String =
        weekdayFmt.format(instant.atZone(zone))

    fun dayMonth(instant: Instant, zone: ZoneId = ZoneId.systemDefault()): String =
        dayMonthFmt.format(instant.atZone(zone))

    fun shortDate(instant: Instant, zone: ZoneId = ZoneId.systemDefault()): String =
        shortDateFmt.format(instant.atZone(zone))

    fun shortDateTime(instant: Instant, zone: ZoneId = ZoneId.systemDefault()): String =
        shortDateTimeFmt.format(instant.atZone(zone))

    /** Within 20 hours "20:09", otherwise "Fri 14:58". */
    fun whenText(instant: Instant, now: Instant = Instant.now(), zone: ZoneId = ZoneId.systemDefault()): String {
        val diffSec = Duration.between(now, instant).seconds
        return if (diffSec < 20 * 3600) hm(instant, zone) else "${weekday(instant, zone)} ${hm(instant, zone)}"
    }

    fun fullTime(instant: Instant, zone: ZoneId = ZoneId.systemDefault()): String =
        fullTimeFmt.format(instant.atZone(zone))

    /** Whole percentages whose ends never round onto 0% or 100% unless they are: "<1%", ">99%". */
    fun percent(value: Double): String {
        val rounded = value.roundToInt()
        if (value > 0.0 && rounded == 0) return "<1%"
        if (value < 100.0 && rounded == 100) return ">99%"
        return "$rounded%"
    }

    /** "1 request", "3 requests". */
    fun plural(n: Int, word: String, many: String? = null): String =
        "${count(n)} ${if (n == 1) word else (many ?: "${word}s")}"

    /** Usage money: USD nanounits; cents above a dollar, four places below. null reads "Unpriced". */
    fun dollars(nanos: Long?): String {
        if (nanos == null) return "Unpriced"
        val v = nanos / 1e9
        val small = v != 0.0 && abs(v) < 1.0
        val fmt = NumberFormat.getCurrencyInstance(enUS).apply {
            minimumFractionDigits = if (small) 4 else 2
            maximumFractionDigits = if (small) 4 else 2
        }
        return fmt.format(v)
    }

    /** Usage's short dollars for bar labels: "$1,234", "$4.20", "$0". */
    fun dollarsShort(v: Double): String {
        if (v >= 10.0) return "$" + NumberFormat.getIntegerInstance(enUS).format(v.roundToLong())
        return if (v > 0.0) String.format(enUS, "$%.2f", v) else "$0"
    }

    /** Usage's compact token counts: "512", "3.4k", "12.0M", "1.23B". */
    fun compact(v: Number?): String {
        if (v == null) return "Unknown"
        val n = v.toDouble()
        return when {
            n >= 1e9 -> String.format(enUS, "%.2fB", n / 1e9)
            n >= 1e6 -> String.format(enUS, "%.1fM", n / 1e6)
            n >= 1e3 -> String.format(enUS, "%.1fk", n / 1e3)
            else -> n.roundToLong().toString()
        }
    }

    fun capitalized(s: String): String =
        if (s.isEmpty()) s else s.substring(0, 1).uppercase(Locale.US) + s.substring(1)
}
