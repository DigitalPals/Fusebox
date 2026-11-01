import SwiftUI

/// What has tripped (`/api/faults`): errors first, then warnings, then pace warnings.
struct FaultsList: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate
    /// The split layout shows the account beside the list.
    var selection: Binding<String?>?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if !store.faults.isEmpty {
                    Text("Expired sign-ins, used-up limits, rate limits and account errors, with the time each comes back.")
                        .font(Typeface.meta).foregroundStyle(Palette.fg3)
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(store.faults.enumerated()), id: \.element.id) { i, f in
                            if i > 0 { Hairline().padding(.horizontal, Metrics.panelPadding) }
                            Button { open(f) } label: {
                                FaultRow(fault: f)
                                    .padding(.horizontal, Metrics.panelPadding)
                                    .padding(.vertical, 14)
                                    .background(isSelected(f) ? Palette.segOn : .clear)
                            }
                            .buttonStyle(RowButtonStyle())
                        }
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .panelBackground()
                }
            }
            .pageContent()
        }
        .background(Palette.bg)
        .navigationTitle(AppSection.faults.pageTitle)
        .refreshable { await store.refreshAll() }
        .overlay {
            if store.faults.isEmpty {
                EmptyState("Nothing has tripped", systemImage: "checkmark.circle", message: "Expired sign-ins, used-up limits, rate limits and account errors show here.")
                    .foregroundStyle(Palette.fg2)
            }
        }
        .accessibilityIdentifier("faults")
    }

    private func isSelected(_ f: Fault) -> Bool {
        guard let id = selection?.wrappedValue else { return false }
        return id == f.accountID
    }

    /// An account's fault opens the account; a provider's or a pace warning the Overview.
    private func open(_ f: Fault) {
        if let id = f.accountID {
            if let selection { selection.wrappedValue = id } else { navigate(.account(id)) }
        } else {
            navigate(f.kind == "provider" ? .accounts : DeepLink.route(path: f.path))
        }
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
