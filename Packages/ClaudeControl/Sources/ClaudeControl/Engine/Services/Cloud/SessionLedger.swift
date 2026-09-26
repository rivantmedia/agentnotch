//
//  SessionLedger.swift
//  ClaudeControl
//
//  Which Claude account each session belonged to, remembered after it ends.
//  With Claude Parallel Profiles every folder's `projects/` is a link to one
//  shared history, so a transcript names no account; only the running
//  session can be attributed (the hub's attribution: the folder it runs in,
//  and for a mirrored `~/.claude`, who it ran as when the process started).
//  The ledger captures that while the session is live and keeps it.
//
//  One entry per session and account. A session resumed under another
//  account (the extension's "hit a limit, switch account, continue the same
//  conversation") keeps its id, so when a running session turns up under a
//  different account than it last ran as, the old account's part ends at
//  that moment and the new account's begins: the session's owners
//  (`SessionOwner`) say who ran it from when, and the transcript scanner
//  gives each account the responses made while it ran it. Each part is sent
//  as its own session row; the website keys sessions by account and id.
//
//  Captured per part: the account (identity id and its contract key), the
//  project (name, and the resolved working directory its key is made from,
//  local only), the transcript path and config folder (local only, never
//  synced), where it ran (CLI, VS Code, Claude Desktop, SDK), start, last
//  activity and end, the model, Claude Code's cost estimate from the status
//  line (sent only for a session one account ran: it is the process's
//  total, which can't be split), and its title (never a prompt). Only sessions of visible,
//  remembered, signed-in accounts that the hub could attribute for certain
//  are captured: an unsure one is never guessed onto an account.
//
//  While the hub can't attribute a running session for certain (a mirrored
//  `~/.claude` around an account switch, a Claude Desktop session whose
//  record isn't found), or it runs as an account the website may not hear
//  of, its new responses count for no account: the session's owners get a
//  stretch of nobody (""), from the last activity seen while it was
//  certain, and the account's part ends there. Once it is certain again,
//  its account takes over from then (from its process's start for a new
//  process; for the same process, whose responses in between were its own
//  all along, from where nobody began). A session first seen unsure is
//  remembered as nobody's from its start (`unattributed`), so a later,
//  certain process of it counts only its own responses. A session the hub
//  simply hasn't placed yet (`waiting`: a folder not grouped yet, a Claude
//  Desktop session whose registry entry hasn't been read) is neither: it
//  runs on as it was, nothing paused, until the hub places it.
//
//  A part that stops being live is ended a minute later (the time it went
//  away when that was seen while capturing, else its last activity), and
//  comes back to life if it shows up again.
//
//  The backfill adds sessions found on disk in folders whose `projects/` is
//  their own (not a link, and reached by no other folder), and only those
//  begun after the app first saw the folder signed in as the account it
//  names now (`CloudFolderLogins`): earlier ones may be another account's.
//  Live capture always wins over it.
//
//  Kept in `cloud-ledger.json` (atomic, 0600); in memory only when sealed.
//

import Foundation
import os.log

/// One session (or, for a session more than one account ran, one account's
/// part of it) the ledger knows.
nonisolated struct CloudLedgerEntry: Codable, Equatable, Sendable {
    enum Origin: String, Codable, Sendable {
        /// Seen running, attributed by the hub.
        case live
        /// Found on disk in a folder only one account uses.
        case backfill
    }

    var sessionId: String
    /// The engine's identity id (`uuid:…`), as of the last sighting.
    var identityId: String
    /// The contract's account key.
    var accountKey: String
    var projectName: String
    /// Local only: the working directory (`~` expanded, links resolved)
    /// the project key is made from. Never sent.
    var projectPath: String
    /// Local only: never sent.
    var transcriptPath: String?
    /// Local only: the config folder the session ran in.
    var configDir: String?
    var source: CloudSessionSource
    var startedAt: Date
    var lastActivityAt: Date
    var endedAt: Date?
    var model: String?
    var costUsd: Double?
    var title: String?
    var origin: Origin

    /// Where the ledger (and what was sent, and summaries) keep it: one
    /// per session and account.
    var key: String { Self.key(sessionId: sessionId, accountKey: accountKey) }

    static func key(sessionId: String, accountKey: String) -> String {
        sessionId + "|" + accountKey
    }
}

