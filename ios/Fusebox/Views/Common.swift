import SafariServices
import SwiftUI

// MARK: - Navigation

enum AppSection: String, CaseIterable, Identifiable, Hashable {
    case overview, accounts, requests, usage, faults, settings
    var id: String { rawValue }
    var title: String {
        switch self {
        case .overview: "Overview"; case .accounts: "Accounts"; case .requests: "Requests"
        case .usage: "Usage"; case .faults: "Faults"; case .settings: "Settings"
        }
    }
    var icon: String {
        switch self {
        case .overview: "gauge.with.dots.needle.33percent"; case .accounts: "person.2"
        case .requests: "list.bullet.rectangle"; case .usage: "chart.bar.xaxis"
        case .faults: "exclamationmark.triangle"; case .settings: "gearshape"
        }
    }
}

/// Moves the scene to a place: an account, a section. Set by MainView from its scene state.
struct Navigate: @unchecked Sendable {
    var go: @MainActor (DeepLink) -> Void
    @MainActor func callAsFunction(_ link: DeepLink) { go(link) }
}

extension EnvironmentValues {
    @Entry var navigate = Navigate { _ in }
}

// MARK: - Panels

/// A `surface` panel with a 1px hairline (DESIGN.md "panel"), optionally with rivets.
struct Card<Content: View>: View {
    var rivets = false
    var padding: CGFloat = 14
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) { content }
            // Cards take the height their text needs at the given width; never truncate notes.
            .fixedSize(horizontal: false, vertical: true)
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Palette.line, lineWidth: 1))
            .overlay { if rivets { Rivets() } }
    }
}

