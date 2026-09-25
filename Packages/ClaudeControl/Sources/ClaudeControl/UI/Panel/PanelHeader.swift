//
//  PanelHeader.swift
//  ClaudeControl
//
//  The top of the sessions panel: the title, the attention strip (what needs
//  you, what failed, what's done, what's running), one chip per account when
//  there is more than one, and the pin, gear and close buttons.
//

import SwiftUI

struct PanelHeader: View {
    let counts: AttentionCounts
    let showsSealedBadge: Bool
    /// One chip per tracked account, only when there is more than one.
    let chips: [RingChip]
    @Binding var ringFilter: String?
    @Binding var isPinned: Bool
    let quickSettings: PanelQuickSettings
    let canMarkAllReviewed: Bool
    let onQuickSettings: (PanelQuickSettings) -> Void
    let onMarkAllReviewed: () -> Void
    let onOpenSettings: () -> Void
    let onClose: () -> Void

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: theme.blockSpacing) {
            HStack(alignment: .center, spacing: 6) {
                Text("Claude sessions")
                    .claudeFont(.title)
                    .foregroundStyle(.ink(.primary))
                    .lineLimit(1)
                    .accessibilityAddTraits(.isHeader)
                SealedModeBadge(isShown: showsSealedBadge)
                Spacer(minLength: 8)
                HStack(spacing: 2) {
                    ClaudeIconButton(
                        systemName: isPinned ? "pin.fill" : "pin",
                        label: isPinned ? "Keep open: on" : "Keep open",
                        isOn: isPinned
                    ) {
                        isPinned.toggle()
                    }
                    GearMenu(
                        quickSettings: quickSettings,
                        canMarkAllReviewed: canMarkAllReviewed,
                        onChange: onQuickSettings,
                        onMarkAllReviewed: onMarkAllReviewed,
                        onOpenSettings: onOpenSettings
                    )
                    ClaudeIconButton(systemName: "xmark", label: "Close", action: onClose)
                }
            }

            if counts.total > 0 {
                AttentionStrip(counts: counts)
            }

            if chips.count > 1 && chips.contains(where: { $0.count > 0 }) {
                RingFilterChips(chips: chips, selection: $ringFilter)
            }
        }
    }
}

// MARK: - Attention strip

/// "◐ 2 need you · ● 1 failed · ○ 2 to review · ◠ 3 working · 4 idle",
/// only the groups that have sessions, each in its signal colour and with
/// the mark its rows carry.
struct AttentionStrip: View {
    let counts: AttentionCounts

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        if counts.total > 0 {
            HStack(spacing: 10) {
                if counts.answerable > 0 {
                    item(.needsInput, "\(counts.answerable) \(counts.answerable == 1 ? "needs" : "need") you", .needsYou)
                }
                if counts.failed > 0 {
                    item(.error, "\(counts.failed) failed", .critical)
                }
                if counts.readyForReview > 0 {
                    item(.review, "\(counts.readyForReview) to review", .review)
                }
                if counts.working > 0 {
                    item(.working, "\(counts.working) working", .working)
                }
                if counts.idle > 0 {
                    item(.idle, "\(counts.idle) idle", .secondary)
                }
                Spacer(minLength: 0)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Self.accessibilityText(counts))
        } else {
            Text("No sessions")
                .claudeFont(.caption)
                .foregroundStyle(.ink(.secondary))
        }
    }

    private func item(_ kind: SessionGlyphKind, _ text: String, _ ink: ClaudeInk.Token) -> some View {
        HStack(spacing: 4) {
            StatusRing(kind: kind, breathKey: counts.answerable)
                .scaleEffect(0.85)
            Text(text)
                .claudeFont(.caption, weight: .medium, monospacedDigits: true)
                .foregroundStyle(.ink(ink))
                .lineLimit(1)
                .fixedSize()
        }
    }

    /// "2 need you, 1 failed, 2 ready for review, 3 working, 4 idle".
    nonisolated static func accessibilityText(_ counts: AttentionCounts) -> String {
        var parts: [String] = []
        if counts.answerable > 0 { parts.append("\(counts.answerable) need\(counts.answerable == 1 ? "s" : "") you") }
        if counts.failed > 0 { parts.append("\(counts.failed) failed") }
        if counts.readyForReview > 0 { parts.append("\(counts.readyForReview) ready for review") }
        if counts.working > 0 { parts.append("\(counts.working) working") }
        if counts.idle > 0 { parts.append("\(counts.idle) idle") }
        return parts.isEmpty ? "No sessions" : parts.joined(separator: ", ")
    }
}

// MARK: - Ring filter

/// One account's chip.
struct RingChip: Identifiable, Equatable {
    let ringID: String
    let label: String
    let colorIndex: Int
    let count: Int
    /// How many of its sessions need you (something to answer; a failed
    /// turn doesn't count). Shown beside the total in the needs-you colour,
    /// never by tinting the total (GUX-13).
    let needsYouCount: Int