/// From `from` on (from the start when nil), the session ran as `accountKey`,
/// until the next owner.
nonisolated struct SessionOwner: Codable, Equatable, Hashable, Sendable {
    var from: Date?
    var accountKey: String
}

/// A session's owners over time. Pure.
nonisolated enum SessionOwners {
    /// Who ran the session at `date` (nil: its start). "" when nobody is known.
    static func owner(at date: Date?, in owners: [SessionOwner]) -> String {
        var result = owners.first?.accountKey ?? ""
        guard let date else { return result }
        for owner in owners {
            if let from = owner.from, from > date { break }
            result = owner.accountKey
        }
        return result
    }

    /// Whether two lists name the same owner at every moment up to `end`
    /// (the latest line counted with `lhs`): the counts made with one are
    /// then right for the other. Owners change only at their `from`, so
    /// those moments (and the start) are the ones to compare.
    static func agree(_ lhs: [SessionOwner], _ rhs: [SessionOwner], through end: Date?) -> Bool {
        if lhs == rhs { return true }
        var moments: [Date?] = [nil]
        if let end {
            moments += (lhs + rhs).compactMap(\.from).filter { $0 <= end }.map { Optional($0) }
        }
        return moments.allSatisfy { owner(at: $0, in: lhs) == owner(at: $0, in: rhs) }
    }

    /// When `accountKey` ran the session: its stretches as (from, to), nil
    /// meaning the start or no end. Nil when the account was its only owner
    /// (the whole session is its).
    static func stretches(of accountKey: String, in owners: [SessionOwner]) -> [(from: Date?, to: Date?)]? {
        guard owners.count > 1 else { return nil }
        var result: [(from: Date?, to: Date?)] = []
        for (index, owner) in owners.enumerated() where owner.accountKey == accountKey {
            result.append((owner.from, index + 1 < owners.count ? owners[index + 1].from : nil))
        }
        return result
    }

    /// The same owners without a stretch that covers no time (one whose next
    /// owner starts at the same moment) or that repeats the owner before it:
    /// the owner at every moment is unchanged. Pure.
    static func normalized(_ owners: [SessionOwner]) -> [SessionOwner] {
        var result: [SessionOwner] = []
        for owner in owners {
            if let last = result.last, last.from == owner.from { result.removeLast() }
            if let last = result.last, last.accountKey == owner.accountKey { continue }
            result.append(owner)
        }
        return result
    }

    /// Whether `date` lies in one of `stretches` (a nil date lies in none).
    static func contains(_ stretches: [(from: Date?, to: Date?)], _ date: Date?) -> Bool {
        guard let date else { return false }
        return stretches.contains { stretch in
            (stretch.from.map { date >= $0 } ?? true) && (stretch.to.map { date < $0 } ?? true)
        }
    }
}

/// What the website is told about an account.
nonisolated struct CloudLedgerAccount: Codable, Equatable, Sendable {
    var identityId: String
    var email: String?
    var organizationName: String?
    var plan: String?
    /// The user's name for it in the app.
    var label: String?
}

/// A running session as the hub attributed it.
nonisolated struct LiveSessionObservation: Equatable, Sendable {
    var sessionId: String
    var identityId: String
    var accountKey: String
    /// The directory the session started in.
    var cwd: String
    var transcriptPath: String?
    var configDir: String?
    var entrypoint: String?
    /// When the app first saw it running.
    var startedAt: Date
    var lastActivityAt: Date
    var model: String?
    var costUsd: Double?
    var title: String?
    /// When the Claude Code process running it started (from the kernel),
    /// if known: where another account's part of a resumed session begins.
    var processStartedAt: Date? = nil
}

