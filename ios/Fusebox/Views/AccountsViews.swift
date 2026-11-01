import Charts
import SwiftUI

// MARK: - List

struct AccountsList: View {
    @Environment(AppStore.self) private var store
    /// Bound in the split layout; nil on a phone, where rows push the detail.
    var selection: Binding<String?>?

    var body: some View {
        Group {
            if let selection {
                List(selection: selection) { rows }
            } else {
                List { rows }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Palette.bg)
        .refreshable { await store.refreshAll() }
        .navigationTitle("Accounts")
        .overlay {
            if store.accounts.isEmpty {
                ContentUnavailableView("No accounts connected", systemImage: "person.crop.circle.badge.plus",
                                       description: Text("Sign in with a subscription or add an API key from the web dashboard."))
            }
        }
        .accessibilityIdentifier("accounts")
    }

    @ViewBuilder private var rows: some View {
        ForEach(store.groups, id: \.provider) { group in
            Section {
                ForEach(group.accounts) { a in
                    if selection != nil {
                        AccountRow(account: a).tag(a.id)
                    } else {
                        NavigationLink(value: a.id) { AccountRow(account: a) }
                    }
                }
            } header: {
                HStack(spacing: 6) {
                    ProviderLogo(group.accounts[0].logo, size: 14)
                    Text(group.name).capsLabel(11.5)
                    Text("\(group.accounts.count)").font(Typeface.data(10.5)).foregroundStyle(Palette.fg3)
                }
            }
            .listRowBackground(Palette.surface)
        }
    }
}

struct AccountRow: View {
    @Environment(AppStore.self) private var store
    var account: Account

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(account.label).font(.subheadline.weight(.medium)).foregroundStyle(Palette.fg).lineLimit(1)
                    Text("\(account.planName.map { "\($0) · " } ?? "")\(account.authName)")
                        .font(.caption).foregroundStyle(Palette.fg3)
                }
                Spacer(minLength: 4)
                StatusLabel(account: account, inFlight: store.inFlight(account.id))
            }
            if account.metered {
                CompactMeters(account: account, mode: store.quotaDisplay)
            }
        }
        .padding(.vertical, 2)
        .accessibilityIdentifier("account-row-\(account.id)")
    }
}

/// The list's small meters: label, segments and percentage only.
struct CompactMeters: View {
    var account: Account
    var mode: QuotaDisplay
    var body: some View {
        VStack(spacing: 4) {
            if !account.noShortLimit { row(short: true) }
            row(short: false)
        }
    }
    private func row(short: Bool) -> some View {
        let w = account.window(short: short)
        let r = QuotaReading(w)
        return HStack(spacing: 8) {
            Text(Quotas.label(w, short: short)).font(Typeface.label(10)).foregroundStyle(Palette.fg3).frame(width: 26, alignment: .leading)
            SegmentedMeter(r, pace: Quotas.elapsed(w), mode: mode, height: 7, label: w.map(Quotas.title) ?? "Quota").frame(maxWidth: 160)
            Text(r.map { Fmt.percent($0.value(mode)) } ?? "–").font(Typeface.data(11)).foregroundStyle(Palette.fg2)
            Spacer(minLength: 0)
        }
    }
}

// MARK: - Detail

struct AccountDetailView: View {
    @Environment(AppStore.self) private var store
    var id: String

    var body: some View {
        if let a = store.account(id) {
            AccountDetail(account: a)
        } else {
            ContentUnavailableView("This account is gone", systemImage: "person.crop.circle.badge.questionmark",
                                   description: Text("It was removed or its credentials changed."))
        }
    }
}

private enum PendingAction: Identifiable {
    case disable, enable, clearError(String), clearCooldowns, bankedReset(quote: String, grant: String, label: String, detail: String)
    var id: String {
        switch self {
        case .disable: "disable"; case .enable: "enable"; case .clearError: "clear-error"; case .clearCooldowns: "cooldowns"
        case .bankedReset: "banked"
        }
    }
}

struct AccountDetail: View {
    @Environment(AppStore.self) private var store
    @Environment(\.openURL) private var openURL
    var account: Account

