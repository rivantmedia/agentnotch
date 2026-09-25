//
//  ClaudeSummaries.swift
//  ClaudeControl
//
//  The value types the app sees: accounts, sessions, attention counts,
//  transitions, panel routes, ring readings and hover-card rows. Plain,
//  Sendable, and free of engine internals, so the bridge never needs one.
//
//  Frozen at Step 0 (design §12); signature changes go through the lead.
//

import CryptoKit
import Foundation

// MARK: - Accounts

/// What an account's settings.json says about our hooks.
public nonisolated struct ClaudeHookStatus: Hashable, Sendable {
    /// Our hook command is registered and the script is in place.
    public var hooksInstalled: Bool
    /// The account's `statusLine` runs our wrapper.
    public var statusLineInstalled: Bool
    /// Upstream Vibe Notch's `claude-island-state.py` hooks are registered.
    public var vibeNotchHooksPresent: Bool
    /// Superpowered Vibe Notch's `superpowered-notch-*.py` hooks are registered.
    public var superpoweredVibeNotchHooksPresent: Bool
    /// The last install or uninstall failed; user-facing text.
    public var lastError: String?
    /// How many folders the account's hooks go into (its run folders:
    /// `~/.claude`, VS Code windows, standalone folders), and in how many of
    /// them they are in place: "Hooks in 3 of 4 folders".
    public var folderCount: Int = 1
    public var installedFolderCount: Int = 0

    public init(
        hooksInstalled: Bool = false,
        statusLineInstalled: Bool = false,
        vibeNotchHooksPresent: Bool = false,
        superpoweredVibeNotchHooksPresent: Bool = false,
        lastError: String? = nil
    ) {
        self.hooksInstalled = hooksInstalled
        self.statusLineInstalled = statusLineInstalled
        self.vibeNotchHooksPresent = vibeNotchHooksPresent
        self.superpoweredVibeNotchHooksPresent = superpoweredVibeNotchHooksPresent
        self.lastError = lastError
    }

    /// Any hooks another app left behind.
    public var legacyHooksPresent: Bool { vibeNotchHooksPresent || superpoweredVibeNotchHooksPresent }
}

