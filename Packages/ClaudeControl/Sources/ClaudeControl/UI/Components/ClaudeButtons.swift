//
//  ClaudeButtons.swift
//  ClaudeControl
//
//  The panel's one button family, drawn like Codenotch's settings buttons:
//  a white pill with dark text for the one action a group leads with, soft
//  translucent pills for the rest, a tinted pill for answers and warnings,
//  and a bare symbol that gains a soft fill under the pointer. Every one
//  answers the pointer, shrinks a touch while pressed and dims when it can't
//  be used.
//

import SwiftUI

struct ClaudeButtonStyle: ButtonStyle {
    enum Kind: Equatable {
        /// White with dark text: Allow, Approve, Turn on.
        case primary
        /// A soft translucent pill.
        case secondary
        /// Text and a faint fill in a signal colour (answers, fixes).
        case tinted(ClaudeInk.Token)
        /// Red text: Forget, Remove.
        case destructive
        /// Text only until the pointer arrives (section actions, links).
        case quiet
    }

    var kind: Kind = .secondary
    /// Smaller type and padding, for a button beside small text.
    var compact = false

    func makeBody(configuration: Configuration) -> some View {
        ClaudeButtonBody(configuration: configuration, kind: kind, compact: compact)
    }
}

extension ButtonStyle where Self == ClaudeButtonStyle {
    static func claude(_ kind: ClaudeButtonStyle.Kind = .secondary, compact: Bool = false) -> ClaudeButtonStyle {
        ClaudeButtonStyle(kind: kind, compact: compact)
    }
}

private struct ClaudeButtonBody: View {
    let configuration: ButtonStyleConfiguration
    let kind: ClaudeButtonStyle.Kind
    let compact: Bool

    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.claudeControlTheme) private var theme
    @State private var isHovered = false

    var body: some View {
        configuration.label
            .font(compact ? theme.caption.weight(.semibold) : theme.button)
            .lineLimit(1)
            .foregroundStyle(foreground)
            .padding(.horizontal, compact ? 7 : 10)
            .frame(minHeight: compact ? theme.controlHeight - 3 : theme.controlHeight)
            .background(Capsule().fill(fill))
            .contentShape(Capsule())
            .fixedSize()
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .opacity(isEnabled ? 1 : 0.4)
            .onHover { hovering in isHovered = hovering && isEnabled }
            .claudeAnimation(ClaudeMotion.quick, value: isHovered)
            .claudeAnimation(ClaudeMotion.quick, value: configuration.isPressed)
    }

    private var foreground: Color {
        switch kind {
        case .primary: return theme.onPrimary
        case .secondary: return theme.textPrimary
        case .tinted(let token): return theme.color(token)
        case .destructive: return theme.critical
        case .quiet: return isHovered ? theme.textPrimary : theme.textSecondary
        }
    }

    private var fill: Color {
        let pressed = configuration.isPressed
        switch kind {
        case .primary:
            return theme.primaryFill.opacity(pressed ? 0.75 : isHovered ? 1 : 0.92)
        case .secondary:
            return pressed || isHovered ? theme.controlFillHover : theme.controlFill
        case .tinted(let token):
            return theme.color(token).opacity(pressed ? 0.28 : isHovered ? 0.22 : 0.13)
        case .destructive:
            return theme.critical.opacity(pressed ? 0.26 : isHovered ? 0.2 : 0.12)
        case .quiet:
            return pressed || isHovered ? theme.controlFill : .clear
        }
    }
}

// MARK: - Icon button

/// A bare symbol with a soft fill under the pointer, labelled for VoiceOver
/// and with a tooltip, because a symbol alone never says enough.
struct ClaudeIconButton: View {
    let systemName: String
    let label: String
    /// A signal colour for the symbol (e.g. review green for "Mark reviewed").
    var tint: ClaudeInk.Token? = nil
    /// Drawn as switched on (the pin while the panel is kept open).
    var isOn = false
    let action: () -> Void

    @Environment(\.claudeControlTheme) private var theme
    @Environment(\.isEnabled) private var isEnabled
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundStyle(foreground)
                .frame(width: 22, height: 20)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(isHovered || isOn ? theme.controlFill : .clear)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(isEnabled ? 1 : 0.4)
        .help(label)
        .accessibilityLabel(label)
        .onHover { isHovered = $0 && isEnabled }
    }

    private var foreground: Color {
        if let tint { return theme.color(tint).opacity(isHovered ? 1 : 0.85) }
        return isHovered || isOn ? theme.textPrimary : theme.textSecondary
    }
}
