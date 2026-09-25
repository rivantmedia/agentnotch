//
//  UIFixtures+B.swift
//  ClaudeControl
//
//  Fixture panels, chats and settings for the snapshots and the UI tests:
//  every attention state, a busy day of 26 sessions over three accounts
//  (two of them with the same email), a conversation with markdown and tool
//  results, and the settings pane in its interesting states. Deterministic:
//  every date is relative to `SampleData.now`. Nothing is read from disk.
//

import Foundation

enum UIFixtures {
    static var now: Date { SampleData.now }
    static var home: String { AccountPaths.homeDirectory }

    // MARK: Accounts

    static let personal = SampleSessions.personal
    static let work = SampleSessions.work
    static let side = SampleData.side

    /// Two accounts (identities spread over `~/.claude`, VS Code windows and
    /// Claude Parallel Profiles stores, `SampleLayout`), both with hooks and
    /// the status line in place. `named: false` drops their custom names, so
    /// the default naming rule shows.
    static func accounts(named: Bool = true) -> [ClaudeAccountSummary] {
        let folders = SampleSessions.accounts.map { folder -> ClaudeAccount in
            var copy = folder
            if !named { copy.customLabel = nil }
            return copy
        }
        let grouping = AccountIdentityGrouping.group(folders, prefs: [:], mirrorsDefault: true, home: home)
        let plans = [ringID(personal): "Max 20x", ringID(work): "Team"]
        return grouping.identities.map { identity in
            var summary = ClaudeHostProjections.account(identity: identity, hookStatuses: [:], home: home)
            summary.planName = plans[identity.ringID] ?? summary.planName
            if !named { summary.ownLabel = identity.defaultLabel }
            var hooks = installed
            hooks.folderCount = identity.runDirs.count
            hooks.installedFolderCount = identity.runDirs.count
            summary.hooks = hooks
            return summary
        }
    }

    /// Three accounts; the first two share an email and have no nickname,
    /// so only their plans tell them apart.
    static func collidingAccounts() -> [ClaudeAccountSummary] {
        var first = summary(personal, plan: "Max 20x")
        first.label = "me@example.com"
        first.email = "me@example.com"
        var second = summary(work, plan: "Team")
        second.label = "me@example.com"
        second.email = "me@example.com"
        return [first, second, summary(side, plan: "Pro")]
    }

    /// The account (identity) a fixture folder belongs to, with every
    /// folder it has in the fixture layout.
    static func summary(_ account: ClaudeAccount, plan: String?, hooks: ClaudeHookStatus = installed) -> ClaudeAccountSummary {
        let grouping = AccountIdentityGrouping.group(SampleSessions.accounts + [side], prefs: [:], home: home)
        var summary: ClaudeAccountSummary
        if let identity = grouping.identities.first(where: { $0.folderIds.contains(account.id) }) {
            summary = ClaudeHostProjections.account(identity: identity, hookStatuses: [:], home: home)
        } else {
            summary = ClaudeHostProjections.account(account, hookStatus: nil, home: home)
        }
        summary.planName = plan
        var hooks = hooks
        hooks.folderCount = max(summary.runDirs.count, 1)
        hooks.installedFolderCount = hooks.hooksInstalled ? hooks.folderCount : 0
        summary.hooks = hooks
        return summary
    }

    static let installed = ClaudeHookStatus(hooksInstalled: true, statusLineInstalled: true)

    // MARK: Usage

