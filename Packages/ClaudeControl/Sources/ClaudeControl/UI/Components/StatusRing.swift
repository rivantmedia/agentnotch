//
//  StatusRing.swift
//  ClaudeControl
//
//  The mark beside a session, in Codenotch's own vocabulary (its hover card's
//  StatusRing): a three-quarter arc turning while Claude works, half a ring
//  held still while it waits on you, a full ring when it is done or idle.
//  A failed turn is a solid dot, so it differs from "needs you" in shape as
//  well as colour.
//

import AppKit
import QuartzCore
import SwiftUI

struct StatusRing: View {
    let kind: SessionGlyphKind
    /// Changing it breathes the needs-you ring again (another session started
    /// waiting, the row reappeared).
    var breathKey: Int = 0

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        Group {
            switch kind {
            case .working:
                ArcSpinner(color: theme.working, lineWidth: theme.statusRingStroke)
            case .needsInput:
                BreathingRing(trim: 0.5, color: theme.needsYou, lineWidth: theme.statusRingStroke)
                    .id(breathKey)
            case .error:
                Circle()
                    .fill(theme.critical)
                    .padding(theme.statusRing * 0.14)
            case .review:
                ring(trim: 1, color: theme.review)
            case .idle:
                ring(trim: 1, color: theme.textSecondary)
            }
        }
        .frame(width: theme.statusRing, height: theme.statusRing)
        .accessibilityHidden(true)
    }

    private func ring(trim: CGFloat, color: Color) -> some View {
        StatusArc(trim: trim)
            .stroke(color, style: StrokeStyle(lineWidth: theme.statusRingStroke, lineCap: .round))
            .padding(theme.statusRingStroke / 2)
    }
}

/// A circle trimmed to `trim`, starting at twelve o'clock, inset by half a
/// stroke so the stroke stays inside the frame.
struct StatusArc: Shape {
    var trim: CGFloat

    func path(in rect: CGRect) -> Path {
        let radius = min(rect.width, rect.height) / 2
        var path = Path()
        path.addArc(
            center: CGPoint(x: rect.midX, y: rect.midY),
            radius: radius,
            startAngle: .degrees(-90),
            endAngle: .degrees(-90 + 360 * Double(trim)),
            clockwise: false
        )
        return path
    }
}

/// Half a ring that brightens a few times when it appears, then holds still.
private struct BreathingRing: View {
    let trim: CGFloat
    let color: Color
    let lineWidth: CGFloat

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.claudeStaticRendering) private var isStatic
    @State private var isBright = false

    var body: some View {
        StatusArc(trim: trim)
            .stroke(color, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
            .padding(lineWidth / 2)
            .opacity(isBright || !breathes ? 1 : 0.35)
            .onAppear {
                guard breathes else { return }
                let halfCycles = ClaudeMotion.breathHalfCycles(reduceMotion: reduceMotion, isStatic: isStatic)
                withAnimation(.easeInOut(duration: ClaudeMotion.breathHalfPeriod)
                    .repeatCount(halfCycles, autoreverses: true)) {
                    isBright = true
                }
            }
    }

    private var breathes: Bool {
        ClaudeMotion.breathHalfCycles(reduceMotion: reduceMotion, isStatic: isStatic) > 0
    }
}

// MARK: - Arc spinner

/// Codenotch's working indicator: a three-quarter arc turning once every
/// 1.4 s. The turn is a Core Animation layer animation, which the render
/// server runs on its own, so a panel left open for hours costs the app no
/// frames. Under Reduce Motion (and in snapshots) the arc holds still, its
/// gap alone saying "in progress".
struct ArcSpinner: View {
    let color: Color
    var lineWidth: CGFloat = 1.6

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.claudeStaticRendering) private var isStatic

    var body: some View {
        if ClaudeMotion.spins(reduceMotion: reduceMotion, isStatic: isStatic) {
            ArcSpinnerLayerView(color: color, lineWidth: lineWidth)
        } else {
            StatusArc(trim: ClaudeMotion.spinnerTrim)
                .stroke(color, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
                .padding(lineWidth / 2)
        }
    }
}

private struct ArcSpinnerLayerView: NSViewRepresentable {
    let color: Color
    let lineWidth: CGFloat

    func makeNSView(context: Context) -> SpinnerNSView {
        SpinnerNSView()
    }

    func updateNSView(_ view: SpinnerNSView, context: Context) {
        let resolved = color.resolve(in: context.environment)
        view.configure(color: resolved.cgColor, lineWidth: lineWidth)
    }

    /// A layer-hosting view: one shape layer, spun by one infinite
    /// `transform.rotation.z` animation that survives resizes.
    final class SpinnerNSView: NSView {
        private let arc = CAShapeLayer()
        private static let animationKey = "claudeControl.spin"

        override init(frame frameRect: NSRect) {
            super.init(frame: frameRect)
            let host = CALayer()
            // y down, like SwiftUI: the arc starts at twelve o'clock and a
            // positive turn is clockwise.
            host.isGeometryFlipped = true
            layer = host
            wantsLayer = true
            arc.fillColor = nil
            arc.lineCap = .round
            host.addSublayer(arc)
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { nil }

        func configure(color: CGColor, lineWidth: CGFloat) {
            arc.strokeColor = color
            arc.lineWidth = lineWidth
            needsLayout = true
        }

        override func layout() {
            super.layout()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            arc.frame = bounds
            let inset = arc.lineWidth / 2
            let radius = max(0, min(bounds.width, bounds.height) / 2 - inset)
            let center = CGPoint(x: bounds.midX, y: bounds.midY)
            let path = CGMutablePath()
            path.addArc(center: center, radius: radius, startAngle: -.pi / 2,
                        endAngle: -.pi / 2 + 2 * .pi * ClaudeMotion.spinnerTrim, clockwise: false)
            arc.path = path
            CATransaction.commit()
            startSpinningIfNeeded()
        }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            startSpinningIfNeeded()
        }

        private func startSpinningIfNeeded() {
            guard window != nil, arc.animation(forKey: Self.animationKey) == nil else { return }
            let spin = CABasicAnimation(keyPath: "transform.rotation.z")
            spin.fromValue = 0
            spin.toValue = 2 * Double.pi
            spin.duration = ClaudeMotion.spinPeriod
            spin.repeatCount = .infinity
            spin.isRemovedOnCompletion = false
            arc.add(spin, forKey: Self.animationKey)
        }
    }
}
