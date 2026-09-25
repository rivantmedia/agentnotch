//
//  ClaudeControlSettings.swift
//  ClaudeControl
//
//  The engine's own preferences, in the host app's defaults domain under
//  `claudeControl.*` (the UserDefaults comes from the frozen configuration).
//  Everything the host already owns stays the host's: sounds, peek, account
//  nicknames, which rings are shown, their order, the weekly ring.
//
//  Ported from Superpowered Vibe Notch's `AppSettings`, without its
//  notification sound, Claude folder override, idle rings and ring-count
//  settings (Codenotch has its own).
//

import Foundation

public nonisolated enum ClaudeControlSettings {
    private static var defaults: UserDefaults { AppIdentity.defaults }

    // MARK: - Keys

    /// Every key, so tests and "reset" can find them.
    public enum Key {
        public static let prefix = "claudeControl."
        public static let hookConsent = prefix + "hookConsent"
        public static let hooksEnabled = prefix + "hooksEnabled"
        /// What the yes to "Turn on" covered (`AccountHookManager.installScope`).
        public static let hookConsentScope = prefix + "hookConsentScope"
        public static let statusLineIntegration = prefix + "statusLineIntegration"
        public static let usageProbeIntervalMinutes = prefix + "usageProbeIntervalMinutes"
        public static let readsDesktopUsageCache = prefix + "readsDesktopUsageCache"
        public static let notifyNeedsInput = prefix + "notifyNeedsInput"
        public static let notifyReadyForReview = prefix + "notifyReadyForReview"
        public static let autoOpen = prefix + "autoOpen"
        public static let holdOpenWhileNeedsYou = prefix + "holdOpenWhileNeedsYou"
        public static let ringBadges = prefix + "ringBadges"
        public static let restingMarks = prefix + "restingMarks"
        public static let dockBadge = prefix + "dockBadge"
        public static let ringClick = prefix + "ringClick"
        public static let sessionClick = prefix + "sessionClick"
        public static let panelPinned = prefix + "panelPinned"
        public static let hotKey = prefix + "hotKey"
        public static let didImportVibeNotch = prefix + "didImportVibeNotch"
        public static let claudeBinaryPath = prefix + "claudeBinaryPath"
        /// The website sync goes to (typed by the user; there is no default).
        public static let cloudWebsiteURL = prefix + "cloudWebsiteURL"
        public static let cloudSyncEnabled = prefix + "cloudSyncEnabled"
        public static let cloudSummariesEnabled = prefix + "cloudSummariesEnabled"
        /// This Mac's id on the website, made once.
        public static let cloudDeviceId = prefix + "cloudDeviceId"
    }

    private static func bool(_ key: String, default value: Bool) -> Bool {
        defaults.object(forKey: key) as? Bool ?? value
    }

    private static func choice<T: RawRepresentable>(_ key: String, default value: T) -> T where T.RawValue == String {
        defaults.string(forKey: key).flatMap(T.init(rawValue:)) ?? value
    }

    // MARK: - Hooks

    /// The hook settings over the configured defaults. Services that write
    /// settings.json take a `Store` so tests can give them their own domain.
    static var store: Store { Store(defaults: defaults) }

    /// The user's answer to "Turn on Claude Code control": true after
    /// [Turn on], false after [Not now], nil while unanswered. Nothing is
    /// written to any settings.json until this is true.
    public static var hookConsent: Bool? {
        get { store.hookConsent }
        set { store.hookConsent = newValue }
    }

    /// Whether the app keeps its hooks installed in every tracked account's
    /// settings.json. Never true before `hookConsent` is.
    public static var hooksEnabled: Bool {
        get { store.hooksEnabled }
        set { store.hooksEnabled = newValue }
    }

    /// Whether the status line wrapper is installed as each account's
    /// `statusLine` (wrapping, not replacing, any status line already there).
    /// It feeds live rate limits and context usage to the app. Defaults to on.
    public static var statusLineIntegration: Bool {
        get { store.statusLineIntegration }
        set { store.statusLineIntegration = newValue }
    }

    /// A `claude` executable the user chose ("Choose claude binary…"), used
    /// before any lookup. Nil: find it automatically.
    public static var claudeBinaryPath: String? {
        get { store.claudeBinaryPath }
        set { store.claudeBinaryPath = newValue }
    }

    // MARK: - Usage

    /// Default for `usageProbeIntervalMinutes`.
    public static let defaultUsageProbeInterval = 5

    /// How often, in minutes, to ask Claude Code for an account's usage when no
    /// fresher data has arrived. 0 turns the background probe off (usage then
    /// comes only from live status lines and Claude Code's own cache).
    public static var usageProbeIntervalMinutes: Int {
        get {
            guard let value = defaults.object(forKey: Key.usageProbeIntervalMinutes) as? Int else {
                return defaultUsageProbeInterval
            }
            return max(0, value)
        }
        set { defaults.set(max(0, newValue), forKey: Key.usageProbeIntervalMinutes) }
    }

    /// Also read Claude Desktop's cached usage (no token involved). Defaults to on.
    public static var readsDesktopUsageCache: Bool {
        get { bool(Key.readsDesktopUsageCache, default: true) }
        set { defaults.set(newValue, forKey: Key.readsDesktopUsageCache) }
    }

    // MARK: - Notifications

    /// Post a macOS notification when a session starts needing the user
    /// (permission, question, plan, a dialog), or a turn fails (its own
    /// "stopped" banner). Defaults to on.
    public static var notifyNeedsInput: Bool {
        get { bool(Key.notifyNeedsInput, default: true) }
        set { defaults.set(newValue, forKey: Key.notifyNeedsInput) }
    }

    /// Post a macOS notification when a session finishes a turn that is now
    /// waiting for review. Defaults to on.
    public static var notifyReadyForReview: Bool {
        get { bool(Key.notifyReadyForReview, default: true) }
        set { defaults.set(newValue, forKey: Key.notifyReadyForReview) }
    }

    // MARK: - Attention

    /// When the sessions panel opens by itself. Defaults to `.needsInput`.
    public static var autoOpen: AutoOpenPolicy {
        get { choice(Key.autoOpen, default: AutoOpenPolicy.needsInput) }
        set { defaults.set(newValue.rawValue, forKey: Key.autoOpen) }
    }

    /// Whether the notch stays open while a session needs you. Defaults to `.auto`.
    public static var holdOpenWhileNeedsYou: HoldOpenPolicy {
        get { choice(Key.holdOpenWhileNeedsYou, default: HoldOpenPolicy.auto) }
        set { defaults.set(newValue.rawValue, forKey: Key.holdOpenWhileNeedsYou) }
    }

    /// Needs-you and review counts on the Claude rings. Defaults to on.
    public static var ringBadges: Bool {
        get { bool(Key.ringBadges, default: true) }
        set { defaults.set(newValue, forKey: Key.ringBadges) }
    }

    /// Dots on the folded notch. Defaults to on.
    public static var restingMarks: Bool {
        get { bool(Key.restingMarks, default: true) }
        set { defaults.set(newValue, forKey: Key.restingMarks) }
    }

    /// The needs-you count on the Dock icon. Defaults to on.
    public static var dockBadge: Bool {
        get { bool(Key.dockBadge, default: true) }
        set { defaults.set(newValue, forKey: Key.dockBadge) }
    }

    // MARK: - Panel

    /// What a click on a Claude ring does. Defaults to `.openPanel`.
    public static var ringClick: RingClickAction {
        get { choice(Key.ringClick, default: RingClickAction.openPanel) }
        set { defaults.set(newValue.rawValue, forKey: Key.ringClick) }
    }

    /// What a click on a hover-card session row does. Defaults to `.smart`.
    public static var sessionClick: SessionClickAction {
        get { choice(Key.sessionClick, default: SessionClickAction.smart) }
        set { defaults.set(newValue.rawValue, forKey: Key.sessionClick) }
    }

    /// The panel header's "Keep open" pin. Defaults to off.
    public static var panelPinned: Bool {
        get { bool(Key.panelPinned, default: false) }
        set { defaults.set(newValue, forKey: Key.panelPinned) }
    }

    /// Global shortcut for the panel. Defaults to `.off`.
    public static var hotKey: PanelHotKey {
        get { choice(Key.hotKey, default: PanelHotKey.off) }
        set { defaults.set(newValue.rawValue, forKey: Key.hotKey) }
    }

    // MARK: - Website

    /// The website's address, as the user entered it (https, or http to this
    /// Mac). Nil until they do: there is no built-in website. Change it
    /// through the hub (`setCloudWebsite`), which signs out of the old one.
    public static var cloudWebsiteURL: String? { store.cloudWebsiteURL }

    /// Upload sessions and usage readings to the website (while signed in).
    /// Defaults to off. Change it through the hub (`setCloudSync`).
    public static var cloudSyncEnabled: Bool { store.cloudSyncEnabled }

    /// Have Claude Code write a one- or two-sentence summary of each
    /// finished session, and send it with the session. Spends the account's
    /// usage. Defaults to off. Change it through the hub (`setSessionSummaries`).
    public static var cloudSummariesEnabled: Bool { store.cloudSummariesEnabled }

    // MARK: - Migration

    /// Superpowered Vibe Notch's accounts and review queue were imported once.
    public static var didImportVibeNotch: Bool {
        get { store.didImportVibeNotch }
        set { store.didImportVibeNotch = newValue }
    }
}

