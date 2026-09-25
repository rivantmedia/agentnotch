//
//  ClaudePanelGeometry.swift
//  ClaudeControl
//
//  Where the sessions panel goes: beside the notch window, on the side facing
//  the screen, its tail pointing at the ring that was clicked (design §7).
//  Pure; every rect is in screen coordinates (origin bottom-left, y up), as
//  AppKit hands them out.
//
//  The panel window is the card plus its tail. The tail's tip sits where
//  Codenotch's own hover cards put theirs: `tooltipInset` in from the bezel,
//  level with the ring. The card is centred on the ring along the edge and
//  kept inside the visible frame; when it has to move to stay there, the tail
//  slides along the card to keep pointing at the ring, but never into its
//  rounded corners.
//
//  Owned by WP-D.
//

import CoreGraphics
import Foundation

public nonisolated enum ClaudePanelEdge: Sendable { case right, left, top, bottom }
public nonisolated enum ClaudePanelMode: Sendable { case list, chat }

/// The notch the panel hangs off.
public nonisolated struct ClaudePanelAnchor: Equatable, Sendable {
    public var edge: ClaudePanelEdge
    public var notchWindowFrame: CGRect
    /// The ring's centre along the notch, from the window's start (top on the
    /// side edges, left on top/bottom): `slack + ringCenter(index:) * sizeScale`.
    public var ringAlong: CGFloat
    /// From the window's bezel edge to where a tail tip sits (`tooltipInset`).
    public var tailTipInset: CGFloat
    public var visibleFrame: CGRect

    public init(edge: ClaudePanelEdge, notchWindowFrame: CGRect, ringAlong: CGFloat,
                tailTipInset: CGFloat, visibleFrame: CGRect) {
        self.edge = edge
        self.notchWindowFrame = notchWindowFrame
        self.ringAlong = ringAlong
        self.tailTipInset = tailTipInset
        self.visibleFrame = visibleFrame
    }
}

/// The card's silhouette metrics (Codenotch's NotchLayout / TooltipTail).
public nonisolated struct ClaudePanelChrome: Equatable, Sendable {
    /// How far the tail reaches out from the card.
    public var tailLength: CGFloat
    /// How wide the tail is where it leaves the card.
    public var tailWidth: CGFloat
    public var corner: CGFloat
    /// The least distance between the card and the visible frame's edges.
    public var margin: CGFloat = 8

    public init(tailLength: CGFloat, tailWidth: CGFloat, corner: CGFloat, margin: CGFloat = 8) {
        self.tailLength = tailLength
        self.tailWidth = tailWidth
        self.corner = corner
        self.margin = margin
    }
}

public nonisolated struct ClaudePanelPlacement: Equatable, Sendable {
    /// The panel window: the card plus its tail.
    public var windowFrame: CGRect
    /// The card, in screen coordinates.
    public var cardRect: CGRect
    /// The tail's offset from the card's centre along the card's edge, the
    /// way SwiftUI measures it (`TooltipSilhouette.tailOffset`): downwards on
    /// the side edges, rightwards on top and bottom.
    public var tailOffset: CGFloat
    public var hasTail: Bool
    public var contentWidth: CGFloat
    public var maxContentHeight: CGFloat

    public init(windowFrame: CGRect, cardRect: CGRect, tailOffset: CGFloat, hasTail: Bool,
                contentWidth: CGFloat, maxContentHeight: CGFloat) {
        self.windowFrame = windowFrame
        self.cardRect = cardRect
        self.tailOffset = tailOffset
        self.hasTail = hasTail
        self.contentWidth = contentWidth
        self.maxContentHeight = maxContentHeight
    }
}