    @State private var busy = false
    @State private var toastText: String?
    @State private var pending: PendingAction?
    @State private var errorText: String?

    private enum Block: String, Identifiable { case limits, resets, cooldowns, activity, routing; var id: String { rawValue } }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                head
                if let errorText { Text(errorText).font(.footnote).foregroundStyle(Palette.err) }
                if account.lastError != nil { errorBox }
                FoldAwareColumns(items: blocks, minColumnWidth: 360) { block($0) }
            }
            .padding(16)
            .clearOfFold()
        }
        .background(Palette.bg)
        .navigationTitle(account.label)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { toolbar }
        .task(id: account.id) { await store.loadActivity(account.id) }
        .refreshable { await store.loadActivity(account.id); store.refreshAccounts() }
        .toast($toastText)
        .confirmationDialog(confirmTitle, isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } }),
                            titleVisibility: .visible, presenting: pending) { action in
            confirmButtons(action)
        } message: { action in
            Text(confirmMessage(action))
        }
    }

    private var blocks: [Block] {
        var b: [Block] = []
        if account.metered { b.append(.limits) }
        if account.hasBankedResets(enabled: store.overview?.bankedResets ?? false) { b.append(.resets) }
        b.append(.cooldowns)
        b.append(.activity)
        b.append(.routing)
        return b
    }

    @ViewBuilder private func block(_ b: Block) -> some View {
        switch b {
        case .limits: limits
        case .resets: BankedResetsCard(account: account, busy: busy, refresh: { await run("Checked") { try await store.refreshQuota(account.id) } }, use: useReset)
        case .cooldowns: cooldowns
        case .activity: ActivityCard(account: account)
        case .routing: routing
        }
    }

    // MARK: Head

    private var head: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                ProviderLogo(account.logo, size: 28)
                VStack(alignment: .leading, spacing: 2) {
                    Text(account.label).font(.title3.weight(.semibold)).foregroundStyle(Palette.fg).lineLimit(1).minimumScaleFactor(0.7)
                        .accessibilityIdentifier("account-detail")
                    Text([account.providerName + (account.planName.map { " \($0)" } ?? ""), account.authName].joined(separator: " · "))
                        .font(.footnote).foregroundStyle(Palette.fg3)
                }
            }
            HStack(spacing: 8) {
                StatusLabel(account: account, inFlight: store.inFlight(account.id))
                Text("· last used \(Fmt.ago(account.lastUsed))").font(.footnote).foregroundStyle(Palette.fg3)
            }
            actions
        }
    }

    /// Refresh and the on/off switch: the dashboard's breaker.
    private var actions: some View {
        HStack(spacing: 10) {
            if account.kind == "oauth" && ["claude", "codex"].contains(account.provider) {
                Button {
                    Task { await run("Quota refreshed") { try await store.refreshQuota(account.id) } }
                } label: { Label("Refresh quota", systemImage: "arrow.clockwise") }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("refresh-quota")
            }
            Toggle(isOn: Binding(get: { !account.disabled }, set: { on in pending = on ? .enable : .disable })) {
                Text(account.disabled ? "Disabled" : "Enabled").font(.subheadline)
            }
            .toggleStyle(.switch)
            .fixedSize()
            .tint(Palette.meter)
            .accessibilityIdentifier("account-toggle")
        }
        .disabled(busy)
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .primaryAction) {
            if busy { ProgressView() }
        }
    }

    // MARK: Blocks

    private var errorBox: some View {
        Card {
            HStack {
                Text("Last error").capsLabel()
                Spacer()
                Button("Clear error") { if let id = account.lastErrorID { pending = .clearError(id) } }
                    .font(.footnote).disabled(account.lastErrorID == nil || busy)
            }
            Text(account.lastError ?? "").font(Typeface.data(12)).foregroundStyle(Palette.err).textSelection(.enabled)
            Text("Clears the saved message. A new failure will appear again.").font(.caption).foregroundStyle(Palette.fg3)
        }
    }

    private var limits: some View {
        Card(rivets: true) {
            HStack {
                Text("Limits · \(store.quotaDisplay.word)").capsLabel()
                Spacer()
                QuotaModeToggle()
            }
            let windows = (account.quota?.windows ?? []).filter { QuotaReading($0) != nil }
            if account.noShortLimit {
                WindowBlock(title: "5-hour window", reading: nil, pace: nil, note: "This account has no five-hour limit; only the weekly limits apply.", mode: store.quotaDisplay, noLimit: true)
            }
            if windows.isEmpty && (account.quota?.limits ?? []).isEmpty {
                ForEach([true, false], id: \.self) { short in
                    if !(short && account.noShortLimit) {
                        WindowBlock(title: short ? "5-hour window" : "Weekly window", reading: nil, pace: nil, note: "Not reported yet", mode: store.quotaDisplay)
                    }
                }
            }
            ForEach(windows, id: \.self) { w in
                let r = QuotaReading(w)
                let back = (r?.exhausted ?? false) || account.windowSpent(w)
                WindowBlock(title: (w.model != nil ? Quotas.scopedTitle(w) : Quotas.title(w)) + " window", reading: r,
                            pace: Quotas.elapsed(w),
                            note: w.resetsAt.map { "\(back ? "Back" : "Resets") \(Fmt.when($0)) · in \(Fmt.left($0))" } ?? "No reset time reported",
                            mode: store.quotaDisplay)
            }
            ForEach(Array((account.quota?.limits ?? []).filter { !(($0.id ?? "").hasSuffix(":gate") && $0.usedPercent == 0) }.enumerated()), id: \.offset) { _, l in
                LimitBlock(limit: l, mode: store.quotaDisplay)
            }
        }
    }

    private var cooldowns: some View {
        let cds = (account.cooldowns ?? [:]).compactMap { k, v in FuseboxDate.parse(v).map { (k, $0) } }
            .filter { $0.1 > .now }.sorted { $0.1 < $1.1 }
        let reason: (String) -> String = { m in
            ["quota": "Usage limit used up", "rate_limit": "Rate limited", "checking": "Checking quota"][account.cooldownKinds?[m] ?? ""] ?? "Rate limited"
        }
        return Card {
            HStack {
                Text("Cooldowns").capsLabel()
                Spacer()
                if !cds.isEmpty { Button("Clear all") { pending = .clearCooldowns }.font(.footnote).disabled(busy) }
            }
            if cds.isEmpty {
                Text("No models cooling down. A rate limit pauses only that model on this account.")
                    .font(.footnote).foregroundStyle(Palette.fg3)
            }
            ForEach(cds, id: \.0) { model, until in
                HStack {
                    Text(model == "*" ? "All models" : model).font(Typeface.data(12)).foregroundStyle(Palette.fg)
                    Spacer()
                    VStack(alignment: .trailing, spacing: 1) {
                        Text(reason(model)).font(.caption).foregroundStyle(Palette.fg2)
                        Text(model == "*" && account.quotaResetUnknown == true ? "Reset time unknown" : "back \(Fmt.when(until)) · \(Fmt.left(until))")
                            .font(.caption).foregroundStyle(Palette.warn)
                    }
                }
            }
        }
    }

    private var routing: some View {
        let p = account.priority ?? 0
        let pick = p > 0 ? 10 : p < 0 ? -10 : 0
        let help = [10: "Takes new sessions before accounts set to Normal or Last.",
                    0: "Takes new sessions by the routing strategy, with the other Normal accounts.",
                    -10: "Takes new sessions only when no other account can."][pick]!
        let c = account.counters ?? Counters()
        return Card {
            HStack {
                Text("New sessions").capsLabel()
                Spacer()
                Picker("Priority", selection: Binding(get: { pick }, set: { v in
                    Task { await run(v > 0 ? "Takes new sessions first" : v < 0 ? "Takes new sessions last" : "Back to normal priority") {
                        try await store.setPriority(account.id, v)
                    } }
                })) {
                    Text("First").tag(10); Text("Normal").tag(0); Text("Last").tag(-10)
                }
                .pickerStyle(.segmented).fixedSize().disabled(busy)
                .accessibilityIdentifier("priority")
            }
            Text(help).font(.caption).foregroundStyle(Palette.fg3)
            Divider().overlay(Palette.line)
            Text("Since start").capsLabel()
            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 6) {
                GridRow { fig(c.requests, "Requests"); fig(c.failures, "Failed"); fig(c.cancelled, "Cancelled") }
                GridRow { fig(c.inputTokens, "Tokens in"); fig(c.outputTokens, "Tokens out"); fig(c.cacheTokens, "Cached") }
            }
            Divider().overlay(Palette.line)
            Text("Sign-in").capsLabel()
            Text(account.signinLine()).font(.footnote).foregroundStyle(Palette.fg2)
            if let file = account.file {
                Text(file).font(Typeface.data(11)).foregroundStyle(Palette.fg3).lineLimit(1).truncationMode(.middle)
            }
        }
    }

    private func fig(_ v: Int?, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(Fmt.count(v)).font(.subheadline.weight(.medium)).monospacedDigit().foregroundStyle(Palette.fg)
            Text(label).font(.caption2).foregroundStyle(Palette.fg3)
        }
    }

    // MARK: Actions

    private func useReset() {
        Task {
            busy = true; defer { busy = false }
            do {
                let r = try await store.bankedQuote(account.id)
                guard let quote = r.quote, r.inventory?.eligible == true, r.error == nil else {
                    errorText = r.error ?? r.inventory?.reason ?? "No reset can be used right now."
                    return
                }
                let grant = r.inventory?.grants?.first { $0.id == r.inventory?.selectedGrant } ?? r.inventory?.grants?.first { $0.usable == true }
                let detail = grant.map { g in
                    "\(g.expiresAt.map { "Expires \($0.formatted(date: .abbreviated, time: .shortened))" } ?? "No expiry reported") · Clears \((g.clears ?? []).joined(separator: ", ").ifEmpty("provider limits"))"
                } ?? (account.provider == "codex" ? "Codex chooses the reset and restores its subscription limits." : "")
                pending = .bankedReset(quote: quote, grant: grant?.id ?? r.inventory?.selectedGrant ?? "", label: grant?.label ?? "Subscription reset", detail: detail)
            } catch {
                errorText = error.localizedDescription
            }
        }
    }

    private func run(_ success: String, _ action: @escaping () async throws -> Void) async {
        busy = true; errorText = nil
        defer { busy = false }
        do {
            try await action()
            toastText = success
        } catch {
            errorText = error.localizedDescription
        }
    }

    private var confirmTitle: String {
        switch pending {
        case .disable: "Disable \(account.label)?"
        case .enable: "Enable \(account.label)?"
        case .clearError: "Clear the saved error?"
        case .clearCooldowns: "Clear all cooldowns?"
        case .bankedReset: "Use 1 reset?"
        case nil: ""
        }
    }

    private func confirmMessage(_ a: PendingAction) -> String {
        switch a {
        case .disable: "It stops taking requests until you enable it again. Its sessions move to another account."
        case .enable: "It goes back into rotation."
        case .clearError: "Clears the saved message. A new failure will appear again."
        case .clearCooldowns: "Every model on this account takes requests again straight away, even if the provider still limits it."
        case .bankedReset(_, _, let label, let detail): "This uses one saved reset: \(label).\n\(detail)"
        }
    }

    @ViewBuilder private func confirmButtons(_ a: PendingAction) -> some View {
        switch a {
        case .disable:
            Button("Disable", role: .destructive) { Task { await run("Disabled") { try await store.setDisabled(account.id, true) } } }
        case .enable:
            Button("Enable") { Task { await run("Enabled") { try await store.setDisabled(account.id, false) } } }
        case .clearError(let id):
            Button("Clear error", role: .destructive) { Task { await run("Error cleared") { try await store.clearError(account.id, errorID: id) } } }
        case .clearCooldowns:
            Button("Clear cooldowns", role: .destructive) { Task { await run("Cooldowns cleared") { try await store.clearCooldowns(account.id) } } }
        case .bankedReset(let quote, let grant, _, _):
            Button("Apply reset", role: .destructive) {
                Task { await run("Reset applied") { try await store.applyBankedReset(account.id, request: quote, grant: grant) } }
            }
        }
        Button("Cancel", role: .cancel) {}
    }
}

