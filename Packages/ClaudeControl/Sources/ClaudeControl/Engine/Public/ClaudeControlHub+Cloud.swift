//
//  ClaudeControlHub+Cloud.swift
//  ClaudeControl
//
//  The hub's website side: where sync goes, signing in (Google, through the
//  website's Supabase project; the host runs the browser step), the sync
//  and session-summary switches, and syncing now. Nothing is uploaded unless
//  the user is signed in and has turned sync on; summaries need their own
//  switch. Both switches go off on sign-out and when the website changes,
//  and a new sign-in starts with them off. Every action does nothing when
//  sealed, where `cloud` is a fixed, signed-in example.
//
//  `cloud` is republished from `CloudSync.shared`. The running sessions the
//  hub attributes for certain feed the session ledger (`feedCloud`).
//

import Foundation

/// The website as the settings pane shows it.
public nonisolated struct ClaudeCloudState: Hashable, Sendable {
    public enum Auth: Hashable, Sendable {
        case signedOut
        case signingIn
        case signedIn(email: String?)
        /// The last sign-in failed; user-facing text.
        case error(String)
    }

    /// The website's address; nil until the user enters one (there is no default).
    public var websiteURL: String?
    /// `AGENTNOTCH_WEB_URL` set the address for this run.
    public var websiteIsOverridden: Bool
    public var auth: Auth
    /// Upload sessions and usage (while signed in). Off by default.
    public var syncEnabled: Bool
    /// Summarise finished sessions with Claude Code and send the summaries. Off by default.
    public var summariesEnabled: Bool
    /// This run may launch Claude Code for summaries (false when sealed,
    /// in a `--no-install` dev run, or before bootstrap).
    public var summariesAvailable: Bool
    public var isSyncing: Bool
    public var lastSyncAt: Date?
    /// What went wrong last; nil after a good sync. User-facing text.
    public var lastError: String?
    /// Sessions changed since they were sent, as of the last pass.
    public var pendingSessions: Int
    /// Usage readings waiting to be sent.
    public var pendingUsage: Int
    /// Sessions summarised on this Mac.
    public var summarizedSessions: Int
    /// "Open dashboard": what the website says, once signed in.
    public var dashboardURL: URL?

    public init(websiteURL: String? = nil, websiteIsOverridden: Bool = false, auth: Auth = .signedOut,
                syncEnabled: Bool = false, summariesEnabled: Bool = false, summariesAvailable: Bool = false,
                isSyncing: Bool = false, lastSyncAt: Date? = nil, lastError: String? = nil,
                pendingSessions: Int = 0, pendingUsage: Int = 0, summarizedSessions: Int = 0,
                dashboardURL: URL? = nil) {
        self.websiteURL = websiteURL
        self.websiteIsOverridden = websiteIsOverridden
        self.auth = auth
        self.syncEnabled = syncEnabled
        self.summariesEnabled = summariesEnabled
        self.summariesAvailable = summariesAvailable
        self.isSyncing = isSyncing
        self.lastSyncAt = lastSyncAt
        self.lastError = lastError
        self.pendingSessions = pendingSessions
        self.pendingUsage = pendingUsage
        self.summarizedSessions = summarizedSessions
        self.dashboardURL = dashboardURL
    }

    public var isSignedIn: Bool {
        if case .signedIn = auth { return true }
        return false
    }

    /// The signed-in email, if the website said.
    public var email: String? {
        if case .signedIn(let email) = auth { return email }
        return nil
    }

    /// Sharing accounts with other people happens on the website: `<dashboard>/pools`.
    public var poolsURL: URL? { dashboardURL?.appendingPathComponent("pools") }

    /// What a sealed run shows: signed in to an example website, sync on,
    /// the last sync three minutes ago. Nothing behind it is real.
    static func sealedFixture(now: Date) -> ClaudeCloudState {
        ClaudeCloudState(
            websiteURL: "https://agentnotch.example.com",
            auth: .signedIn(email: "me@example.com"),
            syncEnabled: true,
            summariesEnabled: false,
            summariesAvailable: false,
            lastSyncAt: now.addingTimeInterval(-180),
            dashboardURL: URL(string: "https://agentnotch.example.com/dashboard")
        )
    }
}

