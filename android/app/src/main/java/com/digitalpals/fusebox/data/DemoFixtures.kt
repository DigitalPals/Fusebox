package com.digitalpals.fusebox.data

import java.time.Instant
import java.time.LocalDate
import java.time.format.DateTimeFormatter

/**
 * Anonymized sample data used solely by Debug builds launched with `--ez demo true`
 * (e.g. emulator UI checks and foldable before/after screenshots).
 */
object DemoFixtures {
    data class Bundle(
        val overview: Overview,
        val accounts: List<Account>,
        val faults: List<Fault>,
        val requests: List<RequestLog>,
        val activity: Map<String, AccountActivity>,
        val usage: UsageDashboard,
        val speed: UsageSpeed,
    )

    fun build(now: Instant = Instant.now()): Bundle {
        val nowMin = now.epochSecond / 60
        val series = (0 until 60).map { i ->
            val m = nowMin - 59 + i
            val base = ((i * 7 + 3) % 14) + 2
            val fail = if (i == 42 || i == 55) 1 else 0
            MinuteBucket(minute = m, requests = base, failed = fail, tokens = base * 9_400L)
        }

        val claude1 = Account(
            id = "file:claude-primary.json",
            provider = "claude",
            label = "alex@studio.example",
            kind = "oauth",
            counters = Counters(requests = 1482, ok = 1479, failures = 3, cancelled = 2, inputTokens = 14_200_000, outputTokens = 1_850_000, cacheTokens = 9_600_000),
            lastUsed = now.minusSeconds(18),
            expiresAt = now.plusSeconds(5 * 3600 + 22 * 60),
            priority = 0,
            quota = Quota(
                plan = "max_20x",
                noFiveHourLimit = true,
                windows = listOf(
                    QuotaWindow(name = "week", used = 62.0, resetsAt = now.plusSeconds(2 * 86_400 + 11 * 3600)),
                    QuotaWindow(name = "week fable", used = 84.0, resetsAt = now.plusSeconds(2 * 86_400 + 11 * 3600), model = "fable"),
                ),
                renewal = Renewal(at = now.plusSeconds(14 * 86_400), period = "monthly", manual = true),
            ),
            bankedResets = BankedResets(
                checkedAt = now.minusSeconds(320),
                inventory = BankedResets.Inventory(
                    available = 2,
                    eligible = true,
                    grants = listOf(
                        BankedResets.Grant(
                            id = "grant-1",
                            label = "Monthly reset",
                            remaining = 2,
                            total = 2,
                            expiresAt = now.plusSeconds(9 * 86_400),
                            clears = listOf("weekly"),
                            usable = true,
                        ),
                    ),
                ),
            ),
            bankedResetPlan = BankedResetPlan(autoRedeem = true, prioritized = false, message = "Waits until the pool is used up."),
        )

        val claude2 = Account(
            id = "file:claude-work.json",
            provider = "claude",
            label = "work@team.example",
            kind = "oauth",
            counters = Counters(requests = 914, ok = 910, failures = 4, inputTokens = 8_100_000, outputTokens = 1_120_000, cacheTokens = 5_400_000),
            lastUsed = now.minusSeconds(420),
            expiresAt = now.plusSeconds(3 * 3600),
            quota = Quota(
                plan = "max_5x",
                windows = listOf(
                    QuotaWindow(name = "5h", used = 44.0, resetsAt = now.plusSeconds(2 * 3600 + 48 * 60)),
                    QuotaWindow(name = "week", used = 78.0, resetsAt = now.plusSeconds(4 * 86_400 + 6 * 3600)),
                ),
            ),
        )

        val codex1 = Account(
            id = "file:codex-main.json",
            provider = "codex",
            label = "dev@codex.example",
            kind = "oauth",
            counters = Counters(requests = 2110, ok = 2098, failures = 12, inputTokens = 18_900_000, outputTokens = 2_310_000, cacheTokens = 12_100_000),
            lastUsed = now.minusSeconds(95),
            expiresAt = now.plusSeconds(8 * 3600),
            cooldowns = mapOf("gpt-6-astra" to FuseboxDate.format(now.plusSeconds(14 * 60))),
            cooldownKinds = mapOf("gpt-6-astra" to "rate_limit"),
            quota = Quota(
                subscriptionPlan = "chatgptpro",
                windows = listOf(
                    QuotaWindow(name = "5h", used = 82.0, resetsAt = now.plusSeconds(1 * 3600 + 25 * 60)),
                    QuotaWindow(name = "week", used = 39.0, resetsAt = now.plusSeconds(5 * 86_400 + 3 * 3600)),
                ),
                renewal = Renewal(at = now.plusSeconds(6 * 86_400 + 4 * 3600), period = "monthly"),
            ),
        )

        val antigravity1 = Account(
            id = "file:antigravity-pro.json",
            provider = "antigravity",
            label = "gemini@cloud.example",
            kind = "oauth",
            counters = Counters(requests = 620, ok = 620, failures = 0, inputTokens = 5_300_000, outputTokens = 710_000),
            lastUsed = now.minusSeconds(210),
            quota = Quota(
                plan = "g1-ultra-tier",
                planName = "Google AI Ultra",
                limits = listOf(
                    QuotaLimit(id = "ag:gem:5h", name = "Gemini Models · 5h", scope = QuotaLimit.Scope("model_families", listOf("gemini-")), usedPercent = 24.0, resetsAt = now.plusSeconds(3 * 3600 + 10 * 60)),
                    QuotaLimit(id = "ag:gem:wk", name = "Gemini Models · weekly", scope = QuotaLimit.Scope("model_families", listOf("gemini-")), usedPercent = 31.0, resetsAt = now.plusSeconds(5 * 86_400)),
                    QuotaLimit(id = "ag:cg:5h", name = "Claude and GPT models · 5h", scope = QuotaLimit.Scope("model_families", listOf("claude-", "gpt-")), usedPercent = 58.0, resetsAt = now.plusSeconds(2 * 3600)),
                    QuotaLimit(id = "ag:cg:wk", name = "Claude and GPT models · weekly", scope = QuotaLimit.Scope("model_families", listOf("claude-", "gpt-")), usedPercent = 47.0, resetsAt = now.plusSeconds(5 * 86_400)),
                ),
            ),
        ).normalized()

        val apiKey = Account(
            id = "key:gemini:1",
            provider = "gemini",
            label = "Gemini API Key",
            kind = "api-key",
            counters = Counters(requests = 340, ok = 340),
            lastUsed = now.minusSeconds(1400),
        )

        val accounts = listOf(claude1, claude2, codex1, antigravity1, apiKey)

        val overview = Overview(
            version = "0.18.0",
            startedAt = now.minusSeconds(4 * 86_400),
            routing = "smart-quota",
            bankedResets = true,
            sessionAffinity = true,
            managementKey = true,
            active = 2,
            series = series,
            pools = listOf(
                Pool(
                    provider = "claude",
                    window = "week",
                    accounts = 2,
                    accountIDs = listOf(claude1.id, claude2.id),
                    reported = 2,
                    used = 67.3,
                    pace = 0.54,
                    priced = true,
                    outlook = Pool.Outlook(state = "tight", nextResetAt = now.plusSeconds(2 * 86_400 + 11 * 3600)),
                ),
                Pool(
                    provider = "claude",
                    window = "week fable",
                    model = "fable",
                    accounts = 2,
                    accountIDs = listOf(claude1.id, claude2.id),
                    reported = 2,
                    used = 84.0,
                    pace = 0.61,
                    priced = true,
                ),
                Pool(
                    provider = "codex",
                    window = "week",
                    accounts = 1,
                    accountIDs = listOf(codex1.id),
                    reported = 1,
                    used = 39.0,
                    pace = 0.48,
                    priced = false,
                    outlook = Pool.Outlook(state = "on_track", nextResetAt = now.plusSeconds(5 * 86_400 + 3 * 3600)),
                ),
            ),
        )

        val faults = listOf(
            Fault(
                key = "rate:${codex1.id}:gpt-6-astra",
                kind = "rate_limit",
                level = "warn",
                provider = "codex",
                providerName = "Codex",
                accountID = codex1.id,
                label = codex1.label,
                title = "gpt-6-astra rate limited",
                detail = "Requests for gpt-6-astra are routing to other accounts until cooldown clears.",
                until = now.plusSeconds(14 * 60),
                path = "#/accounts/${ConnectionHelper.encodePathSegment(codex1.id)}",
            ),
            Fault(
                key = "pace:pool:claude:week",
                kind = "pace",
                level = "warn",
                provider = "claude",
                providerName = "Claude",
                title = "Claude weekly pace is ahead of renewal",
                detail = "Work laptop used 64% of the window.",
                until = now.plusSeconds(2 * 86_400 + 11 * 3600),
                badge = false,
            ),
        )

        val requests = listOf(
            RequestLog(
                id = 901,
                ts = now.minusSeconds(18),
                client = "claude",
                clientApp = "Claude Code 2.1",
                keyLabel = "Work laptop",
                provider = "claude",
                model = "claude-opus-5-5",
                account = claude1.label,
                accountID = claude1.id,
                sessionID = "a8f41c09e27d41bb",
                routingStrategy = "smart-quota",
                routingReason = "session_reused",
                status = 200,
                latencyMs = 6420,
                ttftMs = 820,
                inputTokens = 42_100,
                outputTokens = 1_940,
                cacheTokens = 38_400,
                usageCompleteness = "complete",
                source = RequestLog.Source(ip = "100.64.0.12", via = "forwarded", login = "john@example.ts.net"),
                cachePolicy = RequestLog.CachePolicy("explicit", "explicit", "session"),
            ),
            RequestLog(
                id = 900,
                ts = now.minusSeconds(75),
                client = "responses",
                clientApp = "Codex CLI",
                keyLabel = "The Dragon",
                provider = "codex",
                model = "gpt-6-astra",
                account = codex1.label,
                accountID = codex1.id,
                sessionID = "c3d09b12f941aa20",
                routingStrategy = "smart-quota",
                routingReason = "temporary_detour",
                status = 200,
                latencyMs = 4150,
                ttftMs = 610,
                inputTokens = 18_500,
                outputTokens = 1_120,
                cacheTokens = 12_000,
                transport = "ws",
                attempts = 2,
                usageCompleteness = "complete",
            ),
            RequestLog(
                id = 899,
                ts = now.minusSeconds(210),
                client = "gemini",
                clientApp = "Antigravity",
                keyLabel = "The Dragon",
                provider = "antigravity",
                model = "gemini-4-neon",
                account = antigravity1.label,
                accountID = antigravity1.id,
                sessionID = "9b221e4a071c44de",
                routingStrategy = "smart-quota",
                routingReason = "new_session",
                status = 200,
                latencyMs = 3190,
                ttftMs = 490,
                inputTokens = 26_000,
                outputTokens = 1_480,
                cacheTokens = 19_200,
                usageCompleteness = "complete",
            ),
            RequestLog(
                id = 898,
                ts = now.minusSeconds(410),
                client = "claude",
                clientApp = "Claude Code 2.1",
                keyLabel = "Work laptop",
                provider = "claude",
                model = "claude-sonnet-5-5",
                account = claude2.label,
                accountID = claude2.id,
                routingStrategy = "smart-quota",
                routingReason = "quota_exhausted",
                status = 429,
                latencyMs = 540,
                attempts = 2,
                error = "Upstream returned 429 rate_limit_error; retried on backup account.",
            ),
        )

        val actMap = mapOf(
            claude1.id to AccountActivity(
                series = series.takeLast(60),
                sessions = listOf(
                    AccountActivity.Session(
                        session = "a8f41c09e27d41bb",
                        active = true,
                        clientApp = "Claude Code 2.1",
                        keyLabel = "Work laptop",
                        model = "claude-opus-5-5",
                        requests = 48,
                        lastSeen = now.minusSeconds(18),
                    ),
                ),
            ),
        )

        val today = LocalDate.now()
        val dates = (0 until 7).map { today.minusDays((6 - it).toLong()).format(DateTimeFormatter.ISO_LOCAL_DATE) }
        val provTrend = dates.flatMapIndexed { i, d ->
            listOf(
                UsageAggregate(date = d, group = "anthropic", observations = 140 + i * 18, estimatedCostNanos = (18_500_000_000L + i * 3_200_000_000L)),
                UsageAggregate(date = d, group = "openai", observations = 110 + i * 12, estimatedCostNanos = (11_200_000_000L + i * 1_900_000_000L)),
                UsageAggregate(date = d, group = "google", observations = 65 + i * 9, estimatedCostNanos = (4_400_000_000L + i * 850_000_000L)),
            )
        }
        val usage = UsageDashboard(
            combined = UsageDashboard.Combined(
                totals = UsageAggregate(
                    observations = 2480,
                    estimatedCostNanos = 312_450_000_000L,
                    unpriced = 12,
                    tokens = UsageTokens(input = 41_200_000, output = 5_840_000, cacheRead = 28_600_000, cacheWrite = 4_100_000),
                ),
                trends = UsageDashboard.Trends(provider = provTrend, model = provTrend),
                breakdowns = UsageDashboard.Breakdowns(
                    model = listOf(
                        UsageAggregate(id = "claude-opus-5-5", provider = "anthropic", observations = 920, estimatedCostNanos = 164_200_000_000L),
                        UsageAggregate(id = "gpt-6-astra", provider = "openai", observations = 840, estimatedCostNanos = 92_150_000_000L),
                        UsageAggregate(id = "gemini-4-neon", provider = "google", observations = 720, estimatedCostNanos = 56_100_000_000L),
                    ),
                    provider = listOf(
                        UsageAggregate(id = "anthropic", accounts = 2, observations = 920, estimatedCostNanos = 164_200_000_000L),
                        UsageAggregate(id = "openai", accounts = 1, observations = 840, estimatedCostNanos = 92_150_000_000L),
                        UsageAggregate(id = "google", accounts = 2, observations = 720, estimatedCostNanos = 56_100_000_000L),
                    ),
                    account = listOf(
                        UsageAggregate(id = claude1.id, provider = "anthropic", observations = 610, estimatedCostNanos = 112_800_000_000L),
                        UsageAggregate(id = codex1.id, provider = "openai", observations = 840, estimatedCostNanos = 92_150_000_000L),
                    ),
                    client = listOf(
                        UsageAggregate(id = "Work laptop", observations = 1420, estimatedCostNanos = 198_400_000_000L),
                        UsageAggregate(id = "The Dragon", observations = 1060, estimatedCostNanos = 114_050_000_000L),
                    ),
                ),
            ),
        )
        val speed = UsageSpeed(
            models = listOf(
                UsageSpeed.Row(provider = "anthropic", model = "claude-opus-5-5", speed = "standard", calls = 920, timed = 900, output = 1_850_000, streamMs = 21_500_000, ttftMs = 990_000),
                UsageSpeed.Row(provider = "openai", model = "gpt-6-astra", speed = "fast", calls = 460, timed = 450, output = 920_000, streamMs = 5_100_000, ttftMs = 270_000),
                UsageSpeed.Row(provider = "openai", model = "gpt-6-astra", speed = "standard", calls = 380, timed = 370, output = 740_000, streamMs = 8_200_000, ttftMs = 420_000),
                UsageSpeed.Row(provider = "google", model = "gemini-4-neon", speed = "standard", calls = 720, timed = 710, output = 1_310_000, streamMs = 9_400_000, ttftMs = 380_000),
            ),
            trend = dates.flatMapIndexed { i, d ->
                listOf(
                    UsageSpeed.Row(date = d, provider = "anthropic", model = "claude-opus-5-5", speed = "standard", calls = 120, timed = 115, output = 250_000, streamMs = (2_900_000L - i * 40_000L), ttftMs = 125_000),
                    UsageSpeed.Row(date = d, provider = "openai", model = "gpt-6-astra", speed = "fast", calls = 65, timed = 64, output = 135_000, streamMs = (760_000L - i * 12_000L), ttftMs = 38_000),
                    UsageSpeed.Row(date = d, provider = "google", model = "gemini-4-neon", speed = "standard", calls = 95, timed = 90, output = 180_000, streamMs = (1_320_000L - i * 18_000L), ttftMs = 49_000),
                )
            },
        )

        return Bundle(overview, accounts, faults, requests, actMap, usage, speed)
    }
}
