import Charts
import SwiftUI

// Output speed per model (ui/usage.js usageSpeedData, usageSpeedHTML): tokens a second once the
// first content arrived, from the timings of the calls Fusebox served. Codex models split by the
// tier the call asked for, which is also what pricing uses (Fast calls can come back "default").

/// Timed output summed over calls; rates are taken over the sums, so long answers count more.
struct SpeedSums: Equatable {
    var calls = 0, timed = 0, output = 0, streamMs = 0, ttftMs = 0

    mutating func add(_ r: UsageSpeed.Row) {
        calls += r.calls ?? 0; timed += r.timed ?? 0; output += r.output ?? 0
        streamMs += r.streamMs ?? 0; ttftMs += r.ttftMs ?? 0
    }

    static func of(_ rows: some Sequence<UsageSpeed.Row>) -> SpeedSums {
        var s = SpeedSums()
        for r in rows { s.add(r) }
        return s
    }

    /// Output tokens a second after the first content.
    var rate: Double? { streamMs > 0 ? Double(output) * 1000 / Double(streamMs) : nil }
    /// Seconds to the first content, averaged over the timed calls.
    var first: Double? { timed > 0 ? Double(ttftMs) / Double(timed) / 1000 : nil }
}

enum SpeedTier {
    static func name(_ s: String?) -> String {
        switch s { case "fast": "Fast"; case "ultrafast": "Ultrafast"; default: "Standard" }
    }
    static func order(_ s: String?) -> Int {
        switch s { case "fast": 0; case "ultrafast": 1; default: 2 }
    }
    static func isStandard(_ s: String?) -> Bool { s == nil || s == "standard" }
}

enum SpeedText {
    /// "8.4", "96", "1,204"; "—" when nothing was timed.
    static func rate(_ v: Double?) -> String {
        guard let v else { return "—" }
        return v < 10 ? String(format: "%.1f", v) : Int(v.rounded()).formatted(.number.locale(Locale(identifier: "en_US")))
    }
    /// "1.2 s", "14 s".
    static func seconds(_ v: Double?) -> String {
        guard let v else { return "—" }
        return v < 10 ? String(format: "%.1f s", v) : "\(Int(v.rounded())) s"
    }
}

struct SpeedSummary {
    struct Tier: Identifiable {
        var speed: String
        var sums: SpeedSums
        /// Output speed in the period before the range.
        var before: Double?
        var id: String { speed }
    }

    struct Model: Identifiable {
        var key: String
        var provider: String?
        var model: String?
        /// Fast first, Standard last.
        var tiers: [Tier]
        var sums: SpeedSums
        var before: Double?
        /// Calls that asked for a faster tier than Standard.
        var fastCalls: Int
        var id: String { key }
        var rate: Double? { sums.rate }
        var first: Double? { sums.first }
    }

    /// At most this many models are charted at once (one colour each).
    static let maxPicks = SpeedPalette.colors.count

    var models: [Model] = []
    var calls = 0
    var timed = 0
    /// Providers with a call on a faster tier, and the share of their calls that asked for one.
    var fastProviders: [String] = []
    var fastShare: Double?
    /// The busiest model timed at both Fast and Standard, and how much faster Fast was.
    var gain: (model: String, ratio: Double)?
    var backfilling = false

    static func key(_ r: UsageSpeed.Row) -> String { "\(r.provider ?? "")\u{0}\(r.model ?? "")" }

