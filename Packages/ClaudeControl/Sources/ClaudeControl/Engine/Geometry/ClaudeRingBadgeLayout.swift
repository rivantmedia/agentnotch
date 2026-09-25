//
//  ClaudeRingBadgeLayout.swift
//  ClaudeControl
//
//  Where the needs-you and review badges sit on a Claude ring: on the side
//  facing the screen centre, away from the percent label (design §6). Pure.
//
//  Owned by WP-C. Also the folded notch's dots (`ClaudeRestingMarkLayout`).
//
//  Everything is in the ring's own frame: a `ringDiameter` square, origin at
//  the top left, y growing downward (SwiftUI's convention). Angles are
//  clockwise from 3 o'clock in that frame, so 90° points down.
//
//  | Layout      | Needs-you         | Review            |
//  |-------------|-------------------|-------------------|
//  | Right edge  | 225° (up-left)    | 135° (down-left)  |
//  | Left edge   | 315° (up-right)   | 45° (down-right)  |
//  | Top         | 135° (down-left)  | 45° (down-right)  |
//  | Bottom      | 225° (up-left)    | 315° (up-right)   |
//
//  On a side edge the screen centre is beside the ring and the label below
//  it, so the badges take the two diagonals on the inner side. On the top and
//  bottom edges the screen centre is below or above the ring; the label sits
//  under the ring on both, a full label gap away, so the badges keep to the
//  ring's own square.
//

import CoreGraphics
import Foundation

public nonisolated enum ClaudeRingBadgeSlot: Sendable { case needsYou, review }

public nonisolated enum ClaudeRingBadgeLayout {
    // MARK: - Sizes (ring points, scaled with the notch like the ring itself)

    /// A count badge is a capsule this tall, and at least this wide.
    public static let badgeHeight: CGFloat = 12
    /// Horizontal padding either side of the digits.
    public static let badgePadding: CGFloat = 3
    /// The widest badge: "9+" at 9 pt bold rounded, plus padding.
    public static let maxBadgeWidth: CGFloat = 18
    /// Ring of the notch's own black drawn round a badge, so it reads on top of
    /// the track and the weekly ring.
    public static let knockout: CGFloat = 1.5
    /// A compact badge (the strip beside the camera): a dot, no digits. The
    /// strip draws its rings at roughly two thirds of their size, so this lands
    /// near 5 pt on screen.
    public static let dotDiameter: CGFloat = 7
    /// How far a count badge's centre sits outside the ring's edge, for a ring
    /// of the design size (44 pt); proportional for other sizes. Far enough
    /// that the badge (and its knockout) clears the weekly ring drawn outside
    /// the track (radius 24.4 pt, 1.9 pt stroke, in Codenotch's layout), so
    /// neither arc's tip is ever under a badge (GUX-12); still inside the
    /// body and above the percent label on every edge.
    public static let badgeOutset: CGFloat = 11
    /// The ring size the outset is quoted at.
    public static let designRingDiameter: CGFloat = 44

    // MARK: - Placement

    /// Degrees, clockwise from 3 o'clock (y down).
    public static func angle(_ slot: ClaudeRingBadgeSlot, edge: ClaudePanelEdge) -> Double {
        switch (edge, slot) {
        case (.right, .needsYou): return 225
        case (.right, .review): return 135
        case (.left, .needsYou): return 315
        case (.left, .review): return 45
        case (.top, .needsYou): return 135
        case (.top, .review): return 45
        case (.bottom, .needsYou): return 225
        case (.bottom, .review): return 315
        }
    }

    /// Distance from the ring's centre to a badge's centre.
    ///
    /// A count badge sits outside both arcs (see `badgeOutset`). A compact dot
    /// sits on the ring's own circle: the strip beside the camera is only a
    /// little deeper than the ring, and a dot outside it would be clipped.
    public static func radius(compact: Bool, ringDiameter: CGFloat) -> CGFloat {
        compact ? ringDiameter / 2 : ringDiameter / 2 + badgeOutset * ringDiameter / designRingDiameter
    }

    /// The badge's centre in the ring's own frame.
    public static func center(_ slot: ClaudeRingBadgeSlot, edge: ClaudePanelEdge, compact: Bool,
                              ringDiameter: CGFloat) -> CGPoint {
        let radians = angle(slot, edge: edge) * .pi / 180
        let radius = radius(compact: compact, ringDiameter: ringDiameter)
        let half = ringDiameter / 2
        return CGPoint(x: half + radius * CGFloat(cos(radians)),
                       y: half + radius * CGFloat(sin(radians)))
    }

    /// The area a badge covers in the ring's frame: a dot when `compact`,
    /// otherwise a capsule `badgeWidth` wide (at most `maxBadgeWidth`).
    public static func frame(_ slot: ClaudeRingBadgeSlot, edge: ClaudePanelEdge, compact: Bool,
                             ringDiameter: CGFloat, badgeWidth: CGFloat = maxBadgeWidth) -> CGRect {
        let centre = center(slot, edge: edge, compact: compact, ringDiameter: ringDiameter)
        let size = compact
            ? CGSize(width: dotDiameter, height: dotDiameter)
            : CGSize(width: max(badgeHeight, min(badgeWidth, maxBadgeWidth)), height: badgeHeight)
        return CGRect(x: centre.x - size.width / 2, y: centre.y - size.height / 2,
                      width: size.width, height: size.height)
    }

    // MARK: - Content

    /// The digits a count badge shows: nothing for zero, "9+" past nine.
    public static func label(count: Int) -> String? {
        guard count > 0 else { return nil }
        return count > 9 ? "9+" : String(count)
    }
}