nonisolated final class SessionLedger: @unchecked Sendable {
    static let fileName = "cloud-ledger.json"

    private static var logger: Logger { EngineLog.logger("SessionLedger") }

    /// How long a captured session may be missing before it counts as ended.
    static let endGrace: TimeInterval = 60
    /// Most entries kept (the oldest by last activity go first): months of
    /// heavy use. The website keeps what was sent.
    static let capacity = 10_000
    /// Last activity moves with every hook event: the file is written at
    /// most this often (and when the app quits).
    static let writeDelay: TimeInterval = 10
    /// Hand-overs between accounts kept per session at most: a session that
    /// keeps changing hands (two windows running it at once) stops being
    /// split further. Stretches of nobody are bounded on their own (by the
    /// same number): a hand-over away from nobody is never refused, so a
    /// session is never left counting for no one while the hub is certain.
    static let maxOwners = 32
    /// Sessions first seen unsure kept at most (the oldest go first).
    static let unattributedCapacity = 2_000
    /// Nobody's stretch starts this long after the last certain activity
    /// (transcript times have millisecond precision).
    static let uncountedAfter: TimeInterval = 0.001

    nonisolated struct Contents: Codable, Equatable, Sendable {
        /// 2: one entry per session and account, project paths kept locally.
        static let currentVersion = 2
        var version = Contents.currentVersion
        /// By `CloudLedgerEntry.key`.
        var sessions: [String: CloudLedgerEntry] = [:]
        /// By account key.
        var accounts: [String: CloudLedgerAccount] = [:]
        /// By session id: who ran it from when, for sessions more than one
        /// account ran (the others belong wholly to their one entry's account).
        var owners: [String: [SessionOwner]] = [:]
        /// Sessions with no entry that were seen running while their account
        /// couldn't be told, and since when: their responses so far are no
        /// one's. (Optional: a ledger written before it still loads.)
        var unattributed: [String: Date]?
    }

    private let lock = NSLock()
    private var contents: Contents
    private let file: CloudStateFile<Contents>
    private let home: String
    /// Session id → keys of its entries.
    private var keysOfSession: [String: Set<String>] = [:]
    /// Entries seen live while capturing, and when each was last seen.
    private var seenThisRun: [String: Date] = [:]
    /// Captured entries missing from the live list, since when.
    private var missingSince: [String: Date] = [:]
    /// Start directory → project path (symbolic links resolved once).
    private var projectPaths: [String: String] = [:]
    /// Captured entries not ended yet (what `endMissing` looks at).
    private var openIDs: Set<String> = []

    init(fileURL: URL?, persists: Bool, home: String) {
        file = CloudStateFile(url: fileURL, persists: persists, label: "cloud-ledger", writeDelay: Self.writeDelay)
        if let saved = file.load(), saved.version == Contents.currentVersion {
            contents = saved
        } else {
            contents = Contents()
        }
        self.home = home
        for (key, entry) in contents.sessions {
            keysOfSession[entry.sessionId, default: []].insert(key)
            if entry.origin == .live && entry.endedAt == nil { openIDs.insert(key) }
        }
    }

    // MARK: - Reading

    func entries() -> [CloudLedgerEntry] {
        lock.withLock { Array(contents.sessions.values) }
    }

    /// The session's entry of the account that ran it last.
    func entry(_ sessionId: String) -> CloudLedgerEntry? {
        lock.withLock {
            currentOwner(sessionId).flatMap { contents.sessions[CloudLedgerEntry.key(sessionId: sessionId, accountKey: $0)] }
        }
    }

    func entry(sessionId: String, accountKey: String) -> CloudLedgerEntry? {
        lock.withLock { contents.sessions[CloudLedgerEntry.key(sessionId: sessionId, accountKey: accountKey)] }
    }

    func entry(key: String) -> CloudLedgerEntry? {
        lock.withLock { contents.sessions[key] }
    }

    /// Every account's entry of the session, earliest first.
    func segments(of sessionId: String) -> [CloudLedgerEntry] {
        lock.withLock {
            (keysOfSession[sessionId] ?? []).compactMap { contents.sessions[$0] }
                .sorted { $0.startedAt != $1.startedAt ? $0.startedAt < $1.startedAt : $0.accountKey < $1.accountKey }
        }
    }

    /// Whether any account's entry of the session is known.
    func knows(_ sessionId: String) -> Bool {
        lock.withLock { isKnown(sessionId) }
    }

    /// Whether the session was seen running with its account unknown, and
    /// nobody's responses since are counted for any account.
    func isUncounted(_ sessionId: String) -> Bool {
        lock.withLock { currentOwner(sessionId) == "" }
    }

    /// Who ran the session from when: the kept owners of a session more
    /// than one account ran, else its one account from the start. Empty
    /// when the session isn't known.
    func owners(of sessionId: String) -> [SessionOwner] {
        lock.withLock {
            if let owners = contents.owners[sessionId], !owners.isEmpty { return owners }
            return currentOwner(sessionId).map { [SessionOwner(from: nil, accountKey: $0)] } ?? []
        }
    }

    func account(forKey key: String) -> CloudLedgerAccount? {
        lock.withLock { contents.accounts[key] }
    }

    /// Entries (one per session and account).
    var count: Int { lock.withLock { contents.sessions.count } }

    /// Lock held: the account that ran the session last ("" for nobody:
    /// its account couldn't be told).
    private func currentOwner(_ sessionId: String) -> String? {
        if let last = contents.owners[sessionId]?.last { return last.accountKey }
        let entries = (keysOfSession[sessionId] ?? []).compactMap { contents.sessions[$0] }
        if let last = entries.max(by: { $0.lastActivityAt != $1.lastActivityAt ? $0.lastActivityAt < $1.lastActivityAt : $0.accountKey < $1.accountKey }) {
            return last.accountKey
        }
        return contents.unattributed?[sessionId] != nil ? "" : nil
    }

    /// Lock held: any entry, owner or unsure sighting of the session.
    private func isKnown(_ sessionId: String) -> Bool {
        !(keysOfSession[sessionId] ?? []).isEmpty || contents.owners[sessionId] != nil
            || contents.unattributed?[sessionId] != nil
    }

    // MARK: - Live capture

    /// Take the running sessions the hub attributed for certain, and the
    /// accounts they belong to. `liveIDs` is every session running now,
    /// attributed or not (one the hub can't attribute this time is still
    /// running); `unsure` are those whose account the hub can't tell now;
    /// `waiting` those it hasn't placed yet (a folder not grouped yet, a
    /// Claude Desktop session whose registry entry hasn't been read): they
    /// are running, and neither counted nor paused until it has.
    /// A known session running but not among `live` (nor waiting) counts for
    /// no account from now on (see the file's notes); an unknown one in
    /// `unsure` is remembered as no one's. Returns the keys of the entries
    /// that ended with this call.
    @discardableResult
    func observe(live: [LiveSessionObservation], liveIDs: Set<String>, unsure: Set<String> = [],
                 waiting: Set<String> = [], accounts: [String: CloudLedgerAccount], now: Date) -> [String] {
        let live = live.filter {
            CloudKeys.isUUID($0.sessionId) && CloudKeys.isKey($0.accountKey) && !$0.cwd.isEmpty && !unsure.contains($0.sessionId)
        }
        // Resolve new start directories before taking the lock.
        let newDirs = lock.withLock { Set(live.map(\.cwd)).subtracting(projectPaths.keys) }
        var resolved: [String: String] = [:]
        for dir in newDirs { resolved[dir] = CloudKeys.projectPath(forCwd: dir, home: home) }

        return lock.withLock {
            projectPaths.merge(resolved) { old, _ in old }
            var changed = false
            for (key, account) in accounts where contents.accounts[key] != account {
                contents.accounts[key] = account
                changed = true
            }
            // A session running under two accounts at once (two windows):
            // which one a response came from can't be told, so it stays
            // with the account it ran as (or, new, isn't captured).
            let accountsOf = Dictionary(grouping: live, by: \.sessionId).mapValues { Set($0.map(\.accountKey)) }
            let attributable = live.filter { observation in
                guard let accounts = accountsOf[observation.sessionId], accounts.count > 1 else { return true }
                return currentOwner(observation.sessionId) == observation.accountKey
            }
            for observation in attributable {
                let key = CloudLedgerEntry.key(sessionId: observation.sessionId, accountKey: observation.accountKey)
                var partStart: Date?
                if let current = currentOwner(observation.sessionId), current != observation.accountKey {
                    // Away from nobody always: the hub is certain now.
                    if !current.isEmpty, stretchCount(observation.sessionId, nobody: false) >= Self.maxOwners { continue }
                    partStart = handOver(observation.sessionId, from: current, to: observation, now: now)
                    changed = true
                }
                seenThisRun[key] = now
                missingSince.removeValue(forKey: key)
                let updated = merged(observation, into: contents.sessions[key], partStart: partStart,
                                     isSplit: (contents.owners[observation.sessionId]?.count ?? 0) > 1)
                openIDs.insert(key)
                keysOfSession[observation.sessionId, default: []].insert(key)
                if contents.sessions[key] != updated {
                    contents.sessions[key] = updated
                    changed = true
                }
            }
            // Running, but not as an account it may be counted for (a
            // session not placed yet waits: nothing is known of it yet).
            var ended: [String] = []
            let counted = Set(live.map(\.sessionId))
            let notPlaced = waiting.subtracting(unsure)
            for sessionId in liveIDs.union(unsure).subtracting(counted).subtracting(notPlaced).sorted()
            where CloudKeys.isUUID(sessionId) {
                if isKnown(sessionId) {
                    guard currentOwner(sessionId) != "", let stopped = stopCounting(sessionId, now: now) else { continue }
                    ended += stopped
                    changed = true
                } else if unsure.contains(sessionId) {
                    noteUnattributed(sessionId, now: now)
                    changed = true
                }
            }
            ended += endMissing(liveIDs: liveIDs.union(notPlaced).union(live.map(\.sessionId)), now: now)
            if !ended.isEmpty { changed = true }
            if changed { persist() }
            return ended
        }
    }

    /// Lock held: the session's responses after its last activity seen
    /// while its account was certain (`uncountedAfter` later, so a response
    /// written at that very moment stays its account's; never before an
    /// earlier hand-over) are no one's, and its account's part ends at that
    /// activity. Returns that part's key if it was open; nil when nothing
    /// changed: a session that already lost its account `maxOwners` times
    /// stops being split further and stays with the account it ran as.
    private func stopCounting(_ sessionId: String, now: Date) -> [String]? {
        guard let current = currentOwner(sessionId), !current.isEmpty,
              stretchCount(sessionId, nobody: true) < Self.maxOwners else { return nil }
        let key = CloudLedgerEntry.key(sessionId: sessionId, accountKey: current)
        var owners = contents.owners[sessionId] ?? [SessionOwner(from: nil, accountKey: current)]
        var boundary = contents.sessions[key].map { $0.lastActivityAt.addingTimeInterval(Self.uncountedAfter) } ?? now
        if let previous = owners.last?.from { boundary = max(boundary, previous) }
        owners.append(SessionOwner(from: boundary, accountKey: ""))
        setOwners(sessionId, owners)
        var ended: [String] = []
        if var entry = contents.sessions[key], entry.endedAt == nil {
            entry.endedAt = entry.lastActivityAt
            contents.sessions[key] = entry
            if openIDs.contains(key) { ended.append(key) }
        }
        openIDs.remove(key)
        missingSince.removeValue(forKey: key)
        Self.logger.notice("A session's account can't be told now: its responses count for no account until it can")
        return ended
    }

    /// Lock held: how many of the session's kept owners are nobody's
    /// stretches (`nobody`), or accounts'.
    private func stretchCount(_ sessionId: String, nobody: Bool) -> Int {
        guard let owners = contents.owners[sessionId] else { return nobody ? 0 : 1 }
        return owners.filter { $0.accountKey.isEmpty == nobody }.count
    }

    /// Lock held: a session the ledger has no entry of runs unsure.
    private func noteUnattributed(_ sessionId: String, now: Date) {
        var unattributed = contents.unattributed ?? [:]
        guard unattributed[sessionId] == nil else { return }
        unattributed[sessionId] = now
        if unattributed.count > Self.unattributedCapacity {
            let oldest = unattributed.sorted { $0.value != $1.value ? $0.value < $1.value : $0.key < $1.key }
                .prefix(unattributed.count - Self.unattributedCapacity)
            for (id, _) in oldest { unattributed.removeValue(forKey: id) }
        }
        contents.unattributed = unattributed
    }

    /// Lock held: keep the session's owners, without empty or repeated
    /// stretches; a session back to one owner from its start that is its
    /// only entry's needs none kept.
    private func setOwners(_ sessionId: String, _ owners: [SessionOwner]) {
        let owners = SessionOwners.normalized(owners)
        if owners.count == 1, let only = owners.first, only.from == nil, !only.accountKey.isEmpty,
           (keysOfSession[sessionId] ?? []).subtracting([CloudLedgerEntry.key(sessionId: sessionId, accountKey: only.accountKey)]).isEmpty {
            contents.owners.removeValue(forKey: sessionId)
        } else {
            contents.owners[sessionId] = owners
        }
    }

    /// End the captured entries that have been missing from `liveIDs` for
    /// `endGrace` (the regular tick, when no session changed). Returns them.
    @discardableResult
    func settle(liveIDs: Set<String>, now: Date) -> [String] {
        lock.withLock {
            let ended = endMissing(liveIDs: liveIDs, now: now)
            if !ended.isEmpty { persist() }
            return ended
        }
    }

    /// Capture stopped (sync off, signed out): what was seen while it ran
    /// says nothing about when a session ended during the gap. A session
    /// found gone after capture resumes ends at its last activity.
    func forgetRunState() {
        lock.withLock {
            seenThisRun = [:]
            missingSince = [:]
        }
    }

    /// Lock held: the session, last run as `current`, now runs as the
    /// observation's account. The old account's part ends when the new one
    /// began (its process's start, never before the old part's last
    /// activity or an earlier hand-over, never after now). Returns that
    /// moment. From nobody (`current` ""), the same rule: a new process
    /// takes over from its start, the process that ran through the unsure
    /// stretch from where it began (the stretch goes).
    private func handOver(_ sessionId: String, from current: String, to observation: LiveSessionObservation,
                          now: Date) -> Date {
        let oldKey = CloudLedgerEntry.key(sessionId: sessionId, accountKey: current)
        var owners = contents.owners[sessionId] ?? [SessionOwner(from: nil, accountKey: current)]
        var boundary = min(observation.processStartedAt ?? observation.startedAt, now)
        if let old = contents.sessions[oldKey] { boundary = max(boundary, old.lastActivityAt) }
        if let previous = owners.last?.from { boundary = max(boundary, previous) }
        owners.append(SessionOwner(from: boundary, accountKey: observation.accountKey))
        setOwners(sessionId, owners)
        contents.unattributed?.removeValue(forKey: sessionId)
        if var old = contents.sessions[oldKey], old.endedAt == nil {
            old.endedAt = max(boundary, old.lastActivityAt)
            contents.sessions[oldKey] = old
        }
        openIDs.remove(oldKey)
        missingSince.removeValue(forKey: oldKey)
        if current.isEmpty {
            Self.logger.notice("A session's account is certain again: counted for it from now on")
        } else {
            Self.logger.notice("A session resumed under another account: counted per account from now on")
        }
        return boundary
    }

    /// - Parameters:
    ///   - partStart: where a new part of a split session begins.
    ///   - isSplit: the session has more than one owner: a part's start is
    ///     where its account took over, not when the app first saw the process.
    private func merged(_ observation: LiveSessionObservation, into existing: CloudLedgerEntry?,
                        partStart: Date?, isSplit: Bool) -> CloudLedgerEntry {
        let path = projectPaths[observation.cwd] ?? CloudKeys.projectPath(forCwd: observation.cwd, home: home)
        guard var entry = existing else {
            let start = partStart ?? min(observation.startedAt, observation.lastActivityAt)
            return CloudLedgerEntry(
                sessionId: observation.sessionId,
                identityId: observation.identityId,
                accountKey: observation.accountKey,
                projectName: CloudKeys.projectName(forCwd: observation.cwd),
                projectPath: path,
                transcriptPath: observation.transcriptPath,
                configDir: observation.configDir,
                source: CloudSessionSource.from(entrypoint: observation.entrypoint),
                startedAt: start,
                lastActivityAt: max(observation.lastActivityAt, start),
                endedAt: nil,
                model: observation.model,
                costUsd: observation.costUsd,
                title: observation.title,
                origin: .live
            )
        }
        // A backfilled session seen running is the hub's from now on.
        if entry.origin == .backfill {
            entry.origin = .live
            entry.projectName = CloudKeys.projectName(forCwd: observation.cwd)
            entry.projectPath = path
        }
        // The account's identity id can change (an organization split);
        // its key is what stays.
        entry.identityId = observation.identityId
        entry.transcriptPath = observation.transcriptPath ?? entry.transcriptPath
        entry.configDir = observation.configDir ?? entry.configDir
        if entry.source == .other { entry.source = CloudSessionSource.from(entrypoint: observation.entrypoint) }
        if !isSplit { entry.startedAt = min(entry.startedAt, observation.startedAt) }
        entry.lastActivityAt = max(entry.lastActivityAt, observation.lastActivityAt)
        entry.endedAt = nil
        entry.model = observation.model ?? entry.model
        entry.costUsd = observation.costUsd ?? entry.costUsd
        entry.title = observation.title ?? entry.title
        return entry
    }

    /// Lock held.
    private func endMissing(liveIDs: Set<String>, now: Date) -> [String] {
        var ended: [String] = []
        for key in openIDs.sorted() {
            guard let entry = contents.sessions[key], entry.origin == .live, entry.endedAt == nil else {
                openIDs.remove(key)
                continue
            }
            guard !liveIDs.contains(entry.sessionId) else { continue }
            guard let since = missingSince[key] else {
                missingSince[key] = now
                continue
            }
            guard now.timeIntervalSince(since) >= Self.endGrace else { continue }
            var copy = entry
            // Seen going away while capturing: then. Otherwise (it ended
            // while the app wasn't running, or wasn't capturing): its last
            // activity.
            copy.endedAt = seenThisRun[key] != nil ? max(since, entry.lastActivityAt) : entry.lastActivityAt
            contents.sessions[key] = copy
            missingSince.removeValue(forKey: key)
            openIDs.remove(key)
            ended.append(key)
        }
        return ended
    }

    // MARK: - Backfill

    /// Add sessions found on disk; a session the ledger already knows (under
    /// any account) is left as it is.
    @discardableResult
    func record(backfill found: [CloudLedgerEntry], accounts: [String: CloudLedgerAccount]) -> Int {
        lock.withLock {
            var added = 0
            // A session seen unsure is never backfilled onto an account either.
            for entry in found where !isKnown(entry.sessionId) {
                var entry = entry
                entry.origin = .backfill
                contents.sessions[entry.key] = entry
                keysOfSession[entry.sessionId, default: []].insert(entry.key)
                added += 1
            }
            for (key, account) in accounts where contents.accounts[key] == nil {
                contents.accounts[key] = account
            }
            if added > 0 { persist() }
            return added
        }
    }

    /// Fill in what a later look at the transcript learned (a title, the
    /// last activity, the end of a backfilled one that was still going
    /// when found).
    func refine(_ key: String, lastActivityAt: Date?, title: String?, transcriptPath: String?,
                endedAt: Date? = nil) {
        lock.withLock {
            guard var entry = contents.sessions[key] else { return }
            let before = entry
            if let lastActivityAt { entry.lastActivityAt = max(entry.lastActivityAt, lastActivityAt) }
            if entry.title == nil, let title { entry.title = title }
            if entry.transcriptPath == nil, let transcriptPath { entry.transcriptPath = transcriptPath }
            if let endedAt, entry.origin == .backfill, entry.endedAt == nil { entry.endedAt = max(endedAt, entry.lastActivityAt) }
            if entry != before {
                contents.sessions[key] = entry
                persist()
            }
        }
    }

    // MARK: - Persistence

    /// Lock held.
    private func persist() {
        if contents.sessions.count > Self.capacity {
            let overflow = contents.sessions.values
                .sorted { $0.lastActivityAt < $1.lastActivityAt }
                .prefix(contents.sessions.count - Self.capacity)
            for entry in overflow {
                contents.sessions.removeValue(forKey: entry.key)
                openIDs.remove(entry.key)
                keysOfSession[entry.sessionId]?.remove(entry.key)
                if keysOfSession[entry.sessionId]?.isEmpty == true {
                    keysOfSession.removeValue(forKey: entry.sessionId)
                    contents.owners.removeValue(forKey: entry.sessionId)
                }
            }
        }
        file.save(contents)
    }

    func saveNow() {
        let snapshot = lock.withLock { contents }
        file.saveNow(snapshot)
    }
}