extension String {
    func ifEmpty(_ other: String) -> String { isEmpty ? other : self }
}

/// A window as a big bar with its reset line.
struct WindowBlock: View {
    var title: String
    var reading: QuotaReading?
    var pace: Double?
    var note: String
    var mode: QuotaDisplay
    var noLimit = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(title).font(.subheadline).foregroundStyle(Palette.fg2)
                Spacer()
                Text(noLimit ? "No limit" : reading.map { Fmt.percent($0.value(mode)) } ?? "–")
                    .font(.system(size: noLimit ? 13 : 20, weight: .medium)).monospacedDigit()
                    .foregroundStyle(reading.map { $0.level == .normal ? Palette.fg : Palette.level($0.level) } ?? Palette.fg3)
            }
            if !noLimit { SegmentedMeter(reading, pace: pace, mode: mode, height: 16, label: title) }
            Text(note).font(.caption).foregroundStyle(Palette.fg3)
        }
        .accessibilityElement(children: .combine)
    }
}

struct LimitBlock: View {
    var limit: QuotaLimit
    var mode: QuotaDisplay

    var body: some View {
        let l = limit
        let reading = l.enabled == false ? nil : QuotaReading(QuotaWindow(name: l.name, used: l.usedPercent, resetsAt: l.resetsAt))
        let divisor = l.unit == "USD cents" ? 100.0 : 1
        let unit = divisor == 100 ? "USD" : l.unit
        let amount: (Double) -> String = { ($0 / divisor).formatted(.number.precision(.fractionLength(0...4))) }
        let values = (l.used != nil && l.limit != nil) ? "\(amount(l.used!)) / \(amount(l.limit!))" : l.remaining.map { "\(amount($0)) left" } ?? ""
        let detail = l.unlimited == true ? "Unlimited" : !values.isEmpty ? values + (unit.map { " \($0)" } ?? "") : reading == nil ? "Not reported" : ""
        let scope: String = switch l.scope?.kind {
        case "account": "Applies to all models"
        case "credits": "Extra credits; separate from subscription allowance"
        case "overage": "Optional paid usage; separate from subscription allowance"
        default: "Separate feature allowance"
        }
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(l.name).font(.subheadline).foregroundStyle(Palette.fg2)
                Spacer()
                if let reading { Text(Fmt.percent(reading.value(mode))).font(Typeface.data(13)).foregroundStyle(Palette.fg) }
            }
            if !detail.isEmpty { Text(detail).font(.caption).foregroundStyle(Palette.fg2) }
            if l.enabled == false { Text("Disabled").font(.caption).foregroundStyle(Palette.fg3) }
            if let reading { SegmentedMeter(reading, mode: mode, height: 10, label: l.name) }
            Text(scope).font(.caption).foregroundStyle(Palette.fg3)
        }
    }
}

