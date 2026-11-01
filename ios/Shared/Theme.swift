import SwiftUI
import UIKit

/// DESIGN.md's tokens: a warm near-black operator panel, and the same panel in daylight.
/// Each is an asset-catalogue colour with a dark and a light appearance
/// (`scripts/make-colors.mjs`). Colour is for state (ok, warn, err) and provider logos only;
/// there is no amber anywhere in the app, and the accent is `fg`.
public enum Palette {
    public static let bg = Color("bg")
    public static let surface = Color("surface")
    public static let surfacePop = Color("surface-pop")
    public static let surfaceInset = Color("surface-inset")
    public static let segOn = Color("seg-on")
    public static let ctlHover = Color("ctl-hover")
    public static let line = Color("line")
    public static let lineSoft = Color("line-soft")
    public static let lineStrong = Color("line-strong")
    public static let lineHover = Color("line-hover")
    public static let rivet = Color("rivet")
    public static let fg = Color("fg")
    public static let fg2 = Color("fg-2")
    public static let fg3 = Color("fg-3")
    public static let idle = Color("idle")
    public static let bar = Color("bar")
    public static let meter = Color("meter")
    public static let ok = Color("ok")
    public static let warn = Color("warn")
    public static let err = Color("err")
    public static let errLine = Color("err-line")
    /// Unlit meter segments.
    public static let segOff = line

    public static func level(_ l: QuotaReading.Level) -> Color {
        switch l { case .normal: meter; case .warn: warn; case .err: err }
    }

    /// A state word's colour ("ok", "warn", "err"; anything else reads as plain text).
    public static func state(_ level: String?) -> Color {
        switch level { case "err": err; case "warn": warn; case "ok": ok; default: fg2 }
    }

    /// Usage's stacked chart: neutral steps from `meter` down; colour stays for state.
    public static let chart: [Color] = [Color("chart-1"), Color("chart-2"), Color("chart-3"), Color("chart-4"), Color("chart-5")]
    public static let chartOther = Color("chart-other")

    /// UIKit's view of a token (navigation bar titles, the in-app browser).
    public static func ui(_ name: String) -> UIColor { UIColor(named: name) ?? .label }
}

/// The dashboard's three faces, bundled from ui/fonts (SIL OFL 1.1, licences in Shared/Fonts):
/// IBM Plex Sans for reading, IBM Plex Sans Condensed (capitals, tracked) for labels, DM Mono for
/// data. Every size scales with Dynamic Type relative to a text style, and all three faces have
/// tabular figures by default.
public enum Typeface {
    public enum Weight { case regular, medium, semibold }

    /// IBM Plex Sans (the variable font's named instances).
    public static func sans(_ size: CGFloat, _ weight: Weight = .regular, relativeTo style: Font.TextStyle = .body) -> Font {
        let name = switch weight { case .regular: "IBMPlexSans-Regular"; case .medium: "IBMPlexSans-Medium"; case .semibold: "IBMPlexSans-SemiBold" }
        return .custom(name, size: size, relativeTo: style)
    }

    /// IBM Plex Sans Condensed SemiBold, for capitals labels ("LIMITS · USED", "5H").
    public static func label(_ size: CGFloat = 11, relativeTo style: Font.TextStyle = .caption2) -> Font {
        .custom("IBMPlexSansCond-SemiBold", size: size, relativeTo: style)
    }

    /// DM Mono, for data: times, ids, countdowns, keys, model names.
    public static func data(_ size: CGFloat = 12, weight: Weight = .regular, relativeTo style: Font.TextStyle = .caption) -> Font {
        .custom(weight == .regular ? "DMMono-Regular" : "DMMono-Medium", size: size, relativeTo: style)
    }

    // The roles, phone sizes (DESIGN.md "Typography").
    public static let body = sans(15, relativeTo: .body)
    public static let row = sans(14, relativeTo: .subheadline)
    public static let rowMedium = sans(14, .medium, relativeTo: .subheadline)
    public static let meta = sans(13, relativeTo: .footnote)
    public static let small = sans(12, relativeTo: .caption)
    public static let figure = sans(20, .medium, relativeTo: .title3)
    public static let bigFigure = sans(30, .medium, relativeTo: .largeTitle)
    public static let title = sans(20, .semibold, relativeTo: .title3)
}

extension Text {
    /// Uppercase, tracked condensed label ("SUBSCRIPTIONS", "5H"): `fg-3` unless coloured.
    public func capsLabel(_ size: CGFloat = 11, tracking: CGFloat? = nil, color: Color = Palette.fg3) -> some View {
        self.textCase(.uppercase)
            .font(Typeface.label(size))
            .tracking(tracking ?? size * 0.12)
            .foregroundStyle(color)
    }
}

// MARK: - Controls

/// The dashboard's button: a 1px `line-strong` outline with `fg` text (`.normal`), the one
/// primary per view (`fg` fill, `bg` text), or a destructive one (`err` text). 8pt corners;
/// the touch target is at least 44pt even when the button looks smaller.
public struct FuseButtonStyle: ButtonStyle {
    public enum Kind: Sendable { case normal, primary, destructive, quiet }
    public var kind: Kind
    public var compact: Bool
    public var fill: Bool

    public init(_ kind: Kind = .normal, compact: Bool = false, fill: Bool = false) {
        self.kind = kind; self.compact = compact; self.fill = fill
    }

    public func makeBody(configuration: Configuration) -> some View {
        FuseButtonBody(configuration: configuration, kind: kind, compact: compact, fill: fill)
    }
}

