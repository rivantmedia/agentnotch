//
//  ClaudeInk.swift
//  ClaudeControl
//
//  How views reach the theme: `.foregroundStyle(.ink(.secondary))`,
//  `.fill(.ink(.controlFill))`, `.claudeFont(.body)`. The style resolves the
//  token from the environment when it is drawn, so no view keeps a colour of
//  its own and a whole subtree follows whatever theme the host hands down.
//

import SwiftUI

/// A colour from the environment's `ClaudeControlTheme`.
nonisolated struct ClaudeInk: ShapeStyle {
    enum Token: Hashable, Sendable {
        case primary, secondary, tertiary
        case needsYou, review, working, critical, accent
        case track, barTrack, separator
        case controlFill, controlFillHover
        case rowHover, rowSelection, rowSelectionStroke
        case primaryFill, onPrimary, card
        /// An account's colour, by `colorIndex`.
        case account(Int)
    }

    let token: Token
    var opacity: Double = 1

    func resolve(in environment: EnvironmentValues) -> Color {
        environment.claudeControlTheme.color(token).opacity(opacity)
    }
}

extension ShapeStyle where Self == ClaudeInk {
    /// The theme colour `token`, optionally faded.
    nonisolated static func ink(_ token: ClaudeInk.Token, opacity: Double = 1) -> ClaudeInk {
        ClaudeInk(token: token, opacity: opacity)
    }
}

extension ClaudeControlTheme {
    /// The colour for a token.
    nonisolated func color(_ token: ClaudeInk.Token) -> Color {
        switch token {
        case .primary: return textPrimary
        case .secondary: return textSecondary
        case .tertiary: return textTertiary
        case .needsYou: return needsYou
        case .review: return review
        case .working: return working
        case .critical: return critical
        case .accent: return accent
        case .track: return track
        case .barTrack: return barTrack
        case .separator: return separator
        case .controlFill: return controlFill
        case .controlFillHover: return controlFillHover
        case .rowHover: return rowHover
        case .rowSelection: return rowSelection
        case .rowSelectionStroke: return rowSelectionStroke
        case .primaryFill: return primaryFill
        case .onPrimary: return onPrimary
        case .card: return card
        case .account(let index): return hue(forAccount: index)
        }
    }

    /// The font for a type token.
    nonisolated func font(_ token: ClaudeFontToken) -> Font {
        switch token {
        case .title: return title
        case .rowTitle: return rowTitle
        case .body: return body
        case .caption: return caption
        case .chat: return chat
        case .mono: return mono
        case .sectionTitle: return sectionTitle
        case .button: return button
        case .monoCaption: return caption.monospaced()
        }
    }
}

/// The theme's type scale.
nonisolated enum ClaudeFontToken: Hashable, Sendable {
    case title, rowTitle, body, caption, chat, mono, sectionTitle, button
    /// The caption size in monospace (paths, line numbers).
    case monoCaption
}

private struct ClaudeFontModifier: ViewModifier {
    let token: ClaudeFontToken
    var weight: Font.Weight?
    var monospacedDigits = false

    @Environment(\.claudeControlTheme) private var theme

    func body(content: Content) -> some View {
        var font = theme.font(token)
        if let weight { font = font.weight(weight) }
        if monospacedDigits { font = font.monospacedDigit() }
        return content.font(font)
    }
}

extension View {
    /// The theme font `token`, optionally re-weighted.
    func claudeFont(_ token: ClaudeFontToken, weight: Font.Weight? = nil, monospacedDigits: Bool = false) -> some View {
        modifier(ClaudeFontModifier(token: token, weight: weight, monospacedDigits: monospacedDigits))
    }
}

// MARK: - Static rendering

nonisolated private struct ClaudeStaticRenderingKey: EnvironmentKey {
    static let defaultValue = false
}

nonisolated private struct ClaudeStaticScrollingKey: EnvironmentKey {
    static let defaultValue = false
}

nonisolated private struct ClaudeClockKey: EnvironmentKey {
    static let defaultValue: Date? = nil
}

extension EnvironmentValues {
    /// True while the snapshot renderer draws a view: animations start
    /// settled, timers don't tick and fields show their text, so every PNG is
    /// the resting state rather than one frame of a transition.
    nonisolated var claudeStaticRendering: Bool {
        get { self[ClaudeStaticRenderingKey.self] }
        set { self[ClaudeStaticRenderingKey.self] = newValue }
    }

    /// With static rendering: keep the list's scroll view, so a snapshot
    /// shows exactly the part the panel window shows.
    nonisolated var claudeStaticKeepsScrolling: Bool {
        get { self[ClaudeStaticScrollingKey.self] }
        set { self[ClaudeStaticScrollingKey.self] = newValue }
    }

    /// The panel's clock: its 30-second timeline, or the fixed time a
    /// snapshot draws at. One `now` for the chat's task times and estimate,
    /// as for the list's; nil outside the panel (read the time then).
    nonisolated var claudeClock: Date? {
        get { self[ClaudeClockKey.self] }
        set { self[ClaudeClockKey.self] = newValue }
    }
}
