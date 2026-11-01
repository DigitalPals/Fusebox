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
        Group {
            if let selection {
                List(selection: selection) {
                    chipsRow
                    ForEach(shown) { r in RequestRow(request: r).tag(r.id) }
                }
            } else {
                List {
                    chipsRow
                    ForEach(shown) { r in NavigationLink(value: r.id) { RequestRow(request: r) } }
                }
            }
        }
        .listStyle(.plain)
        .clearOfFold()
        .scrollContentBackground(.hidden)
        .background(Palette.bg)
        .searchable(text: $search, prompt: "Model, account or client")
        .navigationTitle("Requests")
        .task { await store.loadFullRequests() }
        .refreshable { await store.refreshAll() }
        .overlay {
            if shown.isEmpty {
                ContentUnavailableView(store.requests.isEmpty ? "No requests yet" : "No matching requests",
                                       systemImage: "list.bullet.rectangle")
            }
        }
        .accessibilityIdentifier("requests")
    }

    private func matches(_ r: RequestLog) -> Bool {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return true }
        return [r.model, r.account, r.clientApp, r.keyLabel, r.provider, r.status.map(String.init)]
            .compactMap { $0?.lowercased() }.contains { $0.contains(q) }
    }

    private var chipsRow: some View {
        chips
            .listRowInsets(EdgeInsets())
            .listRowSeparator(.hidden)
            .listRowBackground(Color.clear)
            .selectionDisabled()
    }

    private var chips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(RequestFilter.allCases) { f in
                    let n = store.requests.filter(f.matches).count
                    Button { filter = f } label: {
                        HStack(spacing: 4) {
                            Text(f.title).font(.footnote)
                            Text(Fmt.count(n)).font(Typeface.data(11)).foregroundStyle(Palette.fg3)
                        }
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(Palette.surface, in: Capsule())
                        .overlay(Capsule().strokeBorder(filter == f ? Palette.brand : Palette.line, lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(filter == f ? .isSelected : [])
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 6)
        }
    }
}

/// "14:05:09 Anthropic → Claude  200  7.5s" / model / account · routing note.
struct RequestRow: View {
    @Environment(AppStore.self) private var store
    var request: RequestLog
    var compact = false

    var body: some View {
        let r = request
        let acct = store.account(r.accountID)
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(Fmt.clock(r.ts)).font(Typeface.data(11.5)).foregroundStyle(Palette.fg3)
                if r.provider != nil { ProviderLogo(acct?.logo ?? Provider.logo(provider: r.provider), size: 12) }
                Text("\(r.clientName) → \(acct?.providerName ?? Provider.name(r.provider))")
                    .font(.footnote).foregroundStyle(Palette.fg2).lineLimit(1)
                if let tag = r.transportTag ?? ((r.attempts ?? 1) > 1 ? "\(r.attempts!) tries" : nil) {
                    Text(tag).font(Typeface.label(10)).padding(.horizontal, 4).padding(.vertical, 1)
                        .overlay(RoundedRectangle(cornerRadius: 3).strokeBorder(Palette.lineStrong)).foregroundStyle(Palette.fg3)
                }
                Spacer(minLength: 4)
                Text(r.status.map(String.init) ?? "—").font(Typeface.data(12))
                    .foregroundStyle(r.isError ? Palette.err : r.status == 499 ? Palette.fg3 : Palette.ok)
                Text(Fmt.ms(r.latencyMs)).font(Typeface.data(11.5)).foregroundStyle(Palette.fg2).frame(minWidth: 44, alignment: .trailing)
            }
            Text(r.model ?? "—").font(Typeface.data(12)).foregroundStyle(Palette.fg).lineLimit(1)
            HStack(spacing: 4) {
                Text(acct?.label ?? r.account ?? "—").lineLimit(1)
                if let reason = r.reasonText {
                    Text("·")
                    Text(reason).foregroundStyle(r.isRerouted ? Palette.warn : Palette.fg3)
                }
                if !compact, let sid = r.sessionID {
                    Text("·")
                    Text(sid.prefix(8)).font(Typeface.data(11))
                } else if !compact, let w = r.sessionWarning {
                    Text("·")
                    Text(w.0).foregroundStyle(Palette.warn)
                }
            }
            .font(.caption).foregroundStyle(Palette.fg3)
            if !compact, let e = r.errorLine {
                Text(e).font(.caption).foregroundStyle(r.status == 499 ? Palette.fg3 : Palette.err).lineLimit(2)
            }
        }
        .padding(.vertical, 2)
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
                VStack(alignment: .leading, spacing: 14) {
                    RequestRow(request: r)
                    Card {
                        Text("Why this account").capsLabel().accessibilityIdentifier("request-detail")
                        Text(r.whySentence).font(.subheadline).foregroundStyle(Palette.fg)
                        if let w = r.sessionWarning {
                            Text("\(w.0): \(w.1)").font(.caption).foregroundStyle(Palette.warn)
                        }
                    }
                    Card {
                        kv("Client", r.clientDescription)
                        kv("Endpoint", r.endpoint, mono: true)
                        kv("Model", r.model ?? "—", mono: true)
                        kv("Account", store.account(r.accountID)?.label ?? r.account ?? "—")
                        kv("Session", r.sessionID.map { String($0.prefix(8)) } ?? "none", mono: true)
                        if let s = r.source { kv("Source", sourceText(s), mono: true) }
                        if let k = r.keyLabel { kv("Key", k) }
                        kv("Status", r.status.map(String.init) ?? "—", mono: true)
                        kv("Latency", "\(Fmt.ms(r.ttftMs)) to first token · \(Fmt.ms(r.latencyMs)) total", mono: true)
                        kv("Tokens", r.tokensText, mono: true)
                        kv("Time", r.ts.formatted(date: .abbreviated, time: .standard), mono: true)
                    }
                    if let e = r.errorLine {
                        Card {
                            Text(r.status == 499 ? "Cancelled" : "Error").capsLabel()
                            Text(e).font(Typeface.data(12)).foregroundStyle(r.status == 499 ? Palette.fg2 : Palette.err).textSelection(.enabled)
                        }
                    }
                    if let p = r.cachePolicy {
                        Card {
                            Text("Prompt cache").capsLabel()
                            Text("Requested \(cacheLabel(p.requested)) · applied \(cacheLabel(p.applied)) · \(cacheLabel(p.keyScope))")
                                .font(.footnote).foregroundStyle(Palette.fg2)
                        }
                    }
                    if let acct = store.account(r.accountID) {
                        Button("Open account") { navigate(.account(acct.id)) }.buttonStyle(.bordered)
                    }
                }
                .padding(16)
            }
            .background(Palette.bg)
            .navigationTitle("Request \(Fmt.clock(r.ts))")
            .navigationBarTitleDisplayMode(.inline)
        } else {
            ContentUnavailableView("Request not in the log", systemImage: "list.bullet.rectangle",
                                   description: Text("The server keeps the most recent 300 requests."))
        }
    }

    private func kv(_ k: String, _ v: String, mono: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(k).font(.footnote).foregroundStyle(Palette.fg3).frame(width: 74, alignment: .leading)
            Text(v).font(mono ? Typeface.data(12) : .footnote).foregroundStyle(Palette.fg).textSelection(.enabled)
            Spacer(minLength: 0)
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