struct Rivets: View {
    var body: some View {
        GeometryReader { g in
            ForEach(0..<4, id: \.self) { i in
                Circle().fill(Palette.rivet).frame(width: 6, height: 6)
                    .position(x: i % 2 == 0 ? 7 : g.size.width - 7, y: i < 2 ? 7 : g.size.height - 7)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// "SUBSCRIPTIONS" with a meta line, as the dashboard's section heads.
struct SectionHead<Trailing: View>: View {
    var title: String
    var meta: String?
    @ViewBuilder var trailing: Trailing

    init(_ title: String, meta: String? = nil, @ViewBuilder trailing: () -> Trailing = { EmptyView() }) {
        self.title = title; self.meta = meta; self.trailing = trailing()
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).capsLabel(12.5, tracking: 1.5)
                if let meta, !meta.isEmpty {
                    Text(meta).font(.caption).foregroundStyle(Palette.fg3).lineLimit(2)
                }
            }
            Spacer(minLength: 8)
            trailing
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

/// One meter row: "5H [segments] 47% ↺ 2h 14m".
struct MeterRow: View {
    var label: String
    var title: String
    var reading: QuotaReading?
    var pace: Double?
    var resetsAt: Date?
    var mode: QuotaDisplay
    var height: CGFloat = 9

    var body: some View {
        HStack(spacing: 8) {
            Text(label).font(Typeface.label(10.5)).tracking(0.8).foregroundStyle(Palette.fg3).frame(width: 30, alignment: .leading)
            SegmentedMeter(reading, pace: pace, mode: mode, height: height, label: title)
                .frame(maxWidth: 200)
            Text(reading.map { Fmt.percent($0.value(mode)) } ?? "–")
                .font(Typeface.data(12)).monospacedDigit()
                .foregroundStyle(reading.map { $0.level == .normal ? Palette.fg : Palette.level($0.level) } ?? Palette.fg3)
                .frame(width: 40, alignment: .trailing)
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                Text(resetsAt.map { "↺ \(Fmt.compactLeft($0, now: ctx.date))" } ?? "")
                    .font(Typeface.data(11)).foregroundStyle(Palette.fg3)
            }
            .frame(minWidth: 64, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
    }
}

/// An account's 5H and WK rows (the 5H row is left out when there's no 5-hour limit).
struct AccountMeters: View {
    var account: Account
    var mode: QuotaDisplay

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if !account.noShortLimit { row(short: true) }
            row(short: false)
        }
    }

    private func row(short: Bool) -> some View {
        let w = account.window(short: short)
        return MeterRow(label: Quotas.label(w, short: short),
                        title: w.map(Quotas.title) ?? (short ? "5-hour" : "Weekly"),
                        reading: QuotaReading(w), pace: Quotas.elapsed(w), resetsAt: w?.resetsAt, mode: mode)
    }
}

struct StatusLabel: View {
    var account: Account
    var inFlight: Int

    var body: some View {
        let st = account.state()
        HStack(spacing: 6) {
            StatusDot(inFlight > 0 && st.kind == .ready ? Palette.ok : st.color, size: 7)
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                Text(account.statusWord(inFlight: inFlight, now: ctx.date))
                    .font(.footnote)
                    .foregroundStyle(st.kind == .ready ? Palette.fg2 : st.color)
            }
        }
    }
}

// MARK: - Fold-aware columns

/// Lays out cards in columns that never straddle the fold. On iOS 27.1 the scene reports a
/// `.division` reserved region where content should divide (the iPhone Duo's hinge when
/// unfolded); when one runs top to bottom through this view, the cards go into a column on
/// each side of it. Otherwise the column count comes from the width alone.
struct FoldAwareColumns<Item: Identifiable, Content: View>: View {
    var items: [Item]
    var minColumnWidth: CGFloat = 340
    var spacing: CGFloat = 14
    @ViewBuilder var content: (Item) -> Content

    @State private var geometry = FoldGeometry.zero

    var body: some View {
        let plan = layout()
        HStack(alignment: .top, spacing: 0) {
            ForEach(Array(plan.columns.enumerated()), id: \.offset) { index, column in
                VStack(spacing: spacing) {
                    ForEach(column) { content($0) }
                }
                .frame(width: plan.widths[index], alignment: .top)
                if index < plan.columns.count - 1 { Spacer().frame(width: plan.gaps[index]) }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .onGeometryChange(for: FoldGeometry.self) { proxy in
            FoldGeometry(proxy)
        } action: { geometry = $0 }
    }

    private func layout() -> (columns: [[Item]], widths: [CGFloat], gaps: [CGFloat]) {
        let w = max(geometry.width, 1)
        let n = max(1, min(3, Int((w + spacing) / (minColumnWidth + spacing))))
        // Two columns either way, or a hinge that content must not span: divide at the fold.
        if let d = geometry.division, geometry.divisionActive || n == 2 {
            // The region's frame already includes the margins to keep clear of the hinge.
            let leftWidth = max(0, d.minX), rightWidth = max(0, w - d.maxX)
            return (distribute(into: 2), [leftWidth, rightWidth], [w - leftWidth - rightWidth])
        }
        let colWidth = (w - spacing * CGFloat(n - 1)) / CGFloat(n)
        return (distribute(into: n), Array(repeating: colWidth, count: n), Array(repeating: spacing, count: max(0, n - 1)))
    }

    /// Fills columns in order, so related cards stay in reading order: first half left.
    private func distribute(into n: Int) -> [[Item]] {
        guard n > 1 else { return [items] }
        var columns = Array(repeating: [Item](), count: n)
        let per = Int((Double(items.count) / Double(n)).rounded(.up))
        for (i, item) in items.enumerated() { columns[min(n - 1, i / max(per, 1))].append(item) }
        return columns
    }
}

/// The width of a view and the vertical `.division` region running through it, if any.
/// The iPhone Duo reports its hinge as a division region; it is inactive while the phone
/// lies flat (content may span it) and active when half-open (content must divide).
struct FoldGeometry: Equatable, Sendable {
    var width: CGFloat
    var division: CGRect?
    var divisionActive = false

    static let zero = FoldGeometry()
    private init() { width = 0 }

    nonisolated init(_ proxy: GeometryProxy) {
        width = proxy.size.width
        division = nil
        guard #available(iOS 27.1, *) else { return }
        let size = proxy.size
        // A vertical division across this view, with room for a column on each side.
        let region = proxy.reservedRegions(kind: .division, options: .includeInactive)
            .filter { $0.frame.height >= $0.frame.width && $0.frame.minX > 200 && $0.frame.maxX < size.width - 200 }
            .max { a, b in !a.isActive && b.isActive }
        division = region?.frame
        divisionActive = region?.isActive ?? false
    }
}

/// Keeps a view's content off a hinge that runs along its leading or trailing part (a
/// column that starts or ends near the fold): the content is inset to clear it. A hinge
/// through the middle is left to FoldAwareColumns.
struct ClearOfFold: ViewModifier {
    @State private var insets = FoldInsets()

    func body(content: Content) -> some View {
        content
            .padding(.leading, insets.leading)
            .padding(.trailing, insets.trailing)
            .onGeometryChange(for: FoldInsets.self) { FoldInsets($0) } action: { insets = $0 }
    }
}

struct FoldInsets: Equatable, Sendable {
    var leading: CGFloat = 0
    var trailing: CGFloat = 0

    init() {}

    nonisolated init(_ proxy: GeometryProxy) {
        guard #available(iOS 27.1, *) else { return }
        let w = proxy.size.width
        for r in proxy.reservedRegions(kind: .division, options: .includeInactive) {
            let f = r.frame
            guard f.height >= f.width, f.maxX > 0, f.minX < w else { continue }
            if f.midX < w * 0.4 { leading = max(leading, f.maxX) }
            else if f.midX > w * 0.6 { trailing = max(trailing, w - f.minX) }
        }
    }
}

extension View {
    func clearOfFold() -> some View { modifier(ClearOfFold()) }
}

// MARK: - Web dashboard

/// The dashboard in an in-app browser, for what the app doesn't do (sign-in flows, config).
struct SafariView: UIViewControllerRepresentable {
    var url: URL
    func makeUIViewController(context: Context) -> SFSafariViewController {
        let c = SFSafariViewController(url: url)
        c.preferredControlTintColor = UIColor(hex: 0xF3B52F)
        return c
    }
    func updateUIViewController(_ vc: SFSafariViewController, context: Context) {}
}

extension URL: @retroactive Identifiable {
    public var id: String { absoluteString }
}

/// A short-lived confirmation or error, bottom centre like the dashboard's toast.
struct Toast: ViewModifier {
    @Binding var text: String?
    func body(content: Content) -> some View {
        content.overlay(alignment: .bottom) {
            if let text {
                Text(text)
                    .font(.footnote)
                    .foregroundStyle(Palette.bg)
                    .padding(.horizontal, 14).padding(.vertical, 9)
                    .background(Palette.fg, in: Capsule())
                    .padding(.bottom, 24)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .task(id: text) {
                        try? await Task.sleep(for: .seconds(2.4))
                        withAnimation { self.text = nil }
                    }
                    .accessibilityAddTraits(.updatesFrequently)
            }
        }
        .animation(.snappy, value: text)
    }
}

extension View {
    func toast(_ text: Binding<String?>) -> some View { modifier(Toast(text: text)) }
}

#if DEBUG
/// Debug builds, `FUSEBOX_DEBUG_REGIONS=1`: outlines the scene's reserved regions
/// (division in red, occlusion in blue; inactive ones dashed) to check the layout against the fold.
struct ReservedRegionsOverlay: View {
    var body: some View {
        GeometryReader { proxy in
            if #available(iOS 27.1, *) {
                let division = proxy.reservedRegions(kind: .division, options: .includeInactive)
                let occlusion = proxy.reservedRegions(kind: .occlusion, options: .includeInactive)
                ZStack(alignment: .topLeading) {
                    ForEach(division) { r in region(r.frame, r.isActive, .red, "division") }
                    ForEach(occlusion) { r in region(r.frame, r.isActive, .blue, "occlusion") }
                    Text("size \(Int(proxy.size.width))×\(Int(proxy.size.height)) · division \(division.map { "\(Int($0.frame.minX))–\(Int($0.frame.maxX))\($0.isActive ? "" : " inactive")" }.joined(separator: ", "))")
                        .font(.caption2.monospaced()).padding(4).background(.black.opacity(0.7)).foregroundStyle(.white)
                        .position(x: proxy.size.width / 2, y: proxy.size.height - 30)
                }
            }
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
    }

    private func region(_ f: CGRect, _ active: Bool, _ c: Color, _ name: String) -> some View {
        Rectangle()
            .strokeBorder(c, style: StrokeStyle(lineWidth: 2, dash: active ? [] : [6, 4]))
            .background(c.opacity(0.15))
            .frame(width: max(f.width, 2), height: max(f.height, 2))
            .position(x: f.midX, y: f.midY)
    }
}
#endif