// MARK: - Backfill roots

/// Which `projects/` folders hold only one account's history.
nonisolated enum CloudBackfill {
    /// A folder to look in, and whose it is.
    nonisolated struct Root: Equatable, Sendable {
        /// The physical `projects` folder.
        var projects: String
        var identityId: String
        var accountKey: String
        var configDir: String
        /// Signed in as that account since (as far as the app saw): only
        /// transcripts begun after it are its.
        var signedInSince: Date
    }

    /// One folder as the backfill sees it.
    nonisolated struct Folder: Equatable, Sendable {
        var configDir: String
        /// The identity it runs as, when it is a run folder of a visible,
        /// signed-in account that can be backfilled.
        var identityId: String?
        var accountKey: String?
        /// Who is signed in there, as `CloudFolderLogins` records it; nil
        /// for nobody (or unknown).
        var login: String? = nil
        /// Since when the app has seen `login` there; nil when unknown.
        var signedInSince: Date? = nil
    }

    /// The roots to backfill: a run folder's `projects/` that is a real
    /// folder (not a link: a linked history, `~/.claude-shared` and the
    /// like, can only be attributed by the ledger), that no other known
    /// folder reaches, and whose login is known and dated. `realPath`
    /// resolves links (injectable for tests). Pure apart from `realPath`.
    static func roots(folders: [Folder], realPath: (String) -> String = TranscriptLocator.realPath,
                      isDirectory: (String) -> Bool = CloudBackfill.isDirectory) -> [Root] {
        var reachedBy: [String: Set<String>] = [:]
        var candidates: [(real: String, folder: Folder)] = []
        for folder in folders {
            let projects = (AccountPaths.normalize(folder.configDir) as NSString).appendingPathComponent("projects")
            guard isDirectory(projects) else { continue }
            let real = realPath(projects)
            reachedBy[real, default: []].insert(AccountPaths.normalize(folder.configDir))
            let own = realPath(AccountPaths.normalize(folder.configDir)) + "/projects"
            // A link anywhere on the way: shared (or at least not provably its own).
            guard real == projects, own == projects, folder.identityId != nil, folder.accountKey != nil,
                  folder.signedInSince != nil else { continue }
            candidates.append((real, folder))
        }
        return candidates.compactMap { real, folder in
            guard reachedBy[real]?.count == 1, let identityId = folder.identityId, let key = folder.accountKey,
                  let since = folder.signedInSince else { return nil }
            return Root(projects: real, identityId: identityId, accountKey: key,
                        configDir: AccountPaths.normalize(folder.configDir), signedInSince: since)
        }
    }

    static func isDirectory(_ path: String) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory) && isDirectory.boolValue
    }

    /// Who is signed in to a folder, as `CloudFolderLogins` tells logins
    /// apart: a digest of its own `oauthAccount` account UUID, organization
    /// and email (a `/login` as someone else changes it). Nil when nobody is.
    static func login(accountUuid: String?, organizationUuid: String?, email: String?) -> String? {
        func clean(_ value: String?) -> String { value?.trimmingCharacters(in: .whitespaces).lowercased() ?? "" }
        let parts = [clean(accountUuid), clean(organizationUuid), clean(email)]
        guard !parts[0].isEmpty || !parts[2].isEmpty else { return nil }
        return CloudKeys.sha256Hex(parts.joined(separator: "|"))
    }
}

