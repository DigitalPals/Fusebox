import Foundation
import Testing
@testable import Fusebox

/// A load that finishes after sign-out or after connecting to another server
/// must not write its (old server's) state. `FuseboxAPI` is a struct, so the
/// store guards post-await writes with a generation counter instead of an
/// identity check. These cover that guard; the delayed-response run against
/// a stub server stays a manual check on macOS.
@Suite("AppStore connection generations")
@MainActor
struct AppStoreLifecycleTests {
    @Test func aWriteFromTheLiveGenerationApplies() {
        let store = AppStore()
        var applied = false
        #expect(store.applyIfCurrent(store.generation) { applied = true })
        #expect(applied)
    }

    @Test func stopInvalidatesInFlightLoads() {
        let store = AppStore()
        let gen = store.generation
        store.stop()
        var applied = false
        #expect(!store.applyIfCurrent(gen) { applied = true })
        #expect(!applied)
        // A load started after the stop (e.g. resume, or a new connection)
        // captures the new generation and still applies.
        #expect(store.applyIfCurrent(store.generation) { applied = true })
        #expect(applied)
    }

    @Test func signOutDropsLateResultsAndClearsState() {
        let store = AppStore()
        let gen = store.generation
        store.signOut()
        #expect(store.api == nil)
        #expect(store.accounts.isEmpty)
        #expect(store.faults.isEmpty)
        #expect(store.requests.isEmpty)
        // A result captured before sign-out cannot repopulate the store
        // (and, through it, the widget snapshot, badge or Live Activities).
        #expect(!store.applyIfCurrent(gen) { store.loadError = "stale" })
        #expect(store.loadError == nil)
    }

    @Test func loadsWithoutAConnectionDoNothing() async {
        let store = AppStore()
        store.signOut()
        await store.refreshAll()
        await store.loadFullRequests()
        await store.loadActivity("claude-anything.json")
        #expect(store.accounts.isEmpty)
        #expect(store.requests.isEmpty)
        #expect(store.activity.isEmpty)
        #expect(store.loadError == nil)
    }
}