    var needsYou: Bool { needsYouCount > 0 }
    var id: String { ringID }
}

/// "All 13 · ● Personal 6 · ● Work 7": narrows the list to one account.
struct RingFilterChips: View {
    let chips: [RingChip]
    @Binding var selection: String?

    var body: some View {
        FlowLayout(spacing: 5, lineSpacing: 5) {
            chip(label: "All", colorIndex: nil, count: chips.reduce(0) { $0 + $1.count },
                 needsYouCount: chips.reduce(0) { $0 + $1.needsYouCount }, isSelected: selection == nil) {
                selection = nil
            }
            ForEach(chips) { ring in
                chip(label: ring.label, colorIndex: ring.colorIndex, count: ring.count,
                     needsYouCount: ring.needsYouCount, isSelected: selection == ring.ringID) {
                    selection = selection == ring.ringID ? nil : ring.ringID
                }
            }
        }
    }

    private func chip(label: String, colorIndex: Int?, count: Int, needsYouCount: Int,
                      isSelected: Bool, action: @escaping () -> Void) -> some View {
        FilterChip(label: label, colorIndex: colorIndex, count: count, needsYouCount: needsYouCount,
                   isSelected: isSelected, action: action)
    }
}

struct FilterChip: View {
    let label: String
    let colorIndex: Int?
    let count: Int
    let needsYouCount: Int
    let isSelected: Bool
    let action: () -> Void

    @Environment(\.claudeControlTheme) private var theme
    @State private var isHovered = false

    /// "Work, 9 sessions, 1 needs you".
    static func accessibilityLabel(label: String, count: Int, needsYouCount: Int) -> String {
        var text = "\(label), \(count) session\(count == 1 ? "" : "s")"
        if needsYouCount > 0 { text += ", \(needsYouCount) need\(needsYouCount == 1 ? "s" : "") you" }
        return text
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                if let colorIndex {
                    AccountDot(colorIndex: colorIndex)
                }
                Text(label)
                    .foregroundStyle(.ink(isSelected ? .primary : .secondary))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: 150)
                    .fixedSize()
                Text("\(count)")
                    .foregroundStyle(.ink(.secondary))
                    .monospacedDigit()
                if needsYouCount > 0 {
                    // Its own amber count, with the needs-you mark: the
                    // total never reads as a needs-you count.
                    HStack(spacing: 2) {
                        Circle().fill(.ink(.needsYou)).frame(width: 5, height: 5)
                        Text("\(needsYouCount)")
                            .foregroundStyle(.ink(.needsYou))
                            .monospacedDigit()
                    }
                    .accessibilityHidden(true)
                }
            }
            .claudeFont(.caption, weight: .medium)
            .padding(.horizontal, 8)
            .frame(height: theme.controlHeight - 2)
            .background(Capsule().fill(isSelected ? theme.controlFillHover : isHovered ? theme.controlFill : .clear))
            .overlay(Capsule().strokeBorder(isSelected ? .clear : theme.separator, lineWidth: theme.hairline))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .accessibilityLabel(Self.accessibilityLabel(label: label, count: count, needsYouCount: needsYouCount))
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }
}

// MARK: - Gear

/// Quick settings and the way to all of them.
private struct GearMenu: View {
    let quickSettings: PanelQuickSettings
    let canMarkAllReviewed: Bool
    let onChange: (PanelQuickSettings) -> Void
    let onMarkAllReviewed: () -> Void
    let onOpenSettings: () -> Void

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        Menu {
            Picker("Open automatically", selection: binding(\.autoOpen)) {
                ForEach(AutoOpenPolicy.allCases, id: \.self) { policy in
                    Text(policy.settingTitle).tag(policy)
                }
            }
            Toggle("Notify when a session needs you", isOn: binding(\.notifyNeedsInput))
            Toggle("Notify when a session is done", isOn: binding(\.notifyReadyForReview))
            Divider()
            Button("Mark all reviewed", action: onMarkAllReviewed)
                .disabled(!canMarkAllReviewed)
                .keyboardShortcut("r", modifiers: [.command, .shift])
            Divider()
            Button("Open Settings…", action: onOpenSettings)
        } label: {
            Image(systemName: "gearshape")
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundStyle(.ink(.secondary))
                .frame(width: 22, height: 20)
                .contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        // A borderless menu draws its label in the environment's tint (the
        // host's accent), not its foreground style: grey like pin and close
        // (GUX-16).
        .tint(theme.color(.secondary))
        .fixedSize()
        .help("Settings")
        .accessibilityLabel("Settings")
    }

    private func binding<V>(_ keyPath: WritableKeyPath<PanelQuickSettings, V>) -> Binding<V> {
        Binding(
            get: { quickSettings[keyPath: keyPath] },
            set: { value in
                var updated = quickSettings
                updated[keyPath: keyPath] = value
                onChange(updated)
            }
        )
    }
}
