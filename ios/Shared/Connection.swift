import Foundation

/// The app's own preferences (push subscription). The widgets don't need them.
public enum AppGroup {
    public static var defaults: UserDefaults { .standard }
}

public enum Connection {
    public static let defaultURL = ""

    public static var serverURL: URL? {
        get { SharedKeychain.data("server-url").flatMap { String(data: $0, encoding: .utf8) }.flatMap(URL.init(string:)) }
        set { SharedKeychain.set("server-url", newValue.map { Data($0.absoluteString.utf8) }) }
    }

    public static var key: String? {
        SharedKeychain.data("management-key").flatMap { String(data: $0, encoding: .utf8) }
    }

    @discardableResult
    public static func setKey(_ key: String?) -> Bool {
        SharedKeychain.set("management-key", key.map { Data($0.utf8) })
    }

    public static var api: FuseboxAPI? {
        guard let url = serverURL, let key, !key.isEmpty else { return nil }
        return FuseboxAPI(baseURL: url, key: key)
    }

    /// Accepts "fusebox.example.ts.net" or a full URL; keeps the scheme, host, port and path.
    public static func normalize(_ text: String) -> URL? {
        var t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return nil }
        if !t.contains("://") { t = "https://" + t }
        while t.hasSuffix("/") { t.removeLast() }
        guard let url = URL(string: t), let scheme = url.scheme?.lowercased(), ["https", "http"].contains(scheme),
              url.host() != nil else { return nil }
        return url
    }
}

// MARK: - API

public struct APIError: LocalizedError, Sendable, Equatable {
    public var status: Int
    public var message: String
    public var errorDescription: String? { message }

    public var isUnauthorized: Bool { status == 401 }
}

/// `Authorization: Bearer <management-key>` against `<server>/api`.
public struct FuseboxAPI: Sendable {
    public var baseURL: URL
    public var key: String

    public init(baseURL: URL, key: String) { self.baseURL = baseURL; self.key = key }

    static let session: URLSession = {
        let c = URLSessionConfiguration.default
        c.timeoutIntervalForRequest = 20
        c.waitsForConnectivity = false
        c.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: c)
    }()

    public func url(_ path: String) -> URL {
        URL(string: baseURL.absoluteString + "/api" + path)!
    }

    public func request(_ method: String, _ path: String, body: Data? = nil) async throws -> Data {
        var req = URLRequest(url: url(path))
        req.httpMethod = method
        req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            req.httpBody = body
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await Self.session.data(for: req)
        } catch let e as URLError {
            throw APIError(status: 0, message: Self.describe(e))
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            throw APIError(status: status, message: Self.message(status: status, data: data))
        }
        return data
    }

    /// The server's own `{"error": "..."}` text when there is one.
    static func message(status: Int, data: Data) -> String {
        if let e = try? JSONDecoder().decode(ErrorResponse.self, from: data).error, !e.isEmpty { return e }
        switch status {
        case 401: return "The management key was not accepted."
        case 403: return "This server only accepts management from its own machine. Set a management key in its config.yaml."
        case 404: return "Not found (\(status)). This Fusebox may be older than the app."
        default:
            let text = String(data: data.prefix(200), encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return text.isEmpty || text.hasPrefix("<") ? "The server answered \(status) \(HTTPURLResponse.localizedString(forStatusCode: status))." : text
        }
    }

    static func describe(_ e: URLError) -> String {
        switch e.code {
        case .notConnectedToInternet: "This iPhone is offline."
        case .cannotFindHost, .dnsLookupFailed: "Can’t find that server. On a tailnet address, check that Tailscale is connected."
        case .cannotConnectToHost: "The server refused the connection."
        case .timedOut: "The server didn’t answer in time. On a tailnet address, check that Tailscale is connected."
        case .secureConnectionFailed, .serverCertificateUntrusted, .serverCertificateHasBadDate, .serverCertificateNotYetValid:
            "The server’s HTTPS certificate wasn’t accepted."
        case .appTransportSecurityRequiresSecureConnection: "Use an https:// address."
        default: e.localizedDescription
        }
    }

    public func get<T: Decodable>(_ path: String, as: T.Type = T.self) async throws -> T {
        let data = try await request("GET", path)
        return try JSONDecoder.fusebox.decode(T.self, from: data)
    }

    @discardableResult
    public func send(_ method: String, _ path: String, json: some Encodable) async throws -> Data {
        try await request(method, path, body: try JSONEncoder.fusebox.encode(json))
    }

    @discardableResult
    public func send(_ method: String, _ path: String) async throws -> Data {
        try await request(method, path)
    }

    public static func encode(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-._~"))) ?? id
    }

    public func overview() async throws -> Overview { try await get("/overview") }
    public func accounts() async throws -> [Account] { try await get("/accounts") }
    public func faults() async throws -> [Fault] { try await get("/faults") }
    /// The dismissed faults that haven't cleared yet (404 from a Fusebox that can't dismiss).
    public func dismissedFaults() async throws -> [Fault] { try await get("/faults/dismissed") }
    public func requests(limit: Int? = nil) async throws -> [RequestLog] {
        try await get(limit.map { "/requests?limit=\($0)" } ?? "/requests")
    }
    public func activity(_ id: String) async throws -> AccountActivity { try await get("/accounts/\(Self.encode(id))/activity") }

    /// The live socket: the key goes in `?key=`, as the dashboard opens it.
    public var liveURL: URL {
        var c = URLComponents(url: url("/live"), resolvingAgainstBaseURL: false)!
        c.scheme = c.scheme == "http" ? "ws" : "wss"
        c.queryItems = [URLQueryItem(name: "key", value: key)]
        return c.url!
    }
}
