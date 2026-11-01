import SwiftUI

/// The segmented quota meter: 20 segments of 5%, lit for the displayed share and coloured
/// by how much is used, with a thin even-pace mark (mirrored when showing what's left).
public struct SegmentedMeter: View {
    public var reading: QuotaReading?
    public var pace: Double?
    public var mode: QuotaDisplay
    public var height: CGFloat
    public var label: String

    public init(_ reading: QuotaReading?, pace: Double? = nil, mode: QuotaDisplay = .used, height: CGFloat = 9, label: String = "Quota") {
        self.reading = reading; self.pace = pace; self.mode = mode; self.height = height; self.label = label
    }

    public var body: some View {
        let lit = reading.map { Int(($0.value(mode) / 5).rounded()) } ?? 0
        let color = reading.map { Palette.level($0.level) } ?? Palette.segOff
        GeometryReader { geo in
            let gap: CGFloat = 2
            let w = max(1, (geo.size.width - gap * 19) / 20)
            ZStack(alignment: .leading) {
                HStack(spacing: gap) {
                    ForEach(0..<20, id: \.self) { i in
                        RoundedRectangle(cornerRadius: 1)
                            .fill(i < lit ? color : Palette.segOff)
                            .frame(width: w)
                    }
                }
                if let pace, reading != nil {
                    let t = mode == .used ? pace : 1 - pace
                    Rectangle()
                        .fill(Palette.fg3)
                        .frame(width: 2, height: height + 4)
                        .offset(x: max(0, min(geo.size.width - 2, geo.size.width * t - 1)))
                }
            }
            .frame(height: geo.size.height)
        }
        .frame(height: height)
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

/// An 8px dot in a state colour.
public struct StatusDot: View {
    public var color: Color
    public var size: CGFloat
    public init(_ color: Color, size: CGFloat = 8) { self.color = color; self.size = size }
    public var body: some View { Circle().fill(color).frame(width: size, height: size) }
}

extension AccountState {
    public var color: Color {
        switch kind { case .ready: Palette.ok; case .cooling: Palette.warn; case .error: Palette.err; case .disabled: Palette.idle }
    }
}

extension Fault {
    public var color: Color { level == "err" ? Palette.err : level == "ok" ? Palette.ok : Palette.warn }
}
