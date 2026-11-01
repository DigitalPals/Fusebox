import SwiftUI

@main
struct FuseboxApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var store = AppStore()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(store)
                .environment(PushManager.shared)
                .environment(Router.shared)
                .tint(Palette.brand)
                .onOpenURL { Router.shared.pending = DeepLink.route(url: $0) }
        }
        .onChange(of: phase) { _, now in
            switch now {
            case .active: store.resume()
            case .background: store.stop()
            default: break
            }
        }
    }
}

struct RootView: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        Group {
            if store.isConnected {
                MainView()
            } else {
                ConnectView()
            }
        }
        .background(Palette.bg.ignoresSafeArea())
        #if DEBUG
        .overlay {
            if ProcessInfo.processInfo.environment["FUSEBOX_DEBUG_REGIONS"] == "1" { ReservedRegionsOverlay() }
        }
        #endif
    }
}
