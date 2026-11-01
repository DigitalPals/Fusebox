import Charts
import SwiftUI

struct OverviewView: View {
    @Environment(AppStore.self) private var store
    @Environment(\.horizontalSizeClass) private var sizeClass

    private enum Block: String, Identifiable { case faults, load, other, latest; var id: String { rawValue } }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Metrics.sectionGap) {
                header
                if let e = store.loadError, store.overview == nil { errorCard(e) }
                if sizeClass == .compact {
                    // Phones: the dashboard's phone order, subscriptions before load.
                    if !store.faults.isEmpty { FaultsCard() }
                    subscriptions
                    pools
                    LoadCard()
                    FoldAwareColumns(items: bottomBlocks, minColumnWidth: 360) { block(for: $0) }
                } else {
                    FoldAwareColumns(items: topBlocks, minColumnWidth: 360) { block(for: $0) }
                    subscriptions
                    pools
                    FoldAwareColumns(items: bottomBlocks, minColumnWidth: 360) { block(for: $0) }
                }
            }
            .pageContent()
        }
        .background(Palette.bg)
        .refreshable { await store.refreshAll() }
        .navigationTitle(AppSection.overview.pageTitle)
    }

    @ViewBuilder private var subscriptions: some View {
        if !store.subscriptions.isEmpty {
            VStack(alignment: .leading, spacing: 12) {
                SectionHead("Subscriptions", meta: subsMeta) { QuotaModeToggle() }
                FoldAwareColumns(items: store.subscriptions, minColumnWidth: 340) { column in
                    RowsPanel(items: column, rivets: true) { SubscriptionRow(account: $0) }
                }
            }
        }
    }

    @ViewBuilder private var pools: some View {
        let pools = store.weeklyPools
        if !pools.isEmpty {
            VStack(alignment: .leading, spacing: 12) {
                SectionHead("Combined weekly limits", meta: PoolsBasis.text(pools))
                    .accessibilityIdentifier("combined-weekly")
                FoldAwareColumns(items: pools, minColumnWidth: 340) { column in
                    RowsPanel(items: column) { PoolRow(pool: $0) }
                }
            }
        }
    }

    private var topBlocks: [Block] { (store.faults.isEmpty ? [] : [.faults]) + [.load] }

    private var bottomBlocks: [Block] {
        (store.accounts.contains { !$0.metered } ? [.other] : []) + [.latest]
    }

    @ViewBuilder private func block(for b: Block) -> some View {
        switch b {
        case .faults: FaultsCard()
        case .load: LoadCard()
        case .other: OtherCircuitsCard()
        case .latest: LatestRequestsCard()
        }
    }

    private var subsMeta: String {
        let subs = store.subscriptions
        let ready = subs.filter { $0.state().kind == .ready }
        let serving = store.live == .live ? "\(ready.filter { store.inFlight($0.id) > 0 }.count) serving · " : ""
        return "\(serving)\(ready.count) of \(subs.count) ready · \(Routing.sentence(store.overview, short: true))"
    }

    private var header: some View {
        HStack(spacing: 10) {
            LiveBadge(state: store.live).accessibilityIdentifier("overview")
            if let v = store.overview?.version {
                Text("Fusebox \(v.split(separator: "+").first.map(String.init) ?? v)")
                    .font(Typeface.data(12)).foregroundStyle(Palette.fg3)
            }
            Spacer(minLength: 0)
        }
    }

    private func errorCard(_ text: String) -> some View {
        Card {
            HStack(spacing: 8) {
                StatusDot(Palette.err)
                Text("Can’t reach Fusebox").font(Typeface.sans(16, .semibold, relativeTo: .headline)).foregroundStyle(Palette.fg)
            }
            Text(text).font(Typeface.row).foregroundStyle(Palette.fg2)
            Button("Try again") { Task { await store.refreshAll() } }.buttonStyle(.outline)
        }
    }
}

/// A panel of rows separated by hairlines, as the dashboard's tables on a phone.
struct RowsPanel<Item: Identifiable, Row: View>: View {
    var items: [Item]
    var rivets = false
    @ViewBuilder var row: (Item) -> Row

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                if i > 0 { Hairline().padding(.horizontal, Metrics.panelPadding) }
                row(item)
                    .padding(.horizontal, Metrics.panelPadding)
                    .padding(.vertical, 14)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelBackground(rivets: rivets)
    }
}

struct LiveBadge: View {
    var state: AppStore.LiveState
    var body: some View {
        HStack(spacing: 7) {
            StatusDot(state == .live ? Palette.ok : state == .connecting ? Palette.idle : Palette.warn)
            Text(state == .live ? "Live" : state == .connecting ? "Connecting" : "Reconnecting")
                .font(Typeface.meta).foregroundStyle(Palette.fg2)
        }
        .accessibilityElement(children: .combine)
    }
}

struct QuotaModeToggle: View {
    @Environment(AppStore.self) private var store
    var body: some View {
        @Bindable var store = store
        FuseSegmented("Show", selection: $store.quotaDisplay, options: [(.used, "Used"), (.remaining, "Left")])
    }
}

// MARK: - Rows and panels