// MARK: - Store

extension ClaudeControlSettings {
    /// The settings the hook and account services read and write, over one
    /// UserDefaults. The statics above are this over the configured domain.
    nonisolated struct Store: @unchecked Sendable {
        let defaults: UserDefaults

        private func bool(_ key: String, default value: Bool) -> Bool {
            defaults.object(forKey: key) as? Bool ?? value
        }

        var hookConsent: Bool? {
            get { defaults.object(forKey: Key.hookConsent) as? Bool }
            nonmutating set {
                if let newValue { defaults.set(newValue, forKey: Key.hookConsent) }
                else { defaults.removeObject(forKey: Key.hookConsent) }
            }
        }

        var hooksEnabled: Bool {
            get { hookConsent == true && bool(Key.hooksEnabled, default: true) }
            nonmutating set { defaults.set(newValue, forKey: Key.hooksEnabled) }
        }

        /// What the yes covered: 0 before this was recorded (each account's
        /// own folder), `AccountHookManager.installScope` since.
        var hookConsentScope: Int {
            get { defaults.integer(forKey: Key.hookConsentScope) }
            nonmutating set { defaults.set(newValue, forKey: Key.hookConsentScope) }
        }

        var statusLineIntegration: Bool {
            get { bool(Key.statusLineIntegration, default: true) }
            nonmutating set { defaults.set(newValue, forKey: Key.statusLineIntegration) }
        }

        var claudeBinaryPath: String? {
            get { defaults.string(forKey: Key.claudeBinaryPath).flatMap { $0.isEmpty ? nil : $0 } }
            nonmutating set {
                if let newValue, !newValue.isEmpty { defaults.set(newValue, forKey: Key.claudeBinaryPath) }
                else { defaults.removeObject(forKey: Key.claudeBinaryPath) }
            }
        }

        var didImportVibeNotch: Bool {
            get { bool(Key.didImportVibeNotch, default: false) }
            nonmutating set { defaults.set(newValue, forKey: Key.didImportVibeNotch) }
        }

        var cloudWebsiteURL: String? {
            get { defaults.string(forKey: Key.cloudWebsiteURL).flatMap { $0.isEmpty ? nil : $0 } }
            nonmutating set {
                if let newValue, !newValue.isEmpty { defaults.set(newValue, forKey: Key.cloudWebsiteURL) }
                else { defaults.removeObject(forKey: Key.cloudWebsiteURL) }
            }
        }

        var cloudSyncEnabled: Bool {
            get { bool(Key.cloudSyncEnabled, default: false) }
            nonmutating set { defaults.set(newValue, forKey: Key.cloudSyncEnabled) }
        }

        var cloudSummariesEnabled: Bool {
            get { bool(Key.cloudSummariesEnabled, default: false) }
            nonmutating set { defaults.set(newValue, forKey: Key.cloudSummariesEnabled) }
        }

        /// This Mac's id on the website: a UUID made on first use and kept.
        var cloudDeviceId: String {
            if let saved = defaults.string(forKey: Key.cloudDeviceId), UUID(uuidString: saved) != nil { return saved }
            let made = UUID().uuidString
            defaults.set(made, forKey: Key.cloudDeviceId)
            return made
        }
    }
}