/// One Claude account: one signed-in identity, whatever config folders it
/// lives in (with Claude Parallel Profiles: `~/.claude` when it was used
/// last, a working copy per VS Code window, and its account stores).
public nonisolated struct ClaudeAccountSummary: Identifiable, Hashable, Sendable {
    /// The engine's account id: the identity (`uuid:…`, `email:…`), or
    /// `dir:<folder>` for a folder added by hand that nobody signed in to.
    public var id: String
    /// Codenotch's ring id, see `ClaudeRingIdentity`.
    public var ringID: String
    public var configDir: String
    /// The account's complete name ("Claude Gmail", "Work"): the custom
    /// name, else Codenotch's default rule (the email's short form, the full
    /// address when two collide, the folder when signed out). The hub
    /// replaces it with Codenotch's nickname for the ring when one is set.
    public var label: String
    public var email: String?
    /// "Max 20x", "Pro", …
    public var planName: String?
    /// claude.ai organization, for Claude Desktop's usage cache.
    public var organizationUuid: String?
    /// `~/.claude` used without CLAUDE_CONFIG_DIR.
    public var isDefault: Bool
    /// "Track sessions and hooks": off means no hooks and no sessions.
    public var isTracked: Bool
    /// Index into the account colour palette; stable.
    public var colorIndex: Int
    public var hooks: ClaudeHookStatus
    /// Shell line that starts Claude Code in this account.
    public var launchCommand: String
    /// Folders Claude Code runs in as this account, `~/.claude` first then
    /// VS Code windows then standalone folders (where hooks go).
    public var runDirs: [String] = []
    /// Claude Parallel Profiles account stores holding it (read, never written).
    public var storeDirs: [String] = []
    /// How many of `runDirs` are VS Code windows' working copies.
    public var windowCount: Int = 0
    /// Someone is signed in (false only for a folder added by hand).
    public var isSignedIn: Bool = true
    /// The ring ids its folders had when every folder was an account
    /// (`claude`, `claude-work`, …): Codenotch's name, order, on/off and
    /// archived reading for those move to this ring once.
    public var formerRingIDs: [String] = []
    /// Of `formerRingIDs`, those that may not be this account's: `~/.claude`'s
    /// (`claude`) while Claude Parallel Profiles has mirrored this account
    /// into it and the file's own login is nobody known. Their choices move
    /// only when no other former ring has one.
    public var uncertainFormerRingIDs: Set<String> = []
    /// Whether it can be forgotten (Settings' "Forget…"): not the account
    /// `~/.claude` runs as on its own; any account Claude Parallel Profiles
    /// keeps in a store or a VS Code window.
    public var canForget: Bool = true
    /// `launchCommand` starts it from a terminal. False when it runs only in
    /// VS Code windows or only a store holds it: a window's working copy is
    /// that window's (a `/login` there switches the window), and Claude Code
    /// is never run in a store. Settings says how to use it instead.
    public var hasTerminalLaunch: Bool = true
    /// Of `runDirs`, the user's own profiles Claude Parallel Profiles also
    /// copies accounts from (adopted, in its manifest's `stores`).
    public var adoptedDirs: [String] = []

    /// Every folder of the account: run folders, then stores.
    public var configDirs: [String] { runDirs + storeDirs }

    /// The account's own name, before any Codenotch nickname: the custom
    /// name (typed at "New account…", or imported from Superpowered Vibe
    /// Notch), else the default rule, told apart from every other account's
    /// (`AccountNaming`). What the ring is called when it has no nickname,
    /// so the ring, hover card, panel and settings agree (GUX-4, CS-4).
    /// `label` is this with the nickname applied.
    public var ownLabel: String?

    public init(
        id: String,
        ringID: String,
        configDir: String,
        label: String,
        email: String? = nil,
        planName: String? = nil,
        organizationUuid: String? = nil,
        isDefault: Bool,
        isTracked: Bool = true,
        colorIndex: Int = 0,
        hooks: ClaudeHookStatus = ClaudeHookStatus(),
        launchCommand: String,
        ownLabel: String? = nil
    ) {
        self.id = id
        self.ringID = ringID
        self.configDir = configDir
        self.label = label
        self.email = email
        self.planName = planName
        self.organizationUuid = organizationUuid
        self.isDefault = isDefault
        self.isTracked = isTracked
        self.colorIndex = colorIndex
        self.hooks = hooks
        self.launchCommand = launchCommand
        self.ownLabel = ownLabel
    }
}

// MARK: - Sessions

/// What a session needs from the user right now.
public nonisolated enum ClaudeAttention: Hashable, Sendable {
    case needsInput(ClaudeNeedsInput)
    case working
    case readyForReview
    case idle
}

public nonisolated struct ClaudeNeedsInput: Hashable, Sendable {
    public enum Kind: Hashable, Sendable { case permission, question, plan, elicitation, dialog, error }
    public var kind: Kind
    /// Short, user-facing: "Allow Bash · npm test", "Rate limited". Never
    /// prompt or assistant message text.
    public var summary: String

    public init(kind: Kind, summary: String) {
        self.kind = kind
        self.summary = summary
    }
}

public nonisolated struct ClaudeTaskProgress: Hashable, Sendable {
    public var completed: Int
    public var total: Int
    /// The task being worked on, if any.
    public var active: String?

    public init(completed: Int, total: Int, active: String? = nil) {
        self.completed = completed
        self.total = total
        self.active = active
    }
}

