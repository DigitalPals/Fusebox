import Charts
import SwiftUI

struct OverviewView: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate
    @Environment(\.horizontalSizeClass) private var sizeClass

    private enum Block: String, Identifiable { case faults, load, pools, other, latest; var id: String { rawValue } }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                header
                if let e = store.loadError, store.overview == nil { errorCard(e) }
                if sizeClass == .compact {
                    // Phones: the dashboard's phone order, subscriptions before load.
                    if !store.faults.isEmpty { FaultsCard() }
                    subscriptions
                    if !store.weeklyPools.isEmpty { PoolsCard() }
                    LoadCard()
                } else {
                    FoldAwareColumns(items: topBlocks, minColumnWidth: 360) { block(for: $0) }
                    subscriptions
                }
                FoldAwareColumns(items: bottomBlocks, minColumnWidth: 360) { block(for: $0) }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .clearOfFold()
        }
        .background(Palette.bg)
        .refreshable { await store.refreshAll() }
        .navigationTitle("Overview")
    }

    @ViewBuilder private var subscriptions: some View {
                if !store.subscriptions.isEmpty {
                    VStack(alignment: .leading, spacing: 10) {
                        SectionHead("Subscriptions", meta: subsMeta) { QuotaModeToggle() }
                        FoldAwareColumns(items: store.subscriptions, minColumnWidth: 340, spacing: 12) { a in
                            SubscriptionCard(account: a)
                        }
                    }
                }
    }

    private var topBlocks: [Block] {
        (store.faults.isEmpty ? [] : [.faults]) + [.load] + (store.weeklyPools.isEmpty ? [] : [.pools])
    }

    private var bottomBlocks: [Block] {
        (store.accounts.contains { !$0.metered } ? [.other] : []) + [.latest]
    }

    @ViewBuilder private func block(for b: Block) -> some View {
        switch b {
        case .faults: FaultsCard()
        case .load: LoadCard()
        case .pools: PoolsCard()
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
        HStack(spacing: 8) {
            LiveBadge(state: store.live).accessibilityIdentifier("overview")
            if let v = store.overview?.version {
                Text("Fusebox \(v.split(separator: "+").first.map(String.init) ?? v)")
                    .font(Typeface.data(11)).foregroundStyle(Palette.fg3)
            }
            Spacer()
        }
    }

    private func errorCard(_ text: String) -> some View {
        Card {
            Text("Can’t reach Fusebox").font(.headline).foregroundStyle(Palette.fg)
            Text(text).font(.subheadline).foregroundStyle(Palette.fg2)
            Button("Try again") { Task { await store.refreshAll() } }.buttonStyle(.bordered)
        }
    }
}

struct LiveBadge: View {
    var state: AppStore.LiveState
    var body: some View {
        HStack(spacing: 6) {
            StatusDot(state == .live ? Palette.ok : state == .connecting ? Palette.idle : Palette.warn, size: 7)
            Text(state == .live ? "Live" : state == .connecting ? "Connecting" : "Reconnecting")
                .font(.caption).foregroundStyle(Palette.fg2)
        }
        .accessibilityElement(children: .combine)
    }
}

struct QuotaModeToggle: View {
    @Environment(AppStore.self) private var store
    var body: some View {
        @Bindable var store = store
        Picker("Show", selection: $store.quotaDisplay) {
            Text("Used").tag(QuotaDisplay.used)
            Text("Left").tag(QuotaDisplay.remaining)
        }
        .pickerStyle(.segmented)
        .fixedSize()
        .controlSize(.small)
    }
}

// MARK: - Cards

