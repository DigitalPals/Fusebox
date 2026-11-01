import ActivityKit
import SwiftUI

struct SettingsView: View {
    @Environment(AppStore.self) private var store
    @Environment(PushManager.self) private var push
    @State private var web: URL?
    @State private var confirmSignOut = false
    @State private var removing: PushStatus.Device?

    var body: some View {
        @Bindable var store = store
        ScrollView {
            VStack(alignment: .leading, spacing: Metrics.sectionGap) {
                section("Connection", note: "Sign-in flows, API keys, models and the config editor are in the web dashboard.") {
                    Card(spacing: 0) {
                        KeyValueRow(key: "Server", value: store.api?.baseURL.host() ?? "—", mono: true)
                        if let v = store.overview?.version {
                            Hairline()
                            KeyValueRow(key: "Fusebox", value: v, mono: true)
                        }
                        Hairline()
                        HStack {
                            Text("Status").font(Typeface.row).foregroundStyle(Palette.fg3)
                            Spacer()
                            LiveBadge(state: store.live)
                        }
                        .padding(.vertical, 10)
                        Button("Open the web dashboard") { web = store.api?.baseURL }
                            .buttonStyle(FuseButtonStyle(.normal, fill: true))
                            .padding(.top, 6)
                            .accessibilityIdentifier("open-dashboard")
                    }
                }

                section("Notifications", note: "Which events are sent is set for the whole server in the dashboard’s Config, Notifications. The app’s badge shows the number of faults.") {
                    notifications
                }

                section("Display", note: "When every account of a provider is used up, a Live Activity counts down to the first one back. It ends when the provider recovers.") {
                    Card(spacing: 0) {
                        HStack(spacing: 12) {
                            Text("Meters show").font(Typeface.row).foregroundStyle(Palette.fg)
                            Spacer(minLength: 8)
                            FuseSegmented("Meters show", selection: $store.quotaDisplay, options: [(.used, "Used"), (.remaining, "Remaining")])
                        }
                        .padding(.vertical, 8)
                        Hairline()
                        Toggle(isOn: $store.liveActivitiesEnabled) {
                            Text("Live Activity when a provider runs out").font(Typeface.row).foregroundStyle(Palette.fg)
                        }
                        .toggleStyle(BreakerToggleStyle())
                    }
                }

                section(nil, note: "Removes the management key from this iPhone’s Keychain.") {
                    Button("Disconnect from this server") { confirmSignOut = true }
                        .buttonStyle(FuseButtonStyle(.destructive, fill: true))
                }

                Text("Fusebox for iPhone \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "") (\(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""))")
                    .font(Typeface.data(12)).foregroundStyle(Palette.fg3)
                    .frame(maxWidth: .infinity, alignment: .center)
            }
            .pageContent()
        }
        .background(Palette.bg)
        .navigationTitle(AppSection.settings.pageTitle)
        .task {
            await push.refreshAuthorization()
            await push.loadDevices(api: store.api)
        }
        .sheet(item: $web) { SafariView(url: $0).ignoresSafeArea() }
        .confirmationDialog("Disconnect from this server?", isPresented: $confirmSignOut, titleVisibility: .visible) {
            Button("Disconnect", role: .destructive) {
                Task { await push.turnOff(api: store.api); store.signOut() }
            }
        } message: { Text("Notifications for this iPhone are turned off too.") }
        .confirmationDialog("Remove \(removing?.label ?? "this device")?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
                            titleVisibility: .visible, presenting: removing) { d in
            Button("Remove", role: .destructive) { Task { await push.remove(device: d, api: store.api) } }
        } message: { _ in Text("It stops getting Fusebox notifications.") }
        .accessibilityIdentifier("settings")
    }

    private func section<Content: View>(_ title: String?, note: String?, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if let title { Text(title).capsLabel(12, color: Palette.fg2).accessibilityAddTraits(.isHeader) }
            content()
            if let note { Text(note).font(Typeface.small).foregroundStyle(Palette.fg3).fixedSize(horizontal: false, vertical: true) }
        }
    }

    private var notifications: some View {
        Card(spacing: 0) {
            Toggle(isOn: Binding(get: { push.isOn }, set: { on in
                Task { on ? await push.turnOn(api: store.api) : await push.turnOff(api: store.api) }
            })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Notifications on this iPhone").font(Typeface.row).foregroundStyle(Palette.fg)
                    Text(statusLine).font(Typeface.small).foregroundStyle(Palette.fg3)
                }
            }
            .toggleStyle(BreakerToggleStyle())
            .disabled(push.busy)
            .accessibilityIdentifier("push-toggle")
            if push.isOn, let via = push.via {
                Hairline()
                KeyValueRow(key: "Delivery", value: via == "relay" ? "Through the Fusebox push relay (encrypted)" : "Direct")
                    .accessibilityIdentifier("push-via")
            }
            if let m = push.message {
                Text(m).font(Typeface.meta).foregroundStyle(push.messageIsError ? Palette.warn : Palette.fg2)
                    .padding(.vertical, 8)
                    .accessibilityIdentifier("push-message")
            }
            if push.path == .unavailable, !push.isOn {
                Text(PushManager.unavailableText).font(Typeface.meta).foregroundStyle(Palette.fg3)
                    .padding(.vertical, 8)
                    .accessibilityIdentifier("push-unavailable")
            }
            if push.isOn {
                Button("Send test") { Task { await push.sendTest(api: store.api) } }
                    .buttonStyle(FuseButtonStyle(.normal, fill: true))
                    .disabled(push.busy)
                    .padding(.vertical, 6)
                    .accessibilityIdentifier("push-test")
            }
            let others = push.devices.filter { $0.id != push.subscriptionID }
            if !others.isEmpty {
                Hairline().padding(.top, 4)
                Text("Other devices").capsLabel(10.5).padding(.top, 12).padding(.bottom, 2)
                ForEach(Array(others.enumerated()), id: \.element.id) { i, d in
                    if i > 0 { Hairline(color: Palette.lineSoft) }
                    HStack(spacing: 12) {
                        Image(systemName: d.resolvedKind == "apns" ? "iphone" : "globe").font(.system(size: 15)).foregroundStyle(Palette.fg3)
                            .frame(width: 20)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(d.label ?? "Device").font(Typeface.row).foregroundStyle(Palette.fg).lineLimit(1)
                            Text("\(d.resolvedKind == "apns" ? (d.via == "relay" ? "iPhone app · relay" : "iPhone app") : "Web push")\(d.createdAt.map { " · since \($0.formatted(date: .abbreviated, time: .omitted))" } ?? "")")
                                .font(Typeface.small).foregroundStyle(Palette.fg3)
                        }
                        Spacer(minLength: 8)
                        Button("Remove") { removing = d }.buttonStyle(FuseButtonStyle(.destructive, compact: true))
                    }
                    .padding(.vertical, 8)
                }
            }
            if let e = push.statusError { Text(e).font(Typeface.meta).foregroundStyle(Palette.fg3).padding(.vertical, 8) }
        }
    }

    private var statusLine: String {
        switch push.authorization {
        case .denied: "Turned off in iOS Settings"
        case .notDetermined: push.isOn ? "On" : "Off"
        default: push.isOn ? "On · sends to this iPhone" : "Off"
        }
    }
}