public nonisolated struct ClaudeSessionSummary: Identifiable, Hashable, Sendable {
    /// Claude Code's session id.
    public var id: String
    /// The ring (account) it belongs to; `claude` when unknown.
    public var ringID: String
    public var pid: Int32?
    public var title: String
    public var projectName: String
    /// "iTerm2", "Terminal", "VS Code", "tmux", … when known.
    public var hostApp: String?
    public var attention: ClaudeAttention
    /// When the current attention started (wait start, turn start, completion,
    /// last activity).
    public var attentionSince: Date
    public var tasks: ClaudeTaskProgress?
    /// Context window used, 0...100.
    public var contextPercent: Double?
    public var backgroundTasks: Int
    /// The tool a working session is running now ("Bash", "Github - Create
    /// Issue"), nil between tools. Part of the summary so a tool starting or
    /// ending republishes the sessions, and hover rows follow it (BHV-4).
    public var runningTool: String?
    /// What a working session whose turn is over still waits for ("1
    /// workflow", "2 background agents"): they wake Claude when they finish.
    public var backgroundWait: String?

    public init(
        id: String,
        ringID: String,
        pid: Int32? = nil,
        title: String,
        projectName: String,
        hostApp: String? = nil,
        attention: ClaudeAttention,
        attentionSince: Date,
        tasks: ClaudeTaskProgress? = nil,
        contextPercent: Double? = nil,
        backgroundTasks: Int = 0,
        runningTool: String? = nil,
        backgroundWait: String? = nil
    ) {
        self.id = id
        self.ringID = ringID
        self.pid = pid
        self.title = title
        self.projectName = projectName
        self.hostApp = hostApp
        self.attention = attention
        self.attentionSince = attentionSince
        self.tasks = tasks
        self.contextPercent = contextPercent
        self.backgroundTasks = backgroundTasks
        self.runningTool = runningTool
        self.backgroundWait = backgroundWait
    }
}

public nonisolated struct ClaudeAttentionCounts: Hashable, Sendable {
    /// Waiting on something the user can answer now (a permission, question,
    /// plan, elicitation or terminal dialog). Drives every amber signal: the
    /// ring badge, the resting dot, the Dock badge, holding the notch open.
    public var needsYou: Int
    public var review: Int
    public var working: Int
    public var idle: Int
    /// Stopped on an error (rate limit, overload, sign-in, billing): shown as
    /// failed, never as amber "needs you" (GUX-2, BHV-2). The panel's strip
    /// counts it the same way.
    public var failed: Int

    public init(needsYou: Int = 0, review: Int = 0, working: Int = 0, idle: Int = 0, failed: Int = 0) {
        self.needsYou = needsYou
        self.review = review
        self.working = working
        self.idle = idle
        self.failed = failed
    }

    public static let zero = ClaudeAttentionCounts()

    /// Counts for `sessions`.
    public static func of<S: Sequence>(_ sessions: S) -> ClaudeAttentionCounts where S.Element == ClaudeSessionSummary {
        var counts = ClaudeAttentionCounts()
        for session in sessions {
            switch session.attention {
            case .needsInput(let needs) where needs.kind == .error: counts.failed += 1
            case .needsInput: counts.needsYou += 1
            case .readyForReview: counts.review += 1
            case .working: counts.working += 1
            case .idle: counts.idle += 1
            }
        }
        return counts
    }
}

/// A session crossed into needing you, into review, or out of both.
public nonisolated struct ClaudeAttentionTransition: Sendable {
    public enum Kind: Hashable, Sendable { case needsInput, readyForReview, resolved }
    public var kind: Kind
    public var session: ClaudeSessionSummary

    public init(kind: Kind, session: ClaudeSessionSummary) {
        self.kind = kind
        self.session = session
    }

    /// A turn that stopped on an error, not something to answer.
    public var isFailure: Bool {
        if kind == .needsInput, case .needsInput(let needs) = session.attention { return needs.kind == .error }
        return false
    }
}

