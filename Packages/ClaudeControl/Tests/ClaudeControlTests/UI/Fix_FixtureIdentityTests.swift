import Foundation
import Testing
@testable import ClaudeControl

/// CS-2 / GUX-17 / CS-14: fixture lists never share a session id (the list is
/// keyed by it, so a shared id drew one row twice and hid another), and each
/// chat sheet tells its own session's story.
@MainActor
struct Fix_FixtureIdentityTests {
    private func expectUnique(_ sessions: [SessionState], _ name: String) {
        let ids = sessions.map(\.sessionId)
        #expect(Set(ids).count == ids.count, "\(name): \(ids.filter { id in ids.filter { $0 == id }.count > 1 })")
    }

    @Test func everyFixtureListHasUniqueIds() {
        expectUnique(SampleSessions.all(), "SampleSessions.all")
        expectUnique(UIFixtures.everyState(), "everyState")
        expectUnique(UIFixtures.needsYou(), "needsYou")
        expectUnique(UIFixtures.regular(), "regular")
        expectUnique(UIFixtures.density(), "density")
        #expect(UIFixtures.density().count == 25)
    }

    @Test func theNetworkDialogIsListed() {
        #expect(UIFixtures.everyState().contains { $0.sessionId == "needs-dialog-network" })
        #expect(UIFixtures.everyState().contains { $0.sessionId == "needs-dialog" })
    }

    @Test func eachChatSheetHasItsOwnStory() {
        let composer = UIFixtures.chatHistory(for: UIFixtures.reviewDone())
        #expect(composer.allSatisfy { $0.id.hasPrefix("review-tests-") })
        guard case .assistant(let last)? = composer.last?.type else {
            Issue.record("the conversation should end with Claude's reply")
            return
        }
        #expect(last == UIFixtures.reviewDone().lastAssistantMessage)
        #expect(!UIFixtures.chatHistory(for: SampleSessions.plan()).isEmpty)
        #expect(UIFixtures.chatHistory(for: SampleSessions.plan()).first?.id.hasPrefix("needs-plan-") == true)
    }
}
