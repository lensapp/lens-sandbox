import SwiftUI

enum LNSTheme {
    static let canvas = Color(hex: 0x181a1c)
    static let surface = Color(hex: 0x1f2123)
    static let raised = Color(hex: 0x252729)
    static let border = Color(hex: 0x2d2f31)
    static let text = Color(hex: 0xaaacae)
    static let muted = Color(hex: 0x929699)
    static let heading = Color(hex: 0xf1f3f5)
    static let accent = Color(hex: 0x3d90ce)
    static let success = Color(hex: 0x6cbd70)
    static let warning = Color(hex: 0xffb454)
    static let critical = Color(hex: 0xf07973)
}

private extension Color {
    init(hex: UInt32) {
        self.init(.sRGB, red: Double((hex >> 16) & 255) / 255,
                  green: Double((hex >> 8) & 255) / 255, blue: Double(hex & 255) / 255, opacity: 1)
    }
}

extension View {
    func lnsAppearance() -> some View {
        self.font(.system(size: 13))
            .foregroundStyle(LNSTheme.text)
            .tint(LNSTheme.accent)
            .background(LNSTheme.canvas)
            .preferredColorScheme(.dark)
    }

    func lnsPanel() -> some View {
        self.background(LNSTheme.surface, in: RoundedRectangle(cornerRadius: 4))
            .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(LNSTheme.border, lineWidth: 1).allowsHitTesting(false))
    }
}

struct LNSPageHeading: View {
    let title: String
    let subtitle: String

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.system(size: 22, weight: .semibold)).foregroundStyle(LNSTheme.heading)
            Text(subtitle).font(.callout).foregroundStyle(LNSTheme.muted).fixedSize(horizontal: false, vertical: true)
        }
    }
}

struct LNSStatus: View {
    let title: String
    var color: Color = LNSTheme.muted

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 5, height: 5).accessibilityHidden(true)
            Text(title).font(.system(size: 11, weight: .medium))
        }
        .foregroundStyle(color)
        .padding(.horizontal, 8).padding(.vertical, 4)
        .background(color.opacity(0.12), in: RoundedRectangle(cornerRadius: 4))
        .fixedSize()
    }
}

struct LNSNavigationStyle: ButtonStyle {
    let selected: Bool
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: selected ? .medium : .regular))
            .foregroundStyle(selected ? LNSTheme.heading : LNSTheme.text)
            .padding(.horizontal, 12).padding(.vertical, 10)
            .background(selected ? LNSTheme.accent.opacity(0.16) : hovering || configuration.isPressed ? LNSTheme.raised : .clear,
                        in: RoundedRectangle(cornerRadius: 4))
            .overlay(alignment: .leading) {
                if selected { RoundedRectangle(cornerRadius: 1).fill(LNSTheme.accent).frame(width: 2).padding(.vertical, 8) }
            }
            .contentShape(Rectangle())
            .onHover { hovering = $0 }
    }
}