public nonisolated enum ClaudePanelGeometry {
    /// The shortest the card gets, whatever the content asks for.
    public static let minimumHeight: CGFloat = 220
    /// The narrowest a side-edge card is squeezed to on a small screen.
    public static let minimumWidth: CGFloat = 200

    /// The card's width: narrower beside a side notch, where it eats into the
    /// screen's width, and wider for the chat, which reads better with room.
    public static func width(edge: ClaudePanelEdge?, mode: ClaudePanelMode) -> CGFloat {
        switch (edge, mode) {
        case (.right?, .list), (.left?, .list): return 400
        case (.right?, .chat), (.left?, .chat): return 440
        case (_, .list): return 440
        case (_, .chat): return 520
        }
    }

    /// The tallest the card gets before the visible frame has a say.
    public static func heightCap(_ mode: ClaudePanelMode) -> CGFloat {
        mode == .list ? 680 : 780
    }

    public static func place(
        anchor: ClaudePanelAnchor?,
        mode: ClaudePanelMode,
        idealContentHeight: CGFloat,
        chrome: ClaudePanelChrome,
        fallbackVisibleFrame: CGRect
    ) -> ClaudePanelPlacement {
        // A height reported mid-transition can be NaN; it must not reach a window frame.
        let ideal = idealContentHeight.isFinite ? idealContentHeight : minimumHeight
        guard let anchor else {
            return floating(mode: mode, ideal: ideal, chrome: chrome, visible: fallbackVisibleFrame)
        }
        switch anchor.edge {
        case .right, .left: return beside(anchor, mode: mode, ideal: ideal, chrome: chrome)
        case .top, .bottom: return aboveOrBelow(anchor, mode: mode, ideal: ideal, chrome: chrome)
        }
    }

    /// The ring the tail points at, in screen coordinates: `ringAlong` down
    /// from the window's top on the side edges, right from its left edge on
    /// top and bottom, level with the tail tip across.
    public static func ringPoint(_ anchor: ClaudePanelAnchor) -> CGPoint {
        let frame = anchor.notchWindowFrame
        switch anchor.edge {
        case .right: return CGPoint(x: frame.maxX - anchor.tailTipInset, y: frame.maxY - anchor.ringAlong)
        case .left: return CGPoint(x: frame.minX + anchor.tailTipInset, y: frame.maxY - anchor.ringAlong)
        case .top: return CGPoint(x: frame.minX + anchor.ringAlong, y: frame.maxY - anchor.tailTipInset)
        case .bottom: return CGPoint(x: frame.minX + anchor.ringAlong, y: frame.minY + anchor.tailTipInset)
        }
    }

    /// Where the drawn tail's tip lands, in screen coordinates; nil without a
    /// tail. The inverse of the placement, for tests and diagnostics.
    static func tailTip(of placement: ClaudePanelPlacement, edge: ClaudePanelEdge) -> CGPoint? {
        guard placement.hasTail else { return nil }
        let card = placement.cardRect
        let window = placement.windowFrame
        switch edge {
        case .right: return CGPoint(x: window.maxX, y: card.midY - placement.tailOffset)
        case .left: return CGPoint(x: window.minX, y: card.midY - placement.tailOffset)
        case .top: return CGPoint(x: card.midX + placement.tailOffset, y: window.maxY)
        case .bottom: return CGPoint(x: card.midX + placement.tailOffset, y: window.minY)
        }
    }

    // MARK: - Layouts

    /// Right and left: the card beside the notch, centred on the ring's y.
    private static func beside(_ anchor: ClaudePanelAnchor, mode: ClaudePanelMode, ideal: CGFloat,
                               chrome: ClaudePanelChrome) -> ClaudePanelPlacement {
        let visible = anchor.visibleFrame
        let margin = chrome.margin
        let tail = chrome.tailLength
        let ring = ringPoint(anchor)
        let isRight = anchor.edge == .right

        // The card's edge nearest the notch: at the tail's root, or pulled
        // back inside the visible frame when the notch sits over something
        // (a Dock on that side).
        let root = isRight
            ? min(ring.x - tail, visible.maxX - margin)
            : max(ring.x + tail, visible.minX + margin)
        // Never wider than the space between the notch and the far side.
        let room = isRight ? root - (visible.minX + margin) : (visible.maxX - margin) - root
        let width = max(minimumWidth, min(width(edge: anchor.edge, mode: mode), room))

        let maxHeight = max(minimumHeight, min(heightCap(mode), visible.height - 2 * margin))
        let height = clamp(ideal, minimumHeight, maxHeight)
        // Centred on the ring, inside the visible frame; a card taller than
        // the screen (only below the minimum height) keeps its top on it.
        let y = height > visible.height - 2 * margin
            ? visible.maxY - margin - height
            : clamp(ring.y - height / 2, visible.minY + margin, visible.maxY - margin - height)
        let card = CGRect(x: isRight ? root - width : root, y: y, width: width, height: height)

        // Measured downwards from the card's centre, like SwiftUI; screen y
        // grows upwards.
        let offset = clampTail(card.midY - ring.y, length: height, chrome: chrome)
        let window = isRight
            ? CGRect(x: card.minX, y: card.minY, width: width + tail, height: height)
            : CGRect(x: card.minX - tail, y: card.minY, width: width + tail, height: height)
        return ClaudePanelPlacement(windowFrame: window, cardRect: card, tailOffset: offset,
                                    hasTail: true, contentWidth: width, maxContentHeight: maxHeight)
    }

    /// Top and bottom (the bar, or the strips either side of the camera): the
    /// card below or above the notch, centred on the ring's x. Beside the
    /// camera `tailTipInset` already ends just under the menu bar, because
    /// Codenotch's `notchDrawnDepth` measures the hardware's foot there.
    private static func aboveOrBelow(_ anchor: ClaudePanelAnchor, mode: ClaudePanelMode, ideal: CGFloat,
                                     chrome: ClaudePanelChrome) -> ClaudePanelPlacement {
        let visible = anchor.visibleFrame
        let margin = chrome.margin
        let tail = chrome.tailLength
        let ring = ringPoint(anchor)
        let isTop = anchor.edge == .top

        let width = max(minimumWidth, min(width(edge: anchor.edge, mode: mode), visible.width - 2 * margin))
        let x = clamp(ring.x - width / 2, visible.minX + margin, visible.maxX - margin - width)

        let card: CGRect
        let maxHeight: CGFloat
        if isTop {
            let top = min(ring.y - tail, visible.maxY - margin)
            maxHeight = max(minimumHeight, min(heightCap(mode), top - visible.minY - margin))
            let height = clamp(ideal, minimumHeight, maxHeight)
            card = CGRect(x: x, y: top - height, width: width, height: height)
        } else {
            let bottom = max(ring.y + tail, visible.minY + margin)
            maxHeight = max(minimumHeight, min(heightCap(mode), visible.maxY - margin - bottom))
            let height = clamp(ideal, minimumHeight, maxHeight)
            card = CGRect(x: x, y: bottom, width: width, height: height)
        }

        let offset = clampTail(ring.x - card.midX, length: width, chrome: chrome)
        let window = isTop
            ? CGRect(x: card.minX, y: card.minY, width: width, height: card.height + tail)
            : CGRect(x: card.minX, y: card.minY - tail, width: width, height: card.height + tail)
        return ClaudePanelPlacement(windowFrame: window, cardRect: card, tailOffset: offset,
                                    hasTail: true, contentWidth: width, maxContentHeight: maxHeight)
    }

    /// No notch to hang off (hidden, or the ring is switched off): a plain
    /// card under the menu bar of the screen the pointer is on.
    private static func floating(mode: ClaudePanelMode, ideal: CGFloat, chrome: ClaudePanelChrome,
                                 visible: CGRect) -> ClaudePanelPlacement {
        let margin = chrome.margin
        let width = max(0, min(width(edge: nil, mode: mode), visible.width - 2 * margin))
        let maxHeight = max(minimumHeight, visible.height - 2 * margin)
        let height = clamp(ideal, minimumHeight, maxHeight)
        let card = CGRect(x: visible.midX - width / 2, y: visible.maxY - margin - height, width: width, height: height)
        return ClaudePanelPlacement(windowFrame: card, cardRect: card, tailOffset: 0, hasTail: false,
                                    contentWidth: width, maxContentHeight: maxHeight)
    }

    // MARK: - Helpers

    /// ±(cardLength/2 − corner − tailWidth/2): the tail never runs into a
    /// rounded corner, the same limit `TooltipSilhouette` applies.
    public static func tailOffsetLimit(cardLength: CGFloat, chrome: ClaudePanelChrome) -> CGFloat {
        max(0, cardLength / 2 - chrome.corner - chrome.tailWidth / 2)
    }

    static func clampTail(_ offset: CGFloat, length: CGFloat, chrome: ClaudePanelChrome) -> CGFloat {
        let limit = tailOffsetLimit(cardLength: length, chrome: chrome)
        return clamp(offset, -limit, limit)
    }

    /// `value` within `low...high`; `low` wins when the range is empty.
    static func clamp(_ value: CGFloat, _ low: CGFloat, _ high: CGFloat) -> CGFloat {
        guard low <= high else { return low }
        return min(max(value, low), high)
    }
}
