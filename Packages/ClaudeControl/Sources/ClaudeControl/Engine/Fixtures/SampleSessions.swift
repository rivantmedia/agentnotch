//
//  SampleSessions.swift
//  ClaudeControl
//
//  Sample sessions for snapshots, tests and sealed runs: one of every
//  attention state (permission, question, plan, elicitation, dialog, a
//  rate-limited turn, working with and without tasks, ready for review both
//  just finished and older, idle), with task progress, context meters and
//  two accounts. A third account (`side`, with `sideProject()`) is for
//  showing a ring that arrives without a restart (the sealed demo adds it).
//  Deterministic: every date is relative to `SampleData.now`.
//

import Foundation

enum SampleSessions {
    static var now: Date { SampleData.now }

    /// The two accounts' folders the sample sessions run in, with custom
    /// labels so their badges read differently ("PE", "WO"): `~/.claude`,
    /// and a VS Code window's working copy (Claude Parallel Profiles).
    static let personal: ClaudeAccount = {
        var account = SampleLayout.personalFolder("~/.claude", kind: .run)
        account.configDirEnv = nil
        account.customLabel = "Personal"
        return account
    }()

    static let work: ClaudeAccount = {
        var account = SampleLayout.workFolder(SampleLayout.workWindow, kind: .run)
        account.customLabel = "Work"
        return account
    }()

    /// The work identity's second VS Code window.
    static let workSecondWindow: ClaudeAccount = SampleLayout.workFolder(SampleLayout.workSecondWindow, kind: .run)

    /// Every fixture folder: two identities over `~/.claude`, three windows
    /// and three account stores (`SampleLayout`), which make two rings.
    static let accounts = SampleLayout.folders(personal: personal, work: work)

    /// The account the sealed demo adds a few seconds in, to show a ring
    /// arriving without a restart (its usage is in `SampleData.usage`).
    static let side: ClaudeAccount = SampleData.side

    static func usage() -> [String: AccountUsage] {
        SampleData.usage(now: now)
    }

    // MARK: - Sets

    /// Every attention state, as the Sessions tab would list them.
    static func all() -> [SessionState] {
        needsYou() + review() + working() + idle()
    }

    static func needsYou() -> [SessionState] {
        [permission(), question(), plan(), elicitation(), dialog(), rateLimited()]
    }

    /// Where each sample session runs, as the hover rows name it: a fixed
    /// spread over the host apps the engine tells apart.
    static func hostApp(for sessionId: String) -> String {
        let hosts = ["iTerm2", "Terminal", "tmux", "VS Code", "Ghostty", "cmux"]
        if let known = hostApps[sessionId] { return known }
        let seed = sessionId.unicodeScalars.reduce(0) { $0 + Int($1.value) }
        return hosts[seed % hosts.count]
    }

    private static let hostApps: [String: String] = [
        "needs-permission": "iTerm2",
        "needs-question": "VS Code",
        "needs-plan": "Terminal",
        "needs-elicitation": "Ghostty",
        "needs-dialog": "tmux",
        "needs-ratelimit": "iTerm2",
        "review-just-finished": "cmux",
        "review-darkmode": "Terminal",
        "review-devserver": "tmux",
        "work-migration": "iTerm2",
        "work-ci": "VS Code",
        "work-summary": "Ghostty",
        "side-launch-post": "iTerm2",
        "side-landing-page": "VS Code",
    ]

    /// What the late account (`side`) brings along: one session working
    /// through its tasks and one waiting for review, so its ring spins and
    /// carries a badge as soon as it appears. Not part of `all()`.
    static func sideProject() -> [SessionState] {
        [
            make(
                id: "side-launch-post",
                title: "Draft the launch post",
                project: "side-blog",
                account: side,
                phase: .processing,
                tasks: taskList(done: 1, active: "Writing the intro", pending: 2),
                context: 21,
                turnStarted: minutes(-3)
            ),
            make(
                id: "side-landing-page",
                title: "Polish the landing page copy",
                project: "side-site",
                account: side,
                phase: .waitingForInput,
                context: 35,
                turnStarted: minutes(-9),
                completed: minutes(-2),
                lastAssistant: "Tightened the hero copy and the pricing blurb."
            ),
        ]
    }

    static func review() -> [SessionState] {
        [
            make(
                id: "review-just-finished",
                title: "Fix the flaky date test",
                project: "billing-service",
                account: personal,
                phase: .waitingForInput,
                tasks: taskList(done: 3, active: nil, pending: 0),
                context: 27,
                turnStarted: minutes(-6),
                completed: seconds(-20),
                lastAssistant: "The test pinned the time zone to UTC; it now uses a fixed calendar and passes 50 runs in a row."
            ),
            make(
                id: "review-darkmode",
                title: "Add a dark mode toggle to settings",
                project: "acme-web",
                account: personal,
                phase: .waitingForInput,
                tasks: taskList(done: 5, active: nil, pending: 0),
                context: 38,
                turnStarted: minutes(-19),
                completed: minutes(-5),
                lastAssistant: "Added a Dark mode toggle under Settings › Appearance. It follows the system by default, persists the choice, and all 42 tests pass."
            ),
            make(
                id: "review-devserver",
                title: "Set up the local dev server",
                project: "acme-web",
                account: work,
                phase: .waitingForInput,
                context: 22,
                turnStarted: minutes(-31),
                completed: minutes(-12),
                lastAssistant: "The dev server is up on http://localhost:5173 with hot reload. I left the type checker watching in the background.",
                background: 2
            ),
        ]
    }

