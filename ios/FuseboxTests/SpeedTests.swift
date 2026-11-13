import Foundation
import Testing
@testable import Fusebox

@Suite("Output speed per model")
struct SpeedTests {
    private func row(_ provider: String, _ model: String, _ speed: String = "standard", calls: Int = 10, timed: Int = 10,
                     output: Int = 1000, streamMs: Int = 10_000, ttftMs: Int = 5000, date: String? = nil) -> UsageSpeed.Row {
        UsageSpeed.Row(provider: provider, model: model, speed: speed, calls: calls, timed: timed, output: output,
                       streamMs: streamMs, ttftMs: ttftMs, date: date)
    }

    @Test func decodesTheServersAnswer() throws {
        let s = try decode(UsageSpeed.self, "usage-speed")
        let models = try #require(s.models)
        #expect(!models.isEmpty)
        for r in models {
            #expect(r.model?.isEmpty == false)
            #expect(["standard", "fast", "ultrafast"].contains(r.speed ?? ""))
            #expect((r.timed ?? 0) <= (r.calls ?? 0))
        }
        #expect(s.trend?.allSatisfy { $0.date?.count == 10 } == true)
        #expect(s.backfilling == false)
    }

    @Test func ratesAreTakenOverSums() throws {
        let c = SpeedSummary(try decode(UsageSpeed.self, "usage-speed"))
        let sol = try #require(c.models.first { $0.model == "gpt-6.1-sol" })
        #expect(sol.tiers.map(\.speed) == ["fast", "standard"], "Fast first")
        for t in sol.tiers {
            let rate = try #require(t.sums.rate)
            #expect(abs(rate - Double(t.sums.output) * 1000 / Double(t.sums.streamMs)) < 1e-9)
        }
        // The model's rate is over both tiers' sums, not the mean of their rates.
        let both = Double(sol.tiers.reduce(0) { $0 + $1.sums.output }) * 1000 / Double(sol.tiers.reduce(0) { $0 + $1.sums.streamMs })
        #expect(abs(try #require(sol.rate) - both) < 1e-9)
        #expect(sol.fastCalls == sol.tiers[0].sums.calls)
        // Only OpenAI asked for Fast, so the share is of OpenAI's calls.
        #expect(c.fastProviders == ["openai"])
        let openai = c.models.filter { $0.provider == "openai" }
        let share = Double(openai.reduce(0) { $0 + $1.fastCalls }) / Double(openai.reduce(0) { $0 + $1.sums.calls })
        #expect(abs(try #require(c.fastShare) - share) < 1e-9)
        let gain = try #require(c.gain)
        #expect(gain.model == "gpt-6.1-sol")
        #expect(abs(gain.ratio - sol.tiers[0].sums.rate! / sol.tiers[1].sums.rate!) < 1e-9)
        #expect(c.calls == c.models.reduce(0) { $0 + $1.sums.calls })
    }

    @Test func picksStartWithTheFourMostTimedModels() throws {
        let c = SpeedSummary(try decode(UsageSpeed.self, "usage-speed"))
        let picks = c.picks(nil)
        let busiest = c.models.sorted { $0.sums.timed > $1.sums.timed }.prefix(4).map(\.key)
        #expect(picks.count == 4)
        #expect(busiest.enumerated().allSatisfy { picks[$1] == $0 })
        // The user's picks stay while their models are in the range; gone ones fall back.
        let mine = [c.models[0].key: 3]
        #expect(c.picks(mine) == mine)
        #expect(c.picks(["gone\u{0}model": 0]) == picks)
    }

    @Test func sortingPutsUntimedModelsLast() {
        let c = SpeedSummary(UsageSpeed(models: [
            row("anthropic", "slow", output: 100, streamMs: 10_000),
            row("anthropic", "untimed", calls: 50, timed: 0, output: 0, streamMs: 0, ttftMs: 0),
            row("openai", "quick", calls: 3, output: 5000, streamMs: 10_000, ttftMs: 30_000),
        ]))
        #expect(c.sorted(by: "rate", provider: "").map(\.model) == ["quick", "slow", "untimed"])
        #expect(c.sorted(by: "first", provider: "").map(\.model) == ["slow", "quick", "untimed"])
        #expect(c.sorted(by: "calls", provider: "").map(\.model) == ["untimed", "slow", "quick"])
        #expect(c.sorted(by: "rate", provider: "openai").map(\.model) == ["quick"])
        #expect(c.maxRate == 500)
        #expect(c.fastShare == nil && c.gain == nil)
    }

    @Test func chartLinesLeaveGapsOnDaysWithoutTimedCalls() {
        let dates = ["2026-11-10", "2026-11-11", "2026-11-12", "2026-11-13"]
        let trend = [
            row("openai", "gpt", date: dates[0]), row("openai", "gpt", date: dates[1]), row("openai", "gpt", date: dates[3]),
            row("openai", "gpt", "fast", output: 2000, date: dates[2]),
            row("anthropic", "not-picked", date: dates[0]),
        ]
        let lines = SpeedLine.lines(trend, dates: dates, picks: ["openai\u{0}gpt": 1], first: false)
        #expect(lines.map(\.speed) == ["fast", "standard"], "one line per tier, Fast first")
        #expect(lines[1].values == [100, 100, nil, 100])
        #expect(lines[1].runs == [[0, 1], [3]])
        #expect(lines[0].runs == [[2]])
        #expect(lines[0].name == "gpt Fast")
        let first = SpeedLine.lines(trend, dates: dates, picks: ["openai\u{0}gpt": 1], first: true)
        #expect(first[1].values[0] == 0.5)
    }

    @Test func wording() {
        #expect(SpeedText.rate(nil) == "—")
        #expect(SpeedText.rate(8.44) == "8.4")
        #expect(SpeedText.rate(1204.4) == "1,204")
        #expect(SpeedText.seconds(1.23) == "1.2 s")
        #expect(SpeedText.seconds(14.4) == "14 s")
        #expect(SpeedTier.name("ultrafast") == "Ultrafast")
    }
}