struct ConnectView: View {
    @Environment(AppStore.self) private var store
    @State private var url = Connection.serverURL?.absoluteString ?? Connection.defaultURL
    @State private var key = ""
    @State private var checking = false
    @State private var error: String?
    @FocusState private var focus: Field?
    private enum Field { case url, key }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Metrics.sectionGap) {
                VStack(alignment: .leading, spacing: 12) {
                    Image("Mark").renderingMode(.template).resizable().scaledToFit().frame(height: 40)
                        .foregroundStyle(Palette.fg).accessibilityHidden(true)
                    Text("Connect to Fusebox").font(Typeface.sans(26, .semibold, relativeTo: .largeTitle)).foregroundStyle(Palette.fg)
                    Text("Your subscriptions’ limits, faults and traffic, from your own Fusebox server.")
                        .font(Typeface.body).foregroundStyle(Palette.fg2)
                }
                .padding(.top, 32)

                Card(spacing: 8) {
                    Text("Server").capsLabel(11)
                    TextField("https://fusebox.example.ts.net", text: $url)
                        .textContentType(.URL).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .focused($focus, equals: .url)
                        .submitLabel(.next)
                        .onSubmit { focus = .key }
                        .fieldStyle(focused: focus == .url)
                        .accessibilityIdentifier("server-url")
                    Text("The dashboard’s address. A tailnet address needs Tailscale connected on this iPhone.")
                        .font(Typeface.small).foregroundStyle(Palette.fg3)
                    Text("Management key").capsLabel(11).padding(.top, 10)
                    SecureField("Management key", text: $key)
                        .textContentType(.password).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .focused($focus, equals: .key)
                        .submitLabel(.go)
                        .onSubmit { Task { await check() } }
                        .fieldStyle(focused: focus == .key)
                        .accessibilityIdentifier("management-key")
                    Text("`management-key` in the server’s config.yaml. It is kept only in this iPhone’s Keychain.")
                        .font(Typeface.small).foregroundStyle(Palette.fg3)
                }

                if let error {
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        StatusDot(Palette.err)
                        Text(error).font(Typeface.row).foregroundStyle(Palette.fg)
                    }
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Palette.surface, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Palette.errLine, lineWidth: 1))
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("connect-error")
                }

                Button {
                    Task { await check() }
                } label: {
                    HStack(spacing: 10) {
                        if checking { ProgressView().tint(Palette.bg) }
                        Text(checking ? "Checking…" : "Connect")
                    }
                }
                .buttonStyle(FuseButtonStyle(.primary, fill: true))
                .disabled(checking || key.isEmpty || url.isEmpty)
                .accessibilityIdentifier("connect")
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 32)
            .frame(maxWidth: 520)
            .frame(maxWidth: .infinity)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(Palette.bg)
    }

    /// Checks the URL and key against `/api/overview` before saving anything.
    private func check() async {
        guard let u = Connection.normalize(url) else { error = "That isn’t a web address."; return }
        checking = true; error = nil
        defer { checking = false }
        let api = FuseboxAPI(baseURL: u, key: key.trimmingCharacters(in: .whitespacesAndNewlines))
        do {
            let o = try await api.overview()
            if o.managementKey == false {
                error = "This Fusebox has no management key set, so it only accepts management from its own machine."
                return
            }
            store.connect(url: u, key: api.key)
        } catch {
            self.error = error.localizedDescription
        }
    }
}

extension View {
    /// The dashboard's input: `bg` fill, 1px `line-strong` border (`line-hover` when focused),
    /// DM Mono at 16pt so iOS doesn't zoom, 44pt tall.
    func fieldStyle(focused: Bool) -> some View {
        self
            .font(Typeface.data(16, relativeTo: .body))
            .foregroundStyle(Palette.fg)
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(Palette.bg, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(focused ? Palette.lineHover : Palette.lineStrong, lineWidth: 1))
    }
}
