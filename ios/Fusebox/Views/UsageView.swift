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
            VStack(alignment: .leading, spacing: 16) {
                Picker("Range", selection: $days) {
                    Text("Today").tag(1); Text("7 days").tag(7); Text("30 days").tag(30)
                }
                .pickerStyle(.segmented)
                .accessibilityIdentifier("usage-range")
                if let filter {
                    Button { self.filter = nil } label: {
                        Label("\(dimTitle(filter.dim)): \(rowName(dim: filter.dim, id: filter.id))", systemImage: "xmark.circle.fill")
                            .font(.footnote)
                    }
                    .buttonStyle(.bordered)
                }
                if let error {
                    Card {
                        Text(error).font(.footnote).foregroundStyle(Palette.err)
                        Button("Retry") { Task { await load() } }.buttonStyle(.bordered)
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
                    ProgressView("Loading usage…").frame(maxWidth: .infinity).padding(40)
                }
                Text("What your traffic would cost at published API prices, in USD. An estimate, not a subscription bill.")
                    .font(.caption).foregroundStyle(Palette.fg3)
            }
            .padding(16)
            .clearOfFold()
        }
        .background(Palette.bg)
        .navigationTitle("Usage")
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
        let word = days == 1 ? "TODAY" : "LAST \(days) DAYS"
        let avg = t.estimatedCostNanos.map { Fmt.dollars(nanos: $0 / Int64(max(1, days))) } ?? "Unpriced"
        return Card {
            Text("Estimated cost · \(word)").capsLabel()
            Text(Fmt.dollars(nanos: t.estimatedCostNanos ?? ((t.observations ?? 0) == 0 ? 0 : nil)))
                .font(.system(size: 30, weight: .medium)).monospacedDigit().foregroundStyle(Palette.fg)
                .accessibilityIdentifier("usage-total")
            Text(days == 1 ? "So far today · priced calls only" : "Avg \(avg) a day · priced calls only")
                .font(.caption).foregroundStyle(Palette.fg3)
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 8) {
                GridRow { stat(Fmt.count(t.observations), "Calls"); stat(Fmt.compact(tokens?.input), "Tokens in"); stat(Fmt.compact(tokens?.output), "Tokens out") }
                GridRow { stat(Fmt.compact(tokens?.cacheRead), "Cache read"); stat(tokens?.cacheWrite == nil ? "Unknown" : Fmt.compact(tokens?.cacheWrite), "Cache write"); stat(Fmt.count(t.historyOnly ?? 0), "History only") }
            }
            Text(coverage(t)).font(.caption).foregroundStyle(Palette.fg2)
        }
    }

    private func stat(_ v: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(v).font(.subheadline.weight(.medium)).monospacedDigit().foregroundStyle(Palette.fg)
            Text(label).font(.caption2).foregroundStyle(Palette.fg3)
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
        return Card {
            HStack {
                Text("By day").capsLabel()
                Spacer()
                Picker("Stack", selection: $stack) { Text("Provider").tag("provider"); Text("Model").tag("model") }
                    .pickerStyle(.segmented).fixedSize().controlSize(.small)
                Picker("Value", selection: $metric) { Text("Cost").tag("cost"); Text("Calls").tag("calls") }
                    .pickerStyle(.segmented).fixedSize().controlSize(.small)
            }
            if days > 1 {
                Chart(model.parts) { p in
                    BarMark(x: .value("Day", p.day, unit: .day), y: .value(metric == "calls" ? "Calls" : "Cost", p.value))
                        .foregroundStyle(by: .value("Group", groupName(p.key)))
                        .opacity(selectedDay == nil || selectedDay == p.date ? 1 : 0.4)
                }
                .chartForegroundStyleScale(domain: names, range: colors)
                .chartLegend(position: .top, alignment: .leading)
                .chartXSelection(value: Binding(get: { model.days.first { $0.date == selectedDay }?.day },
                                                set: { d in selectedDay = d.flatMap { day in model.days.first { Calendar.current.isDate($0.day, inSameDayAs: day) }?.date } }))
                .chartYAxis {
                    AxisMarks { v in
                        AxisGridLine().foregroundStyle(Palette.line)
                        AxisValueLabel { if let d = v.as(Double.self) { Text(metric == "calls" ? Fmt.compact(Int(d)) : Fmt.dollarsShort(d)) } }
                    }
                }
                .chartXAxis {
                    AxisMarks(values: .stride(by: .day, count: days > 10 ? 7 : 1)) { _ in
                        AxisValueLabel(format: days > 10 ? .dateTime.month(.abbreviated).day() : .dateTime.weekday(.abbreviated), centered: true)
                    }
                }
                .frame(height: 180)
                .accessibilityIdentifier("usage-chart")
            }
            if let d = readout {
                VStack(alignment: .leading, spacing: 3) {
                    Text("\(selectedDay == nil ? "Peak day" : "Day") · \(dayLabel(d.date))")
                        .font(.caption).foregroundStyle(Palette.fg3)
                    Text("\(d.priced ? Fmt.dollars(nanos: d.costNanos) : d.calls > 0 ? "Unpriced" : "$0.00") · \(Fmt.plural(d.calls, "call"))")
                        .font(Typeface.data(12)).foregroundStyle(Palette.fg)
                    ForEach(Array(model.keys.enumerated()), id: \.offset) { i, k in
                        if d.groupCalls[i] > 0 {
                            HStack(spacing: 6) {
                                RoundedRectangle(cornerRadius: 2).fill(colors[i]).frame(width: 8, height: 8)
                                Text(groupName(k)).font(.caption).foregroundStyle(Palette.fg2).lineLimit(1)
                                Spacer()
                                Text(metric == "calls" ? "\(Fmt.count(d.groupCalls[i])) calls" : Fmt.dollars(nanos: d.groupCost[i]))
                                    .font(Typeface.data(11)).foregroundStyle(Palette.fg2)
                            }
                        }
                    }
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
        return Card {
            Text("Breakdown · \(days == 1 ? "today" : "last \(days) days")").capsLabel()
            Picker("Breakdown", selection: $dim) {
                Text("Model").tag("model"); Text("Account").tag("account"); Text("Provider").tag("provider"); Text("Client").tag("client")
            }
            .pickerStyle(.segmented)
            if rows.isEmpty { Text("No records match these filters.").font(.footnote).foregroundStyle(Palette.fg3) }
            ForEach(Array(shown.enumerated()), id: \.offset) { _, r in
                let share = total > 0 ? Double(r.estimatedCostNanos ?? 0) / total : 0
                Button {
                    if let id = r.id { filter = (dim, id) }
                } label: {
                    VStack(alignment: .leading, spacing: 4) {
                        HStack(spacing: 8) {
                            if dim != "client" { ProviderLogo(Provider.logo(provider: Provider.usageKey(dim == "provider" ? r.id : r.provider)), size: 14) }
                            VStack(alignment: .leading, spacing: 1) {
                                Text(rowName(dim: dim, id: r.id)).font(dim == "model" ? Typeface.data(12) : .footnote)
                                    .foregroundStyle(Palette.fg).lineLimit(1)
                                if let sub = rowSub(r) { Text(sub).font(.caption2).foregroundStyle(Palette.fg3) }
                            }
                            Spacer()
                            Text(Fmt.dollars(nanos: r.estimatedCostNanos)).font(Typeface.data(12))
                                .foregroundStyle(r.estimatedCostNanos == nil ? Palette.fg3 : Palette.fg)
                        }
                        HStack(spacing: 8) {
                            GeometryReader { g in
                                ZStack(alignment: .leading) {
                                    Capsule().fill(Palette.segOff)
                                    Capsule().fill(Palette.meter).frame(width: g.size.width * share)
                                }
                            }
                            .frame(height: 4)
                            Text("\(share > 0 ? (share < 0.01 ? "<1%" : "\(Int((share * 100).rounded()))%") : "—") · \(Fmt.count(r.observations)) calls")
                                .font(.caption2).foregroundStyle(Palette.fg3).fixedSize()
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(r.id == nil)
            }
            if rows.count > limit {
                Button(showAll ? "Show top \(limit)" : "Show all \(rows.count)") { showAll.toggle() }.font(.footnote).tint(Palette.brand)
            }
        }
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
