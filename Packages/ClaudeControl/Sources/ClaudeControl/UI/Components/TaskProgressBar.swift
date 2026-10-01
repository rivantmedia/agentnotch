//
//  TaskProgressBar.swift
//  ClaudeControl
//
//  A session's task list as a thin bar on Codenotch's bar track: one segment
//  per task while they fit (done in primary ink, the one in progress in
//  secondary ink, the rest bare track), a continuous fill for longer lists,
//  then "3/7". With an estimate (a working session whose tasks have a pace)
//  the segment in progress fills as far as its time credits it, over a
//  fainter wash of the same ink, and the time left follows: "3/7 · ~4m left".
//

import SwiftUI

struct TaskProgressBar: View {
    /// How much of the time left to say.
    nonisolated enum RemainingStyle: Sendable {
        case hidden
        /// "~4m".
        case short
        /// "~4m left".
        case full
    }

    let tasks: SessionTaskList
    /// A working session's progress at the panel's clock; nil draws completed
    /// tasks only, with no time left.
    var estimate: TaskEstimate? = nil
    var width: CGFloat = 64
    var showsCount = true
    var remainingStyle: RemainingStyle = .full

    /// Lists up to this long draw one segment per task.
    nonisolated static let maxSegments = 12
    /// Narrowest a segment may be; below it the bar is one continuous fill.
    nonisolated static let minSegmentWidth: CGFloat = 5
    /// The uncredited rest of the segment in progress, when an estimate
    /// fills part of it.
    nonisolated static let activeWashOpacity = 0.4

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
            if let remaining = Self.remainingLabel(estimate, style: remainingStyle) {
                if remainingStyle == .full {
                    Text("·")
                        .claudeFont(.caption, weight: .bold)
                        .foregroundStyle(.ink(.tertiary))
                }
                Text(remaining)
                    .claudeFont(.caption, monospacedDigits: true)
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(1)
                    .fixedSize()
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(SessionRowContent.taskSummary(tasks, estimate: estimate))
    }

    @ViewBuilder
    private var bar: some View {
        let items = tasks.items
        if Self.isSegmented(count: items.count, width: width) {
            HStack(spacing: 1.5) {
                ForEach(items) { item in
                    if item.status == .inProgress && item.id == tasks.activeItem?.id {
                        activeFill(credit: Self.activeCredit(estimate))
                    } else {
                        Capsule(style: .continuous)
                            .fill(color(item.status))
                    }
                }
            }
        } else {
            GeometryReader { proxy in
                let fraction = CGFloat(tasks.fraction)
                let taskWidth = proxy.size.width / CGFloat(max(tasks.totalCount, 1))
                ZStack(alignment: .leading) {
                    Capsule(style: .continuous).fill(theme.barTrack)
                    Capsule(style: .continuous)
                        .fill(color(.completed))
                        .frame(width: max(proxy.size.width * fraction, fraction > 0 ? theme.barHeight : 0))
                    if tasks.activeItem != nil, fraction < 1 {
                        activeFill(credit: Self.activeCredit(estimate))
                            .frame(width: max(taskWidth, theme.barHeight))
                            .offset(x: proxy.size.width * fraction)
                    }
                }
            }
        }
    }

    /// The task in progress: solid secondary ink, or with an estimate a
    /// fainter wash filled solid as far as `credit` (0...1) reaches.
    @ViewBuilder
    private func activeFill(credit: Double?) -> some View {
        if let credit {
            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Capsule(style: .continuous)
                        .fill(color(.inProgress).opacity(Self.activeWashOpacity))
                    if credit > 0 {
                        Capsule(style: .continuous)
                            .fill(color(.inProgress))
                            .frame(width: max(proxy.size.width * CGFloat(credit), min(theme.barHeight, proxy.size.width)))
                    }
                }
            }
        } else {
            Capsule(style: .continuous).fill(color(.inProgress))
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

    /// How much of the segment in progress to fill solid, 0...1; nil (all
    /// solid, as without an estimate) when there is no pace to credit it by.
    nonisolated static func activeCredit(_ estimate: TaskEstimate?) -> Double? {
        guard let estimate, estimate.remaining != nil else { return nil }
        return Double(estimate.activePercent) / 100
    }

    /// The time left in `style`, after the count; nil without an estimate.
    nonisolated static func remainingLabel(_ estimate: TaskEstimate?, style: RemainingStyle) -> String? {
        switch style {
        case .hidden: return nil
        case .short: return estimate?.remainingShort
        case .full: return estimate?.remaining
        }
    }
}
