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
    /// The page title as the dashboard sets it: condensed capitals.
    var pageTitle: String { title.uppercased() }
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

/// The page margin and the gaps between panels and sections (DESIGN.md "Layout", phone).
enum Metrics {
    static let page: CGFloat = 16
    static let panelGap: CGFloat = 16
    static let sectionGap: CGFloat = 24
    static let panelPadding: CGFloat = 16
    /// The widest a page's column gets (margins included): an iPhone Duo unfolded shows one
    /// centred column, as on the outer screen, not the page spread across the inner screen.
    static let column: CGFloat = 600
}

// MARK: - Panels

/// A `surface` panel with a 1px `line` hairline and 8pt corners (DESIGN.md "panel"),
/// optionally with the four rivets. Flat: no shadow.
struct Card<Content: View>: View {
    var rivets = false
    var padding: CGFloat = Metrics.panelPadding
    var spacing: CGFloat = 12
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: spacing) { content }
            // Panels take the height their text needs at the given width; never truncate notes.
            .fixedSize(horizontal: false, vertical: true)
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .panelBackground(rivets: rivets)
    }
}

extension View {
    func panelBackground(rivets: Bool = false) -> some View {
        self
            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Palette.line, lineWidth: 1))
            .overlay { if rivets { Rivets() } }
    }

    /// The page's scroll content: a 16pt margin, kept off the hinge, in one column at most
    /// `Metrics.column` wide, centred when the screen is wider (the scroll view stays full width).
    func pageContent() -> some View {
        self
            .padding(.horizontal, Metrics.page)
            .padding(.top, 8)
            .padding(.bottom, 24)
            .clearOfFold()
            .frame(maxWidth: Metrics.column, alignment: .leading)
            .frame(maxWidth: .infinity)
    }
}

