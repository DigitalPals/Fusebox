import Foundation

/// The Fusebox push relay (docs/push-relay.md): it gives out a handle for this device's APNs
/// token, and passes sealed notifications for that handle to Apple. It never sees the key.
struct RelayClient: Sendable {
    var base: URL

    private func url(_ path: String) -> URL {
        var s = base.absoluteString
        while s.hasSuffix("/") { s.removeLast() }
        return URL(string: s + path)!
    }

    private func call(_ method: String, _ path: String, json: [String: String]? = nil) async throws -> (Int, Data) {
        var req = URLRequest(url: url(path))
        req.httpMethod = method
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if let json {
            req.httpBody = try JSONEncoder().encode(json)
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        do {
            let (data, response) = try await FuseboxAPI.session.data(for: req)
            return ((response as? HTTPURLResponse)?.statusCode ?? 0, data)
        } catch let e as URLError {
            throw APIError(status: 0, message: "The Fusebox push relay can’t be reached: \(FuseboxAPI.describe(e))")
        }
    }

    private func failure(_ status: Int, _ data: Data) -> APIError {
        APIError(status: status, message: "The Fusebox push relay: \(FuseboxAPI.message(status: status, data: data))")
    }

    /// `POST /v1/devices`: a new handle for this token.
    func register(token: String, environment: String) async throws -> String {
        struct Handle: Decodable { var handle: String }
        let (status, data) = try await call("POST", "/v1/devices", json: ["token": token, "environment": environment])
        guard (200..<300).contains(status) else { throw failure(status, data) }
        guard let handle = try? JSONDecoder().decode(Handle.self, from: data).handle, !handle.isEmpty else {
            throw APIError(status: status, message: "The Fusebox push relay answered without a handle.")
        }
        return handle
    }

    /// `POST /v1/devices/{handle}/touch` keeps the handle; false when the relay has forgotten it.
    func touch(_ handle: String) async throws -> Bool {
        let (status, data) = try await call("POST", "/v1/devices/\(FuseboxAPI.encode(handle))/touch")
        if status == 410 || status == 404 { return false }
        guard (200..<300).contains(status) else { throw failure(status, data) }
        return true
    }

    /// `DELETE /v1/devices/{handle}`; a handle it doesn't know is gone already.
    func unregister(_ handle: String) async throws {
        let (status, data) = try await call("DELETE", "/v1/devices/\(FuseboxAPI.encode(handle))")
        guard (200..<300).contains(status) || status == 404 || status == 410 else { throw failure(status, data) }
    }
}
