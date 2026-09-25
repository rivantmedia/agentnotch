import ClaudeControl
import SwiftUI

/// What the fork adds to a Claude ring (U7b): the settle switch for the green
/// review arc, and the needs-you and review badges. Other providers' cells,
/// and every cell before the bridge has attached, pass through untouched.
///
/// Fork-only file. Owned by WP-C.
struct ClaudeRingDecoration: ViewModifier {
    let providerID: String

    init(providerID: String) {
        self.providerID = providerID
    }

    func body(content: Content) -> some View {
        if ClaudeBridge.ownsProvider(providerID) {
            content.modifier(ClaudeRingOverlay(ringID: providerID,
                                               state: .shared, settings: .shared))
        } else {
            content
        }
    }
}

/// What a Claude ring's cell tells VoiceOver besides its name and reading
/// (U7c, on the cell's own accessibility element, where the badges and
/// resting marks are hidden): "2 need you, 1 to review, 3 working". Nothing
/// for other providers, or before the bridge has attached (GUX-6).
struct ClaudeRingAccessibility: ViewModifier {
    let providerID: String

    func body(content: Content) -> some View {
        if ClaudeBridge.ownsProvider(providerID) {
            content.modifier(Observed(ringID: providerID, state: .shared))
        } else {
            content
        }
    }

    /// "2 need you, 1 to review, 3 working, 1 failed"; empty when all zero.
    static func text(_ counts: ClaudeAttentionCounts) -> String {
        var parts: [String] = []
        if counts.needsYou > 0 { parts.append("\(counts.needsYou) need\(counts.needsYou == 1 ? "s" : "") you") }
        if counts.review > 0 { parts.append("\(counts.review) to review") }
        if counts.working > 0 { parts.append("\(counts.working) working") }
        if counts.failed > 0 { parts.append("\(counts.failed) failed") }
        return parts.joined(separator: ", ")
    }

    private struct Observed: ViewModifier {
        let ringID: String
        @ObservedObject var state: ClaudeNotchState

        func body(content: Content) -> some View {
            if state.isAttached {
                content.accessibilityValue(ClaudeRingAccessibility.text(state.ringCounts[ringID] ?? .zero))
            } else {
                content
            }
        }
    }
}

/// The settle switch and the badges, for one Claude ring.
///
/// `\.activitySuccessSettles` turns the green arc steady once the ring's
/// "just finished" window (90 s after its newest completion) has passed; the
/// hub republishes at that boundary, so this redraws on its own.
///
/// The badges count sessions that need you (amber) and finished work not yet
/// reviewed (green). Working has none: the spinner already says it. They sit
/// on the side of the ring facing the middle of the screen, where
/// `ClaudeRingBadgeLayout` puts them for this edge, and shrink to dots in the
/// strip beside the camera. They never pulse and never take a click: the ring
/// under them is the button.
private struct ClaudeRingOverlay: ViewModifier {
    let ringID: String
    @ObservedObject var state: ClaudeNotchState
    @ObservedObject var settings: ClaudeNotchSettings
    @Environment(\.claudeCellContext) private var cell
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        if state.isAttached {
            let counts = state.ringCounts[ringID] ?? .zero
            content
                .environment(\.activitySuccessSettles, state.settles(ringID, now: Date()))
                .overlay(alignment: .topLeading) {
                    if settings.ringBadges {
                        ClaudeRingBadges(counts: counts, edge: cell.edge.claudeEdge, compact: cell.compact)
                            .allowsHitTesting(false)
                            .accessibilityHidden(true)
                            .animation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 0.8),
                                       value: counts)
                    }
                }
        } else {
            content
        }
    }
}

/// The two badges in the ring's own frame (`NotchLayout.ringDiameter`
/// square, origin top left).
struct ClaudeRingBadges: View {
    let counts: ClaudeAttentionCounts
    let edge: ClaudePanelEdge
    let compact: Bool

    private let diameter = NotchLayout.ringDiameter

    var body: some View {
        ZStack(alignment: .topLeading) {
            badge(.needsYou, count: counts.needsYou, color: Palette.watch)
            badge(.review, count: counts.review, color: Palette.ample)
        }
        .frame(width: diameter, height: diameter, alignment: .topLeading)
    }

    @ViewBuilder
    private func badge(_ slot: ClaudeRingBadgeSlot, count: Int, color: Color) -> some View {
        if let label = ClaudeRingBadgeLayout.label(count: count) {
            let centre = ClaudeRingBadgeLayout.center(slot, edge: edge, compact: compact, ringDiameter: diameter)
            Group {
                if compact {
                    Circle()
                        .fill(color)
                        .frame(width: ClaudeRingBadgeLayout.dotDiameter, height: ClaudeRingBadgeLayout.dotDiameter)
                        .background(Circle().fill(Palette.notch)
                            .padding(-ClaudeRingBadgeLayout.knockout))
                } else {
                    Text(label)
                        .font(.system(size: 9, weight: .bold, design: .rounded).monospacedDigit())
                        .foregroundStyle(Color.black)
                        .lineLimit(1)
                        .fixedSize()
                        .padding(.horizontal, ClaudeRingBadgeLayout.badgePadding)
                        .frame(minWidth: ClaudeRingBadgeLayout.badgeHeight,
                               maxWidth: ClaudeRingBadgeLayout.maxBadgeWidth,
                               minHeight: ClaudeRingBadgeLayout.badgeHeight,
                               maxHeight: ClaudeRingBadgeLayout.badgeHeight)
                        .background(Capsule().fill(color))
                        .background(Capsule().fill(Palette.notch)
                            .padding(-ClaudeRingBadgeLayout.knockout))
                }
            }
            .position(centre)
            .transition(.scale(scale: 0.4).combined(with: .opacity))
        }
    }
}
