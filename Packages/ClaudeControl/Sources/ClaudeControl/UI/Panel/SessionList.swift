//
//  SessionList.swift
//  ClaudeControl
//
//  The sessions grouped by what they need from you: Needs you (oldest wait
//  first), Ready for review (newest first), Working (longest running first)
//  and Idle. Any section but Needs you folds to a one-line summary from its
//  header; a long idle list starts folded. Rows keep their order while the
//  pointer is over the list, so nothing moves under a click.
//

import SwiftUI

/// The list as values: sections of row models, in display order.
nonisolated struct SessionListLayout: Equatable, Sendable {
    struct Section: Identifiable, Equatable, Sendable {
        let bucket: AttentionBucket
        let rows: [SessionRowModel]
        let isCollapsed: Bool
        /// "Write migration tests, Investigate the flaky CI job and 1 more".
        let summary: String

        var id: Int { bucket.rawValue }
    }

    let sections: [Section]
    /// Rows other than answers go to one line (more than
    /// `SessionSections.compactThreshold` rows drawn).
    let isCompact: Bool

    /// Row ids as displayed (folded sections contribute none): the order the
    /// keyboard moves through.
    var visibleOrder: [String] {
        sections.filter { !$0.isCollapsed }.flatMap { $0.rows.map(\.id) }
    }

    var isEmpty: Bool { sections.isEmpty }

    /// The folded section `id`'s row is in, if it is folded away.
    func foldedBucket(containing id: String?) -> AttentionBucket? {
        guard let id else { return nil }
        return sections.first { $0.isCollapsed && $0.rows.contains { $0.id == id } }?.bucket
    }

    /// The list as one flat run of items, in display order: each section's
    /// header, then its rows (or its folded summary). A row's identity is its
    /// session alone, so a session that moves to another section keeps its
    /// view rather than being torn down and rebuilt (BHV-5: 30 sessions
    /// changing section together stalled the main thread).
    enum Item: Identifiable, Equatable, Sendable {
        case header(Section, isFirst: Bool)
        case folded(Section)
        case row(SessionRowModel, isFirstInSection: Bool)

        var id: String {
            switch self {
            case .header(let section, _): return "header-\(section.bucket.rawValue)"
            case .folded(let section): return "folded-\(section.bucket.rawValue)"
            case .row(let row, _): return "row-\(row.id)"
            }
        }
    }

    var items: [Item] {
        var items: [Item] = []
        for (index, section) in sections.enumerated() {
            items.append(.header(section, isFirst: index == 0))
            if section.isCollapsed {
                items.append(.folded(section))
            } else {
                for (position, row) in section.rows.enumerated() {
                    items.append(.row(row, isFirstInSection: position == 0))
                }
            }
        }
        return items
    }

    func row(id: String) -> SessionRowModel? {
        for section in sections {
            if let row = section.rows.first(where: { $0.id == id }) { return row }
        }
        return nil
    }

    static func make(
        sections: [SessionSection],
        rows: (SessionState) -> SessionRowModel,
        folds: [AttentionBucket: Bool]
    ) -> SessionListLayout {
        let laidOut = sections.map { section in
            Section(
                bucket: section.bucket,
                rows: section.sessions.map(rows),
                isCollapsed: SessionSections.isCollapsed(section.bucket, count: section.sessions.count, overrides: folds),
                summary: SessionSections.collapsedSummary(section)
            )
        }
        // Rows drawn, not sessions: a long idle list folded to one line
        // doesn't squeeze the few rows above it.
        let drawn = laidOut.filter { !$0.isCollapsed }.reduce(0) { $0 + $1.rows.count }
        return SessionListLayout(sections: laidOut, isCompact: SessionSections.isCompact(sessionCount: drawn))
    }
}

struct SessionListView: View {
    let layout: SessionListLayout
    let selectedId: String?
    /// Whether a row's answer can be taken yet, by request id.
    let isArmed: (String) -> Bool
    /// Snapshots: draw this row with its pointer actions showing.
    var hoveredId: String? = nil
    let onToggleSection: (AttentionBucket) -> Void
    let onMarkAllReviewed: () -> Void
    let perform: (ClaudeKeyRouter.Command) -> Void

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        // One ForEach over headers and rows alike, keyed by session for rows
        // (see `SessionListLayout.items`).
        VStack(alignment: .leading, spacing: 0) {
            ForEach(layout.items) { item in
                switch item {
                case .header(let section, let isFirst):
                    SectionHeader(
                        section: section,
                        isFirst: isFirst,
                        onToggle: { onToggleSection(section.bucket) },
                        onMarkAllReviewed: onMarkAllReviewed
                    )
                case .folded(let section):
                    FoldedSummary(section: section) { onToggleSection(section.bucket) }
                case .row(let row, let isFirstInSection):
                    SessionRow(
                        row: row,
                        isCompact: layout.isCompact,
                        isSelected: row.id == selectedId,
                        isAnswerArmed: row.actions.toolUseId.map(isArmed) ?? true,
                        forcesHover: row.id == hoveredId,
                        perform: perform
                    )
                    .equatable()
                    .padding(.top, isFirstInSection || layout.isCompact ? 0 : 2)
                    // The panel scrolls to a row by its session id.
                    .id(row.id)
                    .transition(.opacity)
                }
            }
        }
    }
}