/// Where the dots on the folded notch sit (design §6): a short row centred on
/// the pill, running along it. Screen points, unscaled by the notch size: the
/// pill is only a few points deep at every size, and the dots have to fit it.
public nonisolated enum ClaudeRestingMarkLayout {
    public static let dotDiameter: CGFloat = 4
    /// Space between two dots.
    public static let gap: CGFloat = 3
    /// Needs you is a short bar along the pill, not a dot: told from the
    /// white working dot and the green review one by shape, not by hue
    /// alone, which at 4 pt (and for colour-blind eyes) is not enough
    /// (GUX-11).
    public static let needsYouLength: CGFloat = 9

    /// How long a mark is along the pill.
    public static func length(of mark: ClaudeAttentionPolicy.RestingMark) -> CGFloat {
        mark == .needsYou ? needsYouLength : dotDiameter
    }

    /// Each mark's centre along the pill, relative to the pill's centre, in
    /// drawing order: `gap` apart edge to edge, the run centred on zero.
    public static func alongOffsets(marks: [ClaudeAttentionPolicy.RestingMark]) -> [CGFloat] {
        guard !marks.isEmpty else { return [] }
        let lengths = marks.map(length(of:))
        let run = lengths.reduce(0, +) + CGFloat(marks.count - 1) * gap
        var cursor = -run / 2
        return lengths.map { length in
            defer { cursor += length + gap }
            return cursor + length / 2
        }
    }

    /// Whether these marks fit inside a pill of that length and visible
    /// depth: a point to spare at each end, half a point at each side.
    public static func fits(marks: [ClaudeAttentionPolicy.RestingMark], pillLength: CGFloat, visibleDepth: CGFloat) -> Bool {
        guard !marks.isEmpty else { return true }
        let run = marks.map(length(of:)).reduce(0, +) + CGFloat(marks.count - 1) * gap
        return run + 2 <= pillLength && dotDiameter + 1 <= visibleDepth
    }

    /// Each dot's centre along the pill, relative to the pill's centre, in
    /// drawing order. Symmetric about zero, `dotDiameter + gap` apart.
    public static func alongOffsets(count: Int) -> [CGFloat] {
        guard count > 0 else { return [] }
        let pitch = dotDiameter + gap
        let first = -CGFloat(count - 1) * pitch / 2
        return (0..<count).map { first + CGFloat($0) * pitch }
    }

    /// The dots' centre across the pill, measured in from the bezel: the
    /// middle of what shows of it. The pill is drawn `drawnDepth` deep and
    /// pushed `bleed` past the screen's edge, so only `drawnDepth - bleed` of
    /// it is on screen.
    public static func acrossCentre(drawnDepth: CGFloat, bleed: CGFloat) -> CGFloat {
        max(drawnDepth - bleed, 0) / 2
    }

    /// Whether `count` dots fit inside a pill of that length and visible
    /// depth: a point to spare at each end, half a point at each side.
    public static func fits(count: Int, pillLength: CGFloat, visibleDepth: CGFloat) -> Bool {
        guard count > 0 else { return true }
        let run = CGFloat(count) * dotDiameter + CGFloat(count - 1) * gap
        return run + 2 <= pillLength && dotDiameter + 1 <= visibleDepth
    }
}