    init(_ s: UsageSpeed) {
        backfilling = s.backfilling ?? false
        var order: [String] = []
        var rows: [String: [UsageSpeed.Row]] = [:]
        for r in s.models ?? [] {
            let k = Self.key(r)
            if rows[k] == nil { order.append(k) }
            rows[k, default: []].append(r)
        }
        let before = Dictionary(grouping: s.previous ?? [], by: Self.key)
        models = order.map { k in
            let tiers = rows[k]!.sorted { SpeedTier.order($0.speed) < SpeedTier.order($1.speed) }
            let prev = before[k] ?? []
            return Model(
                key: k, provider: tiers[0].provider, model: tiers[0].model,
                tiers: tiers.map { r in
                    Tier(speed: r.speed ?? "standard", sums: SpeedSums.of([r]),
                         before: SpeedSums.of(prev.filter { $0.speed == r.speed }).rate)
                },
                sums: SpeedSums.of(tiers), before: SpeedSums.of(prev).rate,
                fastCalls: tiers.filter { !SpeedTier.isStandard($0.speed) }.reduce(0) { $0 + ($1.calls ?? 0) })
        }
        calls = models.reduce(0) { $0 + $1.sums.calls }
        timed = models.reduce(0) { $0 + $1.sums.timed }
        var providers: [String] = []
        for m in models where m.fastCalls > 0 && !providers.contains(m.provider ?? "") { providers.append(m.provider ?? "") }
        fastProviders = providers
        let withFast = models.filter { providers.contains($0.provider ?? "") }
        let of = withFast.reduce(0) { $0 + $1.sums.calls }
        fastShare = of > 0 ? Double(withFast.reduce(0) { $0 + $1.fastCalls }) / Double(of) : nil
        let both = models.sorted { $0.sums.calls > $1.sums.calls }.first { m in
            m.tiers.contains { $0.speed == "fast" && $0.sums.rate != nil } && m.tiers.contains { $0.speed == "standard" && $0.sums.rate != nil }
        }
        if let both, let fast = both.tiers.first(where: { $0.speed == "fast" })?.sums.rate,
           let standard = both.tiers.first(where: { $0.speed == "standard" })?.sums.rate {
            gain = (both.model ?? "Unknown model", fast / standard)
        }
    }

    /// The charted models with their colour slots: the user's picks that are still in the range,
    /// or else the four with the most timed calls.
    func picks(_ chosen: [String: Int]?) -> [String: Int] {
        let keys = Set(models.map(\.key))
        let kept = (chosen ?? [:]).filter { keys.contains($0.key) }
        if !kept.isEmpty { return kept }
        var picks: [String: Int] = [:]
        for (i, m) in models.filter({ $0.sums.timed > 0 }).sorted(by: { $0.sums.timed > $1.sums.timed }).prefix(4).enumerated() {
            picks[m.key] = i
        }
        return picks
    }

    /// The models of one provider (all when empty), in the chosen order; untimed ones last.
    func sorted(by sort: String, provider: String) -> [Model] {
        models.filter { provider.isEmpty || ($0.provider ?? "") == provider }.sorted { a, b in
            if sort == "calls" { return a.sums.calls > b.sums.calls }
            let (x, y) = sort == "first" ? (a.first, b.first) : (a.rate, b.rate)
            guard let x, let y else {
                if (x == nil) != (y == nil) { return x != nil }
                return a.sums.calls > b.sums.calls
            }
            // Fastest first: highest rate, or shortest wait for the first token.
            if x == y { return a.sums.calls > b.sums.calls }
            return sort == "first" ? x < y : x > y
        }
    }

    /// The highest rate of any model or tier, the full width of the rate bars.
    var maxRate: Double {
        models.flatMap { [$0.rate] + $0.tiers.map(\.sums.rate) }.compactMap { $0 }.max() ?? 0
    }
}

/// One line of the speed chart: a charted model at one tier, a value per day (nil = not timed).
struct SpeedLine: Identifiable {
    var key: String
    var model: String?
    var speed: String
    var slot: Int
    var values: [Double?]
    var id: String { "\(key)\u{0}\(speed)" }
    var name: String { (model ?? "Unknown model") + (SpeedTier.isStandard(speed) ? "" : " \(SpeedTier.name(speed))") }

    /// Lines for the picked models, by colour slot and then Fast before Standard.
    static func lines(_ trend: [UsageSpeed.Row], dates: [String], picks: [String: Int], first: Bool) -> [SpeedLine] {
        let index = Dictionary(uniqueKeysWithValues: dates.enumerated().map { ($1, $0) })
        var sums: [String: (row: UsageSpeed.Row, slot: Int, days: [SpeedSums])] = [:]
        for r in trend {
            let k = SpeedSummary.key(r)
            guard let slot = picks[k], let i = index[r.date ?? ""] else { continue }
            let id = "\(k)\u{0}\(r.speed ?? "standard")"
            if sums[id] == nil { sums[id] = (r, slot, Array(repeating: SpeedSums(), count: dates.count)) }
            sums[id]!.days[i].add(r)
        }
        return sums.map { id, s in
            SpeedLine(key: SpeedSummary.key(s.row), model: s.row.model, speed: s.row.speed ?? "standard", slot: s.slot,
                      values: s.days.map { first ? $0.first : $0.rate })
        }
        .sorted { $0.slot != $1.slot ? $0.slot < $1.slot : SpeedTier.order($0.speed) < SpeedTier.order($1.speed) }
    }