    static func working() -> [SessionState] {
        [
            make(
                id: "work-migration",
                title: "Write migration tests for the v2 schema",
                project: "billing-service",
                account: work,
                phase: .processing,
                tasks: taskList(done: 2, active: "Writing tests for the v2 schema", pending: 3),
                context: 84,
                turnStarted: minutes(-14)
            ),
            make(
                id: "work-ci",
                title: "Investigate the flaky CI job",
                project: "infra",
                account: personal,
                phase: .processing,
                tasks: taskList(done: 9, active: "Bisecting the failing commit", pending: 5),
                context: 93,
                turnStarted: minutes(-8),
                lastMessage: (role: "tool", tool: "Grep", text: "ETIMEDOUT|socket hang up")
            ),
            make(
                id: "work-summary",
                title: "Summarize the PR review feedback",
                project: "acme-web",
                account: workSecondWindow,
                phase: .processing,
                context: 12,
                turnStarted: seconds(-40)
            ),
        ]
    }

    static func idle() -> [SessionState] {
        [
            make(
                id: "idle-notch",
                title: "Explore the notch APIs",
                project: "superpowered-vibe-notch",
                account: personal,
                phase: .idle,
                context: 7,
                lastActivity: minutes(-50),
                lastMessage: (role: "assistant", tool: nil, text: "NSScreen.auxiliaryTopLeftArea gives the menu bar space left of the notch.")
            ),
            make(
                id: "idle-readme",
                title: "Tidy up the README",
                project: "acme-web",
                account: work,
                phase: .idle,
                lastActivity: hours(-3),
                lastMessage: (role: "user", tool: nil, text: "thanks, that's all for now")
            ),
            make(
                id: "idle-deps",
                title: "Bump dependencies",
                project: "billing-service",
                account: work,
                phase: .idle,
                lastActivity: hours(-5),
                lastMessage: (role: "tool", tool: "Bash", text: "npm outdated")
            ),
            make(
                id: "idle-logo",
                title: "Draft a new logo brief",
                project: "brand",
                account: personal,
                phase: .idle,
                lastActivity: hours(-26)
            ),
        ]
    }

    // MARK: - Needs You

    static func permission() -> SessionState {
        var session = make(
            id: "needs-permission",
            title: "Fix the login redirect loop",
            project: "acme-web",
            account: work,
            phase: .waitingForApproval(
                PermissionContext(
                    toolUseId: "toolu_sample_bash",
                    toolName: "Bash",
                    toolInput: ["command": AnyCodable("npm run test -- --watch=false auth/redirect.spec.ts")],
                    receivedAt: minutes(-2),
                    permissionSuggestions: [
                        AnyCodable([
                            "type": "addRules",
                            "rules": [["toolName": "Bash", "ruleContent": "npm run test:*"]],
                            "behavior": "allow",
                            "destination": "localSettings",
                        ] as [String: Any]),
                    ]
                )
            ),
            tasks: taskList(done: 3, active: "Running the auth test suite", pending: 3),
            context: 42,
            turnStarted: minutes(-9)
        )
        session.lastActivity = minutes(-2)
        return session
    }

    static func question() -> SessionState {
        make(
            id: "needs-question",
            title: "Pick a charting library",
            project: "dashboard",
            account: personal,
            phase: .waitingForApproval(
                PermissionContext(
                    toolUseId: "toolu_sample_question",
                    toolName: "AskUserQuestion",
                    toolInput: [
                        "questions": AnyCodable([
                            [
                                "question": "Which charting library should the dashboard use?",
                                "header": "Charts",
                                "multiSelect": false,
                                "options": [
                                    ["label": "Recharts", "description": "Composable React components"],
                                    ["label": "Chart.js", "description": "Canvas, small bundle"],
                                    ["label": "ECharts", "description": "Feature-rich, larger bundle"],
                                ],
                            ] as [String: Any],
                        ]),
                    ],
                    receivedAt: minutes(-6)
                )
            ),
            context: 18,
            turnStarted: minutes(-8)
        )
    }

    static func plan() -> SessionState {
        make(
            id: "needs-plan",
            title: "Migrate settings storage to SwiftData",
            project: "notes-app",
            account: personal,
            phase: .waitingForApproval(
                PermissionContext(
                    toolUseId: "toolu_sample_plan",
                    toolName: "ExitPlanMode",
                    toolInput: ["plan": AnyCodable("1. Add models\n2. Migrate\n3. Remove UserDefaults")],
                    receivedAt: minutes(-11)
                )
            ),
            context: 61,
            turnStarted: minutes(-16)
        )
    }

