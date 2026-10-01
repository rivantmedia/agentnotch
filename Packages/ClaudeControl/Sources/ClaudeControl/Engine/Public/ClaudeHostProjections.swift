//
//  ClaudeHostProjections.swift
//  ClaudeControl
//
//  Pure mappers from the engine's models (ClaudeAccount, SessionState,
//  AccountUsage) to the public summaries the app draws: accounts, sessions,
//  hover-card rows (design §5) and ring readings (§4.3). No I/O and no
//  state: the hub calls them on every publish and tests call them directly.
//
//  Privacy: a row's `detail` and `waitingFor`, and a session's needs-input
//  summary, are built only from tool names, a short tool-input preview,
//  question headers, task titles, counts and folder names. Never from the
//  prompt or from Claude's messages.
//

import Foundation

nonisolated enum ClaudeHostProjections {
    /// A permission's input preview is cut to this many characters.
    static let inputPreviewLength = 40

    // MARK: - Accounts

    static func account(
        _ account: ClaudeAccount,
        hookStatus: AccountHookStatus?,
        organizationUuid: String? = nil,
        home: String
    ) -> ClaudeAccountSummary {
        ClaudeAccountSummary(
            id: account.id,
            ringID: ClaudeRingIdentity.ringID(configDir: account.configDir, home: home),
            configDir: account.configDir,
            label: account.label,
            email: account.email,
            planName: account.planName,
            // The store's copy (read while Claude Desktop readings are on),
            // else the one the registry keeps from `.claude.json`.
            organizationUuid: organizationUuid ?? account.organizationUuid,
            isDefault: account.isDefault,
            isTracked: !account.isHidden,
            colorIndex: account.colorIndex,
            hooks: ClaudeHookStatus(
                hooksInstalled: hookStatus?.hooksInstalled ?? false,
                statusLineInstalled: hookStatus?.statusLineInstalled ?? false,
                vibeNotchHooksPresent: hookStatus?.vibeNotchHooksPresent ?? false,
                superpoweredVibeNotchHooksPresent: hookStatus?.superpoweredVibeNotchHooksPresent ?? false,
                lastError: hookStatus?.lastError
            ),
            launchCommand: account.launchCommand,
            ownLabel: account.label
        )
        .withFolders(account)
    }

    /// Whose `~/.claude`'s old ring (`claude`) is: while Claude Parallel
    /// Profiles has mirrored another account into it (`mirrored`), the one
    /// its own `accountUuid` names (`owner`), else still whoever holds it,
    /// but only as a last resort.
    struct DefaultRingOwner: Equatable, Sendable {
        var mirrored = false
        var owner: String?
    }

    /// One account (identity) as the app sees it: its name, email and plan,
    /// its folders, and its hooks over every run folder.
    static func account(
        identity: ClaudeIdentityAccount,
        hookStatuses: [String: AccountHookStatus],
        organizationUuid: String? = nil,
        defaultRing: DefaultRingOwner = DefaultRingOwner(),
        adopted: Set<String> = [],
        home: String
    ) -> ClaudeAccountSummary {
        let primary = identity.primaryDir
        var summary = ClaudeAccountSummary(
            id: identity.id,
            ringID: identity.ringID,
            configDir: primary?.configDir ?? AccountRegistry.defaultConfigDir(home: home),
            label: identity.label,
            email: identity.email,
            planName: identity.planName,
            organizationUuid: organizationUuid ?? identity.organizationUuid,
            isDefault: identity.includesDefault,
            isTracked: !identity.isHidden,
            colorIndex: identity.colorIndex,
            hooks: hookStatus(identity: identity, statuses: hookStatuses),
            launchCommand: identity.launchCommand,
            ownLabel: identity.label
        )
        summary.runDirs = identity.runDirs.map(\.configDir)
        summary.storeDirs = identity.storeDirs.map(\.configDir)
        summary.windowCount = identity.windowDirIds.count
        summary.isSignedIn = identity.isSignedIn
        let defaultRingOwner = defaultRing
        summary.canForget = identity.canBeForgotten
        summary.hasTerminalLaunch = identity.terminalLaunchCommand != nil
        summary.adoptedDirs = identity.runDirs.map(\.configDir).filter(adopted.contains)
        if !identity.isStandaloneUnsigned {
            var former = formerRingIDs(of: identity.folders, home: home)
            let defaultRing = ClaudeRingIdentity.ringID(configDir: AccountRegistry.defaultConfigDir(home: home), home: home)
            if defaultRingOwner.mirrored {
                if let owner = defaultRingOwner.owner {
                    // Its owner's, wherever the extension mirrored it.
                    former.removeAll { $0 == defaultRing }
                    if owner == identity.id { former.insert(defaultRing, at: 0) }
                } else if former.contains(defaultRing) {
                    summary.uncertainFormerRingIDs = [defaultRing]
                }
            }
            summary.formerRingIDs = former
        }
        return summary
    }

    /// The per-folder ring ids of `folders` (what they were called when every
    /// folder was an account), default first, no repeats.
    static func formerRingIDs(of folders: [ClaudeAccount], home: String) -> [String] {
        var seen = Set<String>()
        return AccountIdentityGrouping.orderedFolders(folders, home: home)
            .map { ClaudeRingIdentity.ringID(configDir: $0.configDir, home: home) }
            .filter { seen.insert($0).inserted }
    }

    /// An account's hooks over its run folders (where they go): in place
    /// only when in every one; another app's hooks when in any folder,
    /// stores included; the first error.
    static func hookStatus(identity: ClaudeIdentityAccount, statuses: [String: AccountHookStatus]) -> ClaudeHookStatus {
        let run = identity.runDirs.map { statuses[$0.id] }
        let all = identity.folders.compactMap { statuses[$0.id] }
        let installed = run.filter { $0?.hooksInstalled == true }.count
        var status = ClaudeHookStatus(
            hooksInstalled: !run.isEmpty && installed == run.count,
            statusLineInstalled: !run.isEmpty && run.allSatisfy { $0?.statusLineInstalled == true },
            vibeNotchHooksPresent: all.contains { $0.vibeNotchHooksPresent },
            superpoweredVibeNotchHooksPresent: all.contains { $0.superpoweredVibeNotchHooksPresent },
            lastError: run.compactMap { $0?.lastError }.first
        )
        status.folderCount = run.count
        status.installedFolderCount = installed
        return status
    }

    // MARK: - Sessions

    /// The ring a session belongs to: its folder's account's (the identity
    /// that folder runs as), else the folder's own ring id.
    static func ringID(for session: SessionState, home: String) -> String {
        let folder = session.accountId ?? AccountPaths.defaultConfigDir
        return FolderRings.ring(forFolder: folder) ?? ClaudeRingIdentity.ringID(configDir: folder, home: home)
    }

    /// - Parameters:
    ///   - hostApp: where it runs ("iTerm2", "tmux", …), see `HostAppKind`.
    ///   - limitHit: the used-up window of its account, for "Rate limited · resets 14:05".
    static func session(
        _ session: SessionState,
        home: String,
        hostApp: String? = nil,
        limitHit: UsageLimitHit? = nil,
        now: Date = Date(),
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> ClaudeSessionSummary {
        let attention = attention(session, limitHit: limitHit, now: now, calendar: calendar, locale: locale)
        return ClaudeSessionSummary(
            id: session.sessionId,
            ringID: ringID(for: session, home: home),
            pid: session.pid.flatMap { Int32(exactly: $0) },
            title: publicTitle(session),
            projectName: session.displayProjectName,
            hostApp: hostApp,
            attention: attention,
            attentionSince: attentionSince(session, attention: attention),
            tasks: tasks(session.tasks),
            contextPercent: session.contextUsedPercent.flatMap { $0.isFinite ? min(max($0, 0), 100) : nil },
            backgroundTasks: session.backgroundTaskCount,
            runningTool: attention == .working ? runningTool(session) : nil,
            backgroundWait: attention == .working ? session.backgroundWaitDescription : nil
        )
    }

    /// A session's name outside the panel (hover rows and the phone link,
    /// banners): its title from a hook, the registry or the transcript, else
    /// the project folder. Never the first prompt, which
    /// `SessionState.displayTitle` falls back to until Claude Code names the
    /// session: rows travel to the phone over the network, and banners show
    /// on screen and in Notification Center.
    static func publicTitle(_ session: SessionState) -> String {
        func usable(_ text: String?) -> String? {
            guard let line = text.map({ collapsed($0, limit: .max) }), !line.isEmpty else { return nil }
            return line
        }
        if session.titleSource != .derivedName, let title = usable(session.sessionTitle) { return title }
        if let summary = usable(session.conversationInfo.summary) { return summary }
        if let title = usable(session.sessionTitle) { return title }
        return usable(session.projectName) ?? session.displayProjectName
    }

    static func attention(
        _ session: SessionState,
        limitHit: UsageLimitHit? = nil,
        now: Date = Date(),
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> ClaudeAttention {
        switch session.attention {
        case .working: return .working
        case .readyForReview: return .readyForReview
        case .idle: return .idle
        case .needsInput(let reason):
            return .needsInput(needsInput(reason, session: session, limitHit: limitHit, now: now,
                                          calendar: calendar, locale: locale))
        }
    }

    /// What the session wants, in a few words: "Allow Bash · npm test",
    /// "Question · Charts", "Plan ready to approve", "Rate limited · resets 14:05".
    static func needsInput(
        _ reason: NeedsInputReason,
        session: SessionState,
        limitHit: UsageLimitHit?,
        now: Date,
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> ClaudeNeedsInput {
        switch reason {
        case .permission(let tool):
            let name = session.activePermission?.toolName ?? tool
            guard !name.isEmpty else { return ClaudeNeedsInput(kind: .permission, summary: "Needs permission") }
            var summary = "Allow \(MCPToolFormatter.formatToolName(name))"
            if let input = inputPreview(session.activePermission?.offPanelInput) {
                summary += " · \(input)"
            }
            return ClaudeNeedsInput(kind: .permission, summary: summary)
        case .question:
            if let header = questionHeader(session.activePermission?.toolInput) {
                return ClaudeNeedsInput(kind: .question, summary: "Question · \(header)")
            }
            return ClaudeNeedsInput(kind: .question, summary: "Question")
        case .planApproval:
            return ClaudeNeedsInput(kind: .plan, summary: "Plan ready to approve")
        case .elicitation:
            return ClaudeNeedsInput(kind: .elicitation, summary: collapsed(reason.displayText, limit: 60))
        case .dialog:
            return ClaudeNeedsInput(kind: .dialog, summary: collapsed(reason.displayText, limit: 60))
        case .error(let message):
            var summary = collapsed(message, limit: 60)
            // Which window stopped it, so the notch tells hours from days
            // (GUX-14): "Rate limited (weekly) · resets Fri 09:00".
            if isRateLimit(reason), let limitHit {
                summary += " (\(windowName(limitHit.window)))"
                if let phrase = resetPhrase(limitHit.resetsAt, now: now, calendar: calendar, locale: locale) {
                    summary += " · \(phrase)"
                }
            }
            return ClaudeNeedsInput(kind: .error, summary: summary)
        }
    }

    /// "5-hour", "weekly", "Opus weekly".
    static func windowName(_ window: UsageLimitHit.Window) -> String {
        switch window {
        case .session: return "5-hour"
        case .weekly: return "weekly"
        case .scoped(let family): return "\(family) weekly"
        }
    }

    /// A failed turn because the account hit a usage limit.
    static func isRateLimit(_ reason: NeedsInputReason) -> Bool {
        if case .error(let message) = reason {
            return message == NeedsInputReason.humanizedStopError("rate_limit")
        }
        return false
    }

    /// When the current attention started: the wait (the pending request's
    /// arrival), the turn, the completion, or the last activity.
    static func attentionSince(_ session: SessionState, attention: ClaudeAttention) -> Date {
        switch attention {
        case .needsInput: return session.activePermission?.receivedAt ?? session.lastActivity
        case .working: return session.turnStartedAt ?? session.lastActivity
        case .readyForReview: return session.completedAt ?? session.lastActivity
        case .idle: return session.lastActivity
        }
    }

    static func tasks(_ list: SessionTaskList) -> ClaudeTaskProgress? {
        let items = list.items
        guard !items.isEmpty else { return nil }
        // Only what changes with a task event, so the summary republishes on
        // those and not as the clock runs.
        let timing = list.timing
        return ClaudeTaskProgress(
            completed: list.completedCount,
            total: items.count,
            active: list.activeItem?.activeLabel,
            secondsPerTask: timing.secondsPerTask,
            activeSince: timing.activeSince
        )
    }

    /// The tool the session is running now (the newest one started), by its
    /// display name; nil between tools.
    static func runningTool(_ session: SessionState) -> String? {
        guard let tool = session.toolTracker.inProgress.values.max(by: { $0.startTime < $1.startTime }) else {
            return nil
        }
        return MCPToolFormatter.formatToolName(tool.name)
    }

    /// Where a session runs, as rows name it: tmux wins, then the host app,
    /// then "VS Code" for the extension with no app found.
    static func hostAppName(isInTmux: Bool, host: HostApp?, entrypoint: String?) -> String? {
        HostAppKind.classify(isInTmux: isInTmux, host: host, entrypoint: entrypoint)?.displayName
    }

    // MARK: - Hover-card rows

    /// One hover-card row for a session (design §5):
    ///
    /// | attention      | state    | detail / waitingFor                                  | since      |
    /// |----------------|----------|------------------------------------------------------|------------|
    /// | needs input    | .waiting | waitingFor: the needs-input summary                  | wait start |
    /// | failed turn    | .idle    | "Stopped · Rate limited (weekly) · resets Fri 09:00" | activity   |
    /// | working        | .busy    | "3/7 · ~4m left · Writing tests · ctx 42%", "Bash…"  | turn start |
    /// | ready (review) | .success | "Ready for review · <project> · 2 in background"     | completion |
    /// | idle           | .idle    | "<host app> · <project>"                             | activity   |
    ///
    /// The time left (once the session's tasks have a pace) is as of `now`.
    static func activityRow(_ session: ClaudeSessionSummary, runningTool: String? = nil, now: Date = Date()) -> ClaudeActivityRow {
        let runningTool = runningTool ?? session.runningTool
        let state: ClaudeActivityRow.State
        var waitingFor: String?
        let detail: String
        let place = [session.hostApp, session.projectName.isEmpty ? nil : session.projectName]
            .compactMap { $0 }
            .joined(separator: " · ")
        switch session.attention {
        case .needsInput(let input) where input.kind == .error:
            // Nothing to answer: no amber pulse, just what stopped it (GUX-2).
            state = .idle
            detail = "Stopped · \(input.summary)"
        case .needsInput(let input):
            state = .waiting
            waitingFor = input.summary
            detail = place
        case .working:
            state = .busy
            var parts: [String] = []
            if let wait = session.backgroundWait {
                // The turn is over; its agents aren't.
                parts.append("Waiting on \(wait)")
                if let tasks = session.tasks { parts.append("\(tasks.completed)/\(tasks.total)") }
            } else if let tasks = session.tasks {
                parts.append("\(tasks.completed)/\(tasks.total)")
                if let left = tasks.remainingLabel(now: now) { parts.append(left) }
                if let active = tasks.active.map({ collapsed($0, limit: 48) }), !active.isEmpty {
                    parts.append(active)
                }
            } else if let runningTool, !runningTool.isEmpty {
                parts.append("\(runningTool)…")
            } else {
                parts.append("Thinking…")
            }
            if let context = session.contextPercent {
                parts.append("ctx \(Int(context.rounded()))%")
            }
            detail = parts.joined(separator: " · ")
        case .readyForReview:
            state = .success
            var parts = ["Ready for review"]
            if !session.projectName.isEmpty { parts.append(session.projectName) }
            if session.backgroundTasks > 0 { parts.append("\(session.backgroundTasks) in background") }
            detail = parts.joined(separator: " · ")
        case .idle:
            state = .idle
            detail = place
        }
        return ClaudeActivityRow(
            id: session.id,
            name: session.title,
            detail: detail,
            state: state,
            waitingFor: waitingFor,
            since: session.attentionSince,
            pid: session.pid
        )
    }

    // MARK: - Usage

    /// A ring reading from the merged usage of one account (design §4.3).
    ///
    /// | situation                                   | status                  |
    /// |---------------------------------------------|-------------------------|
    /// | `.claude.json` has no claude.ai login       | `.signInNeeded`         |
    /// | windows known                               | `.ok`                   |
    /// | none yet, usage not available for the login | `.unavailable`          |
    /// | none yet, the last probe failed             | `.failed`               |
    /// | none yet                                    | `.waitingForFirstReading` |
    static func ringReading(
        usage: AccountUsage?,
        fetchState: UsageFetchState?,
        plan: String? = nil,
        organizationUuid: String? = nil,
        staleThreshold: TimeInterval = ClaudeRingReading.staleAfter,
        now: Date
    ) -> ClaudeRingReading {
        let windows = usage.map { UsageRingWindows.windows(from: $0, now: now) } ?? []
        let status: ClaudeRingReading.Status
        if fetchState == UsageStore.notSignedIn {
            status = .signInNeeded("Not signed in to Claude")
        } else if !windows.isEmpty {
            status = .ok
        } else {
            switch fetchState {
            case .unavailable(let text)?: status = .unavailable(text)
            case .failed(let text)?: status = .failed(text)
            default: status = .waitingForFirstReading
            }
        }
        var reading = ClaudeRingReading(
            windows: windows,
            plan: plan ?? usage?.subscriptionType.map(planName),
            updatedAt: windows.isEmpty ? nil : usage?.updatedAt,
            status: status
        )
        reading.staleThreshold = staleThreshold
        reading.organizationUuid = organizationUuid
        return reading
    }

    /// "max" → "Max".
    static func planName(_ subscriptionType: String) -> String {
        subscriptionType.prefix(1).uppercased() + subscriptionType.dropFirst()
    }

    // MARK: - Text

    /// "resets 14:05" today, "resets Thu 09:00" within the week, "resets
    /// 3 Oct" later; nil without a future reset time.
    static func resetPhrase(
        _ resetsAt: Date?,
        now: Date,
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String? {
        guard let resetsAt, resetsAt > now else { return nil }
        let template: String
        if calendar.isDate(resetsAt, inSameDayAs: now) {
            template = "jmm"
        } else if resetsAt.timeIntervalSince(now) < 6 * 24 * 60 * 60 {
            template = "EEEjmm"
        } else {
            template = "dMMM"
        }
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.calendar = calendar
        formatter.timeZone = calendar.timeZone
        formatter.dateFormat = DateFormatter.dateFormat(fromTemplate: template, options: 0, locale: locale) ?? "HH:mm"
        return "resets \(formatter.string(from: resetsAt))"
    }

    /// A tool input preview, one line, at most `inputPreviewLength` characters.
    static func inputPreview(_ text: String?) -> String? {
        guard let text else { return nil }
        let line = collapsed(text, limit: inputPreviewLength)
        return line.isEmpty ? nil : line
    }

    /// AskUserQuestion's first header ("Charts"): Claude Code caps headers
    /// at a dozen characters, a label rather than the question itself.
    static func questionHeader(_ toolInput: [String: AnyCodable]?) -> String? {
        guard let questions = toolInput?["questions"]?.value as? [Any],
              let first = questions.first as? [String: Any],
              let header = first["header"] as? String else { return nil }
        let text = collapsed(header, limit: 24)
        return text.isEmpty ? nil : text
    }

    /// Whitespace collapsed to single spaces, cut to `limit` with an ellipsis.
    static func collapsed(_ text: String, limit: Int) -> String {
        let line = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        guard limit > 1, line.count > limit else { return line }
        return String(line.prefix(limit - 1)).trimmingCharacters(in: .whitespaces) + "…"
    }
}

private extension ClaudeAccountSummary {
    /// A single folder's summary: its one folder, where hooks go when it runs.
    nonisolated func withFolders(_ account: ClaudeAccount) -> ClaudeAccountSummary {
        var copy = self
        if account.kind == .store {
            copy.storeDirs = [account.configDir]
        } else {
            copy.runDirs = [account.configDir]
        }
        copy.isSignedIn = account.isSignedIn
        copy.hooks.installedFolderCount = copy.hooks.hooksInstalled ? 1 : 0
        return copy
    }
}
