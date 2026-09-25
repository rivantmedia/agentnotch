//
//  ContextMeter.swift
//  ClaudeControl
//
//  How full a session's context window is: a short bar and "42% context",
//  turning watch-yellow from 80% and critical from 90%, where
//  auto-compaction looms.
//

import SwiftUI

struct ContextMeter: View {
    /// Context used, 0...100.
    let percent: Double
    /// "42%" alone, for compact rows.
    var isCompact = false

    nonisolated enum Level: Equatable, Sendable {
        case normal
        case high
        case critical
    }

    nonisolated static func level(for percent: Double) -> Level {
        if percent >= 90 { return .critical }
        if percent >= 80 { return .high }
        return .normal
    }

    @Environment(\.claudeControlTheme) private var theme

    private let barWidth: CGFloat = 20

    var body: some View {
        let level = Self.level(for: percent)
        HStack(spacing: 4) {
            ZStack(alignment: .leading) {
                Capsule(style: .continuous)
                    .fill(theme.barTrack)
                Capsule(style: .continuous)
                    .fill(color(level, forText: false))
                    .frame(width: max(barWidth * Self.fraction(percent), percent > 0 ? theme.barHeight : 0))
            }
            .frame(width: barWidth, height: theme.barHeight)

            Text(isCompact ? UsageFormatter.percent(percent) : Self.label(percent))
                .claudeFont(.caption, monospacedDigits: true)
                .foregroundStyle(color(level, forText: true))
                .fixedSize()
        }
        .help("Context window \(UsageFormatter.percent(percent)) full")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Context \(UsageFormatter.percent(percent)) full")
    }

    /// "42% context".
    nonisolated static func label(_ percent: Double) -> String {
        "\(UsageFormatter.percent(percent)) context"
    }

    nonisolated static func fraction(_ percent: Double) -> CGFloat {
        guard percent.isFinite else { return 0 }
        return CGFloat(min(max(percent / 100, 0), 1))
    }

    private func color(_ level: Level, forText: Bool) -> Color {
        switch level {
        case .normal: return forText ? theme.textSecondary : theme.textPrimary
        case .high: return theme.needsYou
        case .critical: return theme.critical
        }
    }
}
