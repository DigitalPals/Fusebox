import Charts
import SwiftUI

// MARK: - List

struct AccountsList: View {
    @Environment(AppStore.self) private var store
    /// Bound in the split layout; nil on a phone, where rows push the detail.
    var selection: Binding<String?>?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Metrics.sectionGap) {
                if !store.accounts.isEmpty {
                    SectionHead("\(store.accounts.count) accounts", meta: meta) { QuotaModeToggle() }
                }
                ForEach(store.groups, id: \.provider) { group in
                    VStack(alignment: .leading, spacing: 10) {
                        HStack(spacing: 8) {
                            ProviderLogo(group.accounts[0].logo, size: 16)
                            Text(group.name).capsLabel(12, color: Palette.fg2)
                            Text("\(group.accounts.count)").font(Typeface.data(12)).foregroundStyle(Palette.fg3)
                        }
                        .accessibilityElement(children: .combine)
                        .accessibilityAddTraits(.isHeader)
                        RowsPanel(items: group.accounts, rivets: true) { row($0) }
                    }
                }
            }
            .pageContent()
        }
        .background(Palette.bg)
        .refreshable { await store.refreshAll() }
        .navigationTitle(AppSection.accounts.pageTitle)
        .overlay {
            if store.accounts.isEmpty {
                EmptyState("No accounts connected", systemImage: "person.crop.circle.badge.plus", message: "Sign in with a subscription or add an API key from the web dashboard.")
                    .foregroundStyle(Palette.fg2)
            }
        }
        .accessibilityIdentifier("accounts")
    }

    private var meta: String {
        let ready = store.accounts.filter { $0.state().kind == .ready }.count
        let cooling = store.accounts.filter { $0.state().kind == .cooling }.count
        return "\(ready) ready" + (cooling > 0 ? " · \(cooling) cooling" : "")
    }

    @ViewBuilder private func row(_ a: Account) -> some View {
        if let selection {
            Button { selection.wrappedValue = a.id } label: {
                AccountRowContent(account: a, selected: selection.wrappedValue == a.id)
            }
            .buttonStyle(RowButtonStyle())
            .accessibilityAddTraits(selection.wrappedValue == a.id ? .isSelected : [])
            .accessibilityIdentifier("account-row-\(a.id)")
        } else {
            NavigationLink(value: a.id) { AccountRowContent(account: a) }
                .buttonStyle(RowButtonStyle())
                .accessibilityIdentifier("account-row-\(a.id)")
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
            EmptyState("This account is gone", systemImage: "person.crop.circle.badge.questionmark", message: "It was removed or its credentials changed.")
                .foregroundStyle(Palette.fg2)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Palette.bg)
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

/// One account, laid out as the dashboard's account page: the head (logo, email, plan,
/// status, Refresh and the breaker), then Limits, Banked resets, Cooldowns, Activity, New
/// sessions and Sign-in.
struct AccountDetail: View {
    @Environment(AppStore.self) private var store
    var account: Account

    @State private var busy = false
    @State private var toastText: String?
    @State private var pending: PendingAction?
    @State private var errorText: String?

    private enum Block: String, Identifiable { case limits, resets, cooldowns, activity, priority, signin; var id: String { rawValue } }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Metrics.panelGap) {
                head
                if let errorText {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        StatusDot(Palette.err)
                        Text(errorText).font(Typeface.meta).foregroundStyle(Palette.err)
                    }
                }
                if account.lastError != nil { errorBox }
                FoldAwareColumns(items: blocks, minColumnWidth: 360) { block($0) }
            }
            .pageContent()
        }
        .background(Palette.bg)
        .navigationTitle(account.label)
        .navigationBarTitleDisplayMode(.inline)
        // The email is the page's own title; the bar keeps only the way back.
        .toolbar {
            ToolbarItem(placement: .principal) { Text(account.providerName).capsLabel(12, color: Palette.fg2) }
            ToolbarItem(placement: .primaryAction) { if busy { ProgressView() } }
        }
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
        b += [.cooldowns, .activity, .priority, .signin]
        return b
    }

    @ViewBuilder private func block(_ b: Block) -> some View {
        switch b {
        case .limits: limits
        case .resets: BankedResetsCard(account: account, busy: busy, refresh: { await run("Checked") { try await store.refreshQuota(account.id) } }, use: useReset)
        case .cooldowns: cooldowns
        case .activity: ActivityCard(account: account)
        case .priority: priority
        case .signin: signin
        }
    }

    // MARK: Head

    private var head: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 14) {
                ProviderLogo(account.logo, size: 32).padding(.top, 2)
                VStack(alignment: .leading, spacing: 3) {
                    Text(account.label).font(Typeface.title).foregroundStyle(Palette.fg)
                        .lineLimit(2).minimumScaleFactor(0.75)
                        .accessibilityAddTraits(.isHeader)
                        .accessibilityIdentifier("account-detail")
                    Text([account.providerName, account.planName, account.authName].compactMap { $0 }.joined(separator: " · "))
                        .font(Typeface.meta).foregroundStyle(Palette.fg3)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            // Status, Refresh and the breaker on one line when they fit, else status above the two.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) {
                    status.frame(maxWidth: .infinity, alignment: .leading)
                    actions
                }
                VStack(alignment: .leading, spacing: 10) {
                    status
                    HStack(spacing: 10) { actions }
                }
            }
        }
        .padding(.top, 4)
    }

    private var status: some View {
        StatusLabel(account: account, inFlight: store.inFlight(account.id), font: Typeface.sans(15, relativeTo: .subheadline))
            .accessibilityIdentifier("account-status")
    }

    /// Refresh and the on/off switch: the dashboard's breaker.
    @ViewBuilder private var actions: some View {
        if account.kind == "oauth" && ["claude", "codex"].contains(account.provider) {
            Button {
                Task { await run("Quota refreshed") { try await store.refreshQuota(account.id) } }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise").labelStyle(.titleOnly)
            }
            .buttonStyle(.outline)
            .disabled(busy)
            .accessibilityIdentifier("refresh-quota")
        }
        Toggle(isOn: Binding(get: { !account.disabled }, set: { on in pending = on ? .enable : .disable })) {
            Text(account.disabled ? "Disabled" : "Enabled").font(Typeface.sans(15)).foregroundStyle(Palette.fg)
        }
        .toggleStyle(BreakerToggleStyle(labelFirst: false))
        .padding(.leading, 6).padding(.trailing, 14)
        .frame(height: 40)
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Palette.lineStrong, lineWidth: 1))
        .fixedSize()
        .disabled(busy)
        .accessibilityIdentifier("account-toggle")
    }

    // MARK: Blocks

    private var errorBox: some View {
        Card {
            PanelHead("Last error") {
                Button("Clear error") { if let id = account.lastErrorID { pending = .clearError(id) } }
                    .buttonStyle(FuseButtonStyle(.destructive, compact: true))
                    .disabled(account.lastErrorID == nil || busy)
            }
            Text(account.lastError ?? "").font(Typeface.data(12.5)).foregroundStyle(Palette.err).textSelection(.enabled)
            Text("Clears the saved message. A new failure will appear again.").font(Typeface.small).foregroundStyle(Palette.fg3)
        }
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Palette.errLine, lineWidth: 1))
    }

    private var limits: some View {
        Card(rivets: true, spacing: 18) {
            PanelHead("Limits · \(store.quotaDisplay.word)") { QuotaModeToggle() }
                .accessibilityIdentifier("limits-panel")
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
                            mode: store.quotaDisplay, runsOut: Quotas.runsOut(w))
            }
            let limits = (account.quota?.limits ?? []).filter { !(($0.id ?? "").hasSuffix(":gate") && $0.usedPercent == 0) }
            ForEach(Array(limits.enumerated()), id: \.offset) { _, l in
                Hairline()
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
        return Card(spacing: 10) {
            PanelHead("Cooldowns") {
                if !cds.isEmpty { Button("Clear all") { pending = .clearCooldowns }.buttonStyle(.outlineSmall).disabled(busy) }
            }
            if cds.isEmpty {
                Text("No models cooling down. A rate limit pauses only that model on this account.")
                    .font(Typeface.meta).foregroundStyle(Palette.fg3)
            }
            ForEach(cds, id: \.0) { model, until in
                Hairline()
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        cooldownModel(model)
                        Spacer(minLength: 8)
                        cooldownWhen(model, until, reason(model))
                    }
                    VStack(alignment: .leading, spacing: 4) {
                        cooldownModel(model)
                        cooldownWhen(model, until, reason(model))
                    }
                }
                .padding(.top, 2)
            }
        }
    }

    private func cooldownModel(_ model: String) -> some View {
        Text(model == "*" ? "All models" : model).font(Typeface.data(13, relativeTo: .subheadline)).foregroundStyle(Palette.fg)
    }

    private func cooldownWhen(_ model: String, _ until: Date, _ reason: String) -> some View {
        HStack(spacing: 10) {
            Text(reason).font(Typeface.meta).foregroundStyle(Palette.fg2)
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                Text(model == "*" && account.quotaResetUnknown == true ? "Reset time unknown" : "back \(Fmt.when(until, now: ctx.date)) · \(Fmt.left(until, now: ctx.date))")
                    .font(Typeface.meta).foregroundStyle(Palette.warn)
            }
        }
    }

    private var priority: some View {
        let p = account.priority ?? 0
        let pick = p > 0 ? 10 : p < 0 ? -10 : 0
        let help = [10: "Takes new sessions before accounts set to Normal or Last.",
                    0: "Takes new sessions by the routing strategy, with the other Normal accounts.",
                    -10: "Takes new sessions only when no other account can."][pick]!
        return Card(spacing: 10) {
            PanelHead("New sessions") {
                FuseSegmented("Priority", selection: Binding(get: { pick }, set: { v in
                    guard v != pick else { return }
                    Task { await run(v > 0 ? "Takes new sessions first" : v < 0 ? "Takes new sessions last" : "Back to normal priority") {
                        try await store.setPriority(account.id, v)
                    } }
                }), options: [(10, "First"), (0, "Normal"), (-10, "Last")])
                .disabled(busy)
                .accessibilityIdentifier("priority")
            }
            Text(help).font(Typeface.meta).foregroundStyle(Palette.fg2)
        }
    }

    private var signin: some View {
        let token: String? = {
            guard account.kind == "oauth", let exp = account.expiresAt else { return nil }
            let secs = exp.timeIntervalSinceNow
            return secs > 0 ? "valid \(Fmt.span(secs)) · refreshes on its own" : "expired"
        }()
        return Card(spacing: 0) {
            PanelHead("Sign-in").padding(.bottom, 4)
            Hairline()
            KeyValueRow(key: "Method", value: account.authName)
            if let plan = account.planName {
                Hairline()
                KeyValueRow(key: "Plan", value: "\(account.providerName) \(plan)")
            }
            if let token {
                Hairline()
                KeyValueRow(key: "Token", value: token, valueColor: token == "expired" ? Palette.err : Palette.fg)
            }
            if let file = account.file {
                Hairline()
                VStack(alignment: .leading, spacing: 4) {
                    Text("Stored in").font(Typeface.row).foregroundStyle(Palette.fg3)
                    Text(file).font(Typeface.data(12.5)).foregroundStyle(Palette.fg).textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.vertical, 10)
            }
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

/// A window as the account page draws it: its name and a big percentage, the 20pt meter with
/// the pace mark, and the reset line under it.
struct WindowBlock: View {
    var title: String
    var reading: QuotaReading?
    var pace: Double?
    var note: String
    var mode: QuotaDisplay
    var noLimit = false
    var runsOut: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .lastTextBaseline, spacing: 12) {
                Text(title).font(Typeface.row).foregroundStyle(Palette.fg2)
                Spacer(minLength: 8)
                if noLimit {
                    Text("No limit").font(Typeface.row).foregroundStyle(Palette.fg3)
                } else {
                    Text(reading.map { Fmt.percent($0.value(mode)) } ?? "–")
                        .font(Typeface.bigFigure).tracking(-0.6)
                        .foregroundStyle(reading.map { $0.level == .normal ? Palette.fg : Palette.level($0.level) } ?? Palette.fg3)
                }
            }
            if !noLimit { SegmentedMeter(reading, pace: pace, mode: mode, height: 20, label: title) }
            Text(note).font(Typeface.meta).foregroundStyle(Palette.fg3)
                .fixedSize(horizontal: false, vertical: true)
            if let runsOut {
                Text("At this pace it runs out \(Fmt.when(runsOut)), in \(Fmt.left(runsOut))")
                    .font(Typeface.meta).foregroundStyle(Palette.warn)
            }
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
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(l.name).font(Typeface.row).foregroundStyle(Palette.fg2)
                Spacer(minLength: 8)
                if let reading {
                    Text(Fmt.percent(reading.value(mode))).font(Typeface.data(14, relativeTo: .subheadline)).foregroundStyle(Palette.fg)
                } else {
                    Text(l.enabled == false ? "Disabled" : detail).font(Typeface.meta).foregroundStyle(Palette.fg3)
                }
            }
            if let reading {
                SegmentedMeter(reading, mode: mode, height: 9, label: l.name)
                if !detail.isEmpty { Text(detail).font(Typeface.data(12)).foregroundStyle(Palette.fg2) }
            }
            Text(scope).font(Typeface.small).foregroundStyle(Palette.fg3)
        }
        .accessibilityElement(children: .combine)
    }
}

