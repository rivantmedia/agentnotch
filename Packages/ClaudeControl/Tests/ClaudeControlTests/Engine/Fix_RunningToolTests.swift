import Foundation
import Testing
@testable import ClaudeControl

/// BHV-4: the running tool is part of the published session summary, so a
/// tool starting or ending changes `hub.sessions` (which the notch feed
/// follows) and hover rows read "Bash…" rather than "Thinking…".
struct Fix_RunningToolTests {
    private func working(tool: String?) -> SessionState {
        var state = SessionState(sessionId: "s1", cwd: "/tmp/p")
        state.phase = .processing
        if let tool { state.toolTracker.startTool(id: "t1", name: tool) }
        return state
    }

    @Test func aToolStartingOrEndingChangesTheSummary() {
        let thinking = ClaudeHostProjections.session(working(tool: nil), home: "/Users/x")
        let bash = ClaudeHostProjections.session(working(tool: "Bash"), home: "/Users/x")
        #expect(thinking.runningTool == nil)
        #expect(bash.runningTool == "Bash")
        #expect(thinking != bash)
        #expect(ClaudeHostProjections.activityRow(bash).detail.hasPrefix("Bash…"))
        #expect(ClaudeHostProjections.activityRow(thinking).detail.hasPrefix("Thinking…"))
    }

    @Test func onlyAWorkingSessionNamesItsTool() {
        var idle = working(tool: "Read")
        idle.phase = .idle
        #expect(ClaudeHostProjections.session(idle, home: "/Users/x").runningTool == nil)
    }
}