struct BankedResetsCard: View {
    var account: Account
    var busy: Bool
    var refresh: () async -> Void
    var use: () -> Void

    var body: some View {
        let r = account.bankedResets
        Card {
            HStack {
                Text("Banked resets").capsLabel()
                Spacer()
                if let checked = r?.checkedAt {
                    Text("Checked \(Fmt.ago(checked))").font(.caption2).foregroundStyle(Palette.fg3)
                }
            }
            Text("↻ \(account.bankedLabel)").font(.subheadline.weight(.medium))
                .foregroundStyle(account.bankedNeedsReview ? Palette.warn : Palette.fg)
            ForEach(r?.inventory?.grants ?? []) { g in
                VStack(alignment: .leading, spacing: 2) {
                    HStack {
                        Text(g.label ?? "Subscription reset").font(.footnote.weight(.medium)).foregroundStyle(Palette.fg)
                        Spacer()
                        Text("\(g.remaining ?? 0) left").font(Typeface.data(11)).foregroundStyle(Palette.fg2)
                    }
                    Text("\(g.expiresAt.map { "Expires \($0.formatted(date: .abbreviated, time: .shortened))" } ?? "No expiry reported") · Clears \((g.clears ?? []).joined(separator: ", ").ifEmpty("provider limits"))")
                        .font(.caption).foregroundStyle(Palette.fg3)
                }
            }
            Text(account.bankedPolicyText()).font(.caption).foregroundStyle(Palette.fg3)
            if let reason = r?.inventory?.reason, r?.error == nil { Text(reason).font(.caption).foregroundStyle(Palette.fg3) }
            if let op = r?.operation, let msg = op.message {
                Text(msg).font(.caption).foregroundStyle(["applied", "reconciled_used"].contains(op.status ?? "") ? Palette.ok : Palette.fg2)
            }
            if account.bankedNeedsReview {
                Text("A reset may have been used. Resolve it in the web dashboard before using another.")
                    .font(.caption).foregroundStyle(Palette.warn)
            }
            if let e = r?.error { Text(e).font(.caption).foregroundStyle(Palette.err) }
            HStack {
                Button("Refresh") { Task { await refresh() } }.buttonStyle(.bordered).disabled(busy)
                Button("Use 1 reset", action: use)
                    .buttonStyle(.bordered)
                    .disabled(busy || account.disabled || r?.inventory?.eligible != true || (account.bankedCount ?? 0) == 0 || account.bankedNeedsReview)
                    .accessibilityIdentifier("use-reset")
            }
        }
    }
}