    /// Personal comfortable; Work's weekly limit is spent (so its failed
    /// turn says when the week resets, not the 5-hour window).
    static func readings() -> [String: ClaudeRingReading] {
        let week = now.addingTimeInterval(2 * SampleData.day + 21 * SampleData.hour + 13 * SampleData.minute)
        return [
            ringID(personal): ClaudeRingReading(
                windows: [
                    .init(id: "session", usedFraction: 0.34, resetsAt: now.addingTimeInterval(3 * SampleData.hour)),
                    .init(id: "weekly_all", usedFraction: 0.41, resetsAt: week),
                ],
                plan: "Max 20x", updatedAt: now.addingTimeInterval(-4 * 60), status: .ok
            ),
            ringID(work): ClaudeRingReading(
                windows: [
                    .init(id: "session", usedFraction: 0.62, resetsAt: now.addingTimeInterval(66 * 60)),
                    .init(id: "weekly_all", usedFraction: 1.0, resetsAt: week),
                ],
                plan: "Team", updatedAt: now.addingTimeInterval(-20), status: .ok
            ),
            ringID(side): ClaudeRingReading(status: .waitingForFirstReading),
        ]
    }

    /// The ring of a fixture folder's account (its identity's).
    static func ringID(_ account: ClaudeAccount) -> String {
        AccountIdentityGrouping.baseKey(accountUuid: account.accountUuid, email: account.email)
            .map { AccountIdentityGrouping.ringID(identityKey: $0, home: home) }
            ?? ClaudeRingIdentity.ringID(configDir: account.configDir, home: home)
    }

    // MARK: Sessions

    /// Every state the list draws: permission (with "Always"), a question
    /// with chips, one too rich for chips, a plan, a failed turn, a dialog
    /// only the terminal can answer, work in progress, finished work and
    /// idle sessions.
    static func everyState() -> [SessionState] {
        SampleSessions.needsYou() + [SampleSessions.richQuestion(), dialog()]
            + SampleSessions.review() + [reviewDone()]
            + SampleSessions.working() + [workingTool()]
            + SampleSessions.idle()
    }

    /// Eight sessions: few enough for full rows, one of each kind.
    static func regular() -> [SessionState] {
        [SampleSessions.permission(), SampleSessions.question(), SampleSessions.rateLimited()]
            + [reviewDone()]
            + Array(SampleSessions.working().prefix(2)) + [workingTool()]
            + [SampleSessions.idle()[0]]
    }

    /// Needs-you only: the answers the rows offer.
    static func needsYou() -> [SessionState] {
        SampleSessions.needsYou() + [SampleSessions.richQuestion(), dialog(), longCommand()]
    }

    /// A busy day: 25 sessions over three accounts.
    static func density() -> [SessionState] {
        var sessions: [SessionState] = [SampleSessions.permission(), SampleSessions.question(), SampleSessions.rateLimited()]
        let accounts = [personal, work, side]
        let reviewTitles = ["Tighten the sign-up form validation", "Port the date picker to the new API",
                            "Draft release notes for 3.2", "Speed up the search index build",
                            "Fix flaky snapshot test on CI", "Document the webhook retries"]
        for (index, title) in reviewTitles.enumerated() {
            var session = SampleSessions.make(
                id: "busy-review-\(index)", title: title, project: ["acme-web", "api", "docs"][index % 3],
                account: accounts[index % 3], phase: .waitingForInput,
                tasks: SampleSessions.taskList(done: 3 + index % 3, active: nil, pending: 0),
                context: Double(20 + index * 9),
                completed: now.addingTimeInterval(-Double(index * 7 + 2) * 60),
                lastAssistant: "Done. \(title) is finished and the tests pass."
            )
            session.lastActivity = session.completedAt ?? now
            sessions.append(session)
        }
        let workingTitles = ["Migrate billing to Stripe v3", "Refactor the notification service",
                             "Write e2e tests for checkout", "Upgrade to React 19", "Profile the cold start",
                             "Rename the analytics events", "Split the monorepo CI", "Add dark mode to emails",
                             "Audit accessibility on settings", "Rewrite the CSV importer"]
        for (index, title) in workingTitles.enumerated() {
            sessions.append(SampleSessions.make(
                id: "busy-working-\(index)", title: title, project: ["acme-web", "api", "mobile", "infra"][index % 4],
                account: accounts[index % 3], phase: .processing,
                tasks: SampleSessions.taskList(done: index % 5, active: "Step \(index % 5 + 1) of \(title.lowercased())",
                                               pending: 3 + index % 3),
                context: Double(15 + index * 8),
                turnStarted: now.addingTimeInterval(-Double(index * 4 + 3) * 60)
            ))
        }
        let idleTitles = ["Explore the notch APIs", "Tidy up the README", "Bump dependencies",
                          "Draft a new logo brief", "Look into the memory spike", "Sketch the pricing page"]
        for (index, title) in idleTitles.enumerated() {
            sessions.append(SampleSessions.make(
                id: "busy-idle-\(index)", title: title, project: "sandbox", account: accounts[index % 3],
                phase: .idle, lastActivity: now.addingTimeInterval(-Double(index + 1) * 3_600),
                lastMessage: (role: "assistant", tool: nil, text: "Anything else?")
            ))
        }
        return sessions
    }

