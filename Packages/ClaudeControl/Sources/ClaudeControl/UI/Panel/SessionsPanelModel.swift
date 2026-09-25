//
//  SessionsPanelModel.swift
//  ClaudeControl
//
//  The sessions panel as plain values in and closures out. The live panel
//  (ClaudeSessionsPanel) fills these from the hub and the engine; the
//  snapshots fill them from fixtures, so both draw exactly the same views.
//

import Foundation

/// Everything the panel's list, header and banners show.
struct SessionsPanelModel {
    var sessions: [SessionState]
    /// Every account, tracked or not (labels come from here).
    var accounts: [ClaudeAccountSummary]
    /// Usage by ring id, for "Rate limited · weekly limit resets Thu".
    var readings: [String: ClaudeRingReading] = [:]
    var setup: ClaudeSetupState = ClaudeSetupState()
    /// Sessions whose terminal can be brought to the front.
    var focusable: Set<String> = []
    var hookHealth: HookHealth = HookHealth()
    var quickSettings: PanelQuickSettings = PanelQuickSettings()
    /// The settings.json files "Turn on" would edit, for the consent card.
    var consentFiles: [String] = []
    /// Where "Turn on" installs, in words (Claude Parallel Profiles only).
    var consentScope: String?
    /// Taking over also cleans stores or the shared history.
    var takeoverCleansStores = false
    /// The settings.json files that cleans there.
    var takeoverCleanupFiles: [String] = []
    /// Session id → the ring the hub shows it on (nil: the panel works it
    /// out from the session's folder, as in snapshots). A session the hub
    /// doesn't list belongs to an untracked or forgotten account.
    var sessionRings: [String: String]?
    var home: String = AccountPaths.homeDirectory
    var showsSealedBadge: Bool = false
    /// Accounts the user forgot: their sessions are listed nowhere (BHV-3).
    var forgottenAccountIds: Set<String> = []
    /// Fixed clock for snapshots; nil follows the wall clock.
    var now: Date?

    /// Accounts whose sessions and hooks are tracked.
    var trackedAccounts: [ClaudeAccountSummary] {
        accounts.filter(\.isTracked)
    }

    /// Rows name their account only when there is more than one.
    var showsAccounts: Bool { trackedAccounts.count > 1 }

    /// Display label per account id, collisions told apart.
    var accountLabels: [String: String] {
        AccountLabels.disambiguated(trackedAccounts, home: home)
    }

    /// The account a session belongs to: the hub's ring for it, else the
    /// one whose folders include the folder it runs in (a VS Code window,
    /// `~/.claude`, …).
    func account(for session: SessionState) -> ClaudeAccountSummary? {
        if let ring = sessionRings?[session.sessionId], let account = accounts.first(where: { $0.ringID == ring }) {
            return account
        }
        let id = session.accountId ?? AccountPaths.accountId(forConfigDir: AccountPaths.defaultConfigDir)
        return accounts.first { $0.id == id } ?? accounts.first { $0.configDirs.contains(id) }
    }

    func ringID(of session: SessionState) -> String {
        if let ring = sessionRings?[session.sessionId] { return ring }
        return account(for: session)?.ringID
            ?? ClaudeRingIdentity.ringID(configDir: session.accountId ?? AccountPaths.defaultConfigDir, home: home)
    }

    /// Sessions of tracked accounts (those of an account switched off are
    /// hidden with it), optionally of one ring only.
    func visibleSessions(ringFilter: String?) -> [SessionState] {
        sessions.filter { session in
            if let accountId = session.accountId, forgottenAccountIds.contains(accountId) { return false }
            if let sessionRings, sessionRings[session.sessionId] == nil { return false }
            if let account = account(for: session), !account.isTracked { return false }
            guard let ringFilter else { return true }
            return ringID(of: session) == ringFilter
        }
    }

    /// The limit that is exhausted on the session's account, if any.
    func rateLimit(for session: SessionState, now: Date) -> RateLimitReset? {
        RateLimitReset.current(in: readings[ringID(of: session)], now: now)
    }
}