struct Rivets: View {
    var body: some View {
        GeometryReader { g in
            ForEach(0..<4, id: \.self) { i in
                Circle().fill(Palette.rivet).frame(width: 5, height: 5)
                    .position(x: i % 2 == 0 ? 7 : g.size.width - 7, y: i < 2 ? 7 : g.size.height - 7)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// A 1px `line` rule between rows.
struct Hairline: View {
    var color = Palette.line
    var body: some View { Rectangle().fill(color).frame(height: 1).accessibilityHidden(true) }
}

/// A panel's head: "LIMITS · USED" on the left, a control on the right, aligned on one line.
struct PanelHead<Trailing: View>: View {
    var title: String
    @ViewBuilder var trailing: Trailing

    init(_ title: String, @ViewBuilder trailing: () -> Trailing = { EmptyView() }) {
        self.title = title; self.trailing = trailing()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Text(title).capsLabel(11.5)
                .accessibilityAddTraits(.isHeader)
                .frame(maxWidth: .infinity, alignment: .leading)
            trailing
        }
        .frame(minHeight: 28)
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
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .center, spacing: 12) {
                Text(title).capsLabel(12.5, color: Palette.fg)
                    .accessibilityAddTraits(.isHeader)
                    .frame(maxWidth: .infinity, alignment: .leading)
                trailing
            }
            if let meta, !meta.isEmpty {
                Text(meta).font(Typeface.meta).foregroundStyle(Palette.fg3)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// A label and its value on one line, as the dashboard's Sign-in panel: label `fg-3` left,
/// value `fg` right (DM Mono for data). Long values wrap under the label instead of clipping.
struct KeyValueRow: View {
    var key: String
    var value: String
    var mono = false
    var valueColor = Palette.fg

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .firstTextBaseline, spacing: 16) {
                Text(key).font(Typeface.row).foregroundStyle(Palette.fg3)
                Spacer(minLength: 8)
                valueText.multilineTextAlignment(.trailing).lineLimit(1)
            }
            VStack(alignment: .leading, spacing: 3) {
                Text(key).font(Typeface.row).foregroundStyle(Palette.fg3)
                valueText
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 10)
        .accessibilityElement(children: .combine)
    }

    private var valueText: some View {
        Text(value).font(mono ? Typeface.data(13, relativeTo: .subheadline) : Typeface.row).foregroundStyle(valueColor)
            .textSelection(.enabled)
    }
}

/// A small text button for panel heads ("All", "Clear all"): `fg-2`, 44pt touch target.
struct LinkButton: View {
    var title: String
    var action: () -> Void
    init(_ title: String, action: @escaping () -> Void) { self.title = title; self.action = action }
    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                Text(title)
                Image(systemName: "arrow.right").font(.system(size: 11, weight: .medium))
            }
            .font(Typeface.meta)
            .foregroundStyle(Palette.fg2)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// An empty page or column: a quiet icon, a title and one line, in the dashboard's type.
struct EmptyState: View {
    var title: String
    var systemImage: String
    var message: String?

    init(_ title: String, systemImage: String, message: String? = nil) {
        self.title = title; self.systemImage = systemImage; self.message = message
    }

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: systemImage).font(.system(size: 30, weight: .light)).foregroundStyle(Palette.fg3)
            Text(title).font(Typeface.sans(17, .semibold, relativeTo: .headline)).foregroundStyle(Palette.fg)
            if let message {
                Text(message).font(Typeface.row).foregroundStyle(Palette.fg3).multilineTextAlignment(.center)
            }
        }
        .padding(32)
        .frame(maxWidth: 420)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Meters

/// The reset cell beside a list meter: "↺ 2h 14m" in `fg-3`, or "out 1h 40m" in `warn` when the
/// pace so far uses the window up before it resets (DESIGN.md "Pace mark").
struct ResetCell: View {
    var window: QuotaWindow?
    var resetsAt: Date?
    var runsOutEarly = false

    init(window: QuotaWindow?) { self.window = window; resetsAt = window?.resetsAt }
    init(resetsAt: Date?, runsOutEarly: Bool = false) { window = nil; self.resetsAt = resetsAt; self.runsOutEarly = runsOutEarly }

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { ctx in
            let out = Quotas.runsOut(window, now: ctx.date)
            Group {
                if let out {
                    Text("out \(Fmt.compactLeft(out, now: ctx.date))").foregroundStyle(Palette.warn)
                } else if let resetsAt, resetsAt > ctx.date {
                    Text("↺ \(Fmt.compactLeft(resetsAt, now: ctx.date))").foregroundStyle(runsOutEarly ? Palette.warn : Palette.fg3)
                } else {
                    Text(" ")
                }
            }
            .font(Typeface.data(11.5))
            .lineLimit(1)
        }
    }
}

/// One list meter row: "5H [segments] 47% ↺ 2h 14m".
struct MeterRow: View {
    var label: String
    var title: String
    var reading: QuotaReading?
    var pace: Double?
    var window: QuotaWindow?
    var resetsAt: Date?
    var mode: QuotaDisplay
    var height: CGFloat = 9
    var knockout = Palette.surface

    var body: some View {
        HStack(spacing: 10) {
            Text(label).capsLabel(10.5)
                .frame(width: 26, alignment: .leading)
            SegmentedMeter(reading, pace: pace, mode: mode, height: height, label: title, knockout: knockout)
                .frame(minWidth: 60, maxWidth: 220)
            Text(reading.map { Fmt.percent($0.value(mode)) } ?? "–")
                .font(Typeface.data(13, relativeTo: .footnote))
                .foregroundStyle(reading.map { $0.level == .normal ? Palette.fg : Palette.level($0.level) } ?? Palette.fg3)
                .frame(width: 44, alignment: .trailing)
            Group {
                if let window { ResetCell(window: window) } else { ResetCell(resetsAt: resetsAt) }
            }
            .frame(width: 72, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
    }
}

/// An account's 5H and WK rows (the 5H row is left out when there's no 5-hour limit).
struct AccountMeters: View {
    var account: Account
    var mode: QuotaDisplay
    var knockout = Palette.surface

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if !account.noShortLimit { row(short: true) }
            row(short: false)
        }
    }

