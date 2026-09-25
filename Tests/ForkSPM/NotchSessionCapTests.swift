import Foundation
import Testing
@testable import Codenotch

/// The fork's app-side rules that need Codenotch's own types, as Swift
/// Testing suites the Command Line Tools can run (`Scripts/spm-test.sh app`).
/// Under Xcode they build into CodenotchTests with the XCTest suite.
///
/// GUX-1: each hover card's session cap is solved for that card, not for a
/// worst-case four-window card.
@MainActor
@Suite struct NotchSessionCapTests {
    /// Five rings down the right edge of a 1512×982 MacBook display: three
    /// Claude accounts (two windows each) and two other providers.
    private func sideEdgeModel(edge: NotchEdge = .right) -> NotchViewModel {
        let model = NotchViewModel()
        let base = Fixtures.snapshots()
        let claude = base[0]
        var rings = [claude]
        for slug in ["work", "side"] {
            rings.append(ProviderSnapshot(id: "claude-\(slug)", displayName: "Claude \(slug)", glyph: .claude,
                                          fidelity: .derived, status: .ok, windows: claude.windows))
        }
        rings += base[1...2]
        model.snapshots = rings
        model.edge = edge
        model.screenSize = CGSize(width: 1512, height: 982)
        return model
    }

    @Test func aTwoWindowCardListsMoreSessionsThanTheWorstCaseBudgetAllowed() {
        let model = sideEdgeModel()
        let claude = model.snapshots[0]
        let perCard = model.sessionCap(for: claude)
        // Upstream's cap budgets every card as four windows in two groups.
        #expect(perCard > model.sessionCap)
        #expect(perCard >= 3)
    }

    @Test func everyCappedCardFitsThePanelOnEveryEdge() {
        for edge in [NotchEdge.right, .left, .top, .bottom] {
            let model = sideEdgeModel(edge: edge)
            let panel = model.panelSize
            for snapshot in model.snapshots {
                let cap = model.sessionCap(for: snapshot)
                // The tallest this card is ever drawn: cap rows plus "and N more".
                let height = NotchLayout.cardHeight(
                    windowCount: snapshot.windows.count,
                    groupCount: snapshot.windowGroupCount,
                    sessionCount: cap + 1, sessionCap: cap,
                    hasTokenUsage: snapshot.tokenUsage != nil,
                    hasPlan: snapshot.plan != nil,
                    hasResetCredits: snapshot.hasAvailableResetCredits)
                #expect(height <= model.maxCardHeight(cellCount: model.snapshots.count) + 0.5,
                        "\(edge) \(snapshot.id)")
            }
            // And the panel itself stays on the screen.
            #expect(panel.height <= model.screenSize.height + 0.5, "\(edge)")
            #expect(panel.width <= model.screenSize.width + 0.5, "\(edge)")
        }
    }

    @Test func aCardWithMoreWindowsGetsNoMoreRowsThanASmallerOne() {
        let model = sideEdgeModel()
        var big = model.snapshots[0]
        big.windows += big.windows.map { window in
            LimitWindow(id: window.id + ".extra", label: window.label, usedFraction: 0.5, resetsAt: window.resetsAt)
        }
        #expect(model.sessionCap(for: big) <= model.sessionCap(for: model.snapshots[0]))
    }

    @Test func beforeTheScreenIsKnownTheShippedDefaultHolds() {
        let model = NotchViewModel()
        model.snapshots = Fixtures.snapshots()
        #expect(model.sessionCap(for: model.snapshots[0]) == NotchLayout.defaultSessionCap)
    }
}
