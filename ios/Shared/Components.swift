import SwiftUI

/// The segmented quota meter (DESIGN.md "Segmented quota meter"): 20 square segments of 5%
/// with 2pt gaps and 1pt corners, lit for the shown share and coloured by how much is used;
/// unlit segments are `line`. The even-pace mark is a 2pt `fg-3` tick drawn over the segments
/// and running past the bar above and below, like the dashboard's; a 1pt gap in the panel's
/// colour on each side keeps it readable over lit segments in light and dark.
public struct SegmentedMeter: View {
    public var reading: QuotaReading?
    public var pace: Double?
    public var mode: QuotaDisplay
    public var height: CGFloat
    public var label: String
    /// The colour behind the meter (the panel), for the gap around the pace mark.
    public var knockout: Color

    @Environment(\.displayScale) private var scale

    public init(_ reading: QuotaReading?, pace: Double? = nil, mode: QuotaDisplay = .used, height: CGFloat = 9,
                label: String = "Quota", knockout: Color = Palette.surface) {
        self.reading = reading; self.pace = pace; self.mode = mode; self.height = height; self.label = label
        self.knockout = knockout
    }

    /// How far the pace mark runs past the bar, above and below.
    public static func overhang(_ height: CGFloat) -> CGFloat { height >= 14 ? 4 : 3 }

    public var body: some View {
        let model = MeterModel(reading, pace: pace, mode: mode)
        let over = Self.overhang(height)
        let lit = model.level.map(Palette.level) ?? Palette.segOff
        Canvas { ctx, size in
            let gap: CGFloat = 2
            let n = MeterModel.segments
            let snap = { (v: CGFloat) in (v * scale).rounded() / scale }
            let segment = (size.width - gap * CGFloat(n - 1)) / CGFloat(n)
            guard segment > 0.5 else { return }
            let bar = CGRect(x: 0, y: over, width: size.width, height: height)
            for i in 0..<n {
                let x0 = snap(CGFloat(i) * (segment + gap)), x1 = snap(CGFloat(i) * (segment + gap) + segment)
                let r = CGRect(x: x0, y: bar.minY, width: max(1 / scale, x1 - x0), height: bar.height)
                ctx.fill(Path(roundedRect: r, cornerRadius: 1), with: .color(model.isLit(i) ? lit : Palette.segOff))
            }
            if let p = model.pace {
                let x = snap(min(size.width - 1, max(1, size.width * p)) - 1)
                ctx.fill(Path(CGRect(x: x - 1, y: bar.minY, width: 4, height: bar.height)), with: .color(knockout))
                ctx.fill(Path(roundedRect: CGRect(x: x, y: 0, width: 2, height: size.height), cornerRadius: 1), with: .color(Palette.fg3))
            }
        }
        .frame(height: height + over * 2)
        .accessibilityElement()
        .accessibilityLabel(label)
        .accessibilityValue(accessibilityText)
    }

    private var accessibilityText: String {
        guard let reading else { return "not reported" }
        var s = "\(Fmt.percent(reading.value(mode))) \(mode.word), \(reading.status)"
        if let pace { s += ", even pace \(Int((pace * 100).rounded()))% used by now" }
        return s
    }
}

/// A provider's real logo from the asset catalogue (ui/logos.svg); one-colour marks take `fg`.
public struct ProviderLogo: View {
    public var name: String
    public var size: CGFloat

    public init(_ name: String, size: CGFloat = 16) { self.name = name; self.size = size }

    public var body: some View {
        Image(name)
            .resizable()
            .scaledToFit()
            .foregroundStyle(Palette.fg)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

/// An 8pt dot in a state colour.
public struct StatusDot: View {
    public var color: Color
    public var size: CGFloat
    public init(_ color: Color, size: CGFloat = 8) { self.color = color; self.size = size }
    public var body: some View { Circle().fill(color).frame(width: size, height: size).accessibilityHidden(true) }
}

extension AccountState {
    public var color: Color {
        switch kind { case .ready: Palette.ok; case .cooling: Palette.warn; case .error: Palette.err; case .disabled: Palette.idle }
    }
}

extension Fault {
    public var color: Color { level == "err" ? Palette.err : level == "ok" ? Palette.ok : Palette.warn }
}