struct SubscriptionCard: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate
    var account: Account

    var body: some View {
        Button { navigate(.account(account.id)) } label: {
            Card(padding: 12) {
                HStack(alignment: .top, spacing: 10) {
                    ProviderLogo(account.logo, size: 18).padding(.top, 2)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(account.label).font(.subheadline.weight(.medium)).foregroundStyle(Palette.fg).lineLimit(1)
                        Text(account.subtitle).font(.caption).foregroundStyle(Palette.fg3).lineLimit(1)
                    }
                    Spacer(minLength: 6)
                    StatusLabel(account: account, inFlight: store.inFlight(account.id))
                }
                AccountMeters(account: account, mode: store.quotaDisplay)
                metaLine
            }
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("subscription-\(account.id)")
    }

    @ViewBuilder private var metaLine: some View {
        let st = account.state()
        let banked = account.hasBankedResets(enabled: store.overview?.bankedResets ?? false)
            && (account.bankedNeedsReview || (account.bankedCount ?? 0) > 0)
        VStack(alignment: .leading, spacing: 3) {
            if st.kind == .cooling {
                Text("\(st.model == "*" ? "All models" : st.model ?? "") · \(st.resetUnknown ? "Reset time unknown" : "back \(Fmt.when(st.until ?? .now))")")
                    .font(.caption).foregroundStyle(Palette.fg2)
            }
            if banked {
                Text("↻ \(account.bankedNeedsReview ? "Review reset" : account.bankedLabel)\(account.bankedResetPlan?.prioritized == true ? " · prioritized" : "")")
                    .font(.caption).foregroundStyle(account.bankedNeedsReview || account.bankedResetPlan?.prioritized == true ? Palette.warn : Palette.fg2)
            }
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                Text("\(Fmt.count(account.counters?.requests)) requests · \(store.inFlight(account.id) > 0 ? "in use now" : Fmt.ago(account.lastUsed, now: ctx.date))")
                    .font(.caption).foregroundStyle(Palette.fg3)
            }
        }
    }
}

struct LoadCard: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        let series = store.lastHour
        let nowMinute = Int(Date.now.timeIntervalSince1970 / 60)
        let requests = series.reduce(0) { $0 + $1.requests }
        let failed = series.reduce(0) { $0 + ($1.failed ?? 0) }
        let cancelled = series.reduce(0) { $0 + ($1.cancelled ?? 0) }
        let completed = requests - cancelled
        let success = completed > 0 ? Double(completed - failed) / Double(completed) * 100 : nil
        let tokens = series.reduce(0) { $0 + ($1.tokens ?? 0) }
        Card {
            Text("Load · last 60 min").capsLabel()
            HStack(spacing: 18) {
                figure(Fmt.count(requests), "Requests")
                figure(success.map { String(format: $0 == 100 ? "%.0f%%" : "%.1f%%", $0) } ?? "—", "Success")
                figure(Fmt.count(tokens), "Tokens")
                figure(Fmt.count(store.overview?.active), "In flight")
            }
            Chart(series) { b in
                BarMark(x: .value("Minute", b.date, unit: .minute), y: .value("Requests", b.requests), width: .ratio(0.7))
                    .foregroundStyle(b.minute == nowMinute ? Palette.brand : (b.failed ?? 0) > 0 ? Palette.err : Palette.meter.opacity(0.55))
            }
            .chartYScale(domain: 0...max(4, series.map(\.requests).max() ?? 0))
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .frame(height: 64)
            .accessibilityLabel("\(Fmt.plural(requests, "request")) in the last hour")
            HStack { Text("60 min ago"); Spacer(); Text("now") }
                .font(Typeface.data(10)).foregroundStyle(Palette.fg3)
        }
    }

    private func figure(_ value: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(value).font(.system(size: 17, weight: .medium)).monospacedDigit().foregroundStyle(Palette.fg)
            Text(label).font(.caption2).foregroundStyle(Palette.fg3)
        }
        .accessibilityElement(children: .combine)
    }
}