/// Which accounts silently lack hooks. Their sessions still appear (through
/// Claude Code's session files, which also tell when a turn is done) but
/// their prompts can only be answered in the terminal, so the panel says so
/// instead of leaving them to look fine.
struct HookHealth: Equatable {
    /// Labels of tracked accounts without our hooks, or whose settings.json
    /// could not be read.
    var accountsWithoutHooks: [String] = []
    /// Claude Code control is off ("Not now", or the Hooks switch): said
    /// once, quietly, rather than per account (GUX-9).
    var controlOff = false

    var isHealthy: Bool { accountsWithoutHooks.isEmpty }

    /// The line under `summary`.
    var consequence: String {
        let whose = accountsWithoutHooks.count == 1 ? "Its" : "Their"
        return "\(whose) sessions still show here; answer their prompts where Claude Code runs (VS Code or the terminal) until the hooks are back."
    }

    static let controlOffTitle = "Claude Code control is off"
    static let controlOffMessage = "Answer prompts where Claude Code runs (VS Code or the terminal). Sessions still show here and finish from their transcripts."

    /// Only meaningful once hooks are on: before consent, the consent card
    /// already explains why nothing is live.
    static func make(accounts: [ClaudeAccountSummary], labels: [String: String], hooksEnabled: Bool) -> HookHealth {
        guard hooksEnabled else { return HookHealth() }
        // An account that runs nowhere now (only its Claude Parallel
        // Profiles store holds it) has nothing to hook.
        let missing = accounts
            .filter { $0.isTracked && !$0.hooks.hooksInstalled && $0.hooks.folderCount > 0 }
            .map { labels[$0.id] ?? $0.label }
        return HookHealth(accountsWithoutHooks: missing)
    }

    /// "Hooks are missing in Work." / "…in Work and Side project." / "…in 3 accounts."
    var summary: String {
        switch accountsWithoutHooks.count {
        case 0: return ""
        case 1: return "Hooks are missing in \(accountsWithoutHooks[0])."
        case 2: return "Hooks are missing in \(accountsWithoutHooks[0]) and \(accountsWithoutHooks[1])."
        default: return "Hooks are missing in \(accountsWithoutHooks.count) accounts."
        }
    }
}

/// The gear menu's settings.
struct PanelQuickSettings: Equatable {
    var autoOpen: AutoOpenPolicy = .needsInput
    var notifyNeedsInput = true
    var notifyReadyForReview = true
}

/// What the panel can ask for.
struct SessionsPanelActions {
    var openChat: (_ sessionId: String) -> Void = { _ in }
    var focus: (_ sessionId: String) -> Void = { _ in }
    /// Answer exactly the request `toolUseId`; a request that has moved on
    /// is left alone.
    var approve: (_ sessionId: String, _ toolUseId: String, _ always: Bool) -> Void = { _, _, _ in }
    var deny: (_ sessionId: String, _ toolUseId: String) -> Void = { _, _ in }
    var keepPlanning: (_ sessionId: String, _ toolUseId: String) -> Void = { _, _ in }
    var answer: (_ sessionId: String, _ toolUseId: String, _ answers: [String: String]) -> Void = { _, _, _ in }
    /// Mark reviewed as of `at` (the click, BHV-9), and dismiss any of them
    /// whose turn failed (GUX-2).
    var markReviewed: (_ sessionIds: [String], _ at: Date) -> Void = { _, _ in }
    var turnOnHooks: () -> Void = {}
    var declineHooks: () -> Void = {}
    /// The notice that the yes now covers VS Code workspaces' folders.
    var acknowledgeScope: () -> Void = {}
    var turnOffHooks: () -> Void = {}
    var quitVibeNotch: () -> Void = {}
    var openSettings: () -> Void = {}
    var setQuickSettings: (PanelQuickSettings) -> Void = { _ in }
}
