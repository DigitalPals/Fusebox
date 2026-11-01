import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

@main
struct FuseboxWidgetBundle: WidgetBundle {
    var body: some Widget {
        CombinedWeeklyWidget()
        SubscriptionWidget()
        OutageLiveActivity()
    }
}

// MARK: - Combined weekly limit per provider

struct PoolEntry: TimelineEntry {
    var date: Date
    var data: WidgetData
}

struct PoolProvider: TimelineProvider {
    func placeholder(in context: Context) -> PoolEntry { PoolEntry(date: .now, data: .preview) }

    func getSnapshot(in context: Context, completion: @escaping @Sendable (PoolEntry) -> Void) {
        if context.isPreview { completion(placeholder(in: context)); return }
        Task { completion(PoolEntry(date: .now, data: await WidgetData.load())) }
    }

    func getTimeline(in context: Context, completion: @escaping @Sendable (Timeline<PoolEntry>) -> Void) {
        Task {
            let data = await WidgetData.load()
            completion(Timeline(entries: [PoolEntry(date: .now, data: data)], policy: .after(data.nextRefresh())))
        }
    }
}

struct CombinedWeeklyWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "combined-weekly", provider: PoolProvider()) { entry in
            CombinedWeeklyView(entry: entry)
                .containerBackground(Palette.bg, for: .widget)
                .widgetURL(URL(string: "fusebox://overview"))
        }
        .configurationDisplayName("Combined weekly limit")
        .description("Each provider’s subscriptions together, with the even-pace mark and the next reset.")
        .supportedFamilies([.systemSmall, .systemMedium, .accessoryRectangular, .accessoryCircular])
    }
}

struct CombinedWeeklyView: View {
    @Environment(\.widgetFamily) private var family
    var entry: PoolEntry

    var body: some View {
        let pools = entry.data.pools
        if !entry.data.connected {
            NotConnected()
        } else if pools.isEmpty {
            Text(entry.data.accounts.isEmpty ? "No data" : "No combined limits").font(.caption).foregroundStyle(Palette.fg3)
        } else {
            switch family {
            case .accessoryCircular: circular(pools[0])
            case .accessoryRectangular: rectangular(pools[0])
            case .systemMedium:
                VStack(alignment: .leading, spacing: 8) {
                    header
                    ForEach(pools.prefix(3)) { PoolRow(pool: $0, compact: false) }
                    Spacer(minLength: 0)
                    footer
                }
            default:
                VStack(alignment: .leading, spacing: 6) {
                    header
                    ForEach(pools.prefix(2)) { PoolRow(pool: $0, compact: true) }
                    Spacer(minLength: 0)
                    footer
                }
            }
        }
    }

    private var header: some View {
        HStack(spacing: 4) {
            Text("Combined weekly").capsLabel(10)
            Spacer(minLength: 0)
            let n = entry.data.faults.filter(\.countsOnBadge).count
            if n > 0 {
                Text("\(n)").font(Typeface.data(10, weight: .semibold)).foregroundStyle(Palette.warn)
                Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 9)).foregroundStyle(Palette.warn)
            }
        }
    }

    private var footer: some View {
        Text(entry.data.stale ? "Last seen \(Fmt.hm(entry.data.date))" : "Updated \(Fmt.hm(entry.data.date))")
            .font(.system(size: 9)).foregroundStyle(Palette.fg3)
    }

    private func rectangular(_ p: WeeklyPool) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Text("\(p.providerName) weekly").font(.headline).widgetAccentable()
                Spacer(minLength: 0)
                Text(p.reading.map { Fmt.percent($0.used) } ?? "–").font(.headline).monospacedDigit()
            }
            Gauge(value: p.reading?.used ?? 0, in: 0...100) {}.gaugeStyle(.accessoryLinearCapacity)
            if let r = p.pool.outlook?.nextResetAt {
                Text("↺ \(Fmt.compactLeft(r, now: entry.date)) · \(p.status.text)").font(.caption2)
            } else {
                Text(p.status.text).font(.caption2)
            }
        }
    }

    private func circular(_ p: WeeklyPool) -> some View {
        Gauge(value: p.reading?.used ?? 0, in: 0...100) {
            Text("WK")
        } currentValueLabel: {
            Text(p.reading.map { "\(Int($0.used.rounded()))" } ?? "–")
        }
        .gaugeStyle(.accessoryCircularCapacity)
        .widgetAccentable()
    }
}

