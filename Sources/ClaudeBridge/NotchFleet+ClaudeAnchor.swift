import AppKit
import ClaudeControl

/// The notch the sessions panel hangs off, and where on it.
///
/// Fork-only file (design §7). Owned by WP-D.
struct ClaudeNotchAnchor {
    let controller: NotchWindowController
    /// The Claude ring the tail points at.
    let ringID: String
    let geometry: ClaudePanelAnchor
}

extension NotchEdge {
    /// The same edge, as the package's geometry names it.
    var claudePanelEdge: ClaudePanelEdge {
        switch self {
        case .right: return .right
        case .left: return .left
        case .top: return .top
        case .bottom: return .bottom
        }
    }
}

extension ClaudePanelEdge {
    /// The same edge, as Codenotch names it.
    var notchEdge: NotchEdge {
        switch self {
        case .right: return .right
        case .left: return .left
        case .top: return .top
        case .bottom: return .bottom
        }
    }
}

extension NotchFleet {
    /// Every notch, in a stable order: by screen, left to right, then bottom
    /// to top. The fleet keeps them in a dictionary, whose order is not.
    var claudeControllers: [NotchWindowController] {
        controllersForTesting.sorted { a, b in
            let fa = a.currentScreen()?.frame ?? .zero
            let fb = b.currentScreen()?.frame ?? .zero
            return (fa.minX, fa.minY) < (fb.minX, fb.minY)
        }
    }

    /// The notch under the pointer, else the one on the pointer's screen,
    /// else the one on the menu-bar screen, else the first
    /// (`ClaudePanelPolicy.anchorIndex`).
    func claudeAnchorController(pointer: CGPoint?) -> NotchWindowController? {
        let controllers = claudeControllers
        let index = ClaudePanelPolicy.anchorIndex(
            notchFrames: controllers.map { $0.panelFrameForTesting ?? .zero },
            screenFrames: controllers.map { $0.currentScreen()?.frame ?? .zero },
            pointer: pointer,
            mainScreen: NSScreen.screens.first?.frame
        )
        return index.map { controllers[$0] }
    }

    /// Where the panel hangs for `route`: the notch chosen by the pointer,
    /// and on it the ring the route asks for (`ClaudePanelPolicy.anchorRing`).
    /// Nil, and the panel floats, when the notch is hidden, when there is no
    /// notch, or when that ring is not in it (switched off).
    func claudeAnchor(for route: ClaudePanelRoute, sessionRingID: String?, pointer: CGPoint?,
                      notchHidden: Bool) -> ClaudeNotchAnchor? {
        guard !notchHidden, let controller = claudeAnchorController(pointer: pointer) else { return nil }
        let model = controller.model
        let rings = model.snapshots.map(\.id).filter(ClaudeBridge.ownsProvider)
        guard let ringID = ClaudePanelPolicy.anchorRing(for: route, sessionRingID: sessionRingID, ringsInNotch: rings)
        else { return nil }
        return controller.claudeAnchor(ringID: ringID)
    }
}

extension NotchWindowController {
    /// The geometry to hang the panel off ring `ringID` of this notch, as
    /// Codenotch places its own hover card for that ring: the ring's centre
    /// along the stack (`slack + ringCenter(index:) * sizeScale`, the figure
    /// `tooltipAlong` starts from) and the tail tip `tooltipInset` in from the
    /// bezel, which beside the camera already ends under the menu bar.
    func claudeAnchor(ringID: String) -> ClaudeNotchAnchor? {
        guard let frame = panelFrameForTesting,
              let visible = currentScreen()?.visibleFrame,
              let index = model.snapshots.firstIndex(where: { $0.id == ringID })
        else { return nil }
        let geometry = ClaudePanelAnchor(
            edge: model.edge.claudePanelEdge,
            notchWindowFrame: frame,
            ringAlong: model.slack + model.ringCenter(index: index) * model.sizeScale,
            tailTipInset: model.tooltipInset,
            visibleFrame: visible
        )
        return ClaudeNotchAnchor(controller: self, ringID: ringID, geometry: geometry)
    }
}
