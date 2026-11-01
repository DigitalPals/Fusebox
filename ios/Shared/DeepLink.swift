import Foundation

/// Where a notification tap or a widget lands.
public enum DeepLink: Equatable, Sendable {
    case overview
    case account(String)
    case accounts
    case requests
    case request(Int)
    case usage
    case faults
    case settings

    /// A notification's `path` is the dashboard's hash route ("#/accounts/<url-encoded id>",
    /// "#/config/notifications", "#/overview"…). An account's route opens that account;
    /// anything else opens the Overview.
    public static func route(path: String?) -> DeepLink {
        guard var p = path?.trimmingCharacters(in: .whitespaces), !p.isEmpty else { return .overview }
        if p.hasPrefix("#") { p.removeFirst() }
        if p.hasPrefix("/") { p.removeFirst() }
        let parts = p.split(separator: "/", maxSplits: 1, omittingEmptySubsequences: false)
        guard parts.first == "accounts", parts.count == 2 else { return .overview }
        let raw = String(parts[1]).split(separator: "?").first.map(String.init) ?? ""
        let id = raw.removingPercentEncoding ?? raw
        return id.isEmpty ? .overview : .account(id)
    }

    /// The app's own links (widgets, Live Activity): `fusebox://accounts/<id>`, `fusebox://faults`…
    public static func route(url: URL) -> DeepLink {
        guard url.scheme == "fusebox" else { return .overview }
        let host = url.host() ?? ""
        let rest = url.path().trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        switch host {
        case "accounts":
            if rest.isEmpty { return .accounts }
            let id = rest.removingPercentEncoding ?? rest
            return .account(id)
        case "requests": return .requests
        case "usage": return .usage
        case "faults": return .faults
        case "settings": return .settings
        default: return .overview
        }
    }

    public static func url(account id: String) -> URL {
        URL(string: "fusebox://accounts/\(FuseboxAPI.encode(id))")!
    }
}
