import Foundation
import Testing
@testable import ClaudeControl

/// BHV-5: the list is one run of items; a row is identified by its session
/// alone, so moving to another section keeps its view.
struct Fix_SessionListIdentityTests {
    private func row(_ id: String, review: Bool) -> SessionRowModel {
        var session = SessionState(sessionId: id, cwd: "/tmp/\(id)")
        session.phase = review ? .waitingForInput : .processing
        if review { session.completedAt = UIFixtures.now }
        return SessionRowModel.make(session, account: nil, rateLimit: nil, canFocus: true, now: UIFixtures.now, home: "/Users/x")
    }

    @Test func aRowKeepsItsIdentityAcrossSections() {
        let working = SessionListLayout(sections: [
            .init(bucket: .working, rows: [row("a", review: false), row("b", review: false)], isCollapsed: false, summary: ""),
        ], isCompact: false)
        let moved = SessionListLayout(sections: [
            .init(bucket: .readyForReview, rows: [row("a", review: true)], isCollapsed: false, summary: ""),
            .init(bucket: .working, rows: [row("b", review: false)], isCollapsed: false, summary: ""),
        ], isCompact: false)
        #expect(working.items.map(\.id) == ["header-\(AttentionBucket.working.rawValue)", "row-a", "row-b"])
        let movedIds = moved.items.map(\.id)
        #expect(movedIds.contains("row-a") && movedIds.contains("row-b"))
        #expect(Set(movedIds).count == movedIds.count)
    }

    @Test func aFoldedSectionIsOneItemAndItsRowsAreNotListed() {
        let layout = SessionListLayout(sections: [
            .init(bucket: .idle, rows: [row("x", review: false)], isCollapsed: true, summary: "x"),
        ], isCompact: false)
        #expect(layout.items.map(\.id) == ["header-\(AttentionBucket.idle.rawValue)", "folded-\(AttentionBucket.idle.rawValue)"])
    }

    @Test func onlyTheFirstRowOfASectionHasNoGapAbove() {
        let layout = SessionListLayout(sections: [
            .init(bucket: .working, rows: [row("a", review: false), row("b", review: false)], isCollapsed: false, summary: ""),
        ], isCompact: false)
        let firsts = layout.items.compactMap { item -> Bool? in
            if case .row(_, let isFirst) = item { return isFirst }
            return nil
        }
        #expect(firsts == [true, false])
    }
}