// MARK: - Section header

private struct SectionHeader: View {
    let section: SessionListLayout.Section
    let isFirst: Bool
    let onToggle: () -> Void
    let onMarkAllReviewed: () -> Void

    @Environment(\.claudeControlTheme) private var theme
    @State private var isHovered = false

    private var canFold: Bool { section.bucket != .needsInput }

    var body: some View {
        HStack(alignment: .center, spacing: 6) {
            Button(action: onToggle) {
                HStack(spacing: 5) {
                    Text(section.bucket.sectionTitle)
                        .claudeFont(.sectionTitle)
                        .foregroundStyle(.ink(section.bucket.ink))
                    Text("\(section.rows.count)")
                        .claudeFont(.caption, weight: .medium, monospacedDigits: true)
                        .foregroundStyle(.ink(.secondary))
                    if canFold {
                        Image(systemName: "chevron.down")
                            .font(.system(size: 7.5, weight: .bold))
                            .foregroundStyle(.ink(isHovered ? .primary : .tertiary))
                            .rotationEffect(.degrees(section.isCollapsed ? -90 : 0))
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .allowsHitTesting(canFold)
            .onHover { isHovered = $0 && canFold }
            .accessibilityLabel("\(section.bucket.sectionTitle), \(section.rows.count)")
            .accessibilityValue(canFold ? (section.isCollapsed ? "folded" : "unfolded") : "")
            .accessibilityHint(canFold ? (section.isCollapsed ? "Shows the sessions" : "Folds the section") : "")
            .accessibilityAddTraits(.isHeader)

            Spacer(minLength: 8)

            if section.bucket == .readyForReview && !section.isCollapsed {
                Button("Mark all reviewed", action: onMarkAllReviewed)
                    .buttonStyle(.claude(.quiet, compact: true))
                    .help("Mark every session here reviewed (⌘⇧R). Undo for a few seconds after.")
            }
        }
        .frame(minHeight: 18)
        .padding(.horizontal, 8)
        .padding(.top, isFirst ? 2 : theme.blockSpacing + 4)
        .padding(.bottom, 2)
    }
}

extension AttentionBucket {
    /// The section title's colour.
    var ink: ClaudeInk.Token {
        switch self {
        case .needsInput: return .needsYou
        case .readyForReview: return .review
        case .working: return .working
        case .idle: return .secondary
        }
    }
}

/// A folded section: its first titles and how many more, which unfolds it.
private struct FoldedSummary: View {
    let section: SessionListLayout.Section
    let expand: () -> Void

    @Environment(\.claudeControlTheme) private var theme
    @State private var isHovered = false

    var body: some View {
        Button(action: expand) {
            HStack(spacing: 6) {
                Text(section.summary)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(isHovered ? .primary : .secondary))
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(
                RoundedRectangle(cornerRadius: theme.rowCorner, style: .continuous)
                    .fill(isHovered ? theme.rowHover : .clear)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .accessibilityLabel("\(section.rows.count) \(section.bucket.sectionTitle.lowercased()) sessions: \(section.summary)")
        .accessibilityHint("Shows them")
    }
}

// MARK: - Empty state

struct SessionsEmptyState: View {
    /// Filtered to one account that has no sessions.
    var isFiltered = false

    var body: some View {
        VStack(spacing: 6) {
            StatusRing(kind: .idle)
                .scaleEffect(1.6)
                .padding(.bottom, 6)
            Text(isFiltered ? "No sessions in this account" : "No Claude sessions yet")
                .claudeFont(.body, weight: .medium)
                .foregroundStyle(.ink(.primary))
            Text(isFiltered
                 ? "Choose All to see every account's sessions."
                 : "Start Claude Code in VS Code or a terminal. Sessions from every account show up here.")
                .claudeFont(.caption)
                .foregroundStyle(.ink(.secondary))
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 28)
        .accessibilityElement(children: .combine)
    }
}
