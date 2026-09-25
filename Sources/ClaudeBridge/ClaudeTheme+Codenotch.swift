import ClaudeControl
import SwiftUI

/// ClaudeControl's panel and settings views in Codenotch's design language:
/// its palette (the hover card's inks, the ring's amber, green and red), its
/// type (sized from the design frame's cap heights, like `Typography`) and
/// the hover card's corner and padding.
///
/// The colours are Codenotch's dynamic ones, so on the glass surface they
/// follow the Mac's appearance exactly as the hover card does, and on the
/// dark surfaces they resolve to the frame's hexes.
///
/// Fork-only file (design §7, §8). Owned by WP-D.
extension ClaudeControlTheme {
    /// Cap heights, in design-frame pixels, of the panel's type sizes. Title
    /// and caption are Codenotch's own card title and card body; the others
    /// sit between them on the same scale.
    enum CapHeight {
        static let title: CGFloat = 26      // Typography.cardTitle, 13.7 pt
        static let chat: CGFloat = 24       // 12.6 pt
        static let rowTitle: CGFloat = 22   // 11.6 pt
        static let body: CGFloat = 20       // 10.5 pt
        static let caption: CGFloat = 18    // Typography.cardBody, 9.5 pt
    }

    /// Account colours: away from the attention colours (amber, green, red)
    /// so an account dot is never read as a state.
    static let codenotchAccountHues: [Color] = ClaudeControlTheme.codenotchDark.accountHues

    /// The theme for a panel painted with `surfaceStyle`.
    ///
    /// - `colorScheme` and `reduceTransparency` pick the secondary ink the
    ///   hover card would use on the same surface (`TooltipGlassContrast`).
    /// - `accent` is the notch's accent colour (Appearance), for controls.
    static func codenotch(surfaceStyle: NotchSurfaceStyle, colorScheme: ColorScheme,
                          reduceTransparency: Bool, accent: Color) -> ClaudeControlTheme {
        let glassy = surfaceStyle.isGlass && !reduceTransparency
        return ClaudeControlTheme(
            textPrimary: Palette.textPrimary,
            textSecondary: TooltipGlassContrast.secondaryInk(surfaceStyle: surfaceStyle, colorScheme: colorScheme,
                                                             reduceTransparency: reduceTransparency),
            needsYou: Palette.watch,
            review: Palette.ample,
            working: Palette.textPrimary,
            critical: Palette.critical,
            track: Palette.ringTrack,
            barTrack: Palette.barTrack,
            accent: accent,
            accountHues: codenotchAccountHues,
            // On glass the chrome paints the material; content stays clear so
            // the user's Clear or Tinted choice shows through, as on the card.
            card: glassy ? .clear : Palette.card,
            title: .system(size: Design.fontSize(capPixels: CapHeight.title), weight: .semibold),
            rowTitle: .system(size: Design.fontSize(capPixels: CapHeight.rowTitle), weight: .medium),
            body: .system(size: Design.fontSize(capPixels: CapHeight.body)),
            caption: .system(size: Design.fontSize(capPixels: CapHeight.caption)),
            chat: .system(size: Design.fontSize(capPixels: CapHeight.chat)),
            mono: .system(size: Design.fontSize(capPixels: CapHeight.body), design: .monospaced),
            corner: NotchLayout.cardCorner,
            padding: NotchLayout.cardPadding
        )
    }
}