    /// Waiting on a dialog in the terminal the panel can't answer.
    static func dialog() -> SessionState {
        var session = SampleSessions.make(
            // Not SampleSessions.dialog()'s id: both are in everyState() and
            // needsYou(), and a shared id drew one row twice (CS-2, GUX-17).
            id: "needs-dialog-network", title: "Set up the staging database", project: "infra",
            account: work, phase: .waitingForInput, context: 12,
            turnStarted: now.addingTimeInterval(-4 * 60)
        )
        session.needsInputReason = .dialog("Network access to registry.npmjs.org")
        session.lastActivity = now.addingTimeInterval(-3 * 60)
        return session
    }

    /// A command too long to judge from the row.
    static func longCommand() -> SessionState {
        let command = (1...9).map { "git push origin --delete release/2025.\($0)" }.joined(separator: " && \\\n  ")
        var session = SampleSessions.make(
            id: "needs-long-command", title: "Clean up old release branches", project: "acme-web", account: personal,
            phase: .waitingForApproval(PermissionContext(
                toolUseId: "toolu_sample_long",
                toolName: "Bash",
                toolInput: ["command": AnyCodable(command)],
                receivedAt: now.addingTimeInterval(-40)
            )),
            context: 33,
            turnStarted: now.addingTimeInterval(-6 * 60)
        )
        session.lastActivity = now.addingTimeInterval(-40)
        return session
    }

    /// Finished, with the last message as its preview.
    static func reviewDone() -> SessionState {
        var session = SampleSessions.make(
            id: "review-tests", title: "Speed up the test suite", project: "acme-web", account: work,
            phase: .waitingForInput, tasks: SampleSessions.taskList(done: 4, active: nil, pending: 0), context: 71,
            turnStarted: now.addingTimeInterval(-40 * 60), completed: now.addingTimeInterval(-5 * 60),
            lastAssistant: "The suite now runs in 38 s instead of 2 min 10 s: tests run in parallel and the database is reset once per worker."
        )
        session.lastActivity = session.completedAt ?? now
        return session
    }

    /// Working, on a tool rather than a task list, with context nearly full.
    static func workingTool() -> SessionState {
        SampleSessions.make(
            id: "working-tool", title: "Trace the memory leak in the worker", project: "api", account: personal,
            phase: .processing, context: 93, turnStarted: now.addingTimeInterval(-3 * 60),
            background: 2,
            lastMessage: (role: "tool", tool: "Bash", text: "node --inspect scripts/heap-snapshot.js")
        )
    }

    // MARK: Chat

