//
//  AppEventBus.swift
//  ClaudeControl
//
//  Small main-actor publishers that decouple the hook/socket pipeline from
//  the account and usage services. The socket side publishes; AccountRegistry
//  and UsageStore subscribe. Neither side needs to import the other.
//

import Combine
import Foundation

/// A config dir observed in a hook or status line event.
nonisolated struct AccountSighting: Sendable, Equatable {
    /// Normalized config dir (derived from transcript_path, else CLAUDE_CONFIG_DIR, else ~/.claude).
    let configDir: String
    /// Raw CLAUDE_CONFIG_DIR from the hook's environment; nil when unset.
    let configDirEnv: String?
    let sessionId: String
    let at: Date
}

/// Parsed status line payload from a live terminal session.
nonisolated struct StatusLineUpdate: Sendable, Equatable {
    let sessionId: String
    let transcriptPath: String?
    /// Raw CLAUDE_CONFIG_DIR from the status line process environment; nil when unset.
    let configDirEnv: String?
    /// Account ID derived from transcript_path (preferred) or configDirEnv.
    let accountId: String?
    let receivedAt: Date

    /// `rate_limits.five_hour` / `rate_limits.seven_day`, when Claude Code has them.
    let fiveHour: UsageWindow?
    let sevenDay: UsageWindow?

    /// `context_window.used_percentage`, 0...100.
    let contextUsedPercent: Double?
    /// `context_window.context_window_size` in tokens.
    let contextWindowSize: Int?

    let modelId: String?
    let modelDisplayName: String?
    /// `cost.total_cost_usd` for the session.
    let costUSD: Double?
    let sessionName: String?
    let claudeCodeVersion: String?
}

/// A place in the opened notch panel that another component wants to show,
/// e.g. a notification click opening the session it was about.
nonisolated enum PanelRoute: Equatable, Sendable {
    /// The session list.
    case sessions
    /// One session's chat.
    case session(id: String)
    /// The Usage tab, optionally scrolled to one account.
    case usage(accountId: String?)
    /// The settings menu, optionally with the Accounts section expanded.
    case settings(showAccounts: Bool)
}

@MainActor
final class AppEventBus {
    static let shared = AppEventBus()

    /// A hook event revealed which account a session runs under.
    let accountSightings = PassthroughSubject<AccountSighting, Never>()

    /// A status line update arrived from a live terminal session.
    let statusLineUpdates = PassthroughSubject<StatusLineUpdate, Never>()

    /// Ask the notch to open (if closed) and show a route. The notch view
    /// model subscribes; notifications, hotkeys and views publish.
    let panelRequests = PassthroughSubject<PanelRoute, Never>()

    private init() {}
}