// MARK: - Who was signed in where, since when

/// When the app first saw each config folder signed in as the account it
/// names now: "signed in as <login> since <date>". Set the first time the
/// app sees the folder's login, and again whenever that changes; a folder
/// seen signed out keeps its record (logging out and back in as the same
/// account changes nothing). Before the app first saw it, nothing is known,
/// so the backfill reads only transcripts begun after this date. Local
/// only (`cloud-folder-logins.json`, 0600; logins are digests); never sent.
nonisolated final class CloudFolderLogins: @unchecked Sendable {
    static let fileName = "cloud-folder-logins.json"

    nonisolated struct Record: Codable, Equatable, Sendable {
        var login: String
        var since: Date
    }

    nonisolated struct Contents: Codable, Equatable, Sendable {
        var version = 1
        /// By normalized config folder.
        var folders: [String: Record] = [:]
    }

    private let lock = NSLock()
    private var contents: Contents
    private let file: CloudStateFile<Contents>

    init(fileURL: URL?, persists: Bool) {
        file = CloudStateFile(url: fileURL, persists: persists, label: "cloud-folder-logins")
        contents = file.load() ?? Contents()
    }

    /// The folders' logins as the app reads them now (folder → login; a
    /// folder nobody is signed in to is left out).
    func observe(_ logins: [String: String], now: Date) {
        lock.withLock {
            var changed = false
            for (folder, login) in logins {
                let key = AccountPaths.normalize(folder)
                if contents.folders[key]?.login == login { continue }
                contents.folders[key] = Record(login: login, since: now)
                changed = true
            }
            if changed { file.save(contents) }
        }
    }

    /// Since when `folder` has been seen signed in as `login`; nil when it
    /// never was, or someone else is recorded there.
    func since(folder: String, login: String?) -> Date? {
        guard let login else { return nil }
        return lock.withLock {
            guard let record = contents.folders[AccountPaths.normalize(folder)], record.login == login else { return nil }
            return record.since
        }
    }

    func saveNow() {
        let snapshot = lock.withLock { contents }
        file.saveNow(snapshot)
    }
}
