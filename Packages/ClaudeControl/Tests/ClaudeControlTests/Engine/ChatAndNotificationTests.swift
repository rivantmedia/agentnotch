import Foundation
import Testing
@testable import ClaudeControl

// MARK: - Notification content

struct SessionNotificationContentTests {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    private func session(
        title: String = "Refactor the parser",
        phase: SessionPhase = .waitingForInput
    ) -> SessionState {
        var state = SessionState(sessionId: "sess-1", cwd: "/Users/me/app", phase: phase)
        state.applyTitle(title, source: .hook)
        return state
    }

    private func approval(_ tool: String, input: [String: AnyCodable]?) -> SessionPhase {
        .waitingForApproval(PermissionContext(toolUseId: "toolu_1", toolName: tool, toolInput: input, receivedAt: t0))
    }

    @Test func permissionShowsToolAndInput() {
        let state = session(phase: approval("Bash", input: ["command": AnyCodable("npm   run\n build")]))
        let content = SessionNotificationContent.needsInput(session: state, reason: .permission(tool: "Bash"), accountLabel: "work")
        #expect(content.title == "Refactor the parser needs you")
        #expect(content.subtitle == "work")
        #expect(content.body == "Approve Bash: npm run build")
        #expect(content.identifier == "spcn.needsInput.sess-1")
        #expect(content.kind.categoryIdentifier == NotificationService.needsInputCategory)
    }

    @Test func permissionFromNotificationHasNoInput() {
        let content = SessionNotificationContent.needsInput(session: session(), reason: .permission(tool: "Edit"), accountLabel: nil)
        #expect(content.body == "Approve Edit")
        #expect(content.subtitle == nil)
    }

    // Banners never quote Claude: a question by its header, a plan unread.
    @Test func questionShowsItsHeaderNotItsText() {
        let state = session(phase: approval("AskUserQuestion", input: ChatQuestionTests.input))
        let content = SessionNotificationContent.needsInput(session: state, reason: .question, accountLabel: nil)
        #expect(content.body == "Question · DB (+1 more)")
    }

    @Test func planIsNotQuoted() {
        let state = session(phase: approval("ExitPlanMode", input: ["plan": AnyCodable("\n## Plan: ship rings\n1. Do it")]))
        let content = SessionNotificationContent.needsInput(session: state, reason: .planApproval, accountLabel: nil)
        #expect(content.body == "Plan ready for approval")
    }

    @Test func errorsAndDialogsUseTheReason() {
        let error = SessionNotificationContent.needsInput(session: session(), reason: .error("Rate limited"), accountLabel: nil)
        #expect(error.body == "Rate limited")
        let dialog = SessionNotificationContent.needsInput(session: session(), reason: .dialog("permission prompt"), accountLabel: nil)
        #expect(dialog.body == "Permission prompt")
    }

    @Test func reviewNamesTheProjectNotTheLastMessage() {
        var state = session()
        state.lastAssistantMessage = "All tests pass.\n\nI split the parser into   three files."
        let content = SessionNotificationContent.readyForReview(session: state, accountLabel: nil)
        #expect(content.title == "Done: Refactor the parser")
        #expect(content.body == "Ready for review · app")
        #expect(content.identifier == "spcn.review.sess-1")
        #expect(content.kind.categoryIdentifier == NotificationService.reviewCategory)
    }

    @Test func reviewMentionsBackgroundTasks() {
        var state = session()
        state.backgroundTaskCount = 2
        #expect(SessionNotificationContent.readyForReview(session: state, accountLabel: nil).body
            == "Ready for review · app · 2 background tasks running")
        state.backgroundTaskCount = 1
        state.lastAssistantMessage = String(repeating: "x", count: 400)
        let body = SessionNotificationContent.readyForReview(session: state, accountLabel: nil).body
        #expect(body == "Ready for review · app · 1 background task running")
    }

    @Test func longTitlesAreTruncated() {
        let content = SessionNotificationContent.readyForReview(
            session: session(title: String(repeating: "word ", count: 30)),
            accountLabel: nil
        )
        #expect(content.title.count <= "Done: ".count + SessionNotificationContent.maxTitleLength)
        #expect(content.title.hasSuffix("…"))
    }

    @Test func identifiersRoundTrip() throws {
        for kind in SessionNotificationContent.Kind.allCases {
            let identifier = SessionNotificationContent.identifier(kind: kind, sessionId: "a1b2.c3")
            let parsed = try #require(SessionNotificationContent.parse(identifier: identifier))
            #expect(parsed.kind == kind)
            #expect(parsed.sessionId == "a1b2.c3")
        }
        #expect(SessionNotificationContent.parse(identifier: "review.") == nil)
        #expect(SessionNotificationContent.parse(identifier: "other.sess-1") == nil)
        #expect(SessionNotificationContent.parse(identifier: "no-dot") == nil)
    }

    /// Notifications left over from an earlier run are withdrawn unless the
    /// session is still in that state.
    @Test func staleNotificationsAreDetected() {
        typealias Content = SessionNotificationContent
        #expect(Content.stillApplies(kind: .needsInput, attention: .needsInput(.question)))
        #expect(Content.stillApplies(kind: .needsInput, attention: .needsInput(.error("Rate limited"))))
        #expect(!Content.stillApplies(kind: .needsInput, attention: .working))
        #expect(!Content.stillApplies(kind: .needsInput, attention: nil))
        #expect(Content.stillApplies(kind: .readyForReview, attention: .readyForReview))
        #expect(!Content.stillApplies(kind: .readyForReview, attention: .idle))
        #expect(!Content.stillApplies(kind: .readyForReview, attention: .needsInput(.planApproval)))
        #expect(!Content.stillApplies(kind: .readyForReview, attention: nil))
    }

    @Test func helpers() {
        #expect(SessionNotificationContent.truncated("abcdef", to: 4) == "abc…")
        #expect(SessionNotificationContent.truncated("abc", to: 4) == "abc")
        #expect(SessionNotificationContent.preview("  \n ") == nil)
    }
}
