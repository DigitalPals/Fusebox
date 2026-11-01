import Foundation

/// RFC 3339 parsing without a formatter: Fusebox writes nanoseconds ("…54.326731560Z") and
/// `+00:00` offsets, which ISO8601DateFormatter doesn't accept in one configuration.
public enum FuseboxDate {
    public static func parse(_ s: String) -> Date? {
        let u = Array(s.utf8)
        guard u.count >= 19 else { return nil }
        func digits(_ at: Int, _ n: Int) -> Int? {
            guard at + n <= u.count else { return nil }
            var v = 0
            for i in at..<(at + n) {
                let c = u[i]
                guard c >= 48, c <= 57 else { return nil }
                v = v * 10 + Int(c - 48)
            }
            return v
        }
        guard let y = digits(0, 4), u[4] == 45, let mo = digits(5, 2), u[7] == 45, let d = digits(8, 2),
              u[10] == 84 || u[10] == 116 || u[10] == 32,
              let h = digits(11, 2), u[13] == 58, let mi = digits(14, 2), u[16] == 58, let sec = digits(17, 2),
              (1...12).contains(mo), (1...31).contains(d), h < 24, mi < 60, sec < 61
        else { return nil }
        var i = 19
        var fraction = 0.0
        if i < u.count, u[i] == 46 {
            i += 1
            var scale = 0.1
            while i < u.count, u[i] >= 48, u[i] <= 57 {
                fraction += Double(u[i] - 48) * scale
                scale /= 10
                i += 1
            }
        }
        var offset = 0
        if i < u.count {
            switch u[i] {
            case 90, 122: i += 1 // Z
            case 43, 45:
                let sign = u[i] == 45 ? -1 : 1
                guard let oh = digits(i + 1, 2) else { return nil }
                let om: Int
                if i + 3 < u.count, u[i + 3] == 58, let m = digits(i + 4, 2) { om = m; i += 6 }
                else if let m = digits(i + 3, 2) { om = m; i += 5 }
                else { om = 0; i += 3 }
                offset = sign * (oh * 3600 + om * 60)
            default: return nil
            }
        }
        guard i == u.count else { return nil }
        // Days from the civil date (Howard Hinnant's algorithm).
        let yy = mo <= 2 ? y - 1 : y
        let era = (yy >= 0 ? yy : yy - 399) / 400
        let yoe = yy - era * 400
        let mp = (mo + 9) % 12
        let doy = (153 * mp + 2) / 5 + d - 1
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        let days = era * 146_097 + doe - 719_468
        let seconds = Double(days * 86400 + h * 3600 + mi * 60 + sec - offset) + fraction
        return Date(timeIntervalSince1970: seconds)
    }

    public static func format(_ date: Date) -> String {
        date.formatted(.iso8601.year().month().day().time(includingFractionalSeconds: true).timeZone(separator: .omitted))
    }
}

/// The dashboard's number and time wording (ui/app.js `fmt`, `ms`, `ago`, `left`, `compactLeft`, `span`).
public enum Fmt {
    /// "950", "12.3k", "124k", "2.25M", "1.5B".
    public static func count(_ value: Int?) -> String {
        let n = Double(value ?? 0)
        if n < 1000 { return Int(n).formatted(.number.locale(Locale(identifier: "en_US"))) }
        if n < 1e6 { return trim(String(format: n < 1e5 ? "%.1f" : "%.0f", n / 1e3)) + "k" }
        if n < 1e9 {
            let digits = n < 1e7 ? 2 : n < 1e8 ? 1 : 0
            return trimZeros(String(format: "%.\(digits)f", n / 1e6)) + "M"
        }
        return trimZeros(String(format: "%.2f", n / 1e9)) + "B"
    }

    private static func trim(_ s: String) -> String { s.hasSuffix(".0") ? String(s.dropLast(2)) : s }
    private static func trimZeros(_ s: String) -> String {
        guard s.contains(".") else { return s }
        var t = s
        while t.hasSuffix("0") { t.removeLast() }
        if t.hasSuffix(".") { t.removeLast() }
        return t
    }

    /// Latency: "850ms", "1.2s", "14s", "2m 5s".
    public static func ms(_ v: Int?) -> String {
        guard let v else { return "—" }
        if v < 1000 { return "\(v)ms" }
        if v < 60000 { return String(format: v < 10000 ? "%.1fs" : "%.0fs", Double(v) / 1000) }
        return "\(v / 60000)m \(Int((Double(v % 60000) / 1000).rounded()))s"
    }

    /// "just now", "45s ago", "12m ago", "3h ago", "2d ago", "never".
    public static func ago(_ date: Date?, now: Date = .now) -> String {
        guard let date else { return "never" }
        let s = max(0, now.timeIntervalSince(date))
        if s < 10 { return "just now" }
        if s < 60 { return "\(Int(s))s ago" }
        if s < 3600 { return "\(Int(s / 60))m ago" }
        if s < 86400 { return "\(Int(s / 3600))h ago" }
        return "\(Int(s / 86400))d ago"
    }

