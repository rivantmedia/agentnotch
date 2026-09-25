import ClaudeControl
import SwiftUI

/// What the sessions panel's chrome draws, set by `ClaudePanelController`
/// each time it places the window.
///
/// Fork-only file (design §7). Owned by WP-D.
@MainActor
final class ClaudePanelChromeModel: ObservableObject {
    /// Which side of the notch the card is on, the way Codenotch's hover card
    /// names it; nil for a panel floating with no notch to point at.
    @Published var direction: NotchEdge.TooltipDirection?
    /// The tail's offset from the card's centre along the card's edge
    /// (`ClaudePanelPlacement.tailOffset`, SwiftUI's axes).
    @Published var tailOffset: CGFloat = 0
    @Published var surfaceStyle: NotchSurfaceStyle
    /// The notch's accent colour, for the panel's controls.
    @Published var accent: Color

    init(direction: NotchEdge.TooltipDirection? = nil, tailOffset: CGFloat = 0,
         surfaceStyle: NotchSurfaceStyle = .glass, accent: Color = .accentColor) {
        self.direction = direction
        self.tailOffset = tailOffset
        self.surfaceStyle = surfaceStyle
        self.accent = accent
    }
}

/// The sessions panel's card: Codenotch's hover-card silhouette
/// (`TooltipSilhouette`, card and tail as one outline) painted with the same
/// Liquid Glass or solid black the notch uses, and the panel's content laid
/// out inside the card.
///
/// The window is exactly the card plus its tail (`ClaudePanelGeometry`), so
/// the card is whatever of this view's size the tail does not take.
///
/// Fork-only file (design §7). Owned by WP-D. `ClaudePanelChrome` is the
/// package's geometry struct; this is the view.
struct ClaudePanelChromeView<Content: View>: View {
    @ObservedObject var chrome: ClaudePanelChromeModel
    let content: Content

    @Environment(\.codenotchReduceTransparency) private var reduceTransparency
    @Environment(\.colorScheme) private var colorScheme

    init(chrome: ClaudePanelChromeModel, @ViewBuilder content: () -> Content) {
        self.chrome = chrome
        self.content = content()
    }

    /// Reduce transparency means no see-through chrome, as for the hover card.
    private var glassy: Bool { chrome.surfaceStyle.isGlass && !reduceTransparency }

    var body: some View {
        GeometryReader { proxy in
            let bounds = CGRect(origin: .zero, size: proxy.size)
            let card = Self.cardRect(in: bounds, direction: chrome.direction)
            ZStack(alignment: .topLeading) {
                surface(in: bounds)
                content
                    .frame(width: card.width, height: card.height, alignment: .top)
                    .clipShape(RoundedRectangle(cornerRadius: NotchLayout.cardCorner, style: .circular))
                    .offset(x: card.minX, y: card.minY)
            }
            .frame(width: bounds.width, height: bounds.height, alignment: .topLeading)
        }
        .environment(\.notchSurfaceStyle, chrome.surfaceStyle)
        .environment(\.tooltipSecondaryInk, TooltipGlassContrast.secondaryInk(
            surfaceStyle: chrome.surfaceStyle, colorScheme: colorScheme, reduceTransparency: reduceTransparency))
        .claudeControlTheme(.codenotch(surfaceStyle: chrome.surfaceStyle, colorScheme: colorScheme,
                                       reduceTransparency: reduceTransparency, accent: chrome.accent))
        .tint(chrome.accent)
    }

    /// The card within the window: everything but the tail's strip.
    static func cardRect(in bounds: CGRect, direction: NotchEdge.TooltipDirection?) -> CGRect {
        guard let direction else { return bounds }
        let tail = TooltipTail.size(for: direction)
        switch direction {
        case .leading:  return CGRect(x: 0, y: 0, width: bounds.width - tail.width, height: bounds.height)
        case .trailing: return CGRect(x: tail.width, y: 0, width: bounds.width - tail.width, height: bounds.height)
        case .down:     return CGRect(x: 0, y: tail.height, width: bounds.width, height: bounds.height - tail.height)
        case .up:       return CGRect(x: 0, y: 0, width: bounds.width, height: bounds.height - tail.height)
        }
    }

    /// The card and tail as one outline, or a plain rounded card when there
    /// is no tail. The same outline masks the glass and the solid fill, so
    /// there is no seam where the tail leaves the card.
    private func outline(in bounds: CGRect) -> AnyShape {
        if let direction = chrome.direction {
            return AnyShape(TooltipSilhouette(direction: direction, tailOffset: chrome.tailOffset))
        }
        return AnyShape(RoundedRectangle(cornerRadius: NotchLayout.cardCorner, style: .circular))
    }

    @ViewBuilder
    private func surface(in bounds: CGRect) -> some View {
        let shape = outline(in: bounds)
        ZStack {
            if glassy {
                // `isGlass` is only ever true where `glassEffect` exists; the
                // availability check is what tells the compiler so.
                if #available(macOS 26.0, *) {
                    if let dim = TooltipGlassContrast.dim(surfaceStyle: chrome.surfaceStyle,
                                                          colorScheme: colorScheme,
                                                          reduceTransparency: reduceTransparency) {
                        shape.fill(dim)
                    }
                    Color.clear.glassEffect(chrome.surfaceStyle.glass, in: shape)
                }
            } else {
                shape.fill(Palette.card)
                if reduceTransparency {
                    shape.stroke(Palette.ringTrack, lineWidth: 1)
                }
            }
        }
        .frame(width: bounds.width, height: bounds.height)
    }
}