    /// A conversation with markdown, tool calls and their results.
    static func chatHistory() -> [ChatHistoryItem] {
        var items: [ChatHistoryItem] = []
        func add(_ type: ChatHistoryItemType, _ minutesAgo: Double) {
            items.append(ChatHistoryItem(id: "item-\(items.count)", type: type, timestamp: now.addingTimeInterval(-minutesAgo * 60)))
        }
        add(.user("The login page loops back to itself after signing in on Safari. Can you fix it?"), 12)
        add(.thinking("Safari blocks third-party cookies by default, so the session cookie set by the auth callback on the API domain is probably dropped, and the guard sends the user back to /login."), 12)
        add(.toolCall(ToolCallItem(
            name: "Read", input: ["file_path": "/Users/me/code/acme-web/src/auth/redirect.ts"], status: .success,
            result: nil,
            structuredResult: .read(ReadResult(
                filePath: "/Users/me/code/acme-web/src/auth/redirect.ts",
                content: "export function afterLogin(res: Response) {\n  const next = readNext(res)\n  if (!hasSession()) return redirect('/login')\n  return redirect(next ?? '/')\n}",
                numLines: 5, startLine: 12, totalLines: 48
            )),
            subagentTools: []
        )), 11)
        add(.assistant("""
        Found it. After signing in, `afterLogin` checks `hasSession()` **before** the cookie is readable on Safari:

        1. The callback sets the cookie with `SameSite=None` on the API domain.
        2. Safari drops it as a third-party cookie.
        3. The guard sees no session and redirects to `/login` again.

        I'll set the cookie on the app's own domain instead.
        """), 10)
        add(.toolCall(ToolCallItem(
            name: "Edit",
            input: [
                "file_path": "/Users/me/code/acme-web/src/auth/cookies.ts",
                "old_string": "  domain: API_DOMAIN,\n  sameSite: 'none',",
                "new_string": "  domain: APP_DOMAIN,\n  sameSite: 'lax',",
            ],
            status: .success, result: nil, structuredResult: nil, subagentTools: []
        )), 9)
        add(.toolCall(ToolCallItem(
            name: "Bash", input: ["command": "npm run test -- auth", "description": "Run the auth tests"], status: .success,
            result: nil,
            structuredResult: .bash(BashResult(
                stdout: "PASS auth/redirect.spec.ts\nPASS auth/cookies.spec.ts\n\nTests: 14 passed, 14 total",
                stderr: "", interrupted: false, isImage: false, returnCodeInterpretation: nil, backgroundTaskId: nil
            )),
            subagentTools: []
        )), 8)
        add(.assistant("The redirect loop is fixed and the auth tests pass. Want me to add a Safari case to the e2e suite?"), 7)
        return items
    }

    /// A short conversation of the session's own, matching its title and its
    /// task and context numbers, so a chat sheet never shows another
    /// session's story (CS-14). The Safari login conversation is
    /// `needs-permission`'s ("Fix the login redirect loop").
    static func chatHistory(for session: SessionState) -> [ChatHistoryItem] {
        var items: [ChatHistoryItem] = []
        func add(_ type: ChatHistoryItemType, _ minutesAgo: Double) {
            items.append(ChatHistoryItem(id: "\(session.sessionId)-\(items.count)", type: type,
                                         timestamp: now.addingTimeInterval(-minutesAgo * 60)))
        }
        func bash(_ command: String, _ stdout: String) -> ChatHistoryItemType {
            .toolCall(ToolCallItem(
                name: "Bash", input: ["command": command], status: .success, result: nil,
                structuredResult: .bash(BashResult(stdout: stdout, stderr: "", interrupted: false, isImage: false,
                                                   returnCodeInterpretation: nil, backgroundTaskId: nil)),
                subagentTools: []
            ))
        }
        switch session.sessionId {
        case "needs-permission":
            return chatHistory()
        case "review-tests":
            add(.user("The test suite takes over two minutes. Can you make it faster?"), 40)
            add(.thinking("Most of the time goes to resetting the database before every test file, one worker at a time."), 39)
            add(bash("npm test -- --reporter=summary", "Tests: 212 passed, 212 total\nTime:  130.4 s"), 38)
            add(.assistant("Two changes: run the files in parallel (one worker per core) and reset the database once per worker instead of once per file."), 30)
            add(bash("npm test -- --reporter=summary", "Tests: 212 passed, 212 total\nTime:  38.2 s"), 6)
            add(.assistant(session.lastAssistantMessage ?? "Done."), 5)
        case "work-migration":
            add(.user("Write migration tests for the v2 schema: every table, forward and back."), 14)
            add(.assistant("I'll cover the six tables in order. Accounts and invoices are done; writing the subscription tests now."), 6)
            add(bash("npm test -- migrations/v2", "PASS migrations/v2/accounts.spec.ts\nPASS migrations/v2/invoices.spec.ts"), 5)
        case "needs-question":
            add(.user("Add a usage chart to the dashboard's overview page."), 8)
            add(.assistant("There's no charting library in the project yet. Three fit the stack; which should it be?"), 6)
        case "needs-plan":
            add(.user("Move our settings from UserDefaults to SwiftData, without losing anyone's settings."), 14)
            add(.toolCall(ToolCallItem(
                name: "Read", input: ["file_path": "/Users/me/code/notes-app/Sources/Settings/SettingsStore.swift"],
                status: .success, result: nil, structuredResult: nil, subagentTools: []
            )), 13)
            add(.assistant("Here's the plan. The migration runs once and is safe to interrupt."), 11)
        case "needs-dialog-network":
            add(.user("Set up the staging database and seed it with the demo data."), 4)
            add(.assistant("Installing the database client first; npm needs network access for that."), 3)
        default:
            return chatHistory()
        }
        return items
    }

