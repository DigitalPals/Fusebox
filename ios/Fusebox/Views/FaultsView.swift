import SwiftUI

/// What has tripped (`/api/faults`): errors first, then warnings, then pace warnings.
struct FaultsList: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate
    /// The split layout shows the account beside the list.
    var selection: Binding<String?>?

    var body: some View {
        List {
            ForEach(store.faults) { f in
                Button { open(f) } label: { FaultRow(fault: f) }
                    .buttonStyle(.plain)
                    .listRowBackground(selection?.wrappedValue != nil && selection?.wrappedValue == f.accountID ? Palette.surfaceInset : Palette.surface)
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Palette.bg)
        .navigationTitle("Faults")
        .refreshable { await store.refreshAll() }
        .overlay {
            if store.faults.isEmpty {
                ContentUnavailableView("Nothing has tripped", systemImage: "checkmark.circle",
                                       description: Text("Expired sign-ins, used-up limits, rate limits and account errors show here."))
            }
        }
        .accessibilityIdentifier("faults")
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
        HStack(alignment: .top, spacing: 10) {
            StatusDot(fault.kind == "pace" ? Palette.fg3 : fault.color, size: 8).padding(.top, 5)
            VStack(alignment: .leading, spacing: 2) {
                Text(fault.title).font(.subheadline.weight(.medium)).foregroundStyle(Palette.fg)
                Text([fault.providerName, fault.label].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption).foregroundStyle(Palette.fg2)
                if let d = fault.detail, !d.isEmpty {
                    Text(d).font(.caption).foregroundStyle(Palette.fg3).lineLimit(3)
                }
                if let until = fault.until {
                    TimelineView(.periodic(from: .now, by: 30)) { ctx in
                        Text(until > ctx.date ? "Back \(Fmt.when(until, now: ctx.date)), in \(Fmt.left(until, now: ctx.date))" : "Clearing")
                            .font(.caption).foregroundStyle(fault.color)
                    }
                }
            }
            Spacer(minLength: 0)
            if fault.accountID != nil { Image(systemName: "chevron.right").font(.caption).foregroundStyle(Palette.fg3).padding(.top, 4) }
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("fault-\(fault.key)")
    }
}