extension ClaudeControlHub {
    // MARK: - Actions

    /// Set the website's address (nil or blank clears it). False when it
    /// isn't one the app accepts: https, or http to this Mac. Another
    /// website signs out of the old one.
    @discardableResult
    public func setCloudWebsite(_ address: String?) async -> Bool {
        guard !isSealed else { return false }
        return await CloudSync.shared.setWebsite(address)
    }

    /// Sign in with Google. `browser` opens the URL it is given and returns
    /// the URL the sign-in came back to (`agentnotch://auth-callback…`): an
    /// `ASWebAuthenticationSession` with callback scheme `agentnotch`.
    /// True when signed in; otherwise `cloud` says why.
    @discardableResult
    public func cloudSignIn(presentingBrowser browser: @escaping ClaudeCloudBrowser) async -> Bool {
        guard !isSealed else { return false }
        return await CloudSync.shared.signIn(presentingBrowser: browser)
    }

    /// Sign out of the website on this Mac. Sync and summaries go off, and
    /// readings waiting to be sent go.
    public func cloudSignOut() async {
        guard !isSealed else { return }
        await CloudSync.shared.signOut()
    }

    /// The sync switch. Nothing is uploaded while it is off.
    public func setCloudSync(_ enabled: Bool) {
        guard !isSealed else { return }
        CloudSync.shared.setSyncEnabled(enabled)
    }

    /// The session-summaries switch (spends the account's usage; off by default).
    public func setSessionSummaries(_ enabled: Bool) {
        guard !isSealed else { return }
        CloudSync.shared.setSummariesEnabled(enabled)
    }

    /// Sync now (when signed in and sync is on).
    public func syncCloudNow() async {
        guard !isSealed else { return }
        await CloudSync.shared.syncNow()
    }

    /// "Open dashboard", once the website has said where it is.
    public var cloudDashboardURL: URL? { cloud.dashboardURL }

    /// Where accounts are shared with other people (`<dashboard>/pools`).
    public var cloudPoolsURL: URL? { cloud.poolsURL }

    // MARK: - Feeding the ledger

    /// The running sessions the hub attributed for certain, with their
    /// account, for the session ledger (only while sync is on; the ledger
    /// keeps allowed accounts only). `liveIDs` is every session running now.
    func feedCloud(_ attributed: [(state: SessionState, identity: ClaudeIdentityAccount)], liveIDs: Set<String>) {
        guard !isSealed else { return }
        let observations = attributed.compactMap { Self.cloudObservation(state: $0.state, identity: $0.identity) }
        CloudSync.shared.observeLive(observations, liveIDs: liveIDs)
    }

    /// A running session as the ledger captures it: its account's key (from
    /// the account's own UUID and organization; `CloudSync` keys it again
    /// from the registry's accounts), where it started, its transcript,
    /// where it runs, its times, when its process started, model, cost and
    /// title (never the first prompt). Nil for an account with no account
    /// UUID. Pure.
    nonisolated static func cloudObservation(state: SessionState, identity: ClaudeIdentityAccount) -> LiveSessionObservation? {
        guard let accountKey = CloudKeys.accountKey(identity: identity), !state.cwd.isEmpty else { return nil }
        var title: String?
        if state.titleSource != .derivedName, let text = state.sessionTitle?.trimmingCharacters(in: .whitespacesAndNewlines),
           !text.isEmpty {
            title = text
        } else if let summary = state.conversationInfo.summary?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !summary.isEmpty {
            title = summary
        }
        return LiveSessionObservation(
            sessionId: state.sessionId,
            identityId: identity.id,
            accountKey: accountKey,
            cwd: state.cwd,
            transcriptPath: state.transcriptPath,
            configDir: state.accountId,
            entrypoint: state.entrypoint,
            // When the app first saw it: one Claude Code process can outlive
            // a `/clear` into a new session, so its start isn't the
            // session's. The transcript's first line corrects this later.
            startedAt: state.createdAt,
            lastActivityAt: state.lastActivity,
            model: state.model,
            costUsd: state.costUSD,
            title: title,
            processStartedAt: state.pidStartedAt
        )
    }
}