struct BankedResetsCard: View {
    var account: Account
    var busy: Bool
    var refresh: () async -> Void
    var use: () -> Void

    var body: some View {
        let r = account.bankedResets
        Card(spacing: 10) {
            PanelHead("Banked resets") {
                if let checked = r?.checkedAt {
                    Text("Checked \(Fmt.ago(checked))").font(Typeface.small).foregroundStyle(Palette.fg3)
                }
            }
            Text("↻ \(account.bankedLabel)").font(Typeface.sans(17, .medium, relativeTo: .headline))
                .foregroundStyle(account.bankedNeedsReview ? Palette.warn : Palette.fg)
            ForEach(r?.inventory?.grants ?? []) { g in
                VStack(alignment: .leading, spacing: 2) {
                    HStack {
                        Text(g.label ?? "Subscription reset").font(Typeface.rowMedium).foregroundStyle(Palette.fg)
                        Spacer()
                        Text("\(g.remaining ?? 0) left").font(Typeface.data(12)).foregroundStyle(Palette.fg2)
                    }
                    Text("\(g.expiresAt.map { "Expires \($0.formatted(date: .abbreviated, time: .shortened))" } ?? "No expiry reported") · Clears \((g.clears ?? []).joined(separator: ", ").ifEmpty("provider limits"))")
                        .font(Typeface.small).foregroundStyle(Palette.fg3)
                }
            }
            Text(account.bankedPolicyText()).font(Typeface.meta).foregroundStyle(Palette.fg2)
            if let reason = r?.inventory?.reason, r?.error == nil { Text(reason).font(Typeface.meta).foregroundStyle(Palette.fg3) }
            if let op = r?.operation, let msg = op.message {
                Text(msg).font(Typeface.meta).foregroundStyle(["applied", "reconciled_used"].contains(op.status ?? "") ? Palette.ok : Palette.fg2)
            }
            if account.bankedNeedsReview {
                Text("A reset may have been used. Resolve it in the web dashboard before using another.")
                    .font(Typeface.meta).foregroundStyle(Palette.warn)
            }
            if let e = r?.error { Text(e).font(Typeface.meta).foregroundStyle(Palette.err) }
            HStack(spacing: 10) {
                Button("Refresh") { Task { await refresh() } }.buttonStyle(.outline).disabled(busy)
                Button("Use 1 reset", action: use)
                    .buttonStyle(.outline)
                    .disabled(busy || account.disabled || r?.inventory?.eligible != true || (account.bankedCount ?? 0) == 0 || account.bankedNeedsReview)
                    .accessibilityIdentifier("use-reset")
            }
            .padding(.top, 4)
        }
    }
}