    // MARK: Settings

    /// The pane with consent answered, hooks on and three accounts: one
    /// fine, one with Vibe Notch's old hooks and no hooks of ours yet, and
    /// one not tracked.
    static func settings() -> SettingsPaneModel {
        var model = SettingsPaneModel()
        model.hookConsent = true
        model.hooksEnabled = true
        model.socketPath = "~/Library/Application Support/Agent Notch/Claude/hook.sock"
        model.claudeCodeVersion = "2.1.97"
        model.now = now
        let readings = readings()
        var sideSummary = summary(side, plan: "Pro", hooks: ClaudeHookStatus())
        sideSummary.isTracked = false
        let workSummary = summary(work, plan: "Team", hooks: ClaudeHookStatus(vibeNotchHooksPresent: true,
                                                                               superpoweredVibeNotchHooksPresent: true))
        model.accounts = [
            item(summary(personal, plan: "Max 20x"), nickname: "Personal", isRingShown: true, hooksEnabled: true,
                 reading: readings[ringID(personal)]),
            item(workSummary, nickname: nil, isRingShown: true, hooksEnabled: true, reading: readings[ringID(work)]),
            item(sideSummary, nickname: nil, isRingShown: false, hooksEnabled: true, reading: readings[ringID(side)]),
        ]
        model.parallelProfiles = true
        model.consentScope = ConsentScope(includesDefault: true, windowCount: 3, storeCount: 3, parallelProfiles: true).sentence
        // What the integration added: a login seen under two spellings,
        // another app's hooks reported, a folder offered, the last change
        // and a chosen claude binary.
        model.accounts[1].hasLoginConflict = true
        model.accounts[1].vibeIslandHooksPresent = true
        model.suggestions = [
            FolderSuggestionItem(configDir: home + "/.claude-old", folder: "~/.claude-old",
                                 reason: "Named like a backup copy."),
        ]
        model.hooksChangedNotice = HooksChangedNotice.text(
            changedAccountIds: [AccountPaths.normalize(SampleLayout.workWindow)], accounts: accounts(),
            backups: [AccountPaths.normalize(SampleLayout.workWindow): AccountPaths.normalize(SampleLayout.workWindow) + "/settings.json.agentnotch-20260925-091500-123.bak"],
            windowNames: windowNames, home: home)
        model.claudeBinaryPath = "~/.local/bin/claude"
        return model
    }

    /// The fixture windows' projects, as sessions there tell (UX-6).
    static var windowNames: [String: String] {
        [AccountPaths.normalize(SampleLayout.workWindow): "checkout-web",
         AccountPaths.normalize(SampleLayout.personalWindow): "dotfiles"]
    }

