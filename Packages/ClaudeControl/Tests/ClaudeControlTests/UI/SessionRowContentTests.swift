import Foundation
import Testing
@testable import ClaudeControl

/// Wording, time labels and inline actions of session rows.
struct SessionRowContentTests {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func session(
        phase: SessionPhase = .idle,
        reason: NeedsInputReason? = nil,
        lastMessage: String? = nil,
        role: String? = nil,
        tool: String? = nil
    ) -> SessionState {
        var session = SessionState(
            sessionId: "s1",
            cwd: "/Users/me/code/acme",
            phase: phase,
            conversationInfo: ConversationInfo(
                summary: nil,
                lastMessage: lastMessage,
                lastMessageRole: role,
                lastToolName: tool,
                firstUserMessage: nil,
                lastUserMessageDate: nil
            ),
            lastActivity: now.addingTimeInterval(-7_200)
        )
        session.needsInputReason = reason
        return session
    }

    private func approval(
        _ tool: String,
        input: [String: AnyCodable]? = nil,
        suggestions: [AnyCodable]? = nil,
        receivedAgo: TimeInterval = 90
    ) -> SessionPhase {
        .waitingForApproval(
            PermissionContext(
                toolUseId: "toolu_1",
                toolName: tool,
                toolInput: input,
                receivedAt: now.addingTimeInterval(-receivedAgo),
                permissionSuggestions: suggestions
            )
        )
    }

    /// Tool input as it arrives from the hook: decoded JSON.
    private func decodedInput(_ json: String) throws -> [String: AnyCodable] {
        try JSONDecoder().decode([String: AnyCodable].self, from: Data(json.utf8))
    }

    // MARK: Glyph

    @Test func glyphFollowsAttention() {
        #expect(SessionRowContent.glyph(for: session(phase: approval("Bash"))) == .needsInput)
        #expect(SessionRowContent.glyph(for: session(reason: .error("Overloaded"))) == .error)
        var review = session(phase: .waitingForInput)
        review.completedAt = now
        #expect(SessionRowContent.glyph(for: review) == .review)
        #expect(SessionRowContent.glyph(for: session(phase: .processing)) == .working)
        #expect(SessionRowContent.glyph(for: session()) == .idle)
    }

    // MARK: Elapsed

    @Test func elapsedDependsOnTheBucket() {
        // Needs you: time since the request arrived.
        #expect(SessionRowContent.elapsed(for: session(phase: approval("Bash", receivedAgo: 150)), now: now) == "2m")

        // Working: time since the prompt.
        var working = session(phase: .processing)
        working.turnStartedAt = now.addingTimeInterval(-(3_600 + 12 * 60))
        #expect(SessionRowContent.elapsed(for: working, now: now) == "1h 12m")
        #expect(SessionRowContent.elapsed(for: session(phase: .processing), now: now) == nil)

        // Review: when it finished.
        var review = session(phase: .waitingForInput)
        review.completedAt = now.addingTimeInterval(-5 * 60)
        #expect(SessionRowContent.elapsed(for: review, now: now) == "5m ago")
        review.completedAt = now.addingTimeInterval(-20)
        #expect(SessionRowContent.elapsed(for: review, now: now) == "just now")

        // Idle: last activity.
        #expect(SessionRowContent.elapsed(for: session(), now: now) == "2h ago")
    }

    // MARK: Detail line

    @Test func permissionShowsToolAndInput() {
        let detail = SessionRowContent.detail(
            for: session(phase: approval("Bash", input: ["command": AnyCodable("npm test\n  --watch=false")])),
            now: now
        )
        // The whole command, line breaks kept: nothing hides past the edge.
        #expect(detail == .tool(name: "Bash", input: "npm test\n  --watch=false", isAttention: true))
    }

