import ClaudeControl
import Foundation

/// What each Claude ring is called and whose account it reads, readable from
/// any thread: a usage provider's `displayName`, `account()` and
/// `signInRoute` are nonisolated, and the store asks for them from wherever
/// it happens to be.
///
/// A ring is called what the account is called everywhere else (the panel,
/// settings, banners): the engine's `ownLabel` — the account's custom name
/// (typed at "New account…", or imported from Superpowered Vibe Notch), else
/// Codenotch's own rule for Claude profiles ("Claude Gmail" from the
/// signed-in address's domain, the whole address where two would share a
/// name, the folder, "Claude (work)", before anyone has signed in), extended
/// so the same address in two organizations still reads apart (GUX-4,
/// CS-4). Codenotch's rule is applied here only to an account the engine
/// gave no name. A nickname chosen in Settings wins over all of these; the
/// store applies it.
///
/// Fork-only file. Owned by WP-C.
final class ClaudeRingNames: @unchecked Sendable {
    static let shared = ClaudeRingNames()

    struct Entry: Equatable, Sendable {
        /// The ring's own name, before any nickname.
        var name: String
        var email: String?
        var plan: String?
        /// "Claude Code", or "Claude Code in ~/.claude-work".
        var source: String
        /// How to start Claude Code in this account.
        var launchCommand: String
        /// claude.ai organization, for Claude Desktop's reset grants.
        var organizationUuid: String?
    }

    private let lock = NSLock()
    private var entries: [String: Entry] = [:]

    func entry(_ ringID: String) -> Entry? {
        lock.lock()
        defer { lock.unlock() }
        return entries[ringID]
    }

    /// The ring's own name; a ring the box has not heard of yet is named from
    /// its id, the way an account that has never signed in is.
    func name(for ringID: String) -> String {
        entry(ringID)?.name ?? Self.folderName(ringID: ringID, configDir: nil)
    }

    /// Replace every entry with those for `accounts`. Returns the rings whose
    /// entry changed (added, removed or different).
    @discardableResult
    func update(_ accounts: [ClaudeAccountSummary]) -> Set<String> {
        let fresh = Self.entries(for: accounts)
        lock.lock()
        let old = entries
        entries = fresh
        lock.unlock()
        return Set(old.keys).union(fresh.keys).filter { old[$0] != fresh[$0] }
    }

    // MARK: - Rules (pure)

    /// One entry per ring. Accounts sharing a ring id (the same folder spelled
    /// two ways) take the first.
    static func entries(for accounts: [ClaudeAccountSummary]) -> [String: Entry] {
        var firsts: [ClaudeAccountSummary] = []
        var seen = Set<String>()
        for account in accounts where seen.insert(account.ringID).inserted {
            firsts.append(account)
        }
        let names = displayNames(for: firsts)
        var entries: [String: Entry] = [:]
        for account in firsts {
            entries[account.ringID] = Entry(
                name: names[account.ringID] ?? folderName(ringID: account.ringID, configDir: account.configDir),
                email: account.email,
                plan: account.planName,
                source: source(for: account),
                launchCommand: account.launchCommand,
                organizationUuid: account.organizationUuid
            )
        }
        return entries
    }

    /// The engine's own name for each account; Codenotch's
    /// `ClaudeProfile.displayNames` for any the engine didn't name.
    static func displayNames(for accounts: [ClaudeAccountSummary]) -> [String: String] {
        var named: [String: String] = [:]
        var unnamed: [ClaudeAccountSummary] = []
        for account in accounts {
            if let own = account.ownLabel?.trimmingCharacters(in: .whitespacesAndNewlines), !own.isEmpty {
                named[account.ringID] = own
            } else {
                unnamed.append(account)
            }
        }
        return named.merging(codenotchNames(for: unnamed)) { engine, _ in engine }
    }

    /// Where the account runs, for Codenotch's Accounts pane: "Claude Code"
    /// for `~/.claude` alone, "Claude Code in ~/.claude and 2 VS Code
    /// workspaces" for an account Claude Parallel Profiles spreads over
    /// windows (a workspace's folder outlives its window), and "Claude
    /// Parallel Profiles account (no window open)" for one only a store
    /// holds now.
    static func source(for account: ClaudeAccountSummary) -> String {
        if account.runDirs.isEmpty, !account.storeDirs.isEmpty {
            return "Claude Parallel Profiles account (no window open)"
        }
        let others = account.runDirs.filter { !$0.contains("/.claude-windows/") }
        if account.windowCount == 0, others.count <= 1 {
            return account.isDefault ? "Claude Code" : "Claude Code in \(ClaudeProfile.tilde(others.first ?? account.configDir))"
        }
        var places = others.map { ClaudeProfile.tilde($0) }
        if account.windowCount > 0 {
            places.append(account.windowCount == 1 ? "1 VS Code workspace" : "\(account.windowCount) VS Code workspaces")
        }
        return "Claude Code in " + ListFormatter.localizedString(byJoining: places)
    }

    /// Codenotch's `ClaudeProfile.displayNames`, over our accounts.
    static func codenotchNames(for accounts: [ClaudeAccountSummary]) -> [String: String] {
        func own(_ account: ClaudeAccountSummary) -> String {
            if let label = ClaudeProfile.accountLabel(forAddress: account.email) {
                return "Claude \(label)"
            }
            return folderName(ringID: account.ringID, configDir: account.configDir)
        }
        var sharing: [String: Int] = [:]
        for account in accounts { sharing[own(account), default: 0] += 1 }
        var names: [String: String] = [:]
        for account in accounts {
            let name = own(account)
            if sharing[name, default: 0] > 1, let email = account.email, !email.isEmpty {
                names[account.ringID] = "Claude \(email)"
            } else {
                names[account.ringID] = name
            }
        }
        return names
    }

    /// The name of an account nobody has signed into: "Claude" for the
    /// default folder, "Claude (work)" for `~/.claude-work`, and the folder's
    /// own name for anything else.
    static func folderName(ringID: String, configDir: String?) -> String {
        if ringID == ClaudeRingIdentity.defaultRingID || ringID.hasPrefix("claude-acct-") { return "Claude" }
        if !ringID.hasPrefix("claude-dir-"), let slug = ClaudeProfile.slug(fromProviderID: ringID) {
            return "Claude (\(slug))"
        }
        guard let configDir else { return "Claude" }
        var folder = (configDir as NSString).lastPathComponent
        while folder.hasPrefix(".") { folder.removeFirst() }
        return folder.isEmpty ? "Claude" : "Claude (\(folder))"
    }
}