    /// Runs of days with a value; the chart draws each run as its own line, so a day without
    /// timed calls leaves a gap instead of a line straight across it.
    var runs: [[Int]] {
        var runs: [[Int]] = [], run: [Int] = []
        for (i, v) in values.enumerated() {
            if v != nil { run.append(i) } else if !run.isEmpty { runs.append(run); run = [] }
        }
        if !run.isEmpty { runs.append(run) }
        return runs
    }
}

/// The speed chart's line colours: the web's hues (ui/usage.js usageSpeedColor), oklch(0.6 0.09 h)
/// in light mode and oklch(0.74 0.09 h) in dark, without its hue 90, an ochre, as the app has no amber.
enum SpeedPalette {
    private static let light: [UInt32] = [0xB06B60, 0x578F63, 0x2D8F9E, 0x6B7DB6, 0x9F6C9A]
    private static let dark: [UInt32] = [0xDE9589, 0x81BB8D, 0x5FBACA, 0x95A8E4, 0xCC96C6]

    static let colors: [Color] = zip(light, dark).map { l, d in
        Color(UIColor { $0.userInterfaceStyle == .dark ? rgb(d) : rgb(l) })
    }

    static func color(_ slot: Int) -> Color { colors[slot % colors.count] }

    private static func rgb(_ v: UInt32) -> UIColor {
        UIColor(red: CGFloat(v >> 16 & 0xFF) / 255, green: CGFloat(v >> 8 & 0xFF) / 255, blue: CGFloat(v & 0xFF) / 255, alpha: 1)
    }
}

/// The Usage page's "Output speed by model" panel.
struct SpeedCard: View {
    var speed: UsageSpeed
    var days: Int
    var dates: [String]

    @SceneStorage("usage.speed.sort") private var sort = "rate"
    @SceneStorage("usage.speed.metric") private var metric = "rate"
    @State private var provider = ""
    @State private var chosen: [String: Int]?
    @State private var showAll = false
    @State private var full = false
    @State private var selectedDay: Int?

    private static let rows = 12

    var body: some View {
        let c = SpeedSummary(speed)
        let word = days == 1 ? "today" : "last \(days) days"
        Card(spacing: 12) {
            PanelHead("Output speed by model · \(word)")
                .accessibilityIdentifier("speed-head")
            if c.models.isEmpty {
                Text(c.backfilling ? "Adding timings to earlier calls; this fills in shortly."
                     : "No timed calls in this range. Fusebox times the calls it serves; imported history has no timings.")
                    .font(Typeface.meta).foregroundStyle(Palette.fg3)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                content(c)
            }
        }
    }