    @Test func permissionPreviewShowsFullPathsUnderHome() {
        let edit = SessionRowContent.detail(
            for: session(phase: approval("Edit", input: ["file_path": AnyCodable("/Users/me/code/acme/src/app.ts")])),
            now: now, home: "/Users/me"
        )
        #expect(edit == .tool(name: "Edit", input: "~/code/acme/src/app.ts", isAttention: true))
        #expect(PermissionPreview.text(toolName: "WebFetch", toolInput: ["url": AnyCodable("https://x.dev"), "prompt": AnyCodable("p")], home: "/")
            == "https://x.dev")
        #expect(PermissionPreview.text(toolName: "Bash", toolInput: nil, home: "/") == nil)
    }

    @Test func requestsTooLongForTheRowGoToTheChat() {
        #expect(!PermissionPreview.isTooLongToReviewInline("npm test"))
        #expect(!PermissionPreview.isTooLongToReviewInline(nil))
        #expect(PermissionPreview.isTooLongToReviewInline(String(repeating: "x", count: 201)))
        #expect(PermissionPreview.isTooLongToReviewInline("a\nb\nc\nd\ne"))
        let long = session(phase: approval("Bash", input: ["command": AnyCodable(String(repeating: "rm -rf build && ", count: 20))]))
        guard case .permission(_, _, let needsReview) = SessionRowContent.primaryActions(for: long) else {
            Issue.record("expected a permission")
            return
        }
        #expect(needsReview)
    }

    @Test func permissionSeenOnlyInTheTerminalSaysSo() {
        let detail = SessionRowContent.detail(for: session(reason: .permission(tool: "Edit")), now: now)
        #expect(detail == .tool(name: "Edit", input: "waiting in the terminal", isAttention: true))
        let unnamed = SessionRowContent.detail(for: session(reason: .permission(tool: "")), now: now)
        #expect(unnamed == .tool(name: "Permission", input: "waiting in the terminal", isAttention: true))
    }

    @Test func mcpToolNamesAreFormatted() {
        let detail = SessionRowContent.detail(for: session(phase: approval("mcp__github__create_issue")), now: now)
        guard case .tool(let name, _, true) = detail else {
            Issue.record("expected a tool detail, got \(detail)")
            return
        }
        #expect(name == MCPToolFormatter.formatToolName("mcp__github__create_issue"))
        #expect(name != "mcp__github__create_issue")
    }

    @Test func questionShowsTheFirstQuestion() throws {
        let input = try decodedInput(#"{"questions":[{"question":"Which library?","options":[{"label":"A"},{"label":"B"}]}]}"#)
        let detail = SessionRowContent.detail(for: session(phase: approval("AskUserQuestion", input: input)), now: now)
        #expect(detail == .prompt(label: "Asks", text: "Which library?"))
        let empty = SessionRowContent.detail(for: session(phase: approval("AskUserQuestion")), now: now)
        #expect(empty == .prompt(label: "Asks", text: "A question for you"))
    }

    @Test func planDialogAndElicitation() {
        #expect(SessionRowContent.detail(for: session(phase: approval("ExitPlanMode")), now: now)
            == .text("Plan ready for approval", tone: .primary, lineLimit: 1))
        #expect(SessionRowContent.detail(for: session(reason: .dialog("permission prompt")), now: now)
            == .text("Permission prompt", tone: .primary, lineLimit: 1))
        #expect(SessionRowContent.detail(for: session(reason: .elicitation("Pick a repo")), now: now)
            == .text("Pick a repo", tone: .primary, lineLimit: 1))
    }

    @Test func rateLimitNamesTheWindowThatIsSpent() {
        let limited = session(reason: .error(NeedsInputReason.humanizedStopError("rate_limit")))
        let fiveHour = RateLimitReset(window: "5-hour limit", resetsAt: now.addingTimeInterval(47 * 60))
        #expect(SessionRowContent.detail(for: limited, rateLimit: fiveHour, now: now)
            == .text("Rate limited · 5-hour limit resets in 47m", tone: .error, lineLimit: 1))
        // Nothing known to be spent: just the error.
        #expect(SessionRowContent.detail(for: limited, now: now) == .text("Rate limited", tone: .error, lineLimit: 1))
        // Other errors never get a reset time.
        let overloaded = session(reason: .error("Overloaded"))
        #expect(SessionRowContent.detail(for: overloaded, rateLimit: fiveHour, now: now)
            == .text("Overloaded", tone: .error, lineLimit: 1))
    }

    @Test func theSpentWindowIsTheWeekWhenTheWeekIsOut() {
        let week = now.addingTimeInterval(3 * 86_400)
        let reading = ClaudeRingReading(windows: [
            .init(id: "session", usedFraction: 0.62, resetsAt: now.addingTimeInterval(3_600)),
            .init(id: "weekly_all", usedFraction: 1.0, resetsAt: week),
        ], status: .ok)
        #expect(RateLimitReset.current(in: reading, now: now) == RateLimitReset(window: "weekly limit", resetsAt: week))

        // Both spent: the later reset is the real wait.
        let both = ClaudeRingReading(windows: [
            .init(id: "session", usedFraction: 1.1, resetsAt: now.addingTimeInterval(600)),
            .init(id: "weekly_opus", usedFraction: 1.0, resetsAt: week),
        ], status: .ok)
        #expect(RateLimitReset.current(in: both, now: now) == RateLimitReset(window: "Opus weekly limit", resetsAt: week))

        // Nothing at 100%, a reset already past, or money windows: no claim.
        let fine = ClaudeRingReading(windows: [.init(id: "session", usedFraction: 0.92, resetsAt: week)], status: .ok)
        #expect(RateLimitReset.current(in: fine, now: now) == nil)
        let past = ClaudeRingReading(windows: [.init(id: "session", usedFraction: 1, resetsAt: now.addingTimeInterval(-1))], status: .ok)
        #expect(RateLimitReset.current(in: past, now: now) == nil)
        #expect(RateLimitReset.current(in: nil, now: now) == nil)
    }

    @Test func workingPrefersTheActiveTaskThenTheToolThenThinking() {
        var withTask = session(phase: .processing, lastMessage: "ls", role: "tool", tool: "Bash")
        withTask.tasks.todosReplaced([
            (content: "Write tests", status: "completed", activeForm: nil),
            (content: "Run tests", status: "in_progress", activeForm: "Running tests"),
        ])
        #expect(SessionRowContent.detail(for: withTask, now: now) == .text("Running tests", tone: .secondary, lineLimit: 1))

        let withTool = session(phase: .processing, lastMessage: "src/**/*.swift", role: "tool", tool: "Glob")
        #expect(SessionRowContent.detail(for: withTool, now: now) == .tool(name: "Glob", input: "src/**/*.swift", isAttention: false))

        let thinking = session(phase: .processing, lastMessage: "Sure, let me look", role: "assistant")
        #expect(SessionRowContent.detail(for: thinking, now: now) == .text("Thinking…", tone: .secondary, lineLimit: 1))

        #expect(SessionRowContent.detail(for: session(phase: .compacting), now: now)
            == .text("Compacting context…", tone: .secondary, lineLimit: 1))
    }

    @Test func reviewShowsTheLastReplyOnTwoLines() {
        var review = session(phase: .waitingForInput, lastMessage: "short", role: "assistant")
        review.completedAt = now
        review.lastAssistantMessage = "All done.\n\nTests pass."
        #expect(SessionRowContent.detail(for: review, now: now) == .text("All done. Tests pass.", tone: .primary, lineLimit: 2))
        review.lastAssistantMessage = "   "
        #expect(SessionRowContent.detail(for: review, now: now) == .text("short", tone: .primary, lineLimit: 2))
        review.conversationInfo = ConversationInfo(summary: nil, lastMessage: nil, lastMessageRole: nil, lastToolName: nil, firstUserMessage: nil, lastUserMessageDate: nil)
        #expect(SessionRowContent.detail(for: review, now: now) == .text("Finished", tone: .primary, lineLimit: 2))
    }

    @Test func idleShowsTheLastMessage() {
        #expect(SessionRowContent.detail(for: session(lastMessage: "thanks", role: "user"), now: now)
            == .text("You: thanks", tone: .secondary, lineLimit: 1))
        #expect(SessionRowContent.detail(for: session(lastMessage: "npm outdated", role: "tool", tool: "Bash"), now: now)
            == .tool(name: "Bash", input: "npm outdated", isAttention: false))
        #expect(SessionRowContent.detail(for: session(lastMessage: "Done.", role: "assistant"), now: now)
            == .text("Done.", tone: .secondary, lineLimit: 1))
        #expect(SessionRowContent.detail(for: session(), now: now) == .text("No messages yet", tone: .secondary, lineLimit: 1))
    }

    // MARK: Actions

    @Test func permissionActionsCarryTheirRequestAndOfferAlwaysOnlyForNarrowRules() {
        #expect(SessionRowContent.primaryActions(for: session(phase: approval("Bash")))
            == .permission(toolUseId: "toolu_1", always: nil, needsReview: false))

        let local = AnyCodable(["type": "addRules", "rules": [["toolName": "Bash", "ruleContent": "npm test:*"]],
                                "behavior": "allow", "destination": "localSettings"] as [String: Any])
        guard case let .permission(id, always?, _) = SessionRowContent.primaryActions(for: session(phase: approval("Bash", suggestions: [local]))) else {
            Issue.record("expected Always")
            return
        }
        #expect(id == "toolu_1")
        #expect(always == AlwaysAllowOffer(description: "Don't ask again for Bash(npm test:*) in this project (just you)", isInline: true))

        // A rule for every project, or a mode switch, is offered in the chat
        // only, next to its whole description.
        let everywhere = AnyCodable(["type": "addRules", "rules": [["toolName": "Bash"]], "destination": "userSettings"] as [String: Any])
        #expect(SessionRowContent.alwaysAllowOffer([everywhere])?.isInline == false)
        let mode = AnyCodable(["type": "setMode", "mode": "acceptEdits", "destination": "session"] as [String: Any])
        #expect(SessionRowContent.alwaysAllowOffer([mode]) == AlwaysAllowOffer(description: "Switch to accept-edits mode for this session", isInline: false))
        #expect(SessionRowContent.alwaysAllowOffer([]) == nil)
        #expect(SessionRowContent.alwaysAllowOffer(nil) == nil)
    }

    @Test func planTerminalOnlyAndNothingToAnswer() {
        #expect(SessionRowContent.primaryActions(for: session(phase: approval("ExitPlanMode"))) == .plan(toolUseId: "toolu_1"))
        #expect(SessionRowContent.primaryActions(for: session(phase: .processing)) == .none)
        // A prompt seen only in the terminal is answered there.
        #expect(SessionRowContent.primaryActions(for: session(reason: .permission(tool: "Bash"))) == .answerInTerminal)
        #expect(SessionRowContent.primaryActions(for: session(reason: .dialog("Network access"))) == .answerInTerminal)
        // A failed turn has nothing to answer.
        #expect(SessionRowContent.primaryActions(for: session(reason: .error("Rate limited"))) == .none)
        #expect(!SessionPrimaryActions.none.hasActionBar)
        #expect(SessionPrimaryActions.plan(toolUseId: "t").hasActionBar)
        #expect(SessionPrimaryActions.plan(toolUseId: "t").toolUseId == "t")
        #expect(SessionPrimaryActions.answerInTerminal.toolUseId == nil)
    }

    @Test func simpleQuestionsGetChipsOthersOpenTheChat() throws {
        let simple = try decodedInput(#"{"questions":[{"question":"Which?","header":"Lib","multiSelect":false,"options":[{"label":"A","description":"first"},{"label":"B"}]}]}"#)
        let actions = SessionRowContent.primaryActions(for: session(phase: approval("AskUserQuestion", input: simple)))
        guard case let .questionChips(id, question) = actions else {
            Issue.record("expected chips, got \(actions)")
            return
        }
        #expect(id == "toolu_1")
        #expect(question.question == "Which?")
        #expect(question.header == "Lib")
        #expect(question.options == [.init(label: "A", description: "first"), .init(label: "B", description: nil)])

        let multi = try decodedInput(#"{"questions":[{"question":"Which?","multiSelect":true,"options":[{"label":"A"},{"label":"B"}]}]}"#)
        #expect(SessionRowContent.primaryActions(for: session(phase: approval("AskUserQuestion", input: multi))) == .answerInChat(toolUseId: "toolu_1"))

        let two = try decodedInput(#"{"questions":[{"question":"One?","options":[{"label":"A"}]},{"question":"Two?","options":[{"label":"B"}]}]}"#)
        #expect(SessionRowContent.primaryActions(for: session(phase: approval("AskUserQuestion", input: two))) == .answerInChat(toolUseId: "toolu_1"))

        let many = try decodedInput(#"{"questions":[{"question":"Which?","options":["A","B","C","D","E"]}]}"#)
        #expect(SessionRowContent.primaryActions(for: session(phase: approval("AskUserQuestion", input: many))) == .answerInChat(toolUseId: "toolu_1"))

        let none = try decodedInput(#"{"questions":[{"question":"Free text?","options":[]}]}"#)
        #expect(SessionRowContent.primaryActions(for: session(phase: approval("AskUserQuestion", input: none))) == .answerInChat(toolUseId: "toolu_1"))
    }

    @Test func rowChoicesAreWhatTheBarAndVoiceOverOffer() throws {
        let local = AnyCodable(["type": "addRules", "rules": [["toolName": "Bash", "ruleContent": "ls"]], "destination": "session"] as [String: Any])
        let permission = SessionRowModel.make(session(phase: approval("Bash", suggestions: [local])), account: nil,
                                              rateLimit: nil, canFocus: true, now: now, home: "/")
        #expect(RowAnswerChoices.all(for: permission).map(\.label) == ["Deny", "Always allow", "Allow"])
        #expect(RowAnswerChoices.all(for: permission).last?.command == .allow(sessionId: "s1", toolUseId: "toolu_1"))

        let input = try decodedInput(#"{"questions":[{"question":"Which?","options":[{"label":"A"},{"label":"B"}]}]}"#)
        let question = SessionRowModel.make(session(phase: approval("AskUserQuestion", input: input)), account: nil,
                                            rateLimit: nil, canFocus: false, now: now, home: "/")
        #expect(RowAnswerChoices.all(for: question).map(\.label) == ["A", "B", "Other…"])
        #expect(RowAnswerChoices.all(for: question)[1].command == .chooseOption(sessionId: "s1", toolUseId: "toolu_1", index: 1))

        let plan = SessionRowModel.make(session(phase: approval("ExitPlanMode")), account: nil, rateLimit: nil,
                                        canFocus: false, now: now, home: "/")
        #expect(RowAnswerChoices.all(for: plan).map(\.label) == ["Review plan", "Approve plan"])

        let dialog = SessionRowModel.make(session(reason: .dialog(nil)), account: nil, rateLimit: nil, canFocus: true, now: now, home: "/")
        #expect(RowAnswerChoices.all(for: dialog).map(\.label) == ["Show terminal"])
        let unreachable = SessionRowModel.make(session(reason: .dialog(nil)), account: nil, rateLimit: nil, canFocus: false, now: now, home: "/")
        #expect(RowAnswerChoices.all(for: unreachable).isEmpty)
    }

    // MARK: Row model

    @Test func rowModelMetaAndVoiceOver() {
        var titled = session(phase: .processing)
        titled.applyTitle("Fix the bug", source: .hook)
        titled.contextUsedPercent = 40
        let row = SessionRowModel.make(titled, account: AccountTagModel(label: "Work", colorIndex: 1),
                                       rateLimit: nil, canFocus: true, now: now, home: "/")
        #expect(row.showsMetaLine)
        #expect(row.accessibilityLabel.contains("Fix the bug"))
        #expect(row.accessibilityLabel.contains("account Work"))
        #expect(row.keyTarget == ClaudeKeyRouter.Target(sessionId: "s1", actions: .none, canMarkReviewed: false, canJump: true))
        // Title falls back to the project: nothing more to say.
        #expect(!SessionRowModel.make(session(), account: nil, rateLimit: nil, canFocus: false, now: now, home: "/").showsMetaLine)
        #expect(SessionRowContent.backgroundLabel(count: 2) == "2 background")
        #expect(SessionRowContent.backgroundLabel(count: 0) == nil)
    }

    @Test func voiceOverNeverReadsTheAssistantsMessage() {
        var review = session(phase: .waitingForInput)
        review.completedAt = now.addingTimeInterval(-60)
        review.lastAssistantMessage = "SECRET reply text"
        let label = SessionRowContent.accessibilityLabel(for: review, accountLabel: nil, rateLimit: nil, now: now)
        #expect(!label.contains("SECRET"))
        #expect(label.contains("Ready for review"))
        #expect(label.contains("finished 1m ago"))
    }

    @Test func contextLevelsAndLabels() {
        #expect(ContextMeter.level(for: 79.9) == .normal)
        #expect(ContextMeter.level(for: 80) == .high)
        #expect(ContextMeter.level(for: 90) == .critical)
        #expect(ContextMeter.label(42.7) == "42% context")
        #expect(ContextMeter.fraction(150) == 1)
        #expect(ContextMeter.fraction(.nan) == 0)

        var tasks = SessionTaskList()
        tasks.todosReplaced([
            (content: "a", status: "completed", activeForm: nil),
            (content: "b", status: "in_progress", activeForm: nil),
            (content: "c", status: "pending", activeForm: nil),
        ])
        #expect(TaskProgressBar.countLabel(tasks) == "1/3")
        #expect(TaskProgressBar.isSegmented(count: 3, width: 64))
        #expect(!TaskProgressBar.isSegmented(count: 9, width: 28))
        #expect(!TaskProgressBar.isSegmented(count: 13, width: 400))
    }
}

/// One AskUserQuestion parser for the row's chips, the chat and banners.
struct ChatQuestionParsingTests {
    @Test func parsesQuestionsFromDecodedJSONSkippingBlankOnes() throws {
        let input = try JSONDecoder().decode(
            [String: AnyCodable].self,
            from: Data(#"{"questions":[{"question":"Deploy now?","header":"Deploy","multiSelect":false,"options":[{"label":"Yes","description":"Ship it"},{"label":"No"}]},{"question":"  ","options":[]},{"nope":1}]}"#.utf8)
        )
        let questions = ChatQuestion.parse(toolInput: input)
        #expect(questions.count == 1)
        #expect(questions[0].options.map(\.label) == ["Yes", "No"])
        #expect(ChatQuestion.inline(questions)?.question == "Deploy now?")
        #expect(ChatQuestion.parse(toolInput: nil).isEmpty)
        #expect(ChatQuestion.parse(toolInput: ["questions": AnyCodable("nope")]).isEmpty)
    }

    @Test func bareStringOptionsCount() throws {
        let input = try JSONDecoder().decode([String: AnyCodable].self,
                                             from: Data(#"{"questions":[{"question":"Pick","options":["A"," B "]}]}"#.utf8))
        #expect(ChatQuestion.parse(toolInput: input).first?.options.map(\.label) == ["A", "B"])
    }

    @Test func answersAreKeyedByTheQuestionExactlyAsSent() throws {
        let input = try JSONDecoder().decode([String: AnyCodable].self,
                                             from: Data(#"{"questions":[{"question":"  Which one? ","options":[{"label":"A"}]}]}"#.utf8))
        let question = try #require(ChatQuestion.parse(toolInput: input).first)
        #expect(question.question == "Which one?")
        #expect(ChatQuestionAnswers.answers(for: question, choosing: "A") == ["  Which one? ": "A"])
        var selection = ChatQuestionSelection()
        selection.toggle("A", multiSelect: false)
        #expect(ChatQuestionAnswers.answers(for: [question], selections: [0: selection]) == ["  Which one? ": "A"])
    }

    @Test func chipsNeedOneSingleChoiceQuestionWithFewOptions() {
        func question(options: Int, multi: Bool = false, index: Int = 0) -> ChatQuestion {
            ChatQuestion(index: index, question: "Q?", answerKey: "Q?", header: nil, multiSelect: multi,
                         options: (0..<options).map { ChatQuestionOption(label: "o\($0)", description: nil) })
        }
        #expect(ChatQuestion.inline([question(options: 4)]) != nil)
        #expect(ChatQuestion.inline([question(options: 1)]) != nil)
        #expect(ChatQuestion.inline([question(options: 5)]) == nil)
        #expect(ChatQuestion.inline([question(options: 0)]) == nil)
        #expect(ChatQuestion.inline([question(options: 2, multi: true)]) == nil)
        #expect(ChatQuestion.inline([question(options: 2), question(options: 2, index: 1)]) == nil)
    }
}
