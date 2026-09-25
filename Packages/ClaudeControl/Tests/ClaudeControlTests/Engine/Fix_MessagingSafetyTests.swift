import Foundation
import Testing
@testable import ClaudeControl

/// S1: chat typing is checked again at the last moment, and held while a
/// tool (the only thing a dialog follows) is in flight. S7: tmux types a
/// message that starts with `-` literally.
struct Fix_MessagingSafetyTests {
    private func tools(_ calls: [(String, String?)]) -> ToolTracker {
        var tracker = ToolTracker()
        for (id, agent) in calls { tracker.startTool(id: id, name: "Bash", agentId: agent) }
        return tracker
    }

    @Test func aMainSessionToolInFlightHoldsTyping() {
        #expect(MessageSafety.busyReason(attention: .working, isHookBacked: true, tools: tools([("t1", nil)])) != nil)
        // Between tools (thinking), with hooks, typing may go ahead.
        #expect(MessageSafety.busyReason(attention: .working, isHookBacked: true, tools: ToolTracker()) == nil)
        // A subagent's tool while the turn runs holds it too.
        #expect(MessageSafety.busyReason(attention: .working, isHookBacked: true, tools: tools([("t2", "agent-1")])) != nil)
        // A background agent's call after the turn ended does not.
        #expect(MessageSafety.busyReason(attention: .readyForReview, isHookBacked: true, tools: tools([("t3", "agent-1")])) == nil)
        #expect(MessageSafety.busyReason(attention: .idle, isHookBacked: true, tools: ToolTracker()) == nil)
    }

    @Test func withoutHooksTheWholeTurnHoldsTyping() {
        #expect(MessageSafety.busyReason(attention: .working, isHookBacked: false, tools: ToolTracker()) != nil)
        #expect(MessageSafety.busyReason(attention: .readyForReview, isHookBacked: false, tools: ToolTracker()) == nil)
        #expect(MessageSafety.busyReason(attention: .idle, isHookBacked: false, tools: ToolTracker()) == nil)
    }

    @Test func tmuxEndsTheOptionsBeforeTheMessage() {
        let arguments = TmuxMessageSender.typeArguments(pane: "main:1.0", message: "-R")
        #expect(arguments == ["send-keys", "-t", "main:1.0", "-l", "--", "-R"])
        #expect(TmuxMessageSender.enterArguments(pane: "main:1.0") == ["send-keys", "-t", "main:1.0", "Enter"])
    }

    /// The precondition runs after the script queued ahead has finished,
    /// right before osascript, and a reason stops the script.
    @Test func aQueuedScriptIsCheckedAgainOnceItsTurnComes() async throws {
        guard FileManager.default.isExecutableFile(atPath: "/usr/bin/osascript") else { return }
        let slow = "delay 0.4\nreturn \"\(TerminalScript.successMarker)\""
        let start = Date()
        let checkedAt = Clock()
        async let first = TerminalScriptRunner.shared.run(slow, label: "fix test slow")
        try await Task.sleep(for: .milliseconds(50))
        let second = await TerminalScriptRunner.shared.run("return \"\(TerminalScript.successMarker)\"", label: "fix test gated",
                                                           precondition: { await checkedAt.mark(); return "a dialog opened" })
        #expect(await first == .succeeded)
        #expect(second == .blocked("a dialog opened"))
        let checked = try #require(await checkedAt.date)
        #expect(checked.timeIntervalSince(start) >= 0.35)

        let clear = await TerminalScriptRunner.shared.run("return \"\(TerminalScript.successMarker)\"", label: "fix test clear",
                                                          precondition: { nil })
        #expect(clear == .succeeded)
    }

    @MainActor
    @Test func aSendWaitsOutAToolThenGoesAhead() async {
        let answers = Answers(["Claude is running a tool", "Claude is running a tool", nil])
        // A generous limit: the main actor is shared with the UI suites.
        let reason = await SessionMessenger.waitUntilFree(
            sessionId: "s", tty: "ttys001", limit: 120, poll: 0.01,
            recheck: { _, _ in await answers.next() }, isBusy: { _ in true })
        #expect(reason == nil)
    }

    @MainActor
    @Test func aSendThatStaysBusyIsRefusedAfterTheLimit() async {
        let start = Date()
        let reason = await SessionMessenger.waitUntilFree(
            sessionId: "s", tty: "ttys001", limit: 0.2, poll: 0.02,
            recheck: { _, _ in "Claude is running a tool" }, isBusy: { _ in true })
        #expect(reason == "Claude is running a tool")
        #expect(Date().timeIntervalSince(start) >= 0.2)
    }

    @MainActor
    @Test func aDialogRefusesAtOnce() async {
        let start = Date()
        let reason = await SessionMessenger.waitUntilFree(
            sessionId: "s", tty: "ttys001", limit: 120, poll: 0.02,
            recheck: { _, _ in "Answer the permission prompt first" }, isBusy: { _ in false })
        #expect(reason == "Answer the permission prompt first")
        #expect(Date().timeIntervalSince(start) < 60)
    }

    @MainActor
    @Test func anUnknownSessionIsNeverTypedInto() async {
        #expect(await SessionMessenger.recheck(sessionId: "no-such-session-\(UUID())", tty: "ttys001") != nil)
    }
}

private actor Clock {
    var date: Date?
    func mark() { date = Date() }
}

private actor Answers {
    private var queue: [String?]
    init(_ queue: [String?]) { self.queue = queue }
    func next() -> String? { queue.isEmpty ? nil : queue.removeFirst() }
}
