import ClaudeControl
import SwiftUI

/// Up to three marks on the folded pill (U8a, design §6): an amber bar while
/// a session needs you (a bar, so it never depends on hue alone; GUX-11),
/// a green dot while finished work waits for review, a white dot while any
/// session works. Folded is Codenotch's default, and without these
/// the pill says nothing about Claude until it is opened. Only sessions the
/// open notch would show count (`ClaudeNotchState`): a dot never points at a
/// ring that is switched off.
///
/// Not drawn where the folded notch cannot show them: beside the camera the
/// notch folds into the cutout itself, and anything drawn there is in the
/// hole in the screen. (That case is `ClaudeNotchHold`'s: it keeps those
/// notches open while a session needs you.)
///
/// The amber dot breathes seven half-cycles and ends bright, and does so again
/// whenever the number of sessions needing you changes. Nothing repeats
/// forever: a pill that never stops moving is one the eye learns to ignore.
///
/// Fork-only file. Owned by WP-C.
struct ClaudeRestingMarks: View {
    @ObservedObject var model: NotchViewModel
    let place: NotchPlacement
    @ObservedObject private var state = ClaudeNotchState.shared
    @ObservedObject private var settings = ClaudeNotchSettings.shared

    init(model: NotchViewModel, place: NotchPlacement) {
        self.model = model
        self.place = place
    }

    private var marks: [ClaudeAttentionPolicy.RestingMark] {
        guard state.isAttached, settings.restingMarks, !model.isFlushWithHardware else { return [] }
        return ClaudeAttentionPolicy.restingMarks(state.totalCounts)
    }

    var body: some View {
        let marks = marks
        let offsets = ClaudeRestingMarkLayout.alongOffsets(marks: marks)
        // The pill's centre: the middle of the panel along the edge, and the
        // middle of what shows of it across (see `NotchRootView.notch`).
        let along = (model.edge.isVertical ? place.panelSize.height : place.panelSize.width) / 2
        let across = ClaudeRestingMarkLayout.acrossCentre(
            drawnDepth: model.restingDepth * model.sizeScale, bleed: NotchRootView.bezelBleed)
        ZStack(alignment: .topLeading) {
            ForEach(Array(marks.enumerated()), id: \.element) { index, mark in
                ClaudeRestingDot(mark: mark, needsYou: state.totalCounts.needsYou, alongIsVertical: model.edge.isVertical)
                    .position(place.point(along: along + offsets[index], across: across))
            }
        }
        .frame(width: place.panelSize.width, height: place.panelSize.height, alignment: .topLeading)
        // Gone the moment the notch starts to open, back once it has folded.
        .opacity(model.isExpanded ? 0 : 1)
        .animation(model.isExpanded ? .easeOut(duration: 0.1) : .easeIn(duration: 0.25).delay(0.3),
                   value: model.isExpanded)
        .animation(.easeInOut(duration: 0.25), value: marks)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

private struct ClaudeRestingDot: View {
    let mark: ClaudeAttentionPolicy.RestingMark
    /// Breathing restarts when this changes.
    let needsYou: Int
    /// The pill runs down the screen (a side edge): the bar stands upright.
    let alongIsVertical: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.claudeStillFrame) private var stillFrame
    @State private var bright = true

    private var color: Color {
        switch mark {
        case .needsYou: return Palette.watch
        case .review: return Palette.ample
        case .working: return Color.white
        }
    }

    var body: some View {
        let along = ClaudeRestingMarkLayout.length(of: mark)
        let across = ClaudeRestingMarkLayout.dotDiameter
        Capsule()
            .fill(color)
            .frame(width: alongIsVertical ? across : along, height: alongIsVertical ? along : across)
            .opacity(bright ? 1 : 0.3)
            .onAppear(perform: breathe)
            .onChange(of: needsYou) { _, _ in breathe() }
            .transition(.opacity)
    }

    /// Dim at once, then back to bright over an odd number of half-cycles,
    /// so the animation ends where the value already is.
    private func breathe() {
        guard mark == .needsYou, !reduceMotion, !stillFrame else {
            bright = true
            return
        }
        var dim = Transaction()
        dim.disablesAnimations = true
        withTransaction(dim) { bright = false }
        withAnimation(.easeInOut(duration: 0.8).repeatCount(ClaudeAttentionPolicy.restingBreaths, autoreverses: true)) {
            bright = true
        }
    }
}