struct PoolRow: View {
    var pool: WeeklyPool
    var compact: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 5) {
                ProviderLogo(pool.logo, size: 12)
                Text(pool.providerName).font(.caption.weight(.medium)).foregroundStyle(Palette.fg).lineLimit(1)
                Spacer(minLength: 2)
                Text(pool.reading.map { Fmt.percent($0.used) } ?? "–").font(Typeface.data(12, weight: .medium))
                    .foregroundStyle(pool.reading.map { $0.level == .normal ? Palette.fg : Palette.level($0.level) } ?? Palette.fg3)
            }
            SegmentedMeter(pool.reading, pace: pool.reading == nil ? nil : pool.pool.pace, height: compact ? 7 : 8,
                           label: "\(pool.providerName) combined weekly")
            HStack(spacing: 4) {
                Text(pool.status.text).foregroundStyle(statusColor)
                if let r = pool.pool.outlook?.nextResetAt { Text("· ↺ \(Fmt.compactLeft(r))").foregroundStyle(Palette.fg3) }
            }
            .font(.system(size: compact ? 9.5 : 10.5))
            .lineLimit(1)
        }
    }

    private var statusColor: Color {
        switch pool.status.level { case "err": Palette.err; case "warn": Palette.warn; case "ok": Palette.ok; default: Palette.fg2 }
    }
}

struct NotConnected: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Image("Mark").resizable().scaledToFit().frame(height: 20)
            Text("Open Fusebox to connect").font(.caption).foregroundStyle(Palette.fg2)
        }
    }
}

// MARK: - One subscription's meters

struct AccountEntry: TimelineEntry {
    var date: Date
    var account: Account?
    var data: WidgetData
}

struct AccountProvider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> AccountEntry {
        AccountEntry(date: .now, account: WidgetData.preview.accounts.first, data: .preview)
    }

    func snapshot(for configuration: SelectAccountIntent, in context: Context) async -> AccountEntry {
        if context.isPreview { return placeholder(in: context) }
        return entry(configuration, await WidgetData.load())
    }

    func timeline(for configuration: SelectAccountIntent, in context: Context) async -> Timeline<AccountEntry> {
        let data = await WidgetData.load()
        return Timeline(entries: [entry(configuration, data)], policy: .after(data.nextRefresh()))
    }

    private func entry(_ c: SelectAccountIntent, _ data: WidgetData) -> AccountEntry {
        let account = c.account.flatMap { e in data.accounts.first { $0.id == e.id } } ?? data.accounts.first(where: \.metered)
        return AccountEntry(date: .now, account: account, data: data)
    }
}

struct SubscriptionWidget: Widget {
    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: "subscription", intent: SelectAccountIntent.self, provider: AccountProvider()) { entry in
            SubscriptionWidgetView(entry: entry)
                .containerBackground(Palette.bg, for: .widget)
                .widgetURL(entry.account.map { DeepLink.url(account: $0.id) })
        }
        .configurationDisplayName("Subscription")
        .description("One subscription’s 5-hour and weekly meters with the even-pace mark.")
        .supportedFamilies([.systemSmall, .systemMedium, .accessoryRectangular])
    }
}

struct SubscriptionWidgetView: View {
    @Environment(\.widgetFamily) private var family
    var entry: AccountEntry

    var body: some View {
        if !entry.data.connected {
            NotConnected()
        } else if let a = entry.account {
            switch family {
            case .accessoryRectangular: rectangular(a)
            default: home(a)
            }
        } else {
            Text("Choose a subscription").font(.caption).foregroundStyle(Palette.fg3)
        }
    }