/// What the first-run and error banners need to know.
public nonisolated struct ClaudeSetupState: Hashable, Sendable {
    /// "Turn on Claude Code control" has not been answered yet.
    public var needsHookConsent: Bool
    /// Superpowered Vibe Notch is running; nothing is installed meanwhile.
    public var vibeNotchRunning: Bool
    /// Some tracked account still has another app's hooks.
    public var legacyHooksFound: Bool
    /// The hook socket could not be opened; user-facing text.
    public var socketError: String?
    /// Of those, Superpowered Vibe Notch's: the ones "Take over" replaces.
    public var superpoweredVibeNotchHooksFound: Bool
    /// Upstream Vibe Notch's: left in place by consent and takeover; removed
    /// per account in Settings (GUX-8).
    public var vibeNotchHooksFound: Bool
    /// VS Code workspace folders (Claude Parallel Profiles' working copies)
    /// that got the hooks under a yes given before "Turn on" covered them:
    /// said once, with a way to turn it off.
    public var newInstallFolders: [String] = []

    public init(needsHookConsent: Bool = false, vibeNotchRunning: Bool = false,
                legacyHooksFound: Bool = false, socketError: String? = nil,
                superpoweredVibeNotchHooksFound: Bool = false, vibeNotchHooksFound: Bool = false) {
        self.needsHookConsent = needsHookConsent
        self.vibeNotchRunning = vibeNotchRunning
        self.legacyHooksFound = legacyHooksFound
        self.socketError = socketError
        self.superpoweredVibeNotchHooksFound = superpoweredVibeNotchHooksFound
        self.vibeNotchHooksFound = vibeNotchHooksFound
    }

    public init(needsHookConsent: Bool = false, vibeNotchRunning: Bool = false,
                legacyHooksFound: Bool = false, socketError: String? = nil,
                superpoweredVibeNotchHooksFound: Bool = false, vibeNotchHooksFound: Bool = false,
                newInstallFolders: [String]) {
        self.init(needsHookConsent: needsHookConsent, vibeNotchRunning: vibeNotchRunning,
                  legacyHooksFound: legacyHooksFound, socketError: socketError,
                  superpoweredVibeNotchHooksFound: superpoweredVibeNotchHooksFound,
                  vibeNotchHooksFound: vibeNotchHooksFound)
        self.newInstallFolders = newInstallFolders
    }
}

/// A place in the sessions panel.
public nonisolated enum ClaudePanelRoute: Hashable, Sendable {
    /// The list, pre-filtered to one ring (nil = all accounts).
    case sessions(ringID: String?)
    /// One session's chat.
    case session(id: String)
    /// The consent / setup card.
    case setup
}

public nonisolated enum ClaudeRefreshReason: Sendable {
    /// A ring click: probe only if the newest reading is older than 120 s.
    case ringClick
    /// "Refresh now": probe unless one ran in the last 60 s.
    case forced
}

// MARK: - Ring identity

/// Codenotch ring ids. Pure. An account (a signed-in identity) has
/// `claude-acct-<first 12 hex of sha256(accountUuid)>`, whatever folders it
/// lives in (`ringID(accountKey:)`). A folder nobody has signed in to keeps
/// the per-folder id below, which is also what every folder had before
/// accounts were identities (so their saved names and order can move over):
///
/// - `~/.claude` → `claude`
/// - `~/.claude-<slug>` → `claude-<slug>`, the ids upstream Codenotch uses, so
///   saved order, nicknames and archived readings carry over
/// - anything else → `claude-dir-<first 8 hex of sha256(normalized path)>`
///
/// Every id starts with `claude`, which upstream's `ClaudeProfile.isClaude`
/// relies on (default-on, daily pace, weekly headline, alerts, menu grouping).
public nonisolated enum ClaudeRingIdentity {
    public static let defaultRingID = "claude"

    public static func ringID(configDir: String, home: String) -> String {
        let path = normalize(configDir, home: home)
        let homePath = normalize(home, home: home)
        let parent = (path as NSString).deletingLastPathComponent
        let name = (path as NSString).lastPathComponent
        if parent == homePath {
            if name == ".claude" { return defaultRingID }
            // Upstream's rule (`ClaudeProfile.slug(fromDirectoryName:)`): any
            // non-empty slug.
            if name.hasPrefix(".claude-") {
                let slug = String(name.dropFirst(".claude-".count))
                if !slug.isEmpty { return "claude-" + slug }
            }
        }
        let digest = SHA256.hash(data: Data(path.utf8))
        let hex = digest.prefix(4).map { String(format: "%02x", $0) }.joined()
        return "claude-dir-" + hex
    }

    /// Whether `id` names a Claude ring (the same test upstream applies).
    static func isClaudeRing(_ id: String) -> Bool {
        id == defaultRingID || id.hasPrefix("claude-")
    }

    private static func normalize(_ path: String, home: String) -> String {
        var expanded = path
        if expanded == "~" { expanded = home }
        else if expanded.hasPrefix("~/") { expanded = (home as NSString).appendingPathComponent(String(expanded.dropFirst(2))) }
        var standardized = (expanded as NSString).standardizingPath
        while standardized.count > 1 && standardized.hasSuffix("/") { standardized.removeLast() }
        return standardized
    }
}

