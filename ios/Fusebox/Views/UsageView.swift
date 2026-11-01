import Charts
import SwiftUI

/// The Usage page's main views (ui/usage.js): estimated cost at published API prices,
/// a by-day chart stacked by provider or model, and the breakdowns.
struct UsageView: View {
    @Environment(AppStore.self) private var store
    @SceneStorage("usage.days") private var days = 7
    @SceneStorage("usage.stack") private var stack = "provider"
    @SceneStorage("usage.metric") private var metric = "cost"
    @SceneStorage("usage.dim") private var dim = "model"
    @State private var filter: (dim: String, id: String)?
    @State private var data: UsageDashboard?
    @State private var error: String?
    @State private var loading = false
    @State private var selectedDay: String?
    @State private var showAll = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Metrics.panelGap) {
                FuseSegmented("Range", selection: $days, options: [(1, "Today"), (7, "7 days"), (30, "30 days")], fill: true)
                    .accessibilityIdentifier("usage-range")
                if let filter {
                    Button { self.filter = nil } label: {
                        HStack(spacing: 8) {
                            Text("\(dimTitle(filter.dim)): \(rowName(dim: filter.dim, id: filter.id))").lineLimit(1)
                            Image(systemName: "xmark").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.fg3)
                        }
                    }
                    .buttonStyle(.outlineSmall)
                    .accessibilityLabel("Remove filter \(dimTitle(filter.dim)) \(rowName(dim: filter.dim, id: filter.id))")
                }
                if let error {
                    Card {
                        HStack(spacing: 8) { StatusDot(Palette.err); Text(error).font(Typeface.meta).foregroundStyle(Palette.err) }
                        Button("Retry") { Task { await load() } }.buttonStyle(.outline)
                    }
                }
                if let c = data?.combined {
                    FoldAwareColumns(items: [Part.cost, .chart, .breakdown], minColumnWidth: 380) { part in
                        switch part {
                        case .cost: costCard(c)
                        case .chart: chartCard(c)
                        case .breakdown: breakdownCard(c)
                        }
                    }
                } else if loading {
                    ProgressView("Loading usage…").font(Typeface.meta).foregroundStyle(Palette.fg3)
                        .frame(maxWidth: .infinity).padding(40)
                }
                Text("What your traffic would cost at published API prices, in USD. An estimate, not a subscription bill.")
                    .font(Typeface.small).foregroundStyle(Palette.fg3)
            }
            .pageContent()
        }
        .background(Palette.bg)
        .navigationTitle(AppSection.usage.pageTitle)
        .refreshable { await load() }
        .task(id: "\(days)|\(filter?.dim ?? ""):\(filter?.id ?? "")") { await load() }
        .task {
            // Reads refresh once a minute while Usage is visible.
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                await load()
            }
        }
        .accessibilityIdentifier("usage")
    }

    private enum Part: String, Identifiable { case cost, chart, breakdown; var id: String { rawValue } }

    // MARK: Loading

    private var range: (start: String, end: String, dates: [String]) {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = .current
        let today = cal.startOfDay(for: .now)
        func iso(_ d: Date) -> String {
            let c = cal.dateComponents([.year, .month, .day], from: d)
            return String(format: "%04d-%02d-%02d", c.year!, c.month!, c.day!)
        }
        let start = cal.date(byAdding: .day, value: 1 - days, to: today)!
        let end = cal.date(byAdding: .day, value: 1, to: today)!
        let dates = (0..<days).map { iso(cal.date(byAdding: .day, value: $0, to: start)!) }
        return (iso(start), iso(end), dates)
    }

    private func load() async {
        guard let api = store.api else { return }
        loading = true; defer { loading = false }
        var q = URLComponents()
        q.queryItems = [URLQueryItem(name: "start", value: range.start), URLQueryItem(name: "end", value: range.end),
                        URLQueryItem(name: "timezone", value: TimeZone.current.identifier)]
        if let filter { q.queryItems?.append(URLQueryItem(name: filter.dim, value: filter.id)) }
        do {
            data = try await api.get("/usage/dashboard?\(q.percentEncodedQuery ?? "")")
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    // MARK: Cost

    private func costCard(_ c: UsageDashboard.Combined) -> some View {
        let t = c.totals ?? UsageAggregate()
        let tokens = t.tokens
        let word = days == 1 ? "today" : "last \(days) days"
        let avg = t.estimatedCostNanos.map { Fmt.dollars(nanos: $0 / Int64(max(1, days))) } ?? "Unpriced"
        return Card(spacing: 14) {
            PanelHead("Estimated cost · \(word)")
            VStack(alignment: .leading, spacing: 4) {
                Text(Fmt.dollars(nanos: t.estimatedCostNanos ?? ((t.observations ?? 0) == 0 ? 0 : nil)))
                    .font(Typeface.sans(34, .medium, relativeTo: .largeTitle)).tracking(-0.7).foregroundStyle(Palette.fg)
                    .accessibilityIdentifier("usage-total")
                Text(days == 1 ? "So far today · priced calls only" : "Avg \(avg) a day · priced calls only")
                    .font(Typeface.meta).foregroundStyle(Palette.fg3)
            }
            Hairline()
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 14) {
                GridRow { Figure(value: Fmt.count(t.observations), label: "Calls"); Figure(value: Fmt.compact(tokens?.input), label: "Tokens in"); Figure(value: Fmt.compact(tokens?.output), label: "Tokens out") }
                GridRow { Figure(value: Fmt.compact(tokens?.cacheRead), label: "Cache read"); Figure(value: tokens?.cacheWrite == nil ? "Unknown" : Fmt.compact(tokens?.cacheWrite), label: "Cache write"); Figure(value: Fmt.count(t.historyOnly ?? 0), label: "History only") }
            }
            Text(coverage(t)).font(Typeface.small).foregroundStyle(Palette.fg2)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func coverage(_ t: UsageAggregate) -> String {
        let n = t.observations ?? 0, unpriced = t.unpriced ?? 0
        var parts = [n > 0 ? String(format: "%.1f%% of calls priced", floor((1 - Double(unpriced) / Double(n)) * 1000) / 10) : "No matching calls"]
        if let m = t.matched, m > 0 { parts.append("\(Fmt.count(m)) matched across sources, counted once") }
        if unpriced > 0 { parts.append("\(Fmt.count(unpriced)) unpriced") }
        if let p = t.partial, p > 0 { parts.append("\(Fmt.count(p)) partial") }
        if let p = t.pricingPartial, p > 0 { parts.append("\(Fmt.count(p)) with assumptions") }
        return parts.joined(separator: " · ")
    }

    // MARK: Chart

    struct DayPart: Identifiable {
        var date: String
        var day: Date
        var key: String
        var value: Double
        var id: String { "\(date)|\(key)" }
    }

    private func chartCard(_ c: UsageDashboard.Combined) -> some View {
        let model = UsageChart(trend: (stack == "model" ? c.trends?.model : c.trends?.provider) ?? c.trend ?? [],
                               dates: range.dates, calls: metric == "calls")
        let names = model.keys.map(groupName)
        let colors = model.keys.enumerated().map { i, k in k == UsageChart.other ? Palette.chartOther : Palette.chart[i % Palette.chart.count] }
        let readout = model.days.first { $0.date == selectedDay } ?? model.days[safe: model.peak]
        return Card(spacing: 14) {
            PanelHead("By day")
            HStack(spacing: 8) {
                FuseSegmented("Stack", selection: $stack, options: [("provider", "Provider"), ("model", "Model")])
                Spacer(minLength: 0)
                FuseSegmented("Value", selection: $metric, options: [("cost", "Cost"), ("calls", "Calls")])
            }
            if days > 1 {
                Chart(model.parts) { p in
                    BarMark(x: .value("Day", p.day, unit: .day), y: .value(metric == "calls" ? "Calls" : "Cost", p.value), width: .ratio(0.62))
                        .foregroundStyle(by: .value("Group", groupName(p.key)))
                        .opacity(selectedDay == nil || selectedDay == p.date ? 1 : 0.35)
                }
                .chartForegroundStyleScale(domain: names, range: colors)
                .chartLegend(.hidden)
                .chartXSelection(value: Binding(get: { model.days.first { $0.date == selectedDay }?.day },
                                                set: { d in selectedDay = d.flatMap { day in model.days.first { Calendar.current.isDate($0.day, inSameDayAs: day) }?.date } }))
                .chartYAxis {
                    AxisMarks(position: .leading) { v in
                        AxisGridLine().foregroundStyle(Palette.line)
                        AxisValueLabel { if let d = v.as(Double.self) { Text(metric == "calls" ? Fmt.compact(Int(d)) : Fmt.dollarsShort(d)).font(Typeface.data(10.5)).foregroundStyle(Palette.fg3) } }
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
                .frame(height: 180)
                .accessibilityIdentifier("usage-chart")
                legend(names, colors)
            }
            if let d = readout {
                Hairline()
                VStack(alignment: .leading, spacing: 6) {
                    Text("\(selectedDay == nil ? "Peak day" : "Day") · \(dayLabel(d.date))").capsLabel(10.5)
                    HStack(alignment: .firstTextBaseline) {
                        Text(d.priced ? Fmt.dollars(nanos: d.costNanos) : d.calls > 0 ? "Unpriced" : "$0.00")
                            .font(Typeface.figure).foregroundStyle(Palette.fg)
                        Spacer()
                        Text(Fmt.plural(d.calls, "call")).font(Typeface.data(12.5)).foregroundStyle(Palette.fg2)
                    }
                    .padding(.bottom, 2)
                    ForEach(Array(model.keys.enumerated()), id: \.offset) { i, k in
                        if d.groupCalls[i] > 0 {
                            HStack(spacing: 8) {
                                RoundedRectangle(cornerRadius: 2).fill(colors[i]).frame(width: 9, height: 9)
                                    .overlay(RoundedRectangle(cornerRadius: 2).strokeBorder(Palette.lineStrong, lineWidth: 0.5))
                                Text(groupName(k)).font(Typeface.meta).foregroundStyle(Palette.fg2).lineLimit(1)
                                Spacer()
                                Text(metric == "calls" ? "\(Fmt.count(d.groupCalls[i])) calls" : Fmt.dollars(nanos: d.groupCost[i]))
                                    .font(Typeface.data(12)).foregroundStyle(Palette.fg2)
                            }
                        }
                    }
                }
            }
        }
    }

    /// The stack's groups, darkest step first, wrapping to the panel's width.
    private func legend(_ names: [String], _ colors: [Color]) -> some View {
        FlowLayout(spacing: 12, lineSpacing: 6) {
            ForEach(Array(names.enumerated()), id: \.offset) { i, n in
                HStack(spacing: 6) {
                    RoundedRectangle(cornerRadius: 2).fill(colors[i]).frame(width: 9, height: 9)
                        .overlay(RoundedRectangle(cornerRadius: 2).strokeBorder(Palette.lineStrong, lineWidth: 0.5))
                    Text(n).font(Typeface.small).foregroundStyle(Palette.fg2).lineLimit(1)
                }
            }
        }
    }

    private func groupName(_ k: String) -> String {
        if k == UsageChart.other { return "Other" }
        if stack == "model" { return k.isEmpty ? "Unknown model" : k }
        return k.isEmpty ? "Unknown provider" : Provider.name(Provider.usageKey(k))
    }

    private func dayLabel(_ iso: String) -> String {
        guard let d = UsageChart.day(iso) else { return iso }
        return d.formatted(Date.FormatStyle(timeZone: TimeZone(identifier: "UTC")!).weekday(.abbreviated).month(.abbreviated).day())
    }

    // MARK: Breakdown

    private func breakdownCard(_ c: UsageDashboard.Combined) -> some View {
        let b = c.breakdowns
        let rows: [UsageAggregate] = switch dim {
        case "account": b?.account ?? []
        case "provider": b?.provider ?? []
        case "client": b?.client ?? []
        default: b?.model ?? []
        }
        let limit = dim == "model" ? 20 : 8
        let shown = showAll ? rows : Array(rows.prefix(limit))
        let total = Double(c.totals?.estimatedCostNanos ?? 0)
        return Card(spacing: 12) {
            PanelHead("Breakdown · \(days == 1 ? "today" : "last \(days) days")")
            FuseSegmented("Breakdown", selection: $dim, options: [("model", "Model"), ("account", "Account"), ("provider", "Provider"), ("client", "Client")], fill: true)
            if rows.isEmpty { Text("No records match these filters.").font(Typeface.meta).foregroundStyle(Palette.fg3) }
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(shown.enumerated()), id: \.offset) { i, r in
                    if i > 0 { Hairline() }
                    breakdownRow(r, share: total > 0 ? Double(r.estimatedCostNanos ?? 0) / total : 0)
                }
            }
            if rows.count > limit {
                Button(showAll ? "Show top \(limit)" : "Show all \(rows.count)") { showAll.toggle() }.buttonStyle(.outlineSmall)
            }
        }
    }

    private func breakdownRow(_ r: UsageAggregate, share: Double) -> some View {
        Button {
            if let id = r.id { filter = (dim, id) }
        } label: {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .top, spacing: 10) {
                    if dim != "client" { ProviderLogo(Provider.logo(provider: Provider.usageKey(dim == "provider" ? r.id : r.provider)), size: 15).padding(.top, 2) }
                    VStack(alignment: .leading, spacing: 2) {
                        Text(rowName(dim: dim, id: r.id)).font(dim == "model" ? Typeface.data(13, relativeTo: .subheadline) : Typeface.row)
                            .foregroundStyle(Palette.fg).lineLimit(1)
                        if let sub = rowSub(r) { Text(sub).font(Typeface.small).foregroundStyle(Palette.fg3).lineLimit(1) }
                    }
                    Spacer(minLength: 8)
                    Text(Fmt.dollars(nanos: r.estimatedCostNanos)).font(Typeface.data(13, relativeTo: .subheadline))
                        .foregroundStyle(r.estimatedCostNanos == nil ? Palette.fg3 : Palette.fg)
                }
                HStack(spacing: 10) {
                    GeometryReader { g in
                        ZStack(alignment: .leading) {
                            RoundedRectangle(cornerRadius: 1).fill(Palette.segOff)
                            RoundedRectangle(cornerRadius: 1).fill(Palette.meter).frame(width: max(share > 0 ? 2 : 0, g.size.width * share))
                        }
                    }
                    .frame(height: 4)
                    Text("\(share > 0 ? (share < 0.01 ? "<1%" : "\(Int((share * 100).rounded()))%") : "—") · \(Fmt.count(r.observations)) calls")
                        .font(Typeface.data(11.5)).foregroundStyle(Palette.fg3).lineLimit(1)
                        .frame(width: 128, alignment: .trailing)
                }
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(RowButtonStyle())
        .disabled(r.id == nil)
    }

    private func dimTitle(_ d: String) -> String { ["model": "Model", "account": "Account", "provider": "Provider", "client": "Client"][d] ?? d }

    private func rowName(dim: String, id: String?) -> String {
        switch dim {
        case "model": return id ?? "Unknown model"
        case "provider": return Provider.name(Provider.usageKey(id))
        case "account":
            guard let id else { return "No account reported" }
            return store.account(id)?.label ?? data?.facets?.accounts?.first { $0.id == id }?.label ?? id
        default:
            guard let id else { return "No client reported" }
            return data?.facets?.clients?.first { $0.id == id }?.label ?? id
        }
    }

    private func rowSub(_ r: UsageAggregate) -> String? {
        switch dim {
        case "model": return Provider.name(Provider.usageKey(r.provider))
        case "provider": return (r.accounts ?? 0) > 0 ? Fmt.plural(r.accounts!, "account") : "No account reported"
        case "account": return store.account(r.id).map { "\($0.providerName) · \($0.planName ?? $0.authName)" } ?? Provider.name(Provider.usageKey(r.provider))
        default: return (r.id ?? "").hasPrefix("collector:") ? "Collector" : nil
        }
    }
}

/// UsageTools.chart: the top five groups keep their own colour, the rest merge into Other.
struct UsageChart {
    static let other = "\u{0}other"

    struct Day {
        var date: String
        var day: Date
        var total = 0.0
        var calls = 0
        var costNanos: Int64 = 0
        var priced = false
        var groupCalls: [Int]
        var groupCost: [Int64]
    }

    var keys: [String] = []
    var days: [Day] = []
    var parts: [UsageView.DayPart] = []
    var peak = 0

    static func day(_ iso: String) -> Date? { FuseboxDate.parse("\(iso)T12:00:00Z") }

    init(trend: [UsageAggregate], dates: [String], calls: Bool) {
        let value: (UsageAggregate) -> Double = { calls ? Double($0.observations ?? 0) : Double($0.estimatedCostNanos ?? 0) / 1e9 }
        var totals: [String: Double] = [:], volumes: [String: Int] = [:]
        for r in trend {
            totals[r.group ?? "", default: 0] += value(r)
            volumes[r.group ?? "", default: 0] += r.observations ?? 0
        }
        let ranked = totals.keys.filter { totals[$0]! > 0 || volumes[$0]! > 0 }.sorted {
            totals[$0]! != totals[$1]! ? totals[$0]! > totals[$1]! : volumes[$0]! != volumes[$1]! ? volumes[$0]! > volumes[$1]! : $0 < $1
        }
        let shown = ranked.count > 6 ? Array(ranked.prefix(5)) : ranked
        keys = shown + (ranked.count > shown.count ? [Self.other] : [])
        // Local calendar days, drawn at local noon so the bars sit on their day.
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = .current
        days = dates.map { iso in
            let p = iso.split(separator: "-").compactMap { Int($0) }
            let d = cal.date(from: DateComponents(year: p[0], month: p[1], day: p[2], hour: 12)) ?? .now
            return Day(date: iso, day: d, groupCalls: Array(repeating: 0, count: keys.count), groupCost: Array(repeating: 0, count: keys.count))
        }
        let index = Dictionary(uniqueKeysWithValues: dates.enumerated().map { ($1, $0) })
        var partValues: [String: Double] = [:]
        for r in trend {
            guard let i = index[r.date ?? ""] else { continue }
            let k = shown.contains(r.group ?? "") ? (r.group ?? "") : Self.other
            guard let ki = keys.firstIndex(of: k) else { continue }
            let v = value(r)
            partValues["\(i)|\(ki)", default: 0] += v
            days[i].total += v
            days[i].calls += r.observations ?? 0
            days[i].groupCalls[ki] += r.observations ?? 0
            if let c = r.estimatedCostNanos { days[i].costNanos += c; days[i].priced = true; days[i].groupCost[ki] += c }
        }
        for (i, d) in days.enumerated() {
            for (ki, k) in keys.enumerated() {
                if let v = partValues["\(i)|\(ki)"], v > 0 { parts.append(.init(date: d.date, day: d.day, key: k, value: v)) }
            }
        }
        peak = max(0, days.count - 1)
        for (i, d) in days.enumerated() where d.total > days[peak].total || (d.total == days[peak].total && d.calls > days[peak].calls) {
            peak = i
        }
    }
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}

/// Lays children out left to right, wrapping to a new line when the width runs out.
struct FlowLayout: Layout {
    var spacing: CGFloat = 8
    var lineSpacing: CGFloat = 6

    private func lines(_ width: CGFloat, _ subviews: Subviews) -> [[(Int, CGSize)]] {
        var lines: [[(Int, CGSize)]] = [[]]
        var x: CGFloat = 0
        for (i, s) in subviews.enumerated() {
            let size = s.sizeThatFits(ProposedViewSize(width: width, height: nil))
            if x > 0, x + size.width > width { lines.append([]); x = 0 }
            lines[lines.count - 1].append((i, size))
            x += size.width + spacing
        }
        return lines
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? .greatestFiniteMagnitude
        let ls = lines(width, subviews)
        let height = ls.map { $0.map(\.1.height).max() ?? 0 }.reduce(0, +) + lineSpacing * CGFloat(max(0, ls.count - 1))
        let used = ls.map { l in l.map(\.1.width).reduce(0, +) + spacing * CGFloat(max(0, l.count - 1)) }.max() ?? 0
        return CGSize(width: min(width, used), height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for line in lines(bounds.width, subviews) {
            var x = bounds.minX
            let h = line.map(\.1.height).max() ?? 0
            for (i, size) in line {
                subviews[i].place(at: CGPoint(x: x, y: y + (h - size.height) / 2), anchor: .topLeading, proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += h + lineSpacing
        }
    }
}
