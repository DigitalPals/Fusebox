import SwiftUI

enum RequestFilter: String, CaseIterable, Identifiable {
    case all, failed, rerouted, cancelled
    var id: String { rawValue }
    var title: String {
        switch self { case .all: "All"; case .failed: "Failed"; case .rerouted: "Rerouted"; case .cancelled: "Cancelled" }
    }
    func matches(_ r: RequestLog) -> Bool {
        switch self {
        case .all: true
        case .failed: r.isError
        case .rerouted: r.isRerouted
        case .cancelled: r.status == 499
        }
    }
}

struct RequestsList: View {
    @Environment(AppStore.self) private var store
    /// Bound in the split layout (the inspector shows the selected request); nil on a phone.
    var selection: Binding<Int?>?
    @SceneStorage("requests.filter") private var filter: RequestFilter = .all
    @State private var search = ""

    var body: some View {
        let shown = store.requests.filter { filter.matches($0) && matches($0) }
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                chips
                if !shown.isEmpty {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(shown.enumerated()), id: \.element.id) { i, r in
                            if i > 0 { Hairline().padding(.horizontal, Metrics.panelPadding) }
                            row(r)
                        }
                    }
                    .panelBackground()
                }
            }
            .pageContent()
        }
        .background(Palette.bg)
        .searchable(text: $search, prompt: "Model, account or client")
        .navigationTitle(AppSection.requests.pageTitle)
        .task { await store.loadFullRequests() }
        .refreshable { await store.refreshAll() }
        .overlay {
            if shown.isEmpty {
                EmptyState(store.requests.isEmpty ? "No requests yet" : "No matching requests",
                                       systemImage: "list.bullet.rectangle")
                    .foregroundStyle(Palette.fg2)
            }
        }
        .accessibilityIdentifier("requests")
    }

    @ViewBuilder private func row(_ r: RequestLog) -> some View {
        let content = RequestRow(request: r)
            .padding(.horizontal, Metrics.panelPadding)
            .padding(.vertical, 12)
        if let selection {
            Button { selection.wrappedValue = r.id } label: {
                content.background(selection.wrappedValue == r.id ? Palette.segOn : .clear)
            }
            .buttonStyle(RowButtonStyle())
            .accessibilityAddTraits(selection.wrappedValue == r.id ? .isSelected : [])
        } else {
            NavigationLink(value: r.id) { content }.buttonStyle(RowButtonStyle())
        }
    }

    private func matches(_ r: RequestLog) -> Bool {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return true }
        return [r.model, r.account, r.clientApp, r.keyLabel, r.provider, r.status.map(String.init)]
            .compactMap { $0?.lowercased() }.contains { $0.contains(q) }
    }

    /// The dashboard's filter chips: 13pt with a DM Mono count; the selected one has a
    /// `line-hover` border on `seg-on`.
    private var chips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(RequestFilter.allCases) { f in
                    let n = store.requests.filter(f.matches).count
                    let on = filter == f
                    Button { filter = f } label: {
                        HStack(spacing: 6) {
                            Text(f.title).font(Typeface.meta).foregroundStyle(on ? Palette.fg : Palette.fg2)
                            Text(Fmt.count(n)).font(Typeface.data(12)).foregroundStyle(Palette.fg3)
                        }
                        .padding(.horizontal, 12)
                        .frame(minHeight: 32)
                        .background(on ? Palette.segOn : .clear, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(on ? Palette.lineHover : Palette.line, lineWidth: 1))
                        .contentShape(Rectangle().inset(by: -6))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            .padding(.vertical, 6)
        }
        .scrollClipDisabled()
    }
}

/// "14:05:09 · Claude Code → Claude   200  7.5s" / model / account · routing note.
struct RequestRow: View {
    @Environment(AppStore.self) private var store
    var request: RequestLog
    var compact = false

    var body: some View {
        let r = request
        let acct = store.account(r.accountID)
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(Fmt.clock(r.ts)).font(Typeface.data(12)).foregroundStyle(Palette.fg3)
                if r.provider != nil { ProviderLogo(acct?.logo ?? Provider.logo(provider: r.provider), size: 13) }
                Text("\(r.clientName) → \(acct?.providerName ?? Provider.name(r.provider))")
                    .font(Typeface.meta).foregroundStyle(Palette.fg2).lineLimit(1)
                if let tag = r.transportTag ?? ((r.attempts ?? 1) > 1 ? "\(r.attempts!) tries" : nil) {
                    Text(tag).capsLabel(9.5, color: Palette.fg3)
                        .padding(.horizontal, 4).padding(.vertical, 1)
                        .overlay(RoundedRectangle(cornerRadius: 3).strokeBorder(Palette.lineStrong))
                }
                Spacer(minLength: 6)
                Text(r.status.map(String.init) ?? "—").font(Typeface.data(12.5))
                    .foregroundStyle(r.isError ? Palette.err : r.status == 499 ? Palette.fg3 : Palette.fg2)
                Text(Fmt.ms(r.latencyMs)).font(Typeface.data(12)).foregroundStyle(Palette.fg2).frame(minWidth: 46, alignment: .trailing)
            }
            Text(r.model ?? "—").font(Typeface.data(13, relativeTo: .subheadline)).foregroundStyle(Palette.fg).lineLimit(1)
            HStack(spacing: 4) {
                Text(acct?.label ?? r.account ?? "—").lineLimit(1).truncationMode(.middle)
                if let reason = r.reasonText {
                    Text("·")
                    Text(reason).foregroundStyle(r.isRerouted ? Palette.warn : Palette.fg3).lineLimit(1)
                }
                if !compact, let sid = r.sessionID {
                    Text("·")
                    Text(sid.prefix(8)).font(Typeface.data(11.5))
                } else if !compact, let w = r.sessionWarning {
                    Text("·")
                    Text(w.0).foregroundStyle(Palette.warn)
                }
            }
            .font(Typeface.small).foregroundStyle(Palette.fg3)
            if !compact, let e = r.errorLine {
                Text(e).font(Typeface.small).foregroundStyle(r.status == 499 ? Palette.fg3 : Palette.err).lineLimit(2)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("request-\(r.id)")
    }
}