/// When the sessions panel opens on its own for session activity.
public nonisolated enum AutoOpenPolicy: String, CaseIterable, Sendable {
    /// Never; the rings only show counts and the chime plays.
    case never
    /// When a session needs input (permission, question, plan, a dialog) and
    /// no terminal is visible. Never for a failed turn (rate limit, overload,
    /// sign-in): that gets a soft cue only (`ClaudeAttentionPolicy.decide`).
    case needsInput
    /// Also when a session finishes and is ready for review.
    case needsInputOrDone

    public var displayName: String {
        switch self {
        case .never: return "Never"
        case .needsInput: return "Needs you"
        case .needsInputOrDone: return "Needs you or done"
        }
    }

    /// One line for the picker, saying when it holds back.
    public var detail: String {
        switch self {
        case .never:
            return "The rings and the chime still tell you."
        case .needsInput:
            return "When a session needs you, unless you're already in its terminal."
        case .needsInputOrDone:
            return "Also when a session is done, unless you're already in its terminal."
        }
    }
}

/// Whether the notch is held open while a session needs you.
public nonisolated enum HoldOpenPolicy: String, CaseIterable, Sendable {
    /// Only notches whose folded state cannot show marks (flush with the camera).
    case auto
    case always
    case never

    public var displayName: String {
        switch self {
        case .auto: return "Auto"
        case .always: return "Always"
        case .never: return "Never"
        }
    }
}

/// What a click on a Claude ring does.
public nonisolated enum RingClickAction: String, CaseIterable, Sendable {
    case openPanel
    case refresh

    public var displayName: String {
        switch self {
        case .openPanel: return "Open sessions"
        case .refresh: return "Refresh usage"
        }
    }
}

/// What a click on a session row in the hover card does.
public nonisolated enum SessionClickAction: String, CaseIterable, Sendable {
    /// Needs input opens the panel at that session; anything else jumps to the terminal.
    case smart
    case panel
    case terminal

    public var displayName: String {
        switch self {
        case .smart: return "Smart"
        case .panel: return "Panel"
        case .terminal: return "Terminal"
        }
    }
}

/// The panel's global shortcut.
public nonisolated enum PanelHotKey: String, CaseIterable, Sendable {
    case off
    /// ⌃⌥Space
    case controlOptionSpace
    /// ⌥⌘J
    case optionCommandJ

    public var displayName: String {
        switch self {
        case .off: return "Off"
        case .controlOptionSpace: return "⌃⌥Space"
        case .optionCommandJ: return "⌥⌘J"
        }
    }
}
