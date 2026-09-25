//
//  ClaudeMotion.swift
//  ClaudeControl
//
//  The panel's few movements, and when they are skipped. Nothing here loops
//  in SwiftUI: an auto-opened panel can sit on screen for hours, and a
//  `repeatForever` animation redraws its view every frame for as long as it
//  lives. The needs-you breath runs a fixed number of times; the working arc
//  turns on a Core Animation layer (`ArcSpinner`), which the render server
//  drives without waking the app. Reduce Motion stills both.
//

import SwiftUI

nonisolated enum ClaudeMotion {
    /// Half-cycles of the needs-you breath. Odd, so the last one ends bright.
    static let breathHalfCycles = 7
    /// One half-cycle of the breath, in seconds.
    static let breathHalfPeriod: Double = 0.9
    /// One turn of the working arc, in seconds (Codenotch's StatusRing period).
    static let spinPeriod: Double = 1.4
    /// How much of the circle the working arc covers.
    static let spinnerTrim: CGFloat = 0.75

    /// Breath half-cycles to run: none under Reduce Motion or in a snapshot.
    static func breathHalfCycles(reduceMotion: Bool, isStatic: Bool) -> Int {
        reduceMotion || isStatic ? 0 : breathHalfCycles
    }

    /// Whether the working arc turns.
    static func spins(reduceMotion: Bool, isStatic: Bool) -> Bool {
        !(reduceMotion || isStatic)
    }

    /// `animation`, or nothing under Reduce Motion (the change still happens,
    /// it just doesn't move).
    static func animation(_ animation: Animation, reduceMotion: Bool) -> Animation? {
        reduceMotion ? nil : animation
    }

    /// Rows gliding to new places, sections folding.
    static let glide = Animation.spring(response: 0.32, dampingFraction: 0.86)
    /// Hover fills and small state changes.
    static let quick = Animation.easeOut(duration: 0.12)
}

extension View {
    /// `withAnimation`-free: animate `value` changes with `animation`, unless
    /// Reduce Motion is on.
    func claudeAnimation<V: Equatable>(_ animation: Animation, value: V) -> some View {
        modifier(ClaudeAnimationModifier(animation: animation, value: value))
    }
}

private struct ClaudeAnimationModifier<V: Equatable>: ViewModifier {
    let animation: Animation
    let value: V
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.claudeStaticRendering) private var isStatic

    func body(content: Content) -> some View {
        content.animation(isStatic ? nil : ClaudeMotion.animation(animation, reduceMotion: reduceMotion), value: value)
    }
}
