@_spi(Snapshots) import ClaudeControl
import SwiftUI

/// The panel chrome sheets of `--snapshot-claude`: the card and its tail for
/// each edge (tail pointing right, left, up and down), beside the camera, with
/// the tail pushed to a corner, at the chat's width, and floating with no
/// tail. Each is placed by `ClaudePanelGeometry` against a 14" MacBook screen,
/// exactly as the window would be, with the fixture session list inside, and
/// drawn in its patch of that screen: the bezel, the menu bar, the notch
/// window's bounds (dashed) and the point on the ring the tail aims at, so a
/// sheet shows where the panel sits and not only what it looks like.
///
/// Fork-only file (design §11). Owned by WP-D.
extension ClaudeAppSnapshots {
    /// A 14" MacBook Pro: 1512 x 982 points, a 33 pt menu bar.
    private nonisolated static let screen = CGRect(x: 0, y: 0, width: 1512, height: 982)
    private nonisolated static let visible = CGRect(x: 0, y: 0, width: 1512, height: 949)
    /// Bezel to tail tip at medium size (`tooltipInset`), and beside the camera.
    private nonisolated static let inset: CGFloat = 80.5
    private static let cameraInset: CGFloat = 33 + NotchLayout.tailGap

    private struct Case {
        let name: String
        let edge: NotchEdge?
        let notch: CGRect
        let along: CGFloat
        var inset: CGFloat = ClaudeAppSnapshots.inset
        var mode: ClaudePanelMode = .list
    }

    private static var cases: [Case] {
        [
            Case(name: "panel-chrome-right", edge: .right,
                 notch: CGRect(x: 1512 - 334, y: 100, width: 334, height: 782), along: 391),
            Case(name: "panel-chrome-left", edge: .left,
                 notch: CGRect(x: 0, y: 100, width: 334, height: 782), along: 391),
            Case(name: "panel-chrome-top", edge: .top,
                 notch: CGRect(x: 406, y: 982 - 300, width: 700, height: 300), along: 350),
            Case(name: "panel-chrome-bottom", edge: .bottom,
                 notch: CGRect(x: 406, y: 0, width: 700, height: 300), along: 350),
            // The rings in the strip left of the camera, the tail tip just
            // under the menu bar.
            Case(name: "panel-chrome-top-camera", edge: .top,
                 notch: CGRect(x: 356, y: 982 - 330, width: 800, height: 330), along: 250, inset: cameraInset),
            Case(name: "panel-chrome-right-corner", edge: .right,
                 notch: CGRect(x: 1512 - 334, y: 0, width: 334, height: 982), along: 40),
            Case(name: "panel-chrome-bottom-chat", edge: .bottom,
                 notch: CGRect(x: 406, y: 0, width: 700, height: 300), along: 120, mode: .chat),
            Case(name: "panel-chrome-floating", edge: nil, notch: .zero, along: 0),
        ]
    }

    static func panelSheets() -> [Sheet] {
        cases.map { item in
            let anchor = item.edge.map {
                ClaudePanelAnchor(edge: $0.claudePanelEdge, notchWindowFrame: item.notch, ringAlong: item.along,
                                  tailTipInset: item.inset, visibleFrame: visible)
            }
            let placement = ClaudePanelGeometry.place(anchor: anchor, mode: item.mode, idealContentHeight: 420,
                                                      chrome: ClaudePanelController.chromeMetrics,
                                                      fallbackVisibleFrame: visible)
            let chrome = ClaudePanelChromeModel(
                direction: placement.hasTail ? item.edge?.tooltipDirection : nil,
                tailOffset: placement.tailOffset,
                surfaceStyle: .solid,
                accent: AccentColorChoice.system.color
            )
            let panel = ClaudePanelChromeView(chrome: chrome) {
                ClaudeControlSnapshotSheets.sessionList(width: placement.contentWidth)
            }
            let sheet = ScreenPatch(screen: screen, visible: visible, notch: anchor?.notchWindowFrame,
                                    ring: anchor.map(ClaudePanelGeometry.ringPoint),
                                    window: placement.windowFrame, panel: panel)
                .environment(\.colorScheme, .dark)
            return Sheet(item.name, sheet)
        }
    }
}

/// The part of a screen around the panel and its notch, drawn to scale:
/// screen rects (origin bottom-left, y up) turned into the sheet's own
/// top-left coordinates.
private struct ScreenPatch<Panel: View>: View {
    let screen: CGRect
    let visible: CGRect
    let notch: CGRect?
    let ring: CGPoint?
    let window: CGRect
    let panel: Panel

    /// The panel and the notch with some room around them.
    private var patch: CGRect {
        (notch.map { $0.union(window) } ?? window).insetBy(dx: -24, dy: -24)
    }

    var body: some View {
        let patch = patch
        ZStack(alignment: .topLeading) {
            // Past the screen's edge: the bezel.
            Color.black
            block(screen, in: patch) { Rectangle().fill(Color(white: 0.42)) }
            block(CGRect(x: screen.minX, y: visible.maxY, width: screen.width, height: screen.maxY - visible.maxY),
                  in: patch) { Rectangle().fill(Color(white: 0.58)) }
            if let notch {
                block(notch, in: patch) {
                    Rectangle()
                        .fill(Color.black.opacity(0.12))
                        .overlay(Rectangle().strokeBorder(Color.white.opacity(0.7),
                                                          style: StrokeStyle(lineWidth: 1, dash: [5, 4])))
                }
            }
            block(window, in: patch) { panel }
            if let ring {
                let mark: CGFloat = 9
                block(CGRect(x: ring.x - mark / 2, y: ring.y - mark / 2, width: mark, height: mark), in: patch) {
                    Circle().fill(Palette.watch).overlay(Circle().stroke(Color.black, lineWidth: 1.5))
                }
            }
        }
        .frame(width: patch.width, height: patch.height, alignment: .topLeading)
        .clipped()
    }

    /// `content` filling `rect`, placed where `rect` is within `patch`.
    private func block<Content: View>(_ rect: CGRect, in patch: CGRect,
                                      @ViewBuilder _ content: () -> Content) -> some View {
        content()
            .frame(width: rect.width, height: rect.height)
            .offset(x: rect.minX - patch.minX, y: patch.maxY - rect.maxY)
    }
}