    /// The consent card's file list for the fixture layout (by account, a
    /// window named after its project).
    static var consentFiles: [String] {
        let files = ["~/.claude", SampleLayout.personalWindow, SampleLayout.workWindow, SampleLayout.workSecondWindow]
            .map { AccountPaths.normalize($0) + "/settings.json" }
        return ClaudeControlHub.consentFileLines(files: files, accounts: accounts(), windowNames: windowNames, home: home)
    }

    /// What the takeover cleans in the fixture's stores and shared history.
    static var cleanupFiles: [String] {
        [SampleLayout.personalStore, SampleLayout.workStore, SampleLayout.workSecondStore, SampleLayout.shared]
            .map { $0 + "/settings.json" }
    }

    /// Two accounts with no names of their own: how the default rule reads
    /// in Settings (UX-11).
    static func settingsUnnamed() -> SettingsPaneModel {
        var model = settings()
        let readings = readings()
        model.accounts = accounts(named: false).map { summary in
            item(summary, nickname: nil, isRingShown: true, hooksEnabled: true, reading: readings[summary.ringID])
        }
        model.suggestions = []
        return model
    }

    /// A yes given to an earlier build: the one-time notice naming the VS
    /// Code workspaces' folders it now covers (S2).
    static func settingsScopeNotice() -> SettingsPaneModel {
        var model = settings()
        let windows = [SampleLayout.personalWindow, SampleLayout.workWindow, SampleLayout.workSecondWindow].map(AccountPaths.normalize)
        model.setup = ClaudeSetupState(newInstallFolders: windows)
        model.scopeNoticeFolders = windows.map { WindowFolderNames.label($0, display: AccountPathDisplay.abbreviated($0, home: home), names: windowNames) }
        return model
    }

    /// A settings row as the live pane makes it: the folders listed, each
    /// hooked the way the summary says.
    static func item(_ summary: ClaudeAccountSummary, nickname: String?, isRingShown: Bool, hooksEnabled: Bool,
                     reading: ClaudeRingReading?) -> AccountSettingsItem {
        var item = SettingsPaneItems.item(summary, diskStatus: nil, nickname: nickname, isRingShown: isRingShown,
                                          hooksEnabled: hooksEnabled, reading: reading, parallelProfiles: true, home: home, now: now)
        var statuses: [String: AccountHookStatus] = [:]
        for (index, dir) in summary.runDirs.enumerated() {
            var status = AccountHookStatus()
            status.configDirExists = true
            status.hooksInstalled = index < summary.hooks.installedFolderCount
            status.statusLineInstalled = status.hooksInstalled && summary.hooks.statusLineInstalled
            statuses[dir] = status
        }
        item.folders = SettingsPaneItems.folders(summary, statuses: statuses, hooksEnabled: hooksEnabled,
                                                 windowNames: windowNames, home: home)
        return item
    }

    /// First run: nothing answered, Superpowered Vibe Notch's hooks found.
    static func settingsFirstRun() -> SettingsPaneModel {
        var model = settings()
        model.hookConsent = nil
        model.hooksEnabled = false
        model.setup = ClaudeSetupState(needsHookConsent: true, legacyHooksFound: true,
                                       superpoweredVibeNotchHooksFound: true)
        model.consentFiles = consentFiles
        model.takeoverCleansStores = true
        model.takeoverCleanupFiles = cleanupFiles
        model.notificationsDenied = true
        let readings = readings()
        let workSummary = summary(work, plan: "Team", hooks: ClaudeHookStatus(vibeNotchHooksPresent: true,
                                                                               superpoweredVibeNotchHooksPresent: true))
        model.accounts = [
            item(summary(personal, plan: "Max 20x", hooks: ClaudeHookStatus()), nickname: "Personal", isRingShown: true,
                 hooksEnabled: false, reading: readings[ringID(personal)]),
            item(workSummary, nickname: nil, isRingShown: true, hooksEnabled: false, reading: readings[ringID(work)]),
        ]
        return model
    }
}
