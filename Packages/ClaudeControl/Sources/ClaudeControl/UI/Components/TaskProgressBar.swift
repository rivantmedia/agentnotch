//
//  TaskProgressBar.swift
//  ClaudeControl
//
//  A session's task list as a thin bar on Codenotch's bar track: one segment
//  per task while they fit (done in primary ink, the one in progress in
//  secondary ink, the rest bare track), a continuous fill for longer lists,
//  then "3/7".
//

import SwiftUI

struct TaskProgressBar: View {
    let tasks: SessionTaskList
    var width: CGFloat = 64
    var showsCount = true

    /// Lists up to this long draw one segment per task.
    nonisolated static let maxSegments = 12
    /// Narrowest a segment may be; below it the bar is one continuous fill.
    nonisolated static let minSegmentWidth: CGFloat = 5

    /// Whether `count` tasks draw as separate segments in `width`.
    nonisolated static func isSegmented(count: Int, width: CGFloat) -> Bool {
        count > 0 && count <= maxSegments && width / CGFloat(count) >= minSegmentWidth + 1.5
    }

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        HStack(spacing: 5) {
            bar
                .frame(width: width, height: theme.barHeight)
            if showsCount {
                Text(Self.countLabel(tasks))
                    .claudeFont(.caption, monospacedDigits: true)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize()
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(SessionRowContent.taskSummary(tasks))
    }

    @ViewBuilder
    private var bar: some View {
        let items = tasks.items
        if Self.isSegmented(count: items.count, width: width) {
            HStack(spacing: 1.5) {
                ForEach(items) { item in
                    Capsule(style: .continuous)
                        .fill(color(item.status))
                }
            }
        } else {
            GeometryReader { proxy in
                let fraction = CGFloat(tasks.fraction)
                ZStack(alignment: .leading) {
                    Capsule(style: .continuous).fill(theme.barTrack)
                    Capsule(style: .continuous)
                        .fill(color(.completed))
                        .frame(width: max(proxy.size.width * fraction, fraction > 0 ? theme.barHeight : 0))
                    if tasks.activeItem != nil, fraction < 1 {
                        Capsule(style: .continuous)
                            .fill(color(.inProgress))
                            .frame(width: max(proxy.size.width / CGFloat(max(tasks.totalCount, 1)), theme.barHeight))
                            .offset(x: proxy.size.width * fraction)
                    }
                }
            }
        }
    }

    private func color(_ status: SessionTaskItem.Status) -> Color {
        switch status {
        case .completed: return theme.textPrimary
        case .inProgress: return theme.textSecondary
        case .pending: return theme.barTrack
        }
    }

    /// "3/7".
    nonisolated static func countLabel(_ tasks: SessionTaskList) -> String {
        "\(tasks.completedCount)/\(tasks.totalCount)"
    }
}