/// The account's last hour, its totals since start, and the coding sessions pinned to it.
struct ActivityCard: View {
    @Environment(AppStore.self) private var store
    var account: Account

    var body: some View {
        let act = store.activity[account.id]
        let series = act?.series ?? []
        let c = account.counters ?? Counters()
        Card(spacing: 12) {
            PanelHead("Activity") {
                TimelineView(.periodic(from: .now, by: 30)) { ctx in
                    Text(store.inFlight(account.id) > 0 ? "In use now" : "Last used \(Fmt.ago(account.lastUsed, now: ctx.date))")
                        .font(Typeface.small).foregroundStyle(Palette.fg3)
                }
            }
            Text("Load · last 60 min").capsLabel(10.5)
            if series.isEmpty {
                Text(act == nil ? "Loading…" : "No requests in the last hour.").font(Typeface.meta).foregroundStyle(Palette.fg3)
            } else {
                LoadChart(series: series, height: 56)
                Text("\(Fmt.plural(series.reduce(0) { $0 + $1.requests }, "request")) in the last hour")
                    .font(Typeface.small).foregroundStyle(Palette.fg3)
            }
            Hairline().padding(.vertical, 2)
            Text("Since start").capsLabel(10.5)
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 12) {
                GridRow { Figure(value: Fmt.count(c.requests), label: "Requests"); Figure(value: Fmt.count(c.failures), label: "Failed", color: (c.failures ?? 0) > 0 ? Palette.err : Palette.fg); Figure(value: Fmt.count(c.cancelled), label: "Cancelled") }
                GridRow { Figure(value: Fmt.compact(c.inputTokens), label: "Tokens in"); Figure(value: Fmt.compact(c.outputTokens), label: "Tokens out"); Figure(value: Fmt.compact(c.cacheTokens), label: "Cached") }
            }
            let sessions = act?.sessions ?? []
            Hairline().padding(.vertical, 2)
            HStack {
                Text("Pinned sessions").capsLabel(10.5)
                Spacer()
                Text("\(sessions.count)").font(Typeface.data(12)).foregroundStyle(Palette.fg3)
            }
            if sessions.isEmpty {
                Text("No coding sessions are pinned to this account.").font(Typeface.meta).foregroundStyle(Palette.fg3)
            } else {
                Text("Coding sessions stay on this account so their prompt cache keeps working.")
                    .font(Typeface.small).foregroundStyle(Palette.fg3)
            }
            ForEach(Array(sessions.prefix(8).enumerated()), id: \.element.id) { i, s in
                if i > 0 { Hairline(color: Palette.lineSoft) }
                HStack(alignment: .top, spacing: 10) {
                    StatusDot(s.active == true ? Palette.ok : Palette.idle, size: 7).padding(.top, 6)
                    VStack(alignment: .leading, spacing: 2) {
                        Text([s.clientApp ?? s.client, s.keyLabel].compactMap { $0 }.joined(separator: " · ").ifEmpty("Session"))
                            .font(Typeface.row).foregroundStyle(Palette.fg).lineLimit(1)
                        Text([s.model, s.session.map { String($0.prefix(8)) }].compactMap { $0 }.joined(separator: " · "))
                            .font(Typeface.data(12)).foregroundStyle(Palette.fg3).lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    VStack(alignment: .trailing, spacing: 2) {
                        Text("\(Fmt.count(s.requests)) req").font(Typeface.data(12)).foregroundStyle(Palette.fg2)
                        Text(Fmt.ago(s.lastSeen)).font(Typeface.small).foregroundStyle(Palette.fg3)
                    }
                }
            }
        }
    }
}
