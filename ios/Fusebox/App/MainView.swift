import SwiftUI

/// One scene's navigation: a tab bar (Overview, Accounts, Requests, Usage, Settings) with a
/// NavigationStack in each, and the faults button in every header, folded or unfolded. Its
/// place (section, account, request) is scene state, so folding or unfolding an iPhone Duo, or
/// resizing beside another app, keeps it.
///
/// Unfolded, the app keeps the outer screen's layout: the same tabs and header buttons where iOS
/// puts them, not a sidebar behind a menu button, and the pages in one centred column
/// (`pageContent()`), not split into columns. So the scene is always laid out as compact width.
struct MainView: View {
    @Environment(AppStore.self) private var store
    @Environment(Router.self) private var router

    @SceneStorage("nav.section") private var section: AppSection = .overview
    @SceneStorage("nav.account") private var accountID = ""
    @SceneStorage("nav.request") private var requestID = 0
    @SceneStorage("nav.faults-sheet") private var faultsSheet = false

    var body: some View {
        tabs
            .environment(\.horizontalSizeClass, .compact)
            .environment(\.navigate, Navigate(go: go))
            .onChange(of: router.pending) { _, link in
                guard let link else { return }
                go(link)
                router.pending = nil
            }
            .onAppear {
                if let link = router.pending { go(link); router.pending = nil }
            }
    }

    private func go(_ link: DeepLink) {
        faultsSheet = false
        switch link {
        case .overview: section = .overview
        case .account(let id): section = .accounts; accountID = id
        case .accounts: section = .accounts; accountID = ""
        case .requests: section = .requests
        case .request(let id): section = .requests; requestID = id
        case .usage: section = .usage
        case .faults: faultsSheet = true
        case .settings: section = .settings
        }
    }

    private var tab: Binding<AppSection> {
        Binding(get: { section == .faults ? .overview : section }, set: { section = $0 })
    }

    private var accountPath: Binding<[String]> {
        Binding(get: { accountID.isEmpty ? [] : [accountID] }, set: { accountID = $0.last ?? "" })
    }

    private var requestPath: Binding<[Int]> {
        Binding(get: { requestID == 0 ? [] : [requestID] }, set: { requestID = $0.last ?? 0 })
    }

    private var tabs: some View {
        TabView(selection: tab) {
            Tab(AppSection.overview.title, systemImage: AppSection.overview.icon, value: .overview) {
                NavigationStack { OverviewView().toolbar { faultsButton } }
            }
            Tab(AppSection.accounts.title, systemImage: AppSection.accounts.icon, value: .accounts) {
                NavigationStack(path: accountPath) {
                    AccountsList()
                        .toolbar { faultsButton }
                        .navigationDestination(for: String.self) { AccountDetailView(id: $0) }
                }
            }
            Tab(AppSection.requests.title, systemImage: AppSection.requests.icon, value: .requests) {
                NavigationStack(path: requestPath) {
                    RequestsList()
                        .toolbar { faultsButton }
                        .navigationDestination(for: Int.self) { RequestDetailView(id: $0) }
                }
            }
            Tab(AppSection.usage.title, systemImage: AppSection.usage.icon, value: .usage) {
                NavigationStack { UsageView().toolbar { faultsButton } }
            }
            Tab(AppSection.settings.title, systemImage: AppSection.settings.icon, value: .settings) {
                NavigationStack { SettingsView() }
            }
        }
        .sheet(isPresented: $faultsSheet) {
            NavigationStack {
                FaultsList()
                    .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { faultsSheet = false } } }
            }
            .environment(\.horizontalSizeClass, .compact)
            .presentationDetents([.medium, .large])
            .presentationBackground(Palette.bg)
            .presentationCornerRadius(18)
        }
        // Builds before this one kept Faults as a section of the unfolded sidebar.
        .onAppear { if section == .faults { faultsSheet = true; section = .overview } }
    }

    /// The faults button in every header, with its count (DESIGN.md "Faults").
    @ToolbarContentBuilder private var faultsButton: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Button { go(.faults) } label: {
                // An icon-sized item, so it stays in the bar wherever iOS puts it (on the iPhone
                // Duo's outer screen the bars run down the side); the count is its badge.
                Label("Faults", systemImage: store.faults.isEmpty ? "checkmark" : "exclamationmark.triangle")
                    .labelStyle(.iconOnly)
                    .foregroundStyle(store.faults.isEmpty ? Palette.fg3 : Palette.fg)
            }
            .badge(store.faults.count)
            .accessibilityLabel(store.faults.isEmpty ? "No faults" : "Faults, \(store.faults.count)")
            .accessibilityIdentifier("faults-button")
        }
    }
}