/// "Combined weekly quota": each provider's subscriptions together.
struct PoolsCard: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        let pools = store.weeklyPools
        Card {
            Text("Combined weekly quota").capsLabel().accessibilityIdentifier("combined-weekly")
            Text(basis(pools)).font(.caption).foregroundStyle(Palette.fg3)
            ForEach(pools) { p in
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 8) {
                        ProviderLogo(p.logo, size: 16)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(p.providerName).font(.subheadline.weight(.medium)).foregroundStyle(Palette.fg)
                            Text(p.countText).font(.caption).foregroundStyle(Palette.fg3)
                        }
                    }
                    MeterRow(label: "WK", title: "\(p.providerName) combined weekly", reading: p.reading,
                             pace: p.reading == nil ? nil : p.pool.pace, resetsAt: p.pool.outlook?.nextResetAt, mode: store.quotaDisplay)
                    HStack(spacing: 6) {
                        Text(p.status.text).font(.footnote.weight(.medium)).foregroundStyle(color(p.status.level))
                        Text(p.detail()).font(.caption).foregroundStyle(Palette.fg3)
                    }
                }
                .padding(.top, 4)
            }
        }
    }

    private func color(_ level: String) -> Color {
        level == "err" ? Palette.err : level == "warn" ? Palette.warn : level == "ok" ? Palette.ok : Palette.fg2
    }

    private func basis(_ pools: [WeeklyPool]) -> String {
        let priced = pools.filter { $0.pool.priced == true }.map(\.providerName)
        let equal = pools.filter { $0.pool.priced != true }.map(\.providerName)
        if equal.isEmpty { return "Each provider’s subscriptions together, weighted by plan allowance" }
        if priced.isEmpty { return "Each provider’s subscriptions together, weighted equally" }
        return "\(priced.joined(separator: ", ")) weighted by plan allowance, \(equal.joined(separator: ", ")) equally"
    }
}

struct FaultsCard: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate

    var body: some View {
        Card {
            HStack {
                Text("Faults").capsLabel()
                Text("\(store.faults.count)").font(Typeface.data(11)).foregroundStyle(Palette.fg3)
                Spacer()
                Button("All") { navigate(.faults) }.font(.footnote).tint(Palette.brand)
            }
            ForEach(store.faults.prefix(4)) { f in
                FaultRow(fault: f)
            }
        }
    }
}

struct OtherCircuitsCard: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate

    var body: some View {
        Card {
            Text("Other circuits").capsLabel()
            Text("API keys and sign-ins without usage limits").font(.caption).foregroundStyle(Palette.fg3)
            ForEach(store.accounts.filter { !$0.metered }) { a in
                Button { navigate(.account(a.id)) } label: {
                    HStack(spacing: 8) {
                        StatusDot(a.state().color, size: 7)
                        ProviderLogo(a.logo, size: 16)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(a.providerName).font(.footnote).foregroundStyle(Palette.fg)
                            Text(a.label).font(.caption).foregroundStyle(Palette.fg3).lineLimit(1)
                        }
                        Spacer()
                        VStack(alignment: .trailing, spacing: 1) {
                            Text(a.statusWord(inFlight: store.inFlight(a.id))).font(.caption).foregroundStyle(Palette.fg2)
                            Text("\(Fmt.count(a.counters?.requests)) req").font(.caption2).foregroundStyle(Palette.fg3)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }
}

struct LatestRequestsCard: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate

    var body: some View {
        Card {
            HStack {
                Text("Latest requests").capsLabel()
                Spacer()
                Button("All requests") { navigate(.requests) }.font(.footnote).tint(Palette.brand)
            }
            if store.requests.isEmpty {
                Text("No requests yet.").font(.footnote).foregroundStyle(Palette.fg3)
            }
            ForEach(store.requests.prefix(6)) { r in
                Button { navigate(.request(r.id)) } label: { RequestRow(request: r, compact: true) }
                    .buttonStyle(.plain)
                if r.id != store.requests.prefix(6).last?.id { Divider().overlay(Palette.line) }
            }
        }
    }
}
