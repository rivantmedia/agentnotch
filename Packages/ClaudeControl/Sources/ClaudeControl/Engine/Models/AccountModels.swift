//
//  AccountModels.swift
//  ClaudeControl
//
//  One Claude Code config directory as the app sees it. An *account* is the
//  identity signed in to one or more of these (see `ClaudeIdentityAccount`):
//  with Claude Parallel Profiles one account lives in its stores, a working
//  copy per VS Code window and, when it was used last, `~/.claude`.
//

import Foundation

/// How the app learned about an account.
nonisolated enum AccountSource: String, Codable, Sendable {
    /// Found by scanning the home directory (~/.claude, ~/.claude-*).
    case discovered
    /// Seen in a hook event's transcript_path / CLAUDE_CONFIG_DIR.
    case hook
    /// Added by the user in settings.
    case manual
}

/// One Claude Code account, identified by its config directory.
nonisolated struct ClaudeAccount: Identifiable, Hashable, Codable, Sendable {
    /// Stable key: `AccountPaths.accountId(forConfigDir:)`, the normalized absolute config dir.
    let id: String
    /// Absolute config dir path, e.g. /Users/me/.claude or /Users/me/.claude-work.
    var configDir: String
    /// The raw CLAUDE_CONFIG_DIR value sessions of this account run with; nil
    /// for the default account (env var unset). Claude Code hashes this exact
    /// string to name its keychain item, so it is kept verbatim when known.
    var configDirEnv: String?
    /// User-chosen name shown instead of the default one.
    var customLabel: String?
    /// Every CLAUDE_CONFIG_DIR value sessions of this folder were seen with,
    /// oldest first; "" stands for "unset". More than one means two logins
    /// share the folder (Claude Code keys the login by the exact string), and
    /// `configDirEnv` then stays on one of them instead of flipping.
    var seenConfigDirEnvs: [String]

    // Identity, read from the account's .claude.json `oauthAccount`.
    var email: String?
    var displayName: String?
    var organizationName: String?
    /// The claude.ai organization; keys Claude Desktop's usage cache.
    var organizationUuid: String?
    var accountUuid: String?
    /// "max", "pro", "team", "enterprise"; from get_usage or the org type.
    var subscriptionType: String?
    /// e.g. "default_claude_max_20x".
    var rateLimitTier: String?

    /// Index into the account colour palette; assigned once and kept stable.
    var colorIndex: Int
    var source: AccountSource
    /// Last time a hook event or status line update came from this account.
    var lastSeenAt: Date?
    /// Hidden ("Track sessions and hooks" off): no hooks, no sessions, no
    /// probes. For a folder someone is signed in to, its identity's choice
    /// (the registry copies it in); stored per folder only for the others.
    var isHidden: Bool
    /// What the folder is for (`ConfigDirClassifier`): Claude Code runs
    /// here, or it is a Claude Parallel Profiles store. Set by the registry
    /// from its latest classification; not persisted.
    var kind: ConfigDirKind = .run

    /// This account's name and badge letters among every known account, set
    /// by AccountRegistry (see `AccountNaming`): the short form unless two
    /// accounts would read the same. Not persisted.
    var defaultLabel: String?
    var defaultMonogram: String?

    init(
        configDir: String,
        configDirEnv: String? = nil,
        customLabel: String? = nil,
        seenConfigDirEnvs: [String] = [],
        email: String? = nil,
        displayName: String? = nil,
        organizationName: String? = nil,
        organizationUuid: String? = nil,
        accountUuid: String? = nil,
        subscriptionType: String? = nil,
        rateLimitTier: String? = nil,
        colorIndex: Int = 0,
        source: AccountSource = .discovered,
        lastSeenAt: Date? = nil,
        isHidden: Bool = false,
        kind: ConfigDirKind = .run
    ) {
        self.id = AccountPaths.accountId(forConfigDir: configDir)
        self.configDir = AccountPaths.normalize(configDir)
        self.configDirEnv = configDirEnv
        self.customLabel = customLabel
        self.seenConfigDirEnvs = seenConfigDirEnvs
        self.email = email
        self.displayName = displayName
        self.organizationName = organizationName
        self.organizationUuid = organizationUuid
        self.accountUuid = accountUuid
        self.subscriptionType = subscriptionType
        self.rateLimitTier = rateLimitTier
        self.colorIndex = colorIndex
        self.source = source
        self.lastSeenAt = lastSeenAt
        self.isHidden = isHidden
        self.kind = kind
    }

    /// True for ~/.claude used without CLAUDE_CONFIG_DIR.
    var isDefault: Bool {
        (configDirEnv ?? "").isEmpty && AccountPaths.isDefaultConfigDir(configDir)
    }

    /// The name shown everywhere: the custom name, else the default one
    /// (`Claude Gmail`, `Claude (work)`, Codenotch's rule; see `AccountNaming`).
    var label: String {
        if let customLabel, !customLabel.isEmpty { return customLabel }
        return defaultLabel ?? AccountNaming.baseLabel(for: self)
    }

    /// Two letters for compact badges, told apart from every other
    /// account's by the registry (see `AccountNaming.assign`): the custom
    /// name's, else the email's domain's. Outside the registry, the first
    /// choice.
    var monogram: String {
        defaultMonogram ?? AccountNaming.monogramCandidates(for: self).first ?? "CC"
    }

    /// Human plan name, e.g. "Max 20x", "Pro".
    var planName: String? {
        if let tier = rateLimitTier?.lowercased() {
            if tier.contains("max_20x") { return "Max 20x" }
            if tier.contains("max_5x") { return "Max 5x" }
        }
        switch subscriptionType?.lowercased() {
        case "max": return "Max"
        case "pro": return "Pro"
        case "team": return "Team"
        case "enterprise": return "Enterprise"
        case .some(let other) where !other.isEmpty: return other.capitalized
        default: return nil
        }
    }

    /// Shell line that starts Claude Code in this account. The folder is
    /// single-quoted, so nothing in its name is expanded when pasted.
    var launchCommand: String {
        if isDefault { return "claude" }
        return "CLAUDE_CONFIG_DIR=\(ShellWords.quote(configDirEnv ?? configDir)) claude"
    }

    /// Signed in to claude.ai (per its `.claude.json`).
    var isSignedIn: Bool { email != nil || accountUuid != nil }
}

extension ClaudeAccount {
    static let sampleDefault = ClaudeAccount(
        configDir: "~/.claude",
        email: "me@personal.dev",
        displayName: "Me",
        subscriptionType: "max",
        rateLimitTier: "default_claude_max_20x",
        colorIndex: 0
    )

    static let sampleWork = ClaudeAccount(
        configDir: "~/.claude-work",
        configDirEnv: AccountPaths.normalize("~/.claude-work"),
        email: "me@company.com",
        organizationName: "Company",
        subscriptionType: "team",
        colorIndex: 1
    )
}
