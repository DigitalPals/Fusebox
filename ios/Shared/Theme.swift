import SwiftUI
import UIKit

/// DESIGN.md's palette: a warm near-black operator panel, the same panel in daylight.
public enum Palette {
    private static func dyn(_ dark: UInt32, _ light: UInt32) -> Color {
        Color(uiColor: UIColor { $0.userInterfaceStyle == .light ? UIColor(hex: light) : UIColor(hex: dark) })
    }

    public static let bg = dyn(0x0B0B0A, 0xF6F4EF)
    public static let surface = dyn(0x121210, 0xFCFBF8)
    public static let surfaceInset = dyn(0x0E0E0C, 0xF1EEE7)
    public static let line = dyn(0x24231F, 0xE4E0D6)
    public static let lineStrong = dyn(0x33312C, 0xD3CEC2)
    public static let rivet = dyn(0x2B2A26, 0xDCD7CC)
    public static let fg = dyn(0xF2EFE8, 0x1B1A17)
    public static let fg2 = dyn(0xB4AFA4, 0x4A4741)
    public static let fg3 = dyn(0x857F74, 0x6B665C)
    public static let idle = dyn(0x55524B, 0xA8A397)
    public static let meter = dyn(0xD9D4C8, 0x3A3833)
    public static let segOff = dyn(0x24231F, 0xE4E0D6)
    public static let ok = dyn(0x4ADE80, 0x15803D)
    public static let warn = dyn(0xFBBF24, 0xA16207)
    public static let err = dyn(0xFB7185, 0xBE123C)
    /// The fuse: active tab, live load bar, links, selection. Never a fill.
    public static let brand = Color(uiColor: UIColor(hex: 0xF3B52F))

    public static func level(_ l: QuotaReading.Level) -> Color {
        switch l { case .normal: meter; case .warn: warn; case .err: err }
    }

    /// Usage's chart hues (oklch 0.74/0.09 at 30°, 90°, 150°, 210°, 270°, 330°), approximated in sRGB.
    public static let chart: [Color] = [
        dyn(0xD49A86, 0xA2685A), dyn(0xBCA66E, 0x8A7640), dyn(0x86B88A, 0x55885B),
        dyn(0x6FB3C0, 0x3F8290), dyn(0x9BA3D9, 0x6A72A8), dyn(0xC995C2, 0x986493),
    ]
    public static let chartOther = dyn(0x57534B, 0xA8A397)
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }
}

/// The dashboard's type roles with the system faces: condensed capitals for labels,
/// a plain sans for reading, a monospace for data.
public enum Typeface {
    public static func label(_ size: CGFloat = 11) -> Font { .system(size: size, weight: .semibold).width(.condensed) }
    public static func data(_ size: CGFloat = 12, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }
    public static let figure = Font.system(size: 20, weight: .medium)
}

extension Text {
    /// Uppercase, tracked condensed label ("SUBSCRIPTIONS", "5H").
    public func capsLabel(_ size: CGFloat = 11, tracking: CGFloat = 1.2) -> some View {
        self.textCase(.uppercase).font(Typeface.label(size)).tracking(tracking).foregroundStyle(Palette.fg3)
    }
}