/// One subscription: who, its status, its 5H and WK meters and a meta line. The same row
/// on the Overview and in Accounts.
struct SubscriptionRow: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate
    var account: Account
    /// In Accounts' split layout the row selects instead of navigating.
    var select: (() -> Void)?
    var selected = false

    var body: some View {
        Button { if let select { select() } else { navigate(.account(account.id)) } } label: {
            AccountRowContent(account: account, selected: selected)
        }
        .buttonStyle(RowButtonStyle())
        .accessibilityIdentifier("subscription-\(account.id)")
    }
}

/// A row's pressed state: a `seg-on` wash behind it.
struct RowButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .contentShape(Rectangle())
            .background(configuration.isPressed ? Palette.ctlHover.padding(-8) : nil)
    }
}

struct AccountRowContent: View {
    @Environment(AppStore.self) private var store
    var account: Account
    var selected = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                ProviderLogo(account.logo, size: 20).padding(.top, 1)
                VStack(alignment: .leading, spacing: 2) {
                    Text(account.label).font(Typeface.rowMedium).foregroundStyle(Palette.fg).lineLimit(1)
                        .truncationMode(.middle)
                    Text(account.subtitle).font(Typeface.small).foregroundStyle(Palette.fg3).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                StatusLabel(account: account, inFlight: store.inFlight(account.id), font: Typeface.meta)
                    .padding(.top, 1)
                    .layoutPriority(1)
            }
            if account.metered {
                AccountMeters(account: account, mode: store.quotaDisplay)
            }
            metaLine
        }
        .padding(selected ? 8 : 0)
        .background(selected ? Palette.segOn : .clear, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .padding(selected ? -8 : 0)
        .contentShape(Rectangle())
    }

    @ViewBuilder private var metaLine: some View {
        let st = account.state()
        let banked = account.hasBankedResets(enabled: store.overview?.bankedResets ?? false)
            && (account.bankedNeedsReview || (account.bankedCount ?? 0) > 0)
        VStack(alignment: .leading, spacing: 3) {
            if st.kind == .cooling {
                Text("\(st.model == "*" ? "All models" : st.model ?? "") · \(st.resetUnknown ? "Reset time unknown" : "back \(Fmt.when(st.until ?? .now))")")
                    .font(Typeface.small).foregroundStyle(Palette.fg2)
            }
            if banked {
                Text("↻ \(account.bankedNeedsReview ? "Review reset" : account.bankedLabel)\(account.bankedResetPlan?.prioritized == true ? " · prioritized" : "")")
                    .font(Typeface.small).foregroundStyle(account.bankedNeedsReview ? Palette.warn : Palette.fg2)
            }
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                Text("\(Fmt.count(account.counters?.requests)) requests · \(store.inFlight(account.id) > 0 ? "in use now" : "last used \(Fmt.ago(account.lastUsed, now: ctx.date))")")
                    .font(Typeface.small).foregroundStyle(Palette.fg3)
            }
        }
    }
}

/// A provider's combined weekly limit: its subscriptions together, with the weighted even
/// pace, the next reset and the forecast's verdict.
struct PoolRow: View {
    @Environment(AppStore.self) private var store
    var pool: WeeklyPool

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                ProviderLogo(pool.logo, size: 20)
                VStack(alignment: .leading, spacing: 2) {
                    Text(pool.providerName).font(Typeface.rowMedium).foregroundStyle(Palette.fg)
                    Text(pool.countText).font(Typeface.small).foregroundStyle(Palette.fg3)
                }
            }
            MeterRow(label: "WK", title: "\(pool.providerName) combined weekly", reading: pool.reading,
                     pace: pool.reading == nil ? nil : pool.pool.pace, resetsAt: pool.pool.outlook?.nextResetAt, mode: store.quotaDisplay)
            VStack(alignment: .leading, spacing: 2) {
                Text(pool.status.text).font(Typeface.sans(13, .medium, relativeTo: .footnote)).foregroundStyle(Palette.state(pool.status.level))
                Text(pool.detail()).font(Typeface.small).foregroundStyle(Palette.fg3)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

enum PoolsBasis {
    static func text(_ pools: [WeeklyPool]) -> String {
        let priced = pools.filter { $0.pool.priced == true }.map(\.providerName)
        let equal = pools.filter { $0.pool.priced != true }.map(\.providerName)
        if equal.isEmpty { return "Each provider’s subscriptions together, weighted by plan price" }
        if priced.isEmpty { return "Each provider’s subscriptions together, weighted equally" }
        return "\(priced.joined(separator: ", ")) weighted by plan price, \(equal.joined(separator: ", ")) equally"
    }
}

struct LoadCard: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        let series = store.lastHour
        let requests = series.reduce(0) { $0 + $1.requests }
        let failed = series.reduce(0) { $0 + ($1.failed ?? 0) }
        let cancelled = series.reduce(0) { $0 + ($1.cancelled ?? 0) }
        let completed = requests - cancelled
        let success = completed > 0 ? Double(completed - failed) / Double(completed) * 100 : nil
        let tokens = series.reduce(0) { $0 + ($1.tokens ?? 0) }
        Card(spacing: 14) {
            PanelHead("Load · last 60 min")
            HStack(alignment: .top, spacing: 0) {
                Figure(value: Fmt.count(requests), label: "Requests")
                Figure(value: success.map { String(format: $0 == 100 ? "%.0f%%" : "%.1f%%", $0) } ?? "—", label: "Success",
                       color: success.map { $0 < 95 ? Palette.warn : Palette.fg } ?? Palette.fg)
                Figure(value: Fmt.count(tokens), label: "Tokens")
                Figure(value: Fmt.count(store.overview?.active), label: "In flight")
            }
            LoadChart(series: series, height: 64)
            HStack { Text("60 min ago"); Spacer(); Text("now") }
                .font(Typeface.data(11)).foregroundStyle(Palette.fg3)
        }
    }
}

