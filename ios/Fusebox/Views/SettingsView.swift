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
        Form {
            Section {
                LabeledContent("Server", value: store.api?.baseURL.host() ?? "—")
                if let v = store.overview?.version { LabeledContent("Fusebox", value: v) }
                LabeledContent("Status") { LiveBadge(state: store.live) }
                Button("Open the web dashboard") { web = store.api?.baseURL }
                    .accessibilityIdentifier("open-dashboard")
            } header: { Text("Connection") } footer: {
                Text("Sign-in flows, API keys, models and the config editor are in the web dashboard.")
            }

            notifications

            Section {
                Picker("Meters show", selection: $store.quotaDisplay) {
                    Text("Used").tag(QuotaDisplay.used)
                    Text("Remaining").tag(QuotaDisplay.remaining)
                }
                Toggle("Live Activity when a provider runs out", isOn: $store.liveActivitiesEnabled)
            } header: { Text("Display") } footer: {
                Text("When every account of a provider is used up, a Live Activity counts down to the first one back. It ends when the provider recovers.")
            }

            Section {
                Button("Disconnect from this server", role: .destructive) { confirmSignOut = true }
            } footer: {
                Text("Removes the management key from this iPhone’s Keychain.")
            }

            Section {
                LabeledContent("App", value: "\(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "") (\(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""))")
            }
        }
        .clearOfFold()
        .scrollContentBackground(.hidden)
        .background(Palette.bg)
        .navigationTitle("Settings")
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

    private var notifications: some View {
        Section {
            Toggle(isOn: Binding(get: { push.isOn }, set: { on in
                Task { on ? await push.turnOn(api: store.api) : await push.turnOff(api: store.api) }
            })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Notifications on this iPhone")
                    Text(statusLine).font(.caption).foregroundStyle(Palette.fg3)
                }
            }
            .disabled(push.busy)
            .accessibilityIdentifier("push-toggle")
            if push.isOn {
                Button("Send test") { Task { await push.sendTest(api: store.api) } }
                    .disabled(push.busy)
                    .accessibilityIdentifier("push-test")
            }
            if let m = push.message {
                Text(m).font(.footnote).foregroundStyle(push.messageIsError ? Palette.warn : Palette.fg2)
                    .accessibilityIdentifier("push-message")
            }
            if push.apnsAvailable == false {
                Text("This Fusebox can’t send to iPhones yet: Apple push isn’t configured on the server.")
                    .font(.footnote).foregroundStyle(Palette.fg3)
            }
            let others = push.devices.filter { $0.id != push.subscriptionID }
            if !others.isEmpty {
                ForEach(others) { d in
                    HStack {
                        Image(systemName: d.resolvedKind == "apns" ? "iphone" : "globe").foregroundStyle(Palette.fg3)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(d.label ?? "Device").font(.subheadline)
                            Text("\(d.resolvedKind == "apns" ? "iPhone app" : "Web push")\(d.createdAt.map { " · since \($0.formatted(date: .abbreviated, time: .omitted))" } ?? "")")
                                .font(.caption).foregroundStyle(Palette.fg3)
                        }
                        Spacer()
                        Button("Remove") { removing = d }.font(.footnote).buttonStyle(.borderless)
                    }
                }
            }
            if let e = push.statusError { Text(e).font(.footnote).foregroundStyle(Palette.fg3) }
        } header: { Text("Notifications") } footer: {
            Text("Which events are sent is set for the whole server in the dashboard’s Config, Notifications. The app’s badge shows the number of faults.")
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
        NavigationStack {
            Form {
                Section {
                    VStack(alignment: .leading, spacing: 8) {
                        Image("Mark").resizable().scaledToFit().frame(height: 44).accessibilityHidden(true)
                        Text("Connect to Fusebox").font(.title2.weight(.semibold))
                        Text("Your subscriptions’ limits, faults and traffic, from your own Fusebox server.")
                            .font(.subheadline).foregroundStyle(Palette.fg2)
                    }
                    .listRowBackground(Color.clear)
                }
                Section {
                    TextField("https://fusebox.example.ts.net", text: $url)
                        .textContentType(.URL).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .font(Typeface.data(16))
                        .focused($focus, equals: .url)
                        .accessibilityIdentifier("server-url")
                } header: { Text("Server") } footer: {
                    Text("The dashboard’s address. A tailnet address needs Tailscale connected on this iPhone.")
                }
                Section {
                    SecureField("Management key", text: $key)
                        .textContentType(.password).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .font(Typeface.data(16))
                        .focused($focus, equals: .key)
                        .submitLabel(.go)
                        .onSubmit { Task { await check() } }
                        .accessibilityIdentifier("management-key")
                } header: { Text("Management key") } footer: {
                    Text("`management-key` in the server’s config.yaml. It is kept only in this iPhone’s Keychain.")
                }
                if let error {
                    Section {
                        Label { Text(error).foregroundStyle(Palette.fg) } icon: {
                            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Palette.warn)
                        }
                        .accessibilityIdentifier("connect-error")
                    }
                }
                Section {
                    Button {
                        Task { await check() }
                    } label: {
                        HStack { Text(checking ? "Checking…" : "Connect"); Spacer(); if checking { ProgressView() } }
                    }
                    .disabled(checking || key.isEmpty || url.isEmpty)
                    .accessibilityIdentifier("connect")
                }
            }
            .scrollContentBackground(.hidden)
            .background(Palette.bg)
        }
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