    /// Time left: "2d 22h", "1h 41m", "41m", "45s".
    public static func left(_ date: Date, now: Date = .now) -> String {
        let s = max(0, Int(date.timeIntervalSince(now).rounded()))
        if s >= 86400 { return "\(s / 86400)d \((s % 86400) / 3600)h" }
        if s >= 3600 { return "\(s / 3600)h \(String(format: "%02d", (s % 3600) / 60))m" }
        if s >= 60 { return "\(s / 60)m" }
        return "\(s)s"
    }

    /// The meters' countdown: at most two units, no zero suffix: "3d 7h", "2h 14m", "41m", "<1m", "now".
    public static func compactLeft(_ date: Date, now: Date = .now) -> String {
        let remaining = date.timeIntervalSince(now)
        if remaining <= 0 { return "now" }
        let minutes = Int(remaining / 60)
        if minutes >= 1440 {
            let hours = (minutes % 1440) / 60
            return "\(minutes / 1440)d" + (hours > 0 ? " \(hours)h" : "")
        }
        if minutes >= 60 {
            let rest = minutes % 60
            return "\(minutes / 60)h" + (rest > 0 ? " \(rest)m" : "")
        }
        return minutes > 0 ? "\(minutes)m" : "<1m"
    }

    /// A duration: "45s", "12m", "5h 11m", "2d 3h".
    public static func span(_ secs: TimeInterval) -> String {
        let s = Int(max(0, secs))
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m" }
        if s < 86400 { return "\(s / 3600)h \((s % 3600) / 60)m" }
        return "\(s / 86400)d \((s % 86400) / 3600)h"
    }

    private static let gb = Locale(identifier: "en_GB")

    /// 24-hour clock in the device's time zone, whatever the locale: "14:05".
    public static func hm(_ date: Date) -> String {
        date.formatted(Date.FormatStyle(locale: gb).hour(.twoDigits(amPM: .omitted)).minute(.twoDigits))
    }

    /// "14:05:09", as the request log shows times.
    public static func clock(_ date: Date) -> String {
        date.formatted(Date.FormatStyle(locale: gb).hour(.twoDigits(amPM: .omitted)).minute(.twoDigits).second(.twoDigits))
    }

    /// "Fri".
    public static func weekday(_ date: Date) -> String {
        date.formatted(Date.FormatStyle(locale: gb).weekday(.abbreviated))
    }

    /// Within a day "20:09", otherwise "Fri 14:58".
    public static func when(_ date: Date, now: Date = .now) -> String {
        date.timeIntervalSince(now) < 20 * 3600 ? hm(date) : "\(weekday(date)) \(hm(date))"
    }

    /// The full reset time: "Monday 2 November 2026, 13:59 CET".
    public static func fullTime(_ date: Date) -> String {
        date.formatted(Date.FormatStyle(locale: gb).weekday(.wide).day().month(.wide).year()
            .hour(.twoDigits(amPM: .omitted)).minute(.twoDigits).timeZone(.specificName(.short)))
    }

    /// Whole percentages whose ends never round onto 0% or 100% unless they are: "<1%", ">99%".
    public static func percent(_ value: Double) -> String {
        let rounded = Int(value.rounded())
        if value > 0, rounded == 0 { return "<1%" }
        if value < 100, rounded == 100 { return ">99%" }
        return "\(rounded)%"
    }

    /// "1 request", "3 requests".
    public static func plural(_ n: Int, _ word: String, _ many: String? = nil) -> String {
        "\(count(n)) \(n == 1 ? word : (many ?? word + "s"))"
    }

    /// Usage money: USD nanounits; cents above a dollar, four places below. nil reads "Unpriced".
    public static func dollars(nanos: Int64?) -> String {
        guard let nanos else { return "Unpriced" }
        let v = Double(nanos) / 1e9
        let small = v != 0 && abs(v) < 1
        return v.formatted(.currency(code: "USD").locale(Locale(identifier: "en_US"))
            .precision(.fractionLength(small ? 4 : 2)))
    }

    /// Usage's short dollars for bar labels: "$1,234", "$4.20", "$0".
    public static func dollarsShort(_ v: Double) -> String {
        if v >= 10 { return "$" + Int(v.rounded()).formatted(.number.locale(Locale(identifier: "en_US"))) }
        return v > 0 ? String(format: "$%.2f", v) : "$0"
    }

    /// Usage's compact token counts: "512", "3.4k", "12.0M", "1.23B".
    public static func compact(_ v: Int?) -> String {
        guard let v else { return "Unknown" }
        let n = Double(v)
        if n >= 1e9 { return String(format: "%.2fB", n / 1e9) }
        if n >= 1e6 { return String(format: "%.1fM", n / 1e6) }
        if n >= 1e3 { return String(format: "%.1fk", n / 1e3) }
        return String(Int(n.rounded()))
    }

    public static func capitalized(_ s: String) -> String {
        guard let f = s.first else { return s }
        return f.uppercased() + s.dropFirst()
    }
}