/// A figure and its label, as the dashboard's load panel: 20pt medium value, `fg-3` label.
struct Figure: View {
    var value: String
    var label: String
    var color = Palette.fg
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(value).font(Typeface.figure).foregroundStyle(color).lineLimit(1).minimumScaleFactor(0.7)
            Text(label).font(Typeface.small).foregroundStyle(Palette.fg3).lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

/// Requests a minute over the last hour: neutral `bar` columns, the failed part in `err` on
/// top, the current minute in `meter`.
struct LoadChart: View {
    var series: [MinuteBucket]
    var height: CGFloat

    var body: some View {
        let nowMinute = Int(Date.now.timeIntervalSince1970 / 60)
        Chart(series) { b in
            let failed = min(b.requests, b.failed ?? 0)
            BarMark(x: .value("Minute", b.date, unit: .minute), y: .value("Requests", b.requests - failed), width: .ratio(0.72))
                .foregroundStyle(b.minute == nowMinute ? Palette.meter : Palette.bar)
            if failed > 0 {
                BarMark(x: .value("Minute", b.date, unit: .minute), y: .value("Failed", failed), width: .ratio(0.72))
                    .foregroundStyle(Palette.err)
            }
        }
        .chartYScale(domain: 0...max(4, series.map(\.requests).max() ?? 0))
        .chartXAxis(.hidden)
        .chartYAxis(.hidden)
        .chartPlotStyle { $0.overlay(alignment: .bottom) { Hairline() } }
        .frame(height: height)
        .accessibilityLabel("\(Fmt.plural(series.reduce(0) { $0 + $1.requests }, "request")) in the last hour")
    }
}

struct FaultsCard: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate

    var body: some View {
        Card(spacing: 0) {
            PanelHead("Faults · \(store.faults.count)") { LinkButton("All") { navigate(.faults) } }
                .padding(.bottom, 4)
            ForEach(Array(store.faults.prefix(4).enumerated()), id: \.element.id) { i, f in
                if i > 0 { Hairline() }
                Button { open(f) } label: { FaultRow(fault: f).padding(.vertical, 12) }
                    .buttonStyle(RowButtonStyle())
            }
        }
    }

    private func open(_ f: Fault) {
        if let id = f.accountID { navigate(.account(id)) } else { navigate(.faults) }
    }
}

struct OtherCircuitsCard: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate

    var body: some View {
        let others = store.accounts.filter { !$0.metered }
        Card(spacing: 0) {
            PanelHead("Other circuits").padding(.bottom, 2)
            Text("API keys and sign-ins without usage limits").font(Typeface.small).foregroundStyle(Palette.fg3)
                .padding(.bottom, 6)
            ForEach(Array(others.enumerated()), id: \.element.id) { i, a in
                if i > 0 { Hairline() }
                Button { navigate(.account(a.id)) } label: {
                    HStack(spacing: 12) {
                        ProviderLogo(a.logo, size: 18)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(a.providerName).font(Typeface.row).foregroundStyle(Palette.fg)
                            Text(a.label).font(Typeface.small).foregroundStyle(Palette.fg3).lineLimit(1)
                        }
                        Spacer(minLength: 8)
                        VStack(alignment: .trailing, spacing: 2) {
                            StatusLabel(account: a, inFlight: store.inFlight(a.id), font: Typeface.meta)
                            Text("\(Fmt.count(a.counters?.requests)) req").font(Typeface.data(11.5)).foregroundStyle(Palette.fg3)
                        }
                    }
                    .padding(.vertical, 12)
                }
                .buttonStyle(RowButtonStyle())
            }
        }
    }
}

struct LatestRequestsCard: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate

    var body: some View {
        let latest = Array(store.requests.prefix(6))
        Card(spacing: 0) {
            PanelHead("Latest requests") { LinkButton("All") { navigate(.requests) } }
                .padding(.bottom, 4)
            if latest.isEmpty {
                Text("No requests yet.").font(Typeface.row).foregroundStyle(Palette.fg3).padding(.vertical, 8)
            }
            ForEach(Array(latest.enumerated()), id: \.element.id) { i, r in
                if i > 0 { Hairline() }
                Button { navigate(.request(r.id)) } label: { RequestRow(request: r, compact: true).padding(.vertical, 12) }
                    .buttonStyle(RowButtonStyle())
            }
        }
    }
}
