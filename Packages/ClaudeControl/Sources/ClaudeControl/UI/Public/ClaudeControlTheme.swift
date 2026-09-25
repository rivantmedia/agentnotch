//
//  ClaudeControlTheme.swift
//  ClaudeControl
//
//  Colours, type sizes and metrics for the panel and settings views, so they
//  wear Codenotch's design language without the package importing it. The
//  app builds its own from Codenotch's Palette/Typography/NotchLayout
//  (ClaudeTheme+Codenotch); `codenotchDark` is a hard-coded copy of
//  Codenotch's dark values for snapshots and tests.
//
//  Codenotch draws with two inks (white and #808080) on black, three signal
//  colours (ample green, watch yellow, critical red-orange) and translucent
//  white tracks. Every view in this package reads its colours from here and
//  nowhere else, so a surface the app lightens (Liquid Glass in a light
//  appearance) only has to hand over different tokens.
//

import AppKit
import SwiftUI

public nonisolated struct ClaudeControlTheme: Sendable {
    // MARK: Colours

    public var textPrimary: Color
    public var textSecondary: Color
    /// Needs you (Codenotch's "watch" yellow).
    public var needsYou: Color
    /// Ready for review (Codenotch's "ample" green).
    public var review: Color
    /// Working (primary text colour, like Codenotch's white spinner).
    public var working: Color
    /// Failed turns, limits, destructive actions (Codenotch's "critical").
    public var critical: Color
    /// Ring track.
    public var track: Color
    /// Bar track (task and context bars).
    public var barTrack: Color
    public var accent: Color
    /// Account colours, indexed by `ClaudeAccountSummary.colorIndex % 8`.
    public var accountHues: [Color]
    /// Card background behind the panel content.
    public var card: Color

    // Derived in `init` from `textPrimary` and `card` unless set afterwards,
    // so a host that only knows Codenotch's named colours gets them right in
    // both appearances.

    /// Separators and disabled glyphs. Never body text: it is too faint to read.
    public var textTertiary: Color
    /// Resting fill of secondary buttons, chips and fields.
    public var controlFill: Color
    /// The same fill under the pointer.
    public var controlFillHover: Color
    /// A row under the pointer.
    public var rowHover: Color
    /// The keyboard-selected row.
    public var rowSelection: Color
    /// Outline of the keyboard-selected row.
    public var rowSelectionStroke: Color
    /// Hairline rules between sections.
    public var separator: Color
    /// The one primary button of a group (Allow, Approve).
    public var primaryFill: Color
    /// Text on `primaryFill`.
    public var onPrimary: Color

    // MARK: Type

    public var title: Font
    public var rowTitle: Font
    public var body: Font
    public var caption: Font
    public var chat: Font
    public var mono: Font
    /// Section titles and emphasised captions.
    public var sectionTitle: Font
    /// Button labels.
    public var button: Font

    // MARK: Metrics

    public var corner: CGFloat
    public var padding: CGFloat
    /// Space between blocks (NotchLayout.blockSpacing).
    public var blockSpacing: CGFloat = 7.5
    /// Space between the lines of one row (NotchLayout.sessionRowGap).
    public var lineGap: CGFloat = 3.8
    /// Height of task, context and limit bars (NotchLayout.barHeight).
    public var barHeight: CGFloat = 3.9
    /// Rule thickness (NotchLayout.hairline).
    public var hairline: CGFloat = 0.94
    /// The status ring beside a session (NotchLayout.statusDot, drawn a
    /// little larger in the panel, whose type is larger than the hover card's).
    public var statusRing: CGFloat = 8.5
    /// Its stroke (NotchLayout.statusDotStroke, scaled with it).
    public var statusRingStroke: CGFloat = 1.6
    /// Corner of rows, chips and fields.
    public var rowCorner: CGFloat = 10
    /// Height of buttons and chips.
    public var controlHeight: CGFloat = 20

    public init(
        textPrimary: Color, textSecondary: Color, needsYou: Color, review: Color, working: Color,
        critical: Color, track: Color, barTrack: Color, accent: Color, accountHues: [Color], card: Color,
        title: Font, rowTitle: Font, body: Font, caption: Font, chat: Font, mono: Font,
        corner: CGFloat, padding: CGFloat
    ) {
        self.textPrimary = textPrimary
        self.textSecondary = textSecondary
        self.needsYou = needsYou
        self.review = review
        self.working = working
        self.critical = critical
        self.track = track
        self.barTrack = barTrack
        self.accent = accent
        self.accountHues = accountHues
        self.card = card
        self.title = title
        self.rowTitle = rowTitle
        self.body = body
        self.caption = caption
        self.chat = chat
        self.mono = mono
        self.corner = corner
        self.padding = padding

        self.textTertiary = textPrimary.opacity(0.3)
        self.controlFill = textPrimary.opacity(0.09)
        self.controlFillHover = textPrimary.opacity(0.16)
        self.rowHover = textPrimary.opacity(0.05)
        self.rowSelection = textPrimary.opacity(0.08)
        self.rowSelectionStroke = textPrimary.opacity(0.24)
        self.separator = track
        self.primaryFill = textPrimary.opacity(0.95)
        self.onPrimary = Self.inverse
        self.sectionTitle = caption.weight(.semibold)
        self.button = body.weight(.semibold)
    }

    /// The colour for an account.
    public func hue(forAccount colorIndex: Int) -> Color {
        guard !accountHues.isEmpty else { return accent }
        return accountHues[((colorIndex % accountHues.count) + accountHues.count) % accountHues.count]
    }

    /// Black in a dark appearance, white in a light one: text on a
    /// `textPrimary`-coloured fill.
    private static let inverse = Color(nsColor: NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? .black : .white
    })

    /// Codenotch's dark values (Palette.swift, Typography.swift, NotchLayout).
    public static let codenotchDark: ClaudeControlTheme = {
        var theme = ClaudeControlTheme(
            textPrimary: .white,
            textSecondary: Color(red: 0x80 / 255, green: 0x80 / 255, blue: 0x80 / 255),
            needsYou: Color(red: 0xF2 / 255, green: 0xFF / 255, blue: 0x00 / 255),
            review: Color(red: 0x00 / 255, green: 0xFF / 255, blue: 0x88 / 255),
            working: .white,
            critical: Color(red: 0xFF / 255, green: 0x3F / 255, blue: 0x00 / 255),
            track: Color.white.opacity(0.188),
            barTrack: Color.white.opacity(0.176),
            accent: Color(red: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255),
            accountHues: [
                // Clear of the signal colours (yellow, green, red-orange) and
                // of each other; the first few are the most distinct, since
                // most people have one to three accounts.
                Color(red: 0.36, green: 0.62, blue: 0.98),   // blue
                Color(red: 0.72, green: 0.52, blue: 0.96),   // violet
                Color(red: 0.96, green: 0.45, blue: 0.70),   // pink
                Color(red: 0.25, green: 0.80, blue: 0.80),   // teal
                Color(red: 0.52, green: 0.56, blue: 1.00),   // indigo
                Color(red: 0.40, green: 0.82, blue: 0.96),   // cyan
                Color(red: 1.00, green: 0.58, blue: 0.62),   // rose
                Color(red: 0.84, green: 0.76, blue: 0.62),   // sand
            ],
            card: .black,
            title: .system(size: 13.7, weight: .semibold),
            rowTitle: .system(size: 11.6, weight: .medium),
            body: .system(size: 10.5),
            caption: .system(size: 9.5),
            chat: .system(size: 12.6),
            mono: .system(size: 10.5, design: .monospaced),
            corner: 18.6,
            padding: 12
        )
        theme.onPrimary = .black
        return theme
    }()
}

nonisolated private struct ClaudeControlThemeKey: EnvironmentKey {
    static let defaultValue = ClaudeControlTheme.codenotchDark
}

extension EnvironmentValues {
    /// The theme the panel and settings views draw with.
    nonisolated public var claudeControlTheme: ClaudeControlTheme {
        get { self[ClaudeControlThemeKey.self] }
        set { self[ClaudeControlThemeKey.self] = newValue }
    }
}

extension View {
    /// Draw the panel and settings views below with `theme`.
    public func claudeControlTheme(_ theme: ClaudeControlTheme) -> some View {
        environment(\.claudeControlTheme, theme)
    }
}
