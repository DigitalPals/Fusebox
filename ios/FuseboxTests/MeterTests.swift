import CoreGraphics
import Foundation
import Testing
@testable import Fusebox

/// The segmented meter's maths, as the dashboard draws it (ui/app.js `meterHTML`, DESIGN.md
/// "Segmented quota meter" and "Pace mark").
@Suite("Segmented meter")
struct MeterTests {
    private func model(_ used: Double, pace: Double? = nil, _ mode: QuotaDisplay = .used) -> MeterModel {
        MeterModel(QuotaReading(used: used), pace: pace, mode: mode)
    }

    @Test(arguments: [(0.0, 0), (2.4, 0), (2.5, 1), (5, 1), (47, 9), (47.5, 10), (50, 10), (97.4, 19), (97.5, 20), (100, 20)])
    func litSegmentsShowTheUsedShareInFives(_ used: Double, _ lit: Int) {
        #expect(model(used).lit == lit)
    }

    @Test func leftModeLightsWhatIsLeft() {
        #expect(model(30, .remaining).lit == 14)
        #expect(model(100, .remaining).lit == 0)
        #expect(model(0, .remaining).lit == 20)
        #expect(model(82, .remaining).lit == 4)
    }

    /// `meter` below 75% used, `warn` from 75%, `err` from 95%, in either mode: the colour
    /// always measures what's used, even when the bar shows what's left.
    @Test(arguments: [QuotaDisplay.used, .remaining])
    func coloursFollowWhatIsUsedInBothModes(_ mode: QuotaDisplay) {
        #expect(model(0, mode).level == .normal)
        #expect(model(74.9, mode).level == .normal)
        #expect(model(75, mode).level == .warn)
        #expect(model(94.9, mode).level == .warn)
        #expect(model(95, mode).level == .err)
        #expect(model(100, mode).level == .err)
    }

    @Test func segmentsLightFromTheLeadingEdge() {
        let m = model(15)
        #expect((0..<20).filter(m.isLit) == [0, 1, 2])
    }

    @Test func notReportedLightsNothingAndHasNoPace() {
        let m = MeterModel(nil, pace: 0.5, mode: .used)
        #expect(m.lit == 0)
        #expect(m.level == nil)
        #expect(m.pace == nil)
    }

    @Test func paceSitsAtTheElapsedFraction() {
        #expect(model(10, pace: 0.4).pace == 0.4)
        #expect(model(10, pace: 0).pace == 0)
        #expect(model(10, pace: 1).pace == 1)
    }

    @Test func paceIsMirroredInLeftMode() throws {
        #expect(try #require(model(10, pace: 0.4, .remaining).pace).isApprox(0.6))
        #expect(model(10, pace: 0, .remaining).pace == 1)
        #expect(model(10, pace: 1, .remaining).pace == 0)
    }

    @Test func paceIsClampedToTheBar() {
        #expect(model(10, pace: -0.2).pace == 0)
        #expect(model(10, pace: 1.3).pace == 1)
        #expect(model(10, pace: .nan).pace == nil)
        #expect(model(10, pace: nil).pace == nil)
    }

    @Test func paceFromAWindowsElapsedTime() throws {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        // A 5-hour window with 3 hours to go: 40% of it has passed.
        let w = QuotaWindow(name: "5h", used: 30, resetsAt: now.addingTimeInterval(3 * 3600))
        let t = try #require(Quotas.elapsed(w, now: now))
        #expect(t.isApprox(0.4))
        #expect(try #require(MeterModel(QuotaReading(w, now: now), pace: t, mode: .used).pace).isApprox(0.4))
        #expect(try #require(MeterModel(QuotaReading(w, now: now), pace: t, mode: .remaining).pace).isApprox(0.6))
    }

    // MARK: Runs out early ("out 1h 40m")

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    @Test func runsOutWhenThePaceUsesTheWindowUpBeforeItResets() throws {
        // Weekly window, half gone (3.5 days left), 80% used: 20% more at 80% per 3.5 days
        // runs out in 0.875 days, before the reset.
        let w = QuotaWindow(name: "week", used: 80, resetsAt: now.addingTimeInterval(3.5 * 86400))
        let out = try #require(Quotas.runsOut(w, now: now))
        #expect(abs(out.timeIntervalSince(now) - 0.875 * 86400) < 1)
    }

