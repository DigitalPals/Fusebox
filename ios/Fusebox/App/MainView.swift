import SwiftUI

/// One scene's navigation. Its place (section, account, request) is scene state, so
/// folding or unfolding an iPhone Duo, or resizing beside another app, swaps the layout
/// without losing it: a compact width gets tabs and stacks, a regular width a split view.
/// The layout follows the size class, never the device model or screen size.
struct MainView: View {
    @Environment(AppStore.self) private var store
    @Environment(Router.self) private var router
    @Environment(\.horizontalSizeClass) private var sizeClass

    @SceneStorage("nav.section") private var section: AppSection = .overview
    @SceneStorage("nav.account") private var accountID = ""
    @SceneStorage("nav.request") private var requestID = 0
    @SceneStorage("nav.faults-sheet") private var faultsSheet = false
    @State private var columns: NavigationSplitViewVisibility = .automatic
    /// Where the hinge is (iOS 27.1 `.division` region), to put column edges on it.
    @State private var fold = FoldGeometry.zero

    var body: some View {
        Group {
            if sizeClass == .compact {
                phone
            } else {
                split
            }
        }
        .environment(\.navigate, Navigate(go: go))
        .onGeometryChange(for: FoldGeometry.self) { FoldGeometry($0) } action: { fold = $0 }
        .onChange(of: router.pending) { _, link in
            guard let link else { return }
            go(link)
            router.pending = nil
        }
        .onAppear {
            if let link = router.pending { go(link); router.pending = nil }
        }
        // A list section shows its list and detail; the sidebar steps aside (unfolded, the
        // list fills the page left of the hinge).
        .onChange(of: section) { _, now in
            if sizeClass != .compact, [.accounts, .faults].contains(now) { columns = .doubleColumn }
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
        case .faults:
            if sizeClass == .compact { faultsSheet = true } else { section = .faults }
        case .settings: section = .settings
        }
    }

    // MARK: Folded: tabs and stacks

    private var phoneTab: Binding<AppSection> {
        Binding(get: { section == .faults ? .overview : section }, set: { section = $0 })
    }

    private var accountPath: Binding<[String]> {
        Binding(get: { accountID.isEmpty ? [] : [accountID] }, set: { accountID = $0.last ?? "" })
    }

    private var requestPath: Binding<[Int]> {
        Binding(get: { requestID == 0 ? [] : [requestID] }, set: { requestID = $0.last ?? 0 })
    }

    private var phone: some View {
        TabView(selection: phoneTab) {
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
            .presentationDetents([.medium, .large])
        }
        .onAppear { if section == .faults { faultsSheet = true; section = .overview } }
    }

    /// The faults button in every header, with its count (DESIGN.md "Faults").
    @ToolbarContentBuilder private var faultsButton: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Button { go(.faults) } label: {
                Label("Faults", systemImage: store.faults.isEmpty ? "checkmark.circle" : "exclamationmark.triangle.fill")
                    .labelStyle(.iconOnly)
                    .foregroundStyle(store.faults.contains { $0.level == "err" } ? Palette.err : store.faults.isEmpty ? Palette.fg3 : Palette.warn)
            }
            .badge(store.badgeCount)
            .accessibilityLabel("Faults, \(store.faults.count)")
            .accessibilityIdentifier("faults-button")
        }
    }

    // MARK: Unfolded: sidebar, list and detail

    private var sidebarSelection: Binding<AppSection?> {
        Binding(get: { section }, set: { if let s = $0 { section = s } })
    }

    private var sidebar: some View {
        List(selection: sidebarSelection) {
            Section {
                ForEach([AppSection.overview, .accounts, .requests, .usage, .faults]) { s in
                    NavigationLink(value: s) {
                        Label(s.title, systemImage: s.icon)
                    }
                    .badge(s == .faults ? store.faults.count : 0)
                    .accessibilityIdentifier("sidebar-\(s.rawValue)")
                }
            }
            Section {
                NavigationLink(value: AppSection.settings) { Label(AppSection.settings.title, systemImage: AppSection.settings.icon) }
            }
        }
        .navigationTitle("Fusebox")
        .safeAreaInset(edge: .bottom) {
            HStack { LiveBadge(state: store.live); Spacer() }.padding(.horizontal, 20).padding(.vertical, 10)
        }
    }

    private var selectedAccount: Binding<String?> {
        Binding(get: { accountID.isEmpty ? nil : accountID }, set: { accountID = $0 ?? "" })
    }

    private var selectedRequest: Binding<Int?> {
        Binding(get: { requestID == 0 ? nil : requestID }, set: { requestID = $0 ?? 0 })
    }

    @ViewBuilder private var split: some View {
        switch section {
        case .accounts, .faults:
            NavigationSplitView(columnVisibility: $columns) {
                sidebar
            } content: {
                Group {
                    if section == .accounts { AccountsList(selection: selectedAccount) } else { FaultsList(selection: selectedAccount) }
                }
                .modifier(ListColumnWidth(fold: fold))
            } detail: {
                NavigationStack {
                    if accountID.isEmpty {
                        ContentUnavailableView("Choose an account", systemImage: "person.2", description: Text("Its limits, activity and banked resets show here."))
                    } else {
                        AccountDetailView(id: accountID)
                    }
                }
            }
            // Side by side, never a detail over its list: the list and the detail are the two pages.
            .navigationSplitViewStyle(.balanced)
        default:
            NavigationSplitView(columnVisibility: $columns) {
                sidebar
            } detail: {
                NavigationStack {
                    switch section {
                    case .requests:
                        RequestsList(selection: selectedRequest)
                            .inspector(isPresented: Binding(get: { requestID != 0 }, set: { if !$0 { requestID = 0 } })) {
                                NavigationStack { RequestDetailView(id: requestID) }
                                    .modifier(InspectorWidth(fold: fold))
                            }
                    case .usage: UsageView().toolbar { faultsButton }
                    case .settings: SettingsView()
                    default: OverviewView().toolbar { faultsButton }
                    }
                }
            }
        }
    }
}

/// Unfolded, the list column ends at the hinge, so the list is one page and the detail the
/// other and nothing straddles the fold. Elsewhere the usual widths apply.
private struct ListColumnWidth: ViewModifier {
    var fold: FoldGeometry
    func body(content: Content) -> some View {
        if let d = fold.division, d.minX >= 280 {
            content.navigationSplitViewColumnWidth(min: 300, ideal: d.minX, max: d.minX)
        } else {
            content.navigationSplitViewColumnWidth(min: 300, ideal: 340, max: 420)
        }
    }
}

/// Unfolded, the request inspector takes the page beyond the hinge.
private struct InspectorWidth: ViewModifier {
    var fold: FoldGeometry
    func body(content: Content) -> some View {
        if let d = fold.division, fold.width - d.maxX >= 300 {
            let w = fold.width - d.maxX
            content.inspectorColumnWidth(min: w, ideal: w, max: w)
        } else {
            content.inspectorColumnWidth(min: 320, ideal: 380, max: 480)
        }
    }
}