    private func row(short: Bool) -> some View {
        let w = account.window(short: short)
        return MeterRow(label: Quotas.label(w, short: short),
                        title: w.map(Quotas.title) ?? (short ? "5-hour" : "Weekly"),
                        reading: QuotaReading(w), pace: Quotas.elapsed(w), window: w, mode: mode, knockout: knockout)
    }
}

/// An 8pt dot and a word (DESIGN.md "Status"): Ready is an `ok` dot with an `fg-2` word;
/// Cooling, Error and Disabled take their state colour.
struct StatusLabel: View {
    var account: Account
    var inFlight: Int
    var font = Typeface.row

    var body: some View {
        let st = account.state()
        HStack(spacing: 7) {
            StatusDot(inFlight > 0 && st.kind == .ready ? Palette.ok : st.color)
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                Text(account.statusWord(inFlight: inFlight, now: ctx.date))
                    .font(font)
                    .lineLimit(1)
                    .foregroundStyle(st.kind == .ready ? Palette.fg2 : st.kind == .disabled ? Palette.fg3 : st.color)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Fold-aware columns

/// Lays out panels in columns that never straddle the fold. On iOS 27.1 the scene reports a
/// `.division` reserved region where content should divide (the iPhone Duo's hinge when
/// unfolded); when one runs top to bottom through this view, the panels go into a column on
/// each side of it. Otherwise the column count comes from the width alone.
///
/// The columns always take the width offered to them now (`ColumnsLayout`); the measured
/// geometry only chooses how many columns and where the hinge is. (Build 2 sized the columns
/// from the last measured width, which could grow but never shrink: when the window got
/// narrower, as when the iPhone Duo's outer screen moves its bars to the side or the phone
/// turns to portrait, the page stayed wider than the screen and was clipped on the left.)
struct FoldAwareColumns<Item: Identifiable, Column: View>: View {
    var items: [Item]
    var minColumnWidth: CGFloat = 340
    var spacing: CGFloat = Metrics.panelGap
    var column: ([Item]) -> Column

    @State private var geometry = FoldGeometry.zero

    init(items: [Item], minColumnWidth: CGFloat = 340, spacing: CGFloat = Metrics.panelGap,
         @ViewBuilder column: @escaping ([Item]) -> Column) {
        self.items = items; self.minColumnWidth = minColumnWidth; self.spacing = spacing; self.column = column
    }

    var body: some View {
        let plan = FoldPlan(geometry, minColumnWidth: minColumnWidth, spacing: spacing)
        ColumnsLayout(division: plan.division, spacing: spacing) {
            ForEach(Array(FoldPlan.distribute(items, into: plan.columns).enumerated()), id: \.offset) { _, items in
                column(items)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .onGeometryChange(for: FoldGeometry.self) { FoldGeometry($0) } action: { geometry = $0 }
    }
}

extension FoldAwareColumns {
    /// One view per item, stacked in each column.
    init<Content: View>(items: [Item], minColumnWidth: CGFloat = 340, spacing: CGFloat = Metrics.panelGap,
                        @ViewBuilder content: @escaping (Item) -> Content) where Column == ColumnStack<Item, Content> {
        self.init(items: items, minColumnWidth: minColumnWidth, spacing: spacing) { column in
            ColumnStack(items: column, spacing: spacing, content: content)
        }
    }
}

struct ColumnStack<Item: Identifiable, Content: View>: View {
    var items: [Item]
    var spacing: CGFloat
    var content: (Item) -> Content
    var body: some View {
        VStack(alignment: .leading, spacing: spacing) {
            ForEach(items) { content($0) }
        }
    }
}

/// How many columns, and the hinge to divide at, from the last geometry reading.
struct FoldPlan: Equatable {
    var columns: Int
    var division: CGRect?

    init(_ g: FoldGeometry, minColumnWidth: CGFloat, spacing: CGFloat) {
        let w = max(g.width, 1)
        let n = max(1, min(3, Int((w + spacing) / (minColumnWidth + spacing))))
        // Two columns either way, or a hinge that content must not span: divide at the fold,
        // when a whole column fits on each side of it (in the unfolded Duo's centred column it
        // doesn't, and the page stays one column).
        if let d = g.division, g.divisionActive || n == 2, d.minX >= minColumnWidth, g.width - d.maxX >= minColumnWidth {
            columns = 2; division = d
        } else {
            columns = n; division = nil
        }
    }

    /// Fills columns in order, so related panels stay in reading order: first half left.
    static func distribute<Item>(_ items: [Item], into n: Int) -> [[Item]] {
        guard n > 1, items.count > 1 else { return [items] }
        var columns = Array(repeating: [Item](), count: n)
        let per = Int((Double(items.count) / Double(n)).rounded(.up))
        for (i, item) in items.enumerated() { columns[min(n - 1, i / max(per, 1))].append(item) }
        return columns.filter { !$0.isEmpty }
    }
}

/// Columns side by side in exactly the width offered: equal widths with a gap, or, given a
/// hinge (from an earlier reading), one column on each side of it. A hinge that no longer fits
/// the width it's given is ignored, so the columns can never be wider than the view.
struct ColumnsLayout: Layout {
    var division: CGRect?
    var spacing: CGFloat

    func frames(width total: CGFloat, count n: Int) -> [(x: CGFloat, width: CGFloat)] {
        let total = max(0, total)
        guard n > 1 else { return [(0, total)] }
        if n == 2, let d = division, d.minX >= 160, d.maxX <= total - 160, d.width < total / 3 {
            return [(0, d.minX), (d.maxX, total - d.maxX)]
        }
        let w = max(0, (total - spacing * CGFloat(n - 1)) / CGFloat(n))
        return (0..<n).map { (CGFloat($0) * (w + spacing), w) }
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width: CGFloat
        if let w = proposal.width, w.isFinite {
            width = w
        } else {
            width = subviews.map { $0.sizeThatFits(.unspecified).width }.reduce(0, +) + spacing * CGFloat(max(0, subviews.count - 1))
        }
        let f = frames(width: width, count: subviews.count)
        let height = zip(subviews, f).map { $0.sizeThatFits(ProposedViewSize(width: $1.width, height: nil)).height }.max() ?? 0
        return CGSize(width: width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for (s, f) in zip(subviews, frames(width: bounds.width, count: subviews.count)) {
            s.place(at: CGPoint(x: bounds.minX + f.x, y: bounds.minY), anchor: .topLeading,
                    proposal: ProposedViewSize(width: f.width, height: nil))
        }
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

    init(width: CGFloat, division: CGRect?, active: Bool) {
        self.width = width; self.division = division; divisionActive = active
    }

    nonisolated init(_ proxy: GeometryProxy) {
        width = proxy.size.width
        division = nil
        let size = proxy.size
        var regions: [(frame: CGRect, active: Bool)] = DebugDivision.regions(proxy)
        if #available(iOS 27.1, *) {
            regions += proxy.reservedRegions(kind: .division, options: .includeInactive).map { ($0.frame, $0.isActive) }
        }
        // A vertical division across this view, with room for a column on each side.
        let region = regions
            .filter { $0.frame.height >= $0.frame.width && $0.frame.minX > 200 && $0.frame.maxX < size.width - 200 }
            .max { a, b in !a.active && b.active }
        division = region?.frame
        divisionActive = region?.active ?? false
    }
}

/// Keeps a view's content off an active hinge that runs along its leading or trailing part (a
/// column that starts or ends near the fold): the content is inset to clear it. A hinge
/// through the middle is left to FoldAwareColumns. Only active division regions count, the
/// insets are never negative, each is at most a third of the width, and none applies when
/// what's left would be narrower than a phone column.
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

    init(width w: CGFloat, height h: CGFloat, regions: [(frame: CGRect, active: Bool)]) {
        guard w > 0 else { return }
        for r in regions where r.active {
            let f = r.frame
            // A vertical band that crosses this view top to bottom (not a status area or a corner).
            guard f.width > 0, f.height >= f.width, f.height >= h * 0.5, f.maxX > 0, f.minX < w else { continue }
            if f.midX < w * 0.4 { leading = max(leading, f.maxX) }
            else if f.midX > w * 0.6 { trailing = max(trailing, w - f.minX) }
        }
        leading = min(max(0, leading), w / 3)
        trailing = min(max(0, trailing), w / 3)
        if w - leading - trailing < 280 { leading = 0; trailing = 0 }
    }

    nonisolated init(_ proxy: GeometryProxy) {
        var regions: [(frame: CGRect, active: Bool)] = DebugDivision.regions(proxy)
        if #available(iOS 27.1, *) {
            regions = proxy.reservedRegions(kind: .division, options: .includeInactive).map { (frame: $0.frame, active: $0.isActive) }
        }
        self.init(width: proxy.size.width, height: proxy.size.height, regions: regions)
    }
}

extension View {
    func clearOfFold() -> some View { modifier(ClearOfFold()) }
}

/// Debug builds, `FUSEBOX_DEBUG_DIVISION=<minX>-<maxX>[,active]` (window points, e.g. `455-495`):
/// a vertical division region as the iPhone Duo's hinge reports it, for checking the unfolded
/// layout where the simulator can't unfold (a resizable session at 951×669 has no hinge).
/// Release builds never have one.
enum DebugDivision {
    nonisolated static func regions(_ proxy: GeometryProxy) -> [(frame: CGRect, active: Bool)] {
        #if DEBUG
        guard let spec = ProcessInfo.processInfo.environment["FUSEBOX_DEBUG_DIVISION"] else { return [] }
        let parts = spec.split(separator: ",")
        let xs = parts.first?.split(separator: "-").compactMap { Double($0) } ?? []
        guard xs.count == 2, xs[1] > xs[0] else { return [] }
        let origin = proxy.frame(in: .global).origin
        let frame = CGRect(x: xs[0] - origin.x, y: -origin.y, width: xs[1] - xs[0], height: 10_000)
        return [(frame, parts.dropFirst().contains("active"))]
        #else
        return []
        #endif
    }
}

// MARK: - Web dashboard

/// The dashboard in an in-app browser, for what the app doesn't do (sign-in flows, config).
struct SafariView: UIViewControllerRepresentable {
    var url: URL
    func makeUIViewController(context: Context) -> SFSafariViewController {
        let c = SFSafariViewController(url: url)
        c.preferredControlTintColor = Palette.ui("fg")
        c.preferredBarTintColor = Palette.ui("bg")
        return c
    }
    func updateUIViewController(_ vc: SFSafariViewController, context: Context) {}
}

extension URL: @retroactive Identifiable {
    public var id: String { absoluteString }
}

/// A short-lived confirmation or error, bottom centre like the dashboard's toast: `fg` on `bg`.
struct Toast: ViewModifier {
    @Binding var text: String?
    func body(content: Content) -> some View {
        content.overlay(alignment: .bottom) {
            if let text {
                Text(text)
                    .font(Typeface.meta)
                    .foregroundStyle(Palette.bg)
                    .padding(.horizontal, 14).padding(.vertical, 10)
                    .background(Palette.fg, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .shadow(color: .black.opacity(0.25), radius: 16, y: 8)
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

/// The page title in the dashboard's condensed capitals, set once for every navigation bar.
@MainActor
enum NavigationStyle {
    static func apply() {
        let large = UIFont(name: "IBMPlexSansCond-SemiBold", size: 30) ?? .systemFont(ofSize: 30, weight: .semibold)
        let inline = UIFont(name: "IBMPlexSans-SemiBold", size: 17) ?? .systemFont(ofSize: 17, weight: .semibold)
        let bar = UINavigationBar.appearance()
        bar.largeTitleTextAttributes = [
            .font: UIFontMetrics(forTextStyle: .largeTitle).scaledFont(for: large),
            .foregroundColor: Palette.ui("fg"),
            .kern: 2.0,
        ]
        bar.titleTextAttributes = [
            .font: UIFontMetrics(forTextStyle: .headline).scaledFont(for: inline),
            .foregroundColor: Palette.ui("fg"),
        ]
    }
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
                    ForEach(Array(DebugDivision.regions(proxy).enumerated()), id: \.offset) { _, r in
                        region(r.frame.intersection(CGRect(origin: .zero, size: proxy.size)), r.active, .red, "debug division")
                    }
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