    @Test func lastsWhenOnOrUnderPace() {
        let w = QuotaWindow(name: "week", used: 50, resetsAt: now.addingTimeInterval(3.5 * 86400))
        #expect(Quotas.runsOut(w, now: now) == nil)
        let under = QuotaWindow(name: "week", used: 30, resetsAt: now.addingTimeInterval(3.5 * 86400))
        #expect(Quotas.runsOut(under, now: now) == nil)
    }

    @Test func waitsForAQuarterOfTheWindowAndTwentyPercent() {
        // Early in the window, even a fast start isn't a forecast yet.
        let early = QuotaWindow(name: "5h", used: 60, resetsAt: now.addingTimeInterval(4 * 3600))
        #expect(Quotas.runsOut(early, now: now) == nil)
        // Under 20% used, never (19% after a quarter of a 5-hour window).
        let light = QuotaWindow(name: "5h", used: 19, resetsAt: now.addingTimeInterval(3.75 * 3600))
        #expect(Quotas.runsOut(light, now: now) == nil)
        // 26% a quarter of the way in is on course for 104%: it runs out before the reset.
        let start = QuotaWindow(name: "5h", used: 26, resetsAt: now.addingTimeInterval(3.75 * 3600))
        #expect(Quotas.runsOut(start, now: now) != nil)
    }

    @Test func usedUpIsNotRunningOut() {
        let w = QuotaWindow(name: "5h", used: 100, resetsAt: now.addingTimeInterval(3600))
        #expect(Quotas.runsOut(w, now: now) == nil)
    }
}

/// The layout fix for build 2's clipped account page on the iPhone Duo's outer screen.
@Suite("Fold-aware layout")
struct FoldLayoutTests {
    @Test func columnsAlwaysFitTheOfferedWidth() {
        let layout = ColumnsLayout(division: nil, spacing: 16)
        for width in [320.0, 380, 402, 466, 700, 951] {
            for n in 1...3 {
                let frames = layout.frames(width: width, count: n)
                #expect(frames.allSatisfy { $0.x >= 0 && $0.width >= 0 })
                #expect((frames.last.map { $0.x + $0.width } ?? 0) <= width + 0.001)
            }
        }
    }

    @Test func aHingeDividesTwoColumns() {
        let layout = ColumnsLayout(division: CGRect(x: 455, y: 0, width: 40, height: 669), spacing: 16)
        let f = layout.frames(width: 951, count: 2)
        #expect(f[0].x == 0 && f[0].width == 455)
        #expect(f[1].x == 495 && f[1].width == 456)
    }

    /// A hinge read when the view was wider (unfolded) is ignored once the width no longer
    /// has room for it, instead of placing a column beyond the edge.
    @Test func aStaleHingeIsIgnored() {
        let layout = ColumnsLayout(division: CGRect(x: 455, y: 0, width: 40, height: 669), spacing: 16)
        let f = layout.frames(width: 402, count: 2)
        #expect(f.allSatisfy { $0.x >= 0 && $0.x + $0.width <= 402 })
        let one = layout.frames(width: 402, count: 1)
        #expect(one.count == 1 && one[0].x == 0 && one[0].width == 402)
    }

    @Test func onlyAnActiveHingeInsets() {
        let hinge = CGRect(x: 0, y: 0, width: 40, height: 669)
        #expect(FoldInsets(width: 496, height: 669, regions: [(hinge, false)]).leading == 0)
        #expect(FoldInsets(width: 496, height: 669, regions: [(hinge, true)]).leading == 40)
    }

    @Test func insetsAreNeverNegativeOrHuge() {
        // A region reported in another panel's coordinates, mostly beyond this view.
        let outside = FoldInsets(width: 466, height: 678, regions: [(CGRect(x: -400, y: 0, width: 40, height: 678), true)])
        #expect(outside.leading == 0 && outside.trailing == 0)
        let wide = FoldInsets(width: 466, height: 678, regions: [(CGRect(x: 0, y: 0, width: 300, height: 678), true)])
        #expect(wide.leading >= 0 && wide.leading <= 466 / 3)
        let both = FoldInsets(width: 400, height: 678, regions: [(CGRect(x: 0, y: 0, width: 120, height: 678), true),
                                                                  (CGRect(x: 280, y: 0, width: 120, height: 678), true)])
        #expect(both.leading == 0 && both.trailing == 0)
        // A status area (short, wide) is not a hinge.
        let status = FoldInsets(width: 466, height: 678, regions: [(CGRect(x: 0, y: 0, width: 60, height: 120), true)])
        #expect(status.leading == 0)
    }
}

private extension Double {
    func isApprox(_ other: Double) -> Bool { abs(self - other) < 1e-9 }
}
