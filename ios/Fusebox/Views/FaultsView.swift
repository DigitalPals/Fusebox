import SwiftUI

/// What has tripped (`/api/faults`): errors first, then warnings, then pace warnings. Each
/// fault can be dismissed until it clears; dismissing is shared with the dashboard.
struct FaultsList: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate
    @State private var toastText: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if !store.faults.isEmpty {
                    Text("Expired sign-ins, used-up limits, rate limits and account errors, with the time each comes back.")
                        .font(Typeface.meta).foregroundStyle(Palette.fg3)
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(store.faults.enumerated()), id: \.element.id) { i, f in
                            if i > 0 { Hairline().padding(.horizontal, Metrics.panelPadding) }
                            HStack(alignment: .top, spacing: 0) {
                                Button { open(f) } label: {
                                    FaultRow(fault: f)
                                        .padding(.leading, Metrics.panelPadding)
                                        .padding(.vertical, 14)
                                }
                                .buttonStyle(RowButtonStyle())
                                DismissFaultButton(fault: f) { dismiss(f) }
                                    .padding(.top, 4)
                                    .padding(.trailing, 4)
                            }
                            .contextMenu { DismissFaultMenuItem { dismiss(f) } }
                        }
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .panelBackground()
                }
                if !store.dismissedFaults.isEmpty { dismissedFooter }
            }
            .pageContent()
        }
        .background(Palette.bg)
        .navigationTitle(AppSection.faults.pageTitle)
        .refreshable { await store.refreshAll() }
        .overlay {
            if store.faults.isEmpty {
                EmptyState(store.dismissedFaults.isEmpty ? "Nothing has tripped" : "Nothing else has tripped",
                           systemImage: "checkmark.circle",
                           message: "Expired sign-ins, used-up limits, rate limits and account errors show here.")
                    .foregroundStyle(Palette.fg2)
            }
        }
        .toast($toastText)
        .accessibilityIdentifier("faults")
    }

    /// How many are dismissed, and a way to bring them back.
    private var dismissedFooter: some View {
        let n = store.dismissedFaults.count
        return HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text("\(n) dismissed until \(n == 1 ? "it clears" : "they clear")")
                .font(Typeface.meta).foregroundStyle(Palette.fg3)
            Spacer(minLength: 8)
            LinkButton("Show again") {
                Task {
                    do { try await store.restoreDismissedFaults() } catch { toastText = error.localizedDescription }
                }
            }
            .accessibilityIdentifier("faults-restore")
        }
        .padding(.horizontal, 4)
        .accessibilityIdentifier("faults-dismissed")
    }

    private func dismiss(_ f: Fault) {
        Task {
            do { try await store.dismiss(f) } catch { toastText = error.localizedDescription }
        }
    }

    /// An account's fault opens the account; a provider's or a pace warning the Overview.
    private func open(_ f: Fault) {
        if let id = f.accountID {
            navigate(.account(id))
        } else {
            navigate(f.kind == "provider" ? .accounts : DeepLink.route(path: f.path))
        }
    }
}

/// The ✕ at the end of a fault's row: hides it, here and on the dashboard, until it clears.
struct DismissFaultButton: View {
    var fault: Fault
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Palette.fg3)
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Dismiss \(fault.title)")
        .accessibilityHint("Hidden until it clears")
        .accessibilityIdentifier("dismiss-\(fault.key)")
    }
}

/// The same, in a fault row's long-press menu.
struct DismissFaultMenuItem: View {
    var action: () -> Void
    var body: some View {
        Button(action: action) { Label("Dismiss until it clears", systemImage: "xmark") }
    }
}

struct FaultRow: View {
    var fault: Fault

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            StatusDot(fault.kind == "pace" ? Palette.fg3 : fault.color).padding(.top, 6)
            VStack(alignment: .leading, spacing: 3) {
                Text(fault.title).font(Typeface.rowMedium).foregroundStyle(Palette.fg)
                Text([fault.providerName, fault.label].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(Typeface.small).foregroundStyle(Palette.fg2)
                if let d = fault.detail, !d.isEmpty {
                    Text(d).font(Typeface.small).foregroundStyle(Palette.fg3).lineLimit(3)
                }
                if let until = fault.until {
                    TimelineView(.periodic(from: .now, by: 30)) { ctx in
                        Text(until > ctx.date ? "back \(Fmt.when(until, now: ctx.date)) · \(Fmt.left(until, now: ctx.date))" : "Clearing")
                            .font(Typeface.meta).foregroundStyle(fault.kind == "pace" ? Palette.fg2 : fault.color)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if fault.accountID != nil {
                Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.fg3).padding(.top, 4)
            }
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("fault-\(fault.key)")
    }
}
