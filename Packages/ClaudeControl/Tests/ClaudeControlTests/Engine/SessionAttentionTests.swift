import Foundation
import Testing
@testable import ClaudeControl

struct SessionAttentionTests {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    private func approval(_ tool: String) -> SessionPhase {
        .waitingForApproval(PermissionContext(toolUseId: "toolu_1", toolName: tool, toolInput: nil, receivedAt: t0))
    }

    private func derive(
        _ phase: SessionPhase,
        reason: NeedsInputReason? = nil,
        background: Int = 0,
        completed: Date? = nil,
        reviewed: Date? = nil
    ) -> SessionAttention {
        SessionAttention.derive(
            phase: phase,
            needsInputReason: reason,
            backgroundTaskCount: background,
            completedAt: completed,
            reviewedAt: reviewed
        )
    }

    @Test func approvalsMapToTheirReason() {
        #expect(derive(approval("Bash")) == .needsInput(.permission(tool: "Bash")))
        #expect(derive(approval("AskUserQuestion")) == .needsInput(.question))
        #expect(derive(approval("ExitPlanMode")) == .needsInput(.planApproval))
    }

    @Test func approvalWinsOverEverythingElse() {
        let attention = derive(approval("Edit"), reason: .error("Rate limited"), completed: t0)
        #expect(attention == .needsInput(.permission(tool: "Edit")))
    }

    @Test func explicitReasonWinsOverWorkingAndReview() {
        #expect(derive(.processing, reason: .dialog("input needed")) == .needsInput(.dialog("input needed")))
        #expect(derive(.waitingForInput, reason: .error("Rate limited"), completed: t0) == .needsInput(.error("Rate limited")))
        #expect(derive(.idle, reason: .elicitation("Pick a repo")) == .needsInput(.elicitation("Pick a repo")))
    }

    @Test func processingAndCompactingAreWorking() {
        #expect(derive(.processing) == .working)
        #expect(derive(.compacting, completed: t0) == .working)
    }

    @Test func finishedTurnWithBackgroundTasksIsReadyForReview() {
        #expect(derive(.waitingForInput, background: 2, completed: t0) == .readyForReview)
    }

    @Test func completedAndUnreviewedIsReadyForReview() {
        #expect(derive(.waitingForInput, completed: t0) == .readyForReview)
        #expect(derive(.idle, completed: t0) == .readyForReview)
        #expect(derive(.waitingForInput, completed: t0, reviewed: t0.addingTimeInterval(-60)) == .readyForReview)
    }

    @Test func reviewedAfterCompletionIsIdle() {
        #expect(derive(.waitingForInput, completed: t0, reviewed: t0.addingTimeInterval(1)) == .idle)
        #expect(derive(.waitingForInput, completed: t0, reviewed: t0) == .idle)
        #expect(derive(.waitingForInput) == .idle)
        #expect(derive(.idle) == .idle)
    }

    @Test func bucketsSortNeedsInputFirst() {
        let ordered = [SessionAttention.idle, .working, .readyForReview, .needsInput(.question)]
            .sorted { $0.bucket < $1.bucket }
        #expect(ordered == [.needsInput(.question), .readyForReview, .working, .idle])
    }

    @Test func stopErrorsAreHumanized() {
        #expect(NeedsInputReason.humanizedStopError("rate_limit") == "Rate limited")
        #expect(NeedsInputReason.humanizedStopError("overloaded") == "Overloaded")
        #expect(NeedsInputReason.humanizedStopError("some_new_error") == "Some new error")
        #expect(NeedsInputReason.humanizedStopError(nil) == "Turn failed")
    }

    @Test func permissionPromptToolNameIsExtracted() {
        #expect(SessionStore.toolName(fromPermissionPrompt: "Claude needs your permission to use Bash") == "Bash")
        #expect(SessionStore.toolName(fromPermissionPrompt: "Claude needs your permission to use mcp__github__create_issue.") == "mcp__github__create_issue")
        #expect(SessionStore.toolName(fromPermissionPrompt: "Waiting for input") == nil)
    }
}
