import Foundation
import Testing
@testable import ClaudeControl

/// What the notch counts: the badges, the folded pill's dots and the hold
/// stand only for sessions the notch shows. A ring switched off in the notch
/// counts for nothing there, and a session whose account is not known yet
/// counts on the default ring, where its row is.
struct C_NotchCountsTests {
    typealias Policy = ClaudeAttentionPolicy

    private let counts: [String: ClaudeAttentionCounts] = [
        "claude": ClaudeAttentionCounts(needsYou: 1, review: 1),
        "claude-work": ClaudeAttentionCounts(needsYou: 2, working: 3),
        "claude-off": ClaudeAttentionCounts(needsYou: 4),
        "claude-dir-1a2b3c4d": ClaudeAttentionCounts(needsYou: 1, working: 1),
    ]
    private let rings = ["claude", "claude-work", "claude-off"]

    private func routed(shown: Set<String>?) -> [String: ClaudeAttentionCounts] {
        Policy.notchValues(counts, rings: rings, shown: shown, combine: Policy.combined)
    }

    @Test func beforeAnythingIsKnownEverythingPassesThrough() {
        #expect(routed(shown: nil) == counts)
    }

    /// The switched-off ring's sessions are not counted, and the unknown
    /// account's are counted on the default ring.
    @Test func switchedOffRingsDropAndUnknownAccountsFoldIntoTheDefaultRing() {
        let notch = routed(shown: ["claude", "claude-work", "claude-dir-1a2b3c4d"])
        #expect(notch == [
            "claude": ClaudeAttentionCounts(needsYou: 2, review: 1, working: 1),
            "claude-work": ClaudeAttentionCounts(needsYou: 2, working: 3),
        ])
        #expect(Policy.total(notch.values) == ClaudeAttentionCounts(needsYou: 4, review: 1, working: 4))
        // The switched-off ring's four prompts would otherwise put an amber
        // dot on the pill (and hold the notch open) for a ring it doesn't show.
        #expect(Policy.total(counts.values).needsYou == 8)
    }

    /// With the default ring off, nothing shows the unknown account's rows,
    /// so nothing counts them either (the feed leaves them out of `shown`).
    @Test func withTheDefaultRingOffUnknownAccountsCountNowhere() {
        let notch = routed(shown: ["claude-work"])
        #expect(notch == ["claude-work": ClaudeAttentionCounts(needsYou: 2, working: 3)])
    }

    @Test func nothingShownCountsNothing() {
        #expect(routed(shown: []).isEmpty)
        #expect(Policy.total(routed(shown: []).values) == .zero)
    }

    /// The "just finished" deadlines fold the same way, keeping the later one.
    @Test func deadlinesFoldToTheLatest() {
        let t0 = Date(timeIntervalSince1970: 1_800_000_000)
        let deadlines: [String: Date] = [
            "claude": t0, "claude-dir-1a2b3c4d": t0.addingTimeInterval(60), "claude-off": t0.addingTimeInterval(90),
        ]
        let notch = Policy.notchValues(deadlines, rings: rings, shown: ["claude", "claude-dir-1a2b3c4d"], combine: max)
        #expect(notch == ["claude": t0.addingTimeInterval(60)])
    }

    @Test func countsAddFieldByField() {
        let sum = Policy.combined(ClaudeAttentionCounts(needsYou: 1, review: 2, working: 3, idle: 4),
                                  ClaudeAttentionCounts(needsYou: 10, review: 20, working: 30, idle: 40))
        #expect(sum == ClaudeAttentionCounts(needsYou: 11, review: 22, working: 33, idle: 44))
        #expect(Policy.total([ClaudeAttentionCounts]()) == .zero)
    }
}