private struct FuseButtonBody: View {
    @Environment(\.isEnabled) private var enabled
    var configuration: ButtonStyleConfiguration
    var kind: FuseButtonStyle.Kind
    var compact: Bool
    var fill: Bool

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: compact ? 6 : 8, style: .continuous)
        let height: CGFloat = compact ? 32 : 40
        configuration.label
            .font(compact ? Typeface.meta : (kind == .primary ? Typeface.sans(15, .medium) : Typeface.sans(15)))
            .lineLimit(1)
            .foregroundStyle(text)
            .padding(.horizontal, compact ? 12 : 16)
            .frame(maxWidth: fill ? .infinity : nil, minHeight: height)
            .background(background, in: shape)
            .overlay(shape.strokeBorder(border, lineWidth: 1))
            .contentShape(Rectangle().inset(by: -(44 - height) / 2))
            .opacity(configuration.isPressed ? 0.65 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }

    private var text: Color {
        guard enabled else { return Palette.idle }
        switch kind {
        case .primary: return Palette.bg
        case .destructive: return Palette.err
        case .quiet: return Palette.fg2
        case .normal: return Palette.fg
        }
    }

    private var background: Color {
        guard kind == .primary else { return .clear }
        return enabled ? Palette.fg : Palette.ctlHover
    }

    private var border: Color {
        if kind == .quiet { return .clear }
        if !enabled { return kind == .primary ? Palette.ctlHover : Palette.line }
        return kind == .primary ? Palette.fg : Palette.lineStrong
    }
}

extension ButtonStyle where Self == FuseButtonStyle {
    /// The outline button.
    public static var outline: FuseButtonStyle { FuseButtonStyle(.normal) }
    /// The outline button, 32pt tall (44pt touch target), for panel headers and rows.
    public static var outlineSmall: FuseButtonStyle { FuseButtonStyle(.normal, compact: true) }
    /// The view's one primary button.
    public static var primary: FuseButtonStyle { FuseButtonStyle(.primary) }
    /// A destructive action: `err` text in the outline.
    public static var destructive: FuseButtonStyle { FuseButtonStyle(.destructive) }
}

/// The breaker: a square-knobbed switch, `meter` track when on, no colour tint (DESIGN.md
/// "Breaker"). It keeps the system switch's accessibility (role, value, the UI tests' `switches`).
public struct BreakerToggleStyle: ToggleStyle {
    public var labelFirst: Bool
    public init(labelFirst: Bool = true) { self.labelFirst = labelFirst }

    public func makeBody(configuration: Configuration) -> some View {
        BreakerBody(configuration: configuration, labelFirst: labelFirst)
    }
}

private struct BreakerBody: View {
    @Environment(\.isEnabled) private var enabled
    var configuration: ToggleStyleConfiguration
    var labelFirst: Bool

    var body: some View {
        Button {
            withAnimation(.easeOut(duration: 0.12)) { configuration.isOn.toggle() }
        } label: {
            HStack(spacing: 12) {
                if labelFirst { configuration.label.frame(maxWidth: .infinity, alignment: .leading) }
                BreakerSwitch(isOn: configuration.isOn)
                if !labelFirst { configuration.label }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.45)
        .accessibilityRepresentation {
            Toggle(isOn: configuration.$isOn) { configuration.label }
        }
    }
}

/// The switch itself: 42×24, 5pt corners, a 16pt square knob.
public struct BreakerSwitch: View {
    public var isOn: Bool
    public init(isOn: Bool) { self.isOn = isOn }

    public var body: some View {
        ZStack(alignment: isOn ? .trailing : .leading) {
            RoundedRectangle(cornerRadius: 5, style: .continuous)
                .fill(isOn ? Palette.meter : Palette.ctlHover)
                .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).strokeBorder(isOn ? Palette.meter : Palette.lineStrong, lineWidth: 1))
            RoundedRectangle(cornerRadius: 3, style: .continuous)
                .fill(isOn ? Palette.bg : Palette.idle)
                .frame(width: 16, height: 16)
                .padding(4)
        }
        .frame(width: 42, height: 24)
        .frame(minHeight: 44)
        .accessibilityHidden(true)
    }
}

/// The dashboard's segmented control: 2pt padding inside a 1px `line-strong` border; the
/// selected segment is `seg-on` with `fg` text, the others `fg-3`.
public struct FuseSegmented<Value: Hashable>: View {
    @Binding public var selection: Value
    public var options: [(value: Value, title: String)]
    public var fill: Bool
    public var label: String

    public init(_ label: String, selection: Binding<Value>, options: [(Value, String)], fill: Bool = false) {
        self.label = label
        _selection = selection
        self.options = options.map { (value: $0.0, title: $0.1) }
        self.fill = fill
    }

    public var body: some View {
        HStack(spacing: 0) {
            ForEach(options, id: \.value) { o in
                let on = o.value == selection
                Button {
                    withAnimation(.easeOut(duration: 0.12)) { selection = o.value }
                } label: {
                    Text(o.title)
                        .font(Typeface.meta)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                        .foregroundStyle(on ? Palette.fg : Palette.fg3)
                        .padding(.horizontal, 12)
                        .frame(maxWidth: fill ? .infinity : nil, minHeight: 30)
                        .background(on ? Palette.segOn : .clear, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .contentShape(Rectangle().inset(by: -5))
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Palette.lineStrong, lineWidth: 1))
        .fixedSize(horizontal: !fill, vertical: true)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(label)
    }
}