    /// An MCP server asked for input (an elicitation dialog in the terminal).
    /// On the personal account: the work account's only prompts are the
    /// permission and the rate-limited turn, which the sealed demo answers
    /// a moment in so that ring turns to working.
    static func elicitation() -> SessionState {
        var session = make(
            id: "needs-elicitation",
            title: "Sync the design tokens",
            project: "design-system",
            account: personal,
            phase: .processing,
            context: 33,
            turnStarted: minutes(-5)
        )
        session.needsInputReason = .elicitation("Figma needs you to pick a file")
        session.lastActivity = minutes(-3)
        return session
    }

    /// Some other dialog is open in the terminal (the registry says `waiting`).
    static func dialog() -> SessionState {
        var session = make(
            id: "needs-dialog",
            title: "Clean up old branches",
            project: "infra",
            account: personal,
            phase: .processing,
            context: 15,
            turnStarted: minutes(-4)
        )
        session.needsInputReason = .dialog("worker permission")
        session.lastActivity = minutes(-1)
        return session
    }

    static func rateLimited() -> SessionState {
        var session = make(
            id: "needs-ratelimit",
            title: "Refactor the usage parser",
            project: "superpowered-vibe-notch",
            account: work,
            phase: .waitingForInput,
            context: 55,
            turnStarted: minutes(-26),
            completed: minutes(-14),
            lastAssistant: nil
        )
        session.needsInputReason = .error(NeedsInputReason.humanizedStopError("rate_limit"))
        session.lastActivity = minutes(-14)
        return session
    }

    /// A question too rich for chips (two questions): "Answer…" opens the chat.
    static func richQuestion() -> SessionState {
        make(
            id: "needs-rich-question",
            title: "Plan the onboarding flow",
            project: "mobile-app",
            account: work,
            phase: .waitingForApproval(
                PermissionContext(
                    toolUseId: "toolu_sample_rich",
                    toolName: "AskUserQuestion",
                    toolInput: [
                        "questions": AnyCodable([
                            ["question": "Which screens should onboarding include?", "multiSelect": true,
                             "options": [["label": "Welcome"], ["label": "Permissions"], ["label": "Sign in"]]] as [String: Any],
                            ["question": "Skippable?", "options": [["label": "Yes"], ["label": "No"]]] as [String: Any],
                        ]),
                    ],
                    receivedAt: minutes(-1)
                )
            )
        )
    }

    // MARK: - Builders

    static func make(
        id: String,
        title: String,
        project: String,
        account: ClaudeAccount?,
        phase: SessionPhase,
        tasks: SessionTaskList = SessionTaskList(),
        context: Double? = nil,
        turnStarted: Date? = nil,
        completed: Date? = nil,
        lastAssistant: String? = nil,
        background: Int = 0,
        lastActivity: Date? = nil,
        lastMessage: (role: String, tool: String?, text: String)? = nil
    ) -> SessionState {
        let cwd = "/Users/me/code/\(project)"
        // Stable across runs (String.hashValue is seeded per process).
        let seed = id.unicodeScalars.reduce(0) { $0 + Int($1.value) }
        var session = SessionState(
            sessionId: id,
            cwd: cwd,
            pid: 40_000 + seed % 9_000,
            tty: "ttys00\(seed % 9)",
            phase: phase,
            conversationInfo: ConversationInfo(
                summary: nil,
                lastMessage: lastMessage?.text ?? lastAssistant.map { String($0.prefix(80)) },
                lastMessageRole: lastMessage?.role ?? (lastAssistant == nil ? nil : "assistant"),
                lastToolName: lastMessage?.tool,
                firstUserMessage: nil,
                lastUserMessageDate: nil
            ),
            lastActivity: lastActivity ?? completed ?? turnStarted ?? now,
            createdAt: hours(-6)
        )
        session.applyTitle(title, source: .hook)
        session.accountId = account?.id
        session.configDirEnv = account?.configDirEnv
        session.entrypoint = "cli"
        session.tasks = tasks
        session.contextUsedPercent = context
        session.turnStartedAt = turnStarted
        session.completedAt = completed
        session.lastAssistantMessage = lastAssistant
        session.backgroundTaskCount = background
        return session
    }

    /// A task list with `done` completed tasks, an optional in-progress one
    /// and `pending` more.
    static func taskList(done: Int, active: String?, pending: Int) -> SessionTaskList {
        var list = SessionTaskList()
        var todos: [(content: String, status: String, activeForm: String?)] = []
        for index in 0..<done {
            todos.append(("Step \(index + 1)", "completed", nil))
        }
        if let active {
            todos.append((active, "in_progress", active))
        }
        for index in 0..<pending {
            todos.append(("Follow-up \(index + 1)", "pending", nil))
        }
        list.todosReplaced(todos)
        return list
    }

    private static func seconds(_ value: TimeInterval) -> Date { now.addingTimeInterval(value) }
    private static func minutes(_ value: TimeInterval) -> Date { now.addingTimeInterval(value * 60) }
    private static func hours(_ value: TimeInterval) -> Date { now.addingTimeInterval(value * 3_600) }
}