    @ViewBuilder private func content(_ c: SpeedSummary) -> some View {
        let picks = c.picks(chosen)
        let providers = c.models.reduce(into: [String]()) { if !$0.contains($1.provider ?? "") { $0.append($1.provider ?? "") } }
        let filter = providers.contains(provider) ? provider : ""
        let rows = c.sorted(by: sort, provider: filter)
        let shown = showAll ? rows : Array(rows.prefix(Self.rows))
        let top = c.maxRate
        stats(c)
        Hairline()
        if providers.count > 1 {
            FuseSegmented("Provider", selection: $provider,
                          options: [("", "All")] + providers.map { ($0, Provider.name(Provider.usageKey($0))) }, fill: true)
        }
        FuseSegmented("Sort", selection: $sort, options: [("rate", "Speed"), ("first", "First token"), ("calls", "Calls")], fill: true)
        Text(full ? "Up to \(SpeedSummary.maxPicks) models at a time; tap a charted one to remove it." : "Tap a model to chart it.")
            .font(Typeface.small).foregroundStyle(full ? Palette.fg2 : Palette.fg3)
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(shown.enumerated()), id: \.element.id) { i, m in
                if i > 0 { Hairline() }
                row(m, slot: picks[m.key], top: top, picks: picks)
            }
        }
        if rows.count > Self.rows {
            Button(showAll ? "Show top \(Self.rows)" : "Show all \(rows.count)") { showAll.toggle() }.buttonStyle(.outlineSmall)
        }
        Hairline()
        chart(c, picks: picks)
        Text("Speed is output tokens over the time after generation started, summed over calls, so long answers count more. Reasoning tokens count as output, and first token is when reasoning began. Fast is the tier the call asked for. Not timed: failed and cancelled calls, calls without streamed content or reported output, hidden reasoning with no known start, and answers that arrived in one piece.\(c.backfilling ? " Earlier calls are still getting their timings." : "")")
            .font(Typeface.small).foregroundStyle(Palette.fg3)
            .fixedSize(horizontal: false, vertical: true)
    }

    // MARK: Figures

    private func stats(_ c: SpeedSummary) -> some View {
        Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 14) {
            GridRow {
                Figure(value: Fmt.count(c.models.count), label: "Models")
                Figure(value: Fmt.count(c.timed), label: "Timed calls")
                Figure(value: Fmt.count(c.calls - c.timed), label: "Not timed")
            }
            if c.fastShare != nil || c.gain != nil {
                GridRow {
                    if let share = c.fastShare {
                        Figure(value: "\(Int((share * 100).rounded()))%",
                               label: c.fastProviders.count == 1 ? "\(Provider.name(Provider.usageKey(c.fastProviders[0]))) on Fast" : "On Fast")
                    } else {
                        Color.clear.frame(height: 1)
                    }
                    if let gain = c.gain {
                        Figure(value: String(format: "×%.1f", gain.ratio), label: "Fast vs Standard · \(gain.model)")
                            .gridCellColumns(2)
                    }
                }
            }
        }
    }

    // MARK: Rows

    private func row(_ m: SpeedSummary.Model, slot: Int?, top: Double, picks: [String: Int]) -> some View {
        let only = m.tiers.count == 1 && !SpeedTier.isStandard(m.tiers[0].speed) ? " · \(SpeedTier.name(m.tiers[0].speed)) only" : ""
        let fast = m.fastCalls > 0 && m.tiers.count > 1 ? " · \(Int((Double(m.fastCalls) / Double(max(1, m.sums.calls)) * 100).rounded()))% Fast" : ""
        return Button { toggle(m.key, picks: picks) } label: {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    swatch(slot).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 1 }
                    ProviderLogo(Provider.logo(provider: Provider.usageKey(m.provider)), size: 15)
                        .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 2 }
                    Text(m.model ?? "Unknown model").font(Typeface.data(13, relativeTo: .subheadline))
                        .foregroundStyle(Palette.fg).lineLimit(1)
                    Spacer(minLength: 8)
                    rateText(m.rate)
                }
                bar(m.rate, top: top)
                Text("\(SpeedText.seconds(m.first)) to first token · \(Fmt.plural(m.sums.calls, "call"))\(fast)\(only)")
                    .font(Typeface.small).foregroundStyle(Palette.fg3).lineLimit(1).minimumScaleFactor(0.85)
                if m.tiers.count > 1 {
                    ForEach(m.tiers) { t in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text("↳ \(SpeedTier.name(t.speed))").font(Typeface.small).foregroundStyle(Palette.fg2)
                            Text("\(SpeedText.seconds(t.sums.first)) · \(Fmt.plural(t.sums.calls, "call"))")
                                .font(Typeface.small).foregroundStyle(Palette.fg3).lineLimit(1)
                            Spacer(minLength: 8)
                            rateText(t.sums.rate, size: 12)
                        }
                        .padding(.leading, 17)
                    }
                }
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(RowButtonStyle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(m.model ?? "Unknown model"), \(SpeedText.rate(m.rate)) tokens a second, \(SpeedText.seconds(m.first)) to first token, \(Fmt.plural(m.sums.calls, "call"))\(m.tiers.count > 1 ? ", " + m.tiers.map { "\(SpeedTier.name($0.speed)) \(SpeedText.rate($0.sums.rate))" }.joined(separator: ", ") : "")")
        .accessibilityAddTraits(slot == nil ? [.isButton] : [.isButton, .isSelected])
        .accessibilityHint(slot == nil ? "Charts this model" : "Removes this model from the chart")
        .accessibilityIdentifier("speed-row")
    }

    private func rateText(_ rate: Double?, size: CGFloat = 13) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 3) {
            Text(SpeedText.rate(rate)).font(Typeface.data(size, relativeTo: .subheadline)).foregroundStyle(rate == nil ? Palette.fg3 : Palette.fg)
            Text("tok/s").font(Typeface.data(size - 2)).foregroundStyle(Palette.fg3)
        }
        .fixedSize()
    }

    private func bar(_ rate: Double?, top: Double) -> some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 1).fill(Palette.segOff)
                RoundedRectangle(cornerRadius: 1).fill(Palette.meter)
                    .frame(width: rate.map { top > 0 ? g.size.width * $0 / top : 0 } ?? 0)
            }
        }
        .frame(height: 4)
        .accessibilityHidden(true)
    }

    /// The charted model's colour, or an empty square when it isn't charted.
    private func swatch(_ slot: Int?) -> some View {
        RoundedRectangle(cornerRadius: 2)
            .fill(slot.map { SpeedPalette.color($0) } ?? .clear)
            .frame(width: 9, height: 9)
            .overlay(RoundedRectangle(cornerRadius: 2).strokeBorder(Palette.lineStrong, lineWidth: slot == nil ? 1 : 0.5))
    }

    private func toggle(_ key: String, picks: [String: Int]) {
        var p = picks
        if p[key] != nil {
            p[key] = nil
            full = false
        } else if let slot = (0..<SpeedSummary.maxPicks).first(where: { s in !p.values.contains(s) }) {
            p[key] = slot
            full = false
        } else {
            full = true
            return
        }
        chosen = p
    }

    // MARK: Chart

    private struct Point: Identifiable {
        var line: String
        var run: String
        var day: Date
        var value: Double
        var color: Color
        var dash: [CGFloat]
        var dot: Bool
        var id: String { "\(run)|\(day.timeIntervalSince1970)" }
    }

    @ViewBuilder private func chart(_ c: SpeedSummary, picks: [String: Int]) -> some View {
        let first = metric == "first"
        let lines = SpeedLine.lines(speed.trend ?? [], dates: dates, picks: picks, first: first)
        let dayDates = dates.map(Self.noon)
        HStack(spacing: 8) {
            Text("By day").capsLabel(10.5).accessibilityIdentifier("speed-by-day")
            Spacer(minLength: 0)
            FuseSegmented("Chart", selection: $metric, options: [("rate", "Speed"), ("first", "First token")])
        }
        Text(first ? "Seconds to first token · charted models" : "Output tokens a second · charted models")
            .font(Typeface.small).foregroundStyle(Palette.fg3)
        if days <= 1 {
            Text("Pick a longer range to see speed by day.").font(Typeface.meta).foregroundStyle(Palette.fg3)
        } else if lines.isEmpty {
            Text("Tap a model above to chart it.").font(Typeface.meta).foregroundStyle(Palette.fg3)
        } else {
            let points = Self.points(lines, days: dayDates)
            Chart {
                ForEach(points) { p in
                    LineMark(x: .value("Day", p.day), y: .value(first ? "Seconds" : "Tokens a second", p.value), series: .value("Line", p.run))
                        .foregroundStyle(p.color)
                        .lineStyle(StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round, dash: p.dash))
                    if p.dot {
                        PointMark(x: .value("Day", p.day), y: .value(first ? "Seconds" : "Tokens a second", p.value))
                            .foregroundStyle(p.color).symbolSize(30)
                    }
                }
                if let i = selectedDay, dayDates.indices.contains(i) {
                    RuleMark(x: .value("Day", dayDates[i])).foregroundStyle(Palette.lineStrong).lineStyle(StrokeStyle(lineWidth: 1))
                }
            }
            // Whole days, so each day's label sits under its value at noon.
            .chartXScale(domain: dayDates.first!.addingTimeInterval(-43_200)...dayDates.last!.addingTimeInterval(43_200))
            .chartYScale(domain: .automatic(includesZero: true))
            .chartXSelection(value: Binding(
                get: { selectedDay.flatMap { dayDates[safe: $0] } },
                set: { d in selectedDay = d.map { d in dayDates.indices.min { abs(dayDates[$0].timeIntervalSince(d)) < abs(dayDates[$1].timeIntervalSince(d)) } ?? 0 } }))
            .chartYAxis {
                AxisMarks(position: .leading) { v in
                    AxisGridLine().foregroundStyle(Palette.line)
                    AxisValueLabel {
                        if let d = v.as(Double.self) {
                            Text(first ? String(format: d < 10 ? "%.1fs" : "%.0fs", d) : Int(d.rounded()).formatted(.number.locale(Locale(identifier: "en_US"))))
                                .font(Typeface.data(10.5)).foregroundStyle(Palette.fg3)
                        }
                    }
                }
            }
            .chartXAxis {
                AxisMarks(values: .stride(by: .day, count: days > 10 ? 7 : 1)) { v in
                    AxisValueLabel(centered: true) {
                        if let d = v.as(Date.self) {
                            Text(days > 10 ? d.formatted(.dateTime.month(.abbreviated).day()) : d.formatted(.dateTime.weekday(.abbreviated)))
                                .font(Typeface.label(10.5)).foregroundStyle(Palette.fg3)
                        }
                    }
                }
            }
            .frame(height: 170)
            .accessibilityLabel("\(first ? "Seconds to first token" : "Output speed") by day for the charted models")
            .accessibilityValue(lines.map { l in "\(l.name): \(l.values.compactMap { $0 }.last.map { first ? SpeedText.seconds($0) : SpeedText.rate($0) + " tokens a second" } ?? "no timed calls")" }.joined(separator: "; "))
            .accessibilityIdentifier("speed-chart")
            legend(lines)
            readout(lines, days: dayDates, first: first)
        }
    }

    /// Each line's runs as chart points; a dot marks a run of one day and each line's last value.
    private static func points(_ lines: [SpeedLine], days: [Date]) -> [Point] {
        var points: [Point] = []
        for l in lines {
            let color = SpeedPalette.color(l.slot)
            let dash: [CGFloat] = l.speed == "fast" ? [6, 4] : l.speed == "ultrafast" ? [2, 3] : []
            let last = l.values.lastIndex { $0 != nil }
            for (n, run) in l.runs.enumerated() {
                for i in run {
                    points.append(Point(line: l.id, run: "\(l.id)#\(n)", day: days[i], value: l.values[i]!, color: color, dash: dash,
                                        dot: run.count == 1 || i == last))
                }
            }
        }
        return points
    }

    private func legend(_ lines: [SpeedLine]) -> some View {
        FlowLayout(spacing: 12, lineSpacing: 6) {
            ForEach(lines) { l in
                HStack(spacing: 6) {
                    let color = SpeedPalette.color(l.slot)
                    if SpeedTier.isStandard(l.speed) {
                        RoundedRectangle(cornerRadius: 2).fill(color).frame(width: 9, height: 9)
                    } else {
                        Path { p in p.move(to: CGPoint(x: 0, y: 4)); p.addLine(to: CGPoint(x: 16, y: 4)) }
                            .stroke(color, style: StrokeStyle(lineWidth: 2, dash: l.speed == "fast" ? [4, 3] : [1.5, 2.5]))
                            .frame(width: 16, height: 8)
                    }
                    Text(l.model ?? "Unknown model").font(Typeface.data(11.5)).foregroundStyle(Palette.fg2).lineLimit(1)
                    if !SpeedTier.isStandard(l.speed) { Text(SpeedTier.name(l.speed)).font(Typeface.small).foregroundStyle(Palette.fg3) }
                }
            }
        }
        .accessibilityHidden(true)
    }

    /// The values of the touched day, or of the last day with any.
    @ViewBuilder private func readout(_ lines: [SpeedLine], days: [Date], first: Bool) -> some View {
        let latest = (0..<days.count).last { i in lines.contains { $0.values[i] != nil } }
        if let i = selectedDay ?? latest, days.indices.contains(i) {
            Hairline()
            VStack(alignment: .leading, spacing: 6) {
                Text("\(selectedDay == nil ? "Latest day" : "Day") · \(days[i].formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day()))").capsLabel(10.5)
                ForEach(lines) { l in
                    HStack(spacing: 8) {
                        RoundedRectangle(cornerRadius: 2).fill(SpeedPalette.color(l.slot)).frame(width: 9, height: 9)
                        Text(l.name).font(Typeface.meta).foregroundStyle(Palette.fg2).lineLimit(1)
                        Spacer()
                        Text(l.values[i].map { first ? SpeedText.seconds($0) : "\(SpeedText.rate($0)) tok/s" } ?? "—")
                            .font(Typeface.data(12)).foregroundStyle(l.values[i] == nil ? Palette.fg3 : Palette.fg2)
                    }
                }
            }
        }
    }

    /// A local calendar day at noon, where the chart puts its value.
    static func noon(_ iso: String) -> Date {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = .current
        let p = iso.split(separator: "-").compactMap { Int($0) }
        guard p.count == 3 else { return .now }
        return cal.date(from: DateComponents(year: p[0], month: p[1], day: p[2], hour: 12)) ?? .now
    }
}
