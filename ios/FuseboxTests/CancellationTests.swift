import Foundation
import Testing
@testable import Fusebox

/// A read the app stops itself (SwiftUI ends a pull to refresh or leaves a page) is not a
/// failure: it must not reach a page as an error with a Retry button ("cancelled").
@Suite("Cancelled reads")
struct CancellationTests {
    @Test func aCancelledReadThrowsCancellationErrorNotAnAPIError() async {
        // TEST-NET-1 is never routed, so the read is still connecting when it is cancelled.
        let api = FuseboxAPI(baseURL: URL(string: "http://192.0.2.1:8317")!, key: "test")
        let read = Task { try await api.overview() }
        try? await Task.sleep(for: .milliseconds(100))
        read.cancel()
        let result = await read.result
        switch result {
        case .success: Issue.record("an unrouted read can't succeed")
        case .failure(let error):
            #expect(error is CancellationError, "got \(error)")
            #expect(!(error is APIError))
        }
    }
}