    private func home(_ a: Account) -> some View {
        let st = a.state(now: entry.date)
        return VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 6) {
                ProviderLogo(a.logo, size: 14)
                VStack(alignment: .leading, spacing: 0) {
                    Text(a.label).font(.caption.weight(.medium)).foregroundStyle(Palette.fg).lineLimit(1)
                    Text(a.planName.map { "\(a.providerName) \($0)" } ?? a.providerName).font(.system(size: 10)).foregroundStyle(Palette.fg3)
                }
                Spacer(minLength: 0)
                StatusDot(st.color, size: 7)
            }
            ForEach([true, false], id: \.self) { short in
                if !(short && a.noShortLimit) { meter(a, short: short) }
            }
            Spacer(minLength: 0)
            if st.kind == .cooling, let until = st.until {
                (Text("\(st.word) · back in ") + Text(until, style: .relative)).font(.system(size: 10)).foregroundStyle(Palette.warn).lineLimit(1)
            } else {
                Text(entry.data.stale ? "Last seen \(Fmt.hm(entry.data.date))" : "Updated \(Fmt.hm(entry.data.date))")
                    .font(.system(size: 9)).foregroundStyle(Palette.fg3)
            }
        }
    }

    private func meter(_ a: Account, short: Bool) -> some View {
        let w = a.window(short: short, now: entry.date)
        let r = QuotaReading(w, now: entry.date)
        return VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 4) {
                Text(Quotas.label(w, short: short)).font(Typeface.label(10)).foregroundStyle(Palette.fg3)
                Spacer(minLength: 0)
                Text(r.map { Fmt.percent($0.used) } ?? "–").font(Typeface.data(11, weight: .medium))
                    .foregroundStyle(r.map { $0.level == .normal ? Palette.fg : Palette.level($0.level) } ?? Palette.fg3)
                if let reset = w?.resetsAt, family != .systemSmall {
                    Text("↺ \(Fmt.compactLeft(reset, now: entry.date))").font(Typeface.data(10)).foregroundStyle(Palette.fg3)
                }
            }
            SegmentedMeter(r, pace: Quotas.elapsed(w, now: entry.date), height: 7, label: w.map(Quotas.title) ?? "Quota")
        }
    }

    private func rectangular(_ a: Account) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(a.label).font(.headline).lineLimit(1).widgetAccentable()
            ForEach([true, false], id: \.self) { short in
                if !(short && a.noShortLimit) {
                    let w = a.window(short: short, now: entry.date)
                    let r = QuotaReading(w, now: entry.date)
                    HStack(spacing: 4) {
                        Text(Quotas.label(w, short: short)).font(.caption2.weight(.semibold)).frame(width: 22, alignment: .leading)
                        Gauge(value: r?.used ?? 0, in: 0...100) {}.gaugeStyle(.accessoryLinearCapacity)
                        Text(r.map { Fmt.percent($0.used) } ?? "–").font(.caption2).monospacedDigit()
                    }
                }
            }
        }
    }
}

// MARK: - Live Activity: a provider out of accounts

struct OutageLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: OutageAttributes.self) { context in
            OutageLockScreen(context: context)
                .activityBackgroundTint(Palette.bg)
                .activitySystemActionForegroundColor(Palette.fg)
                .widgetURL(URL(string: "fusebox://accounts"))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    HStack(spacing: 6) {
                        ProviderLogo(Provider.logo(provider: context.attributes.provider), size: 18)
                        Text(context.attributes.providerName).font(.headline)
                    }
                }
                DynamicIslandExpandedRegion(.trailing) {
                    countdown(context.state).font(.title3.monospacedDigit()).foregroundStyle(Palette.brand)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    Text(context.state.recovered ? context.state.title : "\(context.state.title). \(context.state.detail).")
                        .font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
            } compactLeading: {
                ProviderLogo(Provider.logo(provider: context.attributes.provider), size: 16)
            } compactTrailing: {
                countdown(context.state).monospacedDigit().frame(maxWidth: 64).foregroundStyle(Palette.brand)
            } minimal: {
                Image(systemName: context.state.recovered ? "checkmark" : "bolt.slash.fill").foregroundStyle(Palette.brand)
            }
            .widgetURL(URL(string: "fusebox://accounts"))
        }
    }

    @ViewBuilder func countdown(_ s: OutageAttributes.ContentState) -> some View {
        if s.recovered {
            Text("Back")
        } else if let back = s.backAt, back > .now {
            Text(timerInterval: Date.now...back, countsDown: true)
        } else {
            Text("—")
        }
    }
}

struct OutageLockScreen: View {
    var context: ActivityViewContext<OutageAttributes>

    var body: some View {
        let s = context.state
        HStack(alignment: .center, spacing: 12) {
            ProviderLogo(Provider.logo(provider: context.attributes.provider), size: 28)
            VStack(alignment: .leading, spacing: 3) {
                Text(s.title).font(.headline).foregroundStyle(Palette.fg).lineLimit(2)
                Text(s.recovered ? s.detail : s.detail).font(.caption).foregroundStyle(Palette.fg3)
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 2) {
                if s.recovered {
                    Image(systemName: "checkmark.circle.fill").font(.title2).foregroundStyle(Palette.ok)
                } else if let back = s.backAt, back > .now {
                    Text(timerInterval: Date.now...back, countsDown: true)
                        .font(.title2.weight(.semibold).monospacedDigit())
                        .foregroundStyle(Palette.brand)
                        .multilineTextAlignment(.trailing)
                        .frame(maxWidth: 120, alignment: .trailing)
                    Text("Back \(Fmt.when(back))").font(.caption2).foregroundStyle(Palette.fg3)
                } else {
                    Text("Sign in again").font(.caption).foregroundStyle(Palette.warn)
                }
            }
        }
        .padding(16)
    }
}