// MARK: - Ring readings

/// One Claude ring's usage, in the shape Codenotch's ring and hover card draw.
public nonisolated struct ClaudeRingReading: Hashable, Sendable {
    public struct Window: Hashable, Sendable {
        /// Money a window is metered in (extra usage).
        public struct Money: Hashable, Sendable {
            public var currency: String
            public var spent: Double
            public var remaining: Double

            public init(currency: String, spent: Double, remaining: Double) {
                self.currency = currency
                self.spent = spent
                self.remaining = remaining
            }
        }

        /// Codenotch's window ids: `session`, `weekly_all`, `weekly_<model>`, `extra_usage`.
        public var id: String
        /// Nil: the bridge uses Codenotch's own label for `id`.
        public var label: String?
        /// 0...1 (can exceed 1).
        public var usedFraction: Double
        public var resetsAt: Date?
        public var duration: TimeInterval?
        public var money: Money?

        public init(id: String, label: String? = nil, usedFraction: Double, resetsAt: Date? = nil,
                    duration: TimeInterval? = nil, money: Money? = nil) {
            self.id = id
            self.label = label
            self.usedFraction = usedFraction
            self.resetsAt = resetsAt
            self.duration = duration
            self.money = money
        }
    }

    public enum Status: Hashable, Sendable {
        case ok
        case waitingForFirstReading
        case signInNeeded(String)
        case unavailable(String)
        case failed(String)
    }

    public var windows: [Window]
    public var plan: String?
    public var updatedAt: Date?
    public var status: Status
    /// How old `updatedAt` may get before the reading shows as stale (see
    /// `isStale(now:)`). The hub sets it from the probe interval: never less
    /// than `staleAfter`, and one and a half intervals at longer settings.
    public var staleThreshold: TimeInterval = ClaudeRingReading.staleAfter
    /// The claude.ai organization of the account, when known (Claude
    /// Desktop's usage cache and its reset grants are keyed by it).
    public var organizationUuid: String?

    public init(windows: [Window] = [], plan: String? = nil, updatedAt: Date? = nil, status: Status) {
        self.windows = windows
        self.plan = plan
        self.updatedAt = updatedAt
        self.status = status
    }

    /// Readings older than this show as stale on the ring (at the default
    /// probe interval; see `staleThreshold`).
    public static let staleAfter: TimeInterval = 15 * 60

    /// The used-up window the account is waiting on, if any: of the windows
    /// at or over 100% that haven't reset, the one that resets last (a
    /// session limit hit during a used-up week lifts before the week does).
    /// Money-metered windows (extra usage) don't block. This is the window a
    /// "Rate limited" session waits for.
    public func exhaustedWindow(now: Date) -> Window? {
        windows
            .filter { window in
                window.money == nil && window.usedFraction >= 1
                    && (window.resetsAt.map { $0 > now } ?? true)
            }
            .max { ($0.resetsAt ?? .distantFuture) < ($1.resetsAt ?? .distantFuture) }
    }

    /// Whether the ring should show the reading as out of date: an `.ok`
    /// reading older than `staleThreshold`. A used-up window is never stale
    /// before its reset (it can't come down until then), so an account out
    /// of quota keeps its colour.
    public func isStale(now: Date) -> Bool {
        guard status == .ok, let updatedAt else { return false }
        guard now.timeIntervalSince(updatedAt) > staleThreshold else { return false }
        return exhaustedWindow(now: now) == nil
    }
}

// MARK: - Hover-card rows

/// One row of a Claude ring's hover card (Codenotch's `AgentSession`, field
/// for field). Never carries prompt or assistant message text.
public nonisolated struct ClaudeActivityRow: Hashable, Sendable {
    public enum State: Hashable, Sendable { case busy, waiting, success, idle }

    public var id: String
    public var name: String
    public var detail: String
    public var state: State
    public var waitingFor: String?
    public var since: Date
    public var pid: Int32?

    public init(id: String, name: String, detail: String, state: State,
                waitingFor: String? = nil, since: Date, pid: Int32? = nil) {
        self.id = id
        self.name = name
        self.detail = detail
        self.state = state
        self.waitingFor = waitingFor
        self.since = since
        self.pid = pid
    }
}
