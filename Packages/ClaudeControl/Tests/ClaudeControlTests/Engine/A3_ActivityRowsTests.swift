import Foundation
import Testing
@testable import ClaudeControl

/// Hover-card rows, one per row of the design's §5 table, and the privacy
/// rule: no prompt or assistant text in `detail` or `waitingFor`.
struct A3_ActivityRowsTests {
    let now = Date(timeIntervalSince1970: 1_800_000_000)
    let home = "/Users/me"
    var calendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }
    let locale = Locale(identifier: "en_GB")

    private func state(
        _ id: String = "s1",
        phase: SessionPhase,
        reason: NeedsInputReason? = nil,
        tasks: SessionTaskList = SessionTaskList(),
        context: Double? = nil,
        background: Int = 0
    ) -> SessionState {
        var session = SessionState(sessionId: id, cwd: "/Users/me/code/acme-web", phase: phase)
        session.applyTitle("Fix the login loop", source: .hook)
        session.accountId = "/Users/me/.claude-work"
        session.needsInputReason = reason
        session.tasks = tasks
        session.contextUsedPercent = context
        session.backgroundTaskCount = background
        session.turnStartedAt = now.addingTimeInterval(-600)
        session.lastActivity = now.addingTimeInterval(-60)
        session.pid = 4242
        // Private text that must never reach a row.
        session.lastAssistantMessage = "SECRET-ASSISTANT"
        session.conversationInfo = ConversationInfo(
            summary: nil, lastMessage: "SECRET-LAST", lastMessageRole: "assistant",
            lastToolName: nil, firstUserMessage: "SECRET-PROMPT", lastUserMessageDate: nil
        )
        return session
    }

    private func approval(_ tool: String, _ input: [String: AnyCodable]?, at: Date? = nil) -> SessionPhase {
        .waitingForApproval(PermissionContext(toolUseId: "toolu_1", toolName: tool, toolInput: input,
                                              receivedAt: at ?? now.addingTimeInterval(-120)))
    }

    private func row(_ session: SessionState, host: String? = "iTerm2", hit: UsageLimitHit? = nil, tool: String? = nil) -> ClaudeActivityRow {
        let summary = ClaudeHostProjections.session(session, home: home, hostApp: host, limitHit: hit, now: now,
                                                    calendar: calendar, locale: locale)
        #expect(summary.ringID == "claude-work")
        #expect(summary.pid == 4242)
        return ClaudeHostProjections.activityRow(summary, runningTool: tool, now: now)
    }

    private func expectPrivate(_ row: ClaudeActivityRow) {
        for text in [row.detail, row.waitingFor ?? ""] {
            #expect(!text.contains("SECRET"), "\(text)")
        }
    }

    // MARK: Needs input

    @Test func permissionNamesTheToolAndAShortPreview() {
        let command = "npm run test -- --watch=false auth/redirect.spec.ts --reporter=dot"
        let row = row(state(phase: approval("Bash", ["command": AnyCodable(command)])))
        #expect(row.state == .waiting)
        let waiting = row.waitingFor ?? ""
        #expect(waiting.hasPrefix("Allow Bash · npm run test"))
        // The preview is at most 40 characters.
        let preview = String(waiting.dropFirst("Allow Bash · ".count))
        #expect(preview.count <= ClaudeHostProjections.inputPreviewLength)
        #expect(preview.hasSuffix("…"))
        #expect(row.detail == "iTerm2 · acme-web")
        #expect(row.since == now.addingTimeInterval(-120))
        #expect(row.name == "Fix the login loop")
        expectPrivate(row)
    }

    @Test func mcpToolsAndFileToolsReadWell() {
        let mcp = row(state(phase: approval("mcp__github__create_issue", ["title": AnyCodable("Bug")])))
        #expect(mcp.waitingFor?.hasPrefix("Allow Github - Create Issue") == true)
        let edit = row(state(phase: approval("Edit", ["file_path": AnyCodable("/Users/me/code/acme-web/src/auth.ts")])))
        #expect(edit.waitingFor == "Allow Edit · auth.ts")
        // A permission prompt seen only as a notification has no input.
        let bare = row(state(phase: .processing, reason: .permission(tool: "")))
        #expect(bare.waitingFor == "Needs permission")
    }

    @Test func questionShowsItsHeader() {
        let input: [String: AnyCodable] = ["questions": AnyCodable([
            ["question": "SECRET-QUESTION-TEXT?", "header": "Charts", "options": [["label": "A"]]] as [String: Any],
        ])]
        let row = row(state(phase: approval("AskUserQuestion", input)))
        #expect(row.state == .waiting)
        #expect(row.waitingFor == "Question · Charts")
        expectPrivate(row)
        let noHeader = self.row(state(phase: approval("AskUserQuestion", nil)))
        #expect(noHeader.waitingFor == "Question")
    }

    @Test func planElicitationAndDialog() {
        let plan = row(state(phase: approval("ExitPlanMode", ["plan": AnyCodable("SECRET-PLAN")])))
        #expect(plan.waitingFor == "Plan ready to approve")
        expectPrivate(plan)
        let elicitation = row(state(phase: .processing, reason: .elicitation("Figma needs you to pick a file")))
        #expect(elicitation.waitingFor == "Figma needs you to pick a file")
        let dialog = row(state(phase: .processing, reason: .dialog("worker permission")))
        #expect(dialog.waitingFor == "Worker permission")
        #expect(dialog.since == now.addingTimeInterval(-60))
    }

    /// GUX-2/GUX-14: a failed turn is "Stopped", not an amber wait, and a
    /// rate limit names its window and reset.
    @Test func rateLimitNamesTheWindowItWaitsFor() {
        let limited = state(phase: .waitingForInput, reason: .error("Rate limited"))
        let resetsAt = now.addingTimeInterval(47 * 60)
        let hit = UsageLimitHit(window: .session, resetsAt: resetsAt)
        let row = row(limited, hit: hit)
        #expect(row.state == .idle)
        #expect(row.waitingFor == nil)
        let expected = ClaudeHostProjections.resetPhrase(resetsAt, now: now, calendar: calendar, locale: locale)
        #expect(row.detail == "Stopped · Rate limited (5-hour) · \(expected ?? "")")
        #expect(expected?.hasPrefix("resets ") == true)
        let weekly = self.row(limited, hit: UsageLimitHit(window: .weekly, resetsAt: resetsAt))
        #expect(weekly.detail.hasPrefix("Stopped · Rate limited (weekly) · resets "))
        // Without a known limit (nothing at 100%): just the reason.
        #expect(self.row(limited).detail == "Stopped · Rate limited")
        // Other failures never get a reset time.
        #expect(self.row(state(phase: .waitingForInput, reason: .error("Overloaded")), hit: hit).detail == "Stopped · Overloaded")
    }

    // MARK: Working, review, idle

    @Test func workingShowsTasksToolOrThinking() {
        let tasks = SampleSessions.taskList(done: 3, active: "Writing tests", pending: 4)
        let withTasks = row(state(phase: .processing, tasks: tasks, context: 42.4))
        #expect(withTasks.state == .busy)
        #expect(withTasks.detail == "3/8 · Writing tests · ctx 42%")
        #expect(withTasks.waitingFor == nil)
        #expect(withTasks.since == now.addingTimeInterval(-600))

        let tool = row(state(phase: .processing), tool: "Bash")
        #expect(tool.detail == "Bash…")
        let thinking = row(state(phase: .processing, context: 7))
        #expect(thinking.detail == "Thinking… · ctx 7%")
        expectPrivate(withTasks)
    }

    @Test func workingTasksWithAPaceSayHowLongIsLeft() {
        // 3 done at 4 minutes each; "Writing tests" started 1 minute ago; 4 more.
        let tasks = SampleSessions.taskList(done: 3, active: "Writing tests", pending: 4,
                                            minutesEach: 4, activeFor: 1, now: now)
        let summary = ClaudeHostProjections.session(state(phase: .processing, tasks: tasks, context: 42.4), home: home,
                                                    hostApp: "iTerm2", now: now, calendar: calendar, locale: locale)
        let progress = summary.tasks
        #expect(progress?.secondsPerTask == 240)
        #expect(progress?.activeSince == now.addingTimeInterval(-60))
        // 3 minutes of the active task, then 4 × 4.
        #expect(progress?.remaining(now: now) == 19.0 * 60)
        #expect(progress?.percent(now: now) == 40)
        let row = ClaudeHostProjections.activityRow(summary, now: now)
        #expect(row.detail == "3/8 · ~19m left · Writing tests · ctx 42%")
        // The summary holds no clock: a minute on, only the row's text moves.
        let later = ClaudeHostProjections.activityRow(summary, now: now.addingTimeInterval(60))
        #expect(later.detail == "3/8 · ~18m left · Writing tests · ctx 42%")
    }

    @Test func reviewWaitsUntilReviewed() {
        var done = state(phase: .waitingForInput, background: 2)
        done.completedAt = now.addingTimeInterval(-30)
        let row = row(done)
        #expect(row.state == .success)
        #expect(row.detail == "Ready for review · acme-web · 2 in background")
        #expect(row.since == now.addingTimeInterval(-30))
        expectPrivate(row)

        done.reviewedAt = now
        #expect(self.row(done).state == .idle)
    }

    @Test func idleSaysWhereItRuns() {
        let idle = row(state(phase: .idle), host: "VS Code")
        #expect(idle.state == .idle)
        #expect(idle.detail == "VS Code · acme-web")
        #expect(idle.since == now.addingTimeInterval(-60))
        #expect(row(state(phase: .idle), host: nil).detail == "acme-web")
    }

    @Test func runningToolIsTheNewestStarted() {
        var session = state(phase: .processing)
        session.toolTracker.startTool(id: "a", name: "Read")
        session.toolTracker.startTool(id: "b", name: "mcp__github__list_issues")
        #expect(ClaudeHostProjections.runningTool(session) == "Github - List Issues")
    }

    // MARK: Host apps

    @Test func hostAppNames() {
        func host(_ bundle: String?, name: String? = nil) -> HostApp {
            HostApp(pid: 10, bundleIdentifier: bundle, bundleURL: nil, name: name)
        }
        typealias P = ClaudeHostProjections
        #expect(P.hostAppName(isInTmux: true, host: host("com.googlecode.iterm2"), entrypoint: nil) == "tmux")
        #expect(P.hostAppName(isInTmux: false, host: host("com.googlecode.iterm2"), entrypoint: nil) == "iTerm2")
        #expect(P.hostAppName(isInTmux: false, host: host("com.apple.Terminal"), entrypoint: nil) == "Terminal")
        #expect(P.hostAppName(isInTmux: false, host: host("com.microsoft.VSCode"), entrypoint: nil) == "VS Code")
        #expect(P.hostAppName(isInTmux: false, host: host("com.todesktop.230313mzl4w4u92"), entrypoint: nil) == "Cursor")
        #expect(P.hostAppName(isInTmux: false, host: host("com.mitchellh.ghostty"), entrypoint: nil) == "Ghostty")
        #expect(P.hostAppName(isInTmux: false, host: host("com.cmuxterm.app"), entrypoint: nil) == "cmux")
        #expect(P.hostAppName(isInTmux: false, host: host("dev.warp.Warp-Stable", name: "Warp"), entrypoint: nil) == "Warp")
        #expect(P.hostAppName(isInTmux: false, host: nil, entrypoint: "claude-vscode") == "VS Code")
        #expect(P.hostAppName(isInTmux: false, host: nil, entrypoint: "cli") == nil)
    }

    @Test func resetPhrases() {
        typealias P = ClaudeHostProjections
        let today = now.addingTimeInterval(60 * 60)
        let later = now.addingTimeInterval(3 * 24 * 60 * 60)
        let far = now.addingTimeInterval(20 * 24 * 60 * 60)
        #expect(P.resetPhrase(nil, now: now) == nil)
        #expect(P.resetPhrase(now.addingTimeInterval(-1), now: now) == nil)
        let todayText = P.resetPhrase(today, now: now, calendar: calendar, locale: locale)
        #expect(todayText == "resets 09:00")
        let laterText = P.resetPhrase(later, now: now, calendar: calendar, locale: locale) ?? ""
        #expect(laterText.hasPrefix("resets ") && laterText.contains("08:00") && laterText.count > "resets 08:00".count)
        let farText = P.resetPhrase(far, now: now, calendar: calendar, locale: locale) ?? ""
        #expect(farText.hasPrefix("resets ") && !farText.contains(":"))
    }
}