struct RequestDetailView: View {
    @Environment(AppStore.self) private var store
    @Environment(\.navigate) private var navigate
    var id: Int

    var body: some View {
        if let r = store.requests.first(where: { $0.id == id }) {
            ScrollView {
                VStack(alignment: .leading, spacing: Metrics.panelGap) {
                    summary(r)
                    Card(spacing: 8) {
                        PanelHead("Why this account").accessibilityIdentifier("request-detail")
                        Text(r.whySentence).font(Typeface.body).foregroundStyle(Palette.fg)
                            .fixedSize(horizontal: false, vertical: true)
                        if let w = r.sessionWarning {
                            Text("\(w.0): \(w.1)").font(Typeface.meta).foregroundStyle(Palette.warn)
                        }
                    }
                    if let e = r.errorLine {
                        Card(spacing: 8) {
                            PanelHead(r.status == 499 ? "Cancelled" : "Error")
                            Text(e).font(Typeface.data(12.5)).foregroundStyle(r.status == 499 ? Palette.fg2 : Palette.err).textSelection(.enabled)
                        }
                        .overlay { if r.status != 499 { RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Palette.errLine, lineWidth: 1) } }
                    }
                    Card(spacing: 0) {
                        PanelHead("Request").padding(.bottom, 4)
                        rows(r)
                    }
                    if let p = r.cachePolicy {
                        Card(spacing: 8) {
                            PanelHead("Prompt cache")
                            Text("Requested \(cacheLabel(p.requested)) · applied \(cacheLabel(p.applied)) · \(cacheLabel(p.keyScope))")
                                .font(Typeface.row).foregroundStyle(Palette.fg2)
                        }
                    }
                    if let acct = store.account(r.accountID) {
                        Button("Open \(acct.label)") { navigate(.account(acct.id)) }.buttonStyle(FuseButtonStyle(.normal, fill: true))
                    }
                }
                .pageContent()
            }
            .background(Palette.bg)
            .navigationTitle("Request \(Fmt.clock(r.ts))")
            .navigationBarTitleDisplayMode(.inline)
        } else {
            EmptyState("Request not in the log", systemImage: "list.bullet.rectangle", message: "The server keeps the most recent 300 requests.")
                .foregroundStyle(Palette.fg2)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Palette.bg)
        }
    }

    /// The model, the status and the timings, big, as a figure row.
    private func summary(_ r: RequestLog) -> some View {
        let acct = store.account(r.accountID)
        return VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                if r.provider != nil { ProviderLogo(acct?.logo ?? Provider.logo(provider: r.provider), size: 22) }
                Text(r.model ?? "—").font(Typeface.data(17, relativeTo: .headline)).foregroundStyle(Palette.fg)
                    .lineLimit(2).minimumScaleFactor(0.8)
            }
            Text("\(r.clientName) → \(acct?.providerName ?? Provider.name(r.provider)) · \(acct?.label ?? r.account ?? "—")")
                .font(Typeface.meta).foregroundStyle(Palette.fg3)
            HStack(alignment: .top, spacing: 0) {
                Figure(value: r.status.map(String.init) ?? "—", label: "Status",
                       color: r.isError ? Palette.err : r.status == 499 ? Palette.fg3 : Palette.fg)
                Figure(value: Fmt.ms(r.ttftMs), label: "First token")
                Figure(value: Fmt.ms(r.latencyMs), label: "Total")
            }
        }
        .padding(.top, 4)
    }

    @ViewBuilder private func rows(_ r: RequestLog) -> some View {
        let items: [(String, String, Bool)] = [
            ("Client", r.clientDescription, false),
            ("Endpoint", r.endpoint, true),
            ("Account", store.account(r.accountID)?.label ?? r.account ?? "—", false),
            ("Session", r.sessionID.map { String($0.prefix(8)) } ?? "none", true),
        ] + (r.source.map { [("Source", sourceText($0), true)] } ?? [])
          + (r.keyLabel.map { [("Key", $0, false)] } ?? [])
          + [("Tokens", r.tokensText, true), ("Time", r.ts.formatted(date: .abbreviated, time: .standard), true)]
        ForEach(Array(items.enumerated()), id: \.offset) { _, item in
            Hairline()
            KeyValueRow(key: item.0, value: item.1, mono: item.2)
        }
    }

    private func sourceText(_ s: RequestLog.Source) -> String {
        let via: String
        switch s.via {
        case "forwarded": via = s.login.map { "via Tailscale · \($0)" } ?? "via reverse proxy"
        case "proxy_protocol": via = "via TCP forwarder"
        case "unknown": via = "proxy gave no readable address"
        default: via = (s.ip ?? "").hasPrefix("127.") || s.ip == "::1" ? "this machine" : "direct"
        }
        return "\(s.ip ?? "") · \(via)"
    }

    private func cacheLabel(_ v: String?) -> String {
        let labels = ["provider_default": "provider default", "prewarm": "prewarm", "automatic": "automatic",
                      "explicit": "explicit boundaries", "explicit_no_breakpoints": "explicit without boundaries",
                      "none": "no cache controls", "rejected": "rejected", "caller": "caller key", "session": "session",
                      "client": "client", "workspace": "workspace", "session_fallback": "session fallback",
                      "not_sent": "no cache key sent"]
        return labels[v ?? ""] ?? v ?? "—"
    }
}