/// The account's last hour, and the coding sessions pinned to it.
struct ActivityCard: View {
    @Environment(AppStore.self) private var store
    var account: Account

    var body: some View {
        let act = store.activity[account.id]
        let series = act?.series ?? []
        let nowMinute = Int(Date.now.timeIntervalSince1970 / 60)
        Card {
            Text("Load · last 60 min").capsLabel()
            if series.isEmpty {
                Text(act == nil ? "Loading…" : "No requests in the last hour.").font(.footnote).foregroundStyle(Palette.fg3)
            } else {
                Chart(series) { b in
                    BarMark(x: .value("Minute", b.date, unit: .minute), y: .value("Requests", b.requests), width: .ratio(0.7))
                        .foregroundStyle(b.minute == nowMinute ? Palette.brand : (b.failed ?? 0) > 0 ? Palette.err : Palette.meter.opacity(0.55))
                }
                .chartYScale(domain: 0...max(4, series.map(\.requests).max() ?? 0))
                .chartXAxis(.hidden).chartYAxis(.hidden)
                .frame(height: 54)
                Text("\(Fmt.plural(series.reduce(0) { $0 + $1.requests }, "request")) in the last hour")
                    .font(.caption).foregroundStyle(Palette.fg3)
            }
            let sessions = act?.sessions ?? []
            Divider().overlay(Palette.line)
            Text("Pinned sessions · \(sessions.count)").capsLabel()
            if sessions.isEmpty {
                Text("No coding sessions are pinned to this account.").font(.footnote).foregroundStyle(Palette.fg3)
            }
            ForEach(sessions.prefix(8)) { s in
                HStack(alignment: .firstTextBaseline) {
                    StatusDot(s.active == true ? Palette.ok : Palette.idle, size: 6)
                    VStack(alignment: .leading, spacing: 1) {
                        Text([s.clientApp ?? s.client, s.keyLabel].compactMap { $0 }.joined(separator: " · ").ifEmpty("Session"))
                            .font(.footnote).foregroundStyle(Palette.fg)
                        Text([s.model, s.session.map { String($0.prefix(8)) }].compactMap { $0 }.joined(separator: " · "))
                            .font(Typeface.data(11)).foregroundStyle(Palette.fg3)
                    }
                    Spacer()
                    VStack(alignment: .trailing, spacing: 1) {
                        Text("\(Fmt.count(s.requests)) req").font(.caption).foregroundStyle(Palette.fg2)
                        Text(Fmt.ago(s.lastSeen)).font(.caption2).foregroundStyle(Palette.fg3)
                    }
                }
            }
        }
    }
}
