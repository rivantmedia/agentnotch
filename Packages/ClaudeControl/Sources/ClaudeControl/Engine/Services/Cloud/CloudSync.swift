//
//  CloudSync.swift
//  ClaudeControl
//
//  Sends the website (`web/contract/README.md`) what this Mac knows about
//  the user's Claude accounts: each session's project name, title, model,
//  times, token counts and cost, and each account's usage readings over
//  time. Names and numbers only: never a file path, never a prompt. A
//  session's summary goes along only while session summaries are on.
//
//  Consent: nothing is uploaded unless the user is signed in to the website
//  and has turned sync on; summaries need their own switch (off by default)
//  because they spend the account's usage. Sessions are captured, and usage
//  readings recorded, only while both hold. The switches belong to one
//  sign-in on one website: signing out (or being signed out), or changing
//  the website, turns both off, and a new sign-in starts with them off.
//  Turning sync off stops a pass in flight; turning summaries off (or sync,
//  or signing out) stops a summary in flight. Turning summaries off (which
//  signing out and changing the website do too) deletes the summaries the
//  website doesn't have yet; those it has stay there. A pass belongs to the
//  sign-in and website it started with. Only visible, remembered,
//  signed-in accounts are ever mentioned, and a session the engine can't
//  attribute for certain is never guessed onto one: while it can't, its new
//  responses count for no account (`SessionLedger`). A session Claude
//  Desktop hosts is attributed by the hub from Desktop's own record of it
//  (`DesktopHostedSessions`); one found only on disk can't be, and isn't
//  backfilled.
//
//  Schedule: every 5 minutes while on, 30 seconds after a session ends or a
//  summary is written, and on demand; at most five requests a pass, the rest
//  30 seconds later; after a failure, 30 s doubling up to
//  30 minutes (or what the website's Retry-After says), and nothing but the
//  user's "Sync now" comes sooner. What was sent is remembered per session
//  and account (a hash of it, whether it had ended, and the transcript it
//  came from) so an unchanged session isn't sent again; every other one is
//  looked at again each pass (a stat when nothing moved). Readings leave the
//  outbox once the website has them.
//
//  A sealed run shows a fixed, signed-in state and does nothing else: no
//  network, no file, no `claude`.
//

import Combine
import CryptoKit
import Foundation
import os.log
import SystemConfiguration

// MARK: - What the service reads

/// One Claude account as the website may hear of it: visible, remembered,
/// signed in, with an account UUID to derive its key from.
nonisolated struct CloudAccountInfo: Equatable, Sendable {
    var identityId: String
    var accountKey: String
    var accountUuid: String
    var email: String?
    var organizationName: String?
    var plan: String?
    /// What the app calls it (the ring's nickname or the account's name).
    var label: String?

    var contract: CloudSyncRequest.Account {
        CloudSyncRequest.Account(key: accountKey, email: email, organizationName: organizationName, plan: plan, label: label)
    }

    var ledgerAccount: CloudLedgerAccount {
        CloudLedgerAccount(identityId: identityId, email: email, organizationName: organizationName, plan: plan, label: label)
    }
}

/// A run folder a summary may run in, checked just now to be signed in as
/// the session's account (never a Claude Parallel Profiles store).
nonisolated struct CloudSummaryFolder: Equatable, Sendable {
    var configDir: String
    /// Raw CLAUDE_CONFIG_DIR; nil for `~/.claude`.
    var configDirEnv: String?
    /// Run folders the binary is looked for next to.
    var configDirs: [String]
}

/// What the service reads from the rest of the engine: the account
/// registry's view, and where a summary may run. Tests give their own.
@MainActor
protocol CloudSyncEnvironment: AnyObject {
    /// Visible, remembered, signed-in accounts with an account UUID.
    func accounts() -> [CloudAccountInfo]
    /// Folder → who is signed in there (`CloudBackfill.login`), for every
    /// folder someone is; nil until the registry has read them.
    func folderLogins() -> [String: String]?
    /// Every folder the registry knows, with the account of those the
    /// backfill may read (see `CloudBackfill.roots`).
    func backfillFolders() -> [CloudBackfill.Folder]
    /// How much of the account's 5-hour window is used (0–100), if known.
    func sessionUtilization(identityId: String) -> Double?
    /// A run folder signed in as the identity right now, or nil.
    func summaryFolder(forIdentity identityId: String) async -> CloudSummaryFolder?
    /// After the run: still signed in as the identity.
    func summaryFolderStillRuns(_ folder: CloudSummaryFolder, identityId: String) async -> Bool
    /// Claude Code is being launched for something else (a usage check).
    var isLaunchingClaude: Bool { get }
}

// MARK: - What was sent

/// A transcript as it was when a session was built from it: its size and
/// when it was last written (a stat tells whether it moved since).
nonisolated struct TranscriptStamp: Codable, Equatable, Sendable {
    var bytes: UInt64
    var modified: Double

    /// The file's now; nil when it can't be read.
    static func of(_ path: String) -> TranscriptStamp? {
        var info = stat()
        guard stat(path, &info) == 0 else { return nil }
        return TranscriptStamp(bytes: UInt64(max(0, info.st_size)),
                               modified: Double(info.st_mtimespec.tv_sec) + Double(info.st_mtimespec.tv_nsec) / 1_000_000_000)
    }
}

/// What the website already has, so an unchanged session isn't sent again.
/// Kept per website user: another user (or website) starts over. Kept in
/// `cloud-sync-state.json`, by ledger entry (one per session and account).
nonisolated final class CloudSyncMemory: @unchecked Sendable {
    static let fileName = "cloud-sync-state.json"

    /// A session as last sent: the hash of its payload without the summary,
    /// and of the summary sent with it; whether it had ended; the transcript
    /// it was built from.
    nonisolated struct Record: Codable, Equatable, Sendable {
        var base: String
        var summary: String?
        var ended = false
        var transcript: TranscriptStamp?
    }

    nonisolated struct Contents: Codable, Equatable, Sendable {
        /// 2: by ledger entry key, with `ended` and `transcript`.
        static let currentVersion = 2
        var version = Contents.currentVersion
        var userId: String?
        var website: String?
        var sessions: [String: Record] = [:]
        var lastSyncAt: Date?
        var dashboardUrl: String?
    }

    private let lock = NSLock()
    private var contents: Contents
    private let file: CloudStateFile<Contents>

    init(fileURL: URL?, persists: Bool) {
        file = CloudStateFile(url: fileURL, persists: persists, label: "cloud-sync")
        if let saved = file.load(), saved.version == Contents.currentVersion {
            contents = saved
        } else {
            contents = Contents()
        }
    }

    var lastSyncAt: Date? { lock.withLock { contents.lastSyncAt } }
    var dashboardUrl: String? { lock.withLock { contents.dashboardUrl } }
    var userId: String? { lock.withLock { contents.userId } }
    var sentCount: Int { lock.withLock { contents.sessions.count } }

    /// Signed in as `userId` on `website`: what was sent to anyone else is
    /// forgotten.
    func adopt(userId: String?, website: String, dashboardUrl: String?) {
        lock.withLock {
            if contents.userId != userId || contents.website != website {
                contents = Contents()
                contents.userId = userId
                contents.website = website
            }
            contents.dashboardUrl = dashboardUrl ?? contents.dashboardUrl
            file.save(contents)
        }
    }

    /// What was last sent for the ledger entry.
    func sent(_ key: String) -> Record? {
        lock.withLock { contents.sessions[key] }
    }

    /// Whether the session has changed since it was sent (or a new summary
    /// came). Pure given the memory.
    func needsSending(_ key: String, _ record: Record) -> Bool {
        lock.withLock {
            guard let sent = contents.sessions[key] else { return true }
            if sent.base != record.base { return true }
            if let summary = record.summary, summary != sent.summary { return true }
            return false
        }
    }

    /// The session was built again from a transcript that moved, and came
    /// out as it was sent: remember the transcript, so the next pass needs
    /// only a stat.
    func noteTranscript(_ key: String, _ transcript: TranscriptStamp?) {
        lock.withLock {
            guard var sent = contents.sessions[key], sent.transcript != transcript else { return }
            sent.transcript = transcript
            contents.sessions[key] = sent
            file.save(contents)
        }
    }

    func markSent(_ records: [String: Record], at date: Date) {
        lock.withLock {
            for (key, record) in records {
                let previous = contents.sessions[key]
                contents.sessions[key] = Record(base: record.base, summary: record.summary ?? previous?.summary,
                                                ended: record.ended, transcript: record.transcript)
            }
            contents.lastSyncAt = date
            file.save(contents)
        }
    }

    func noteSynced(at date: Date) {
        lock.withLock {
            contents.lastSyncAt = date
            file.save(contents)
        }
    }

    func saveNow() {
        let snapshot = lock.withLock { contents }
        file.saveNow(snapshot)
    }
}

// MARK: - The files

/// The service's state on disk (in memory only when not persisted), and
/// this install's secret for project keys.
nonisolated final class CloudStores: @unchecked Sendable {
    let ledger: SessionLedger
    let scanner: SessionTokenScanner
    let recorder: UsageHistoryRecorder
    let summaries: SessionSummaryStore
    let memory: CloudSyncMemory
    let folderLogins: CloudFolderLogins
    private let directory: URL?
    private let persists: Bool
    private let lock = NSLock()
    private var madeSecret: Data?

    init(directory: URL?, persists: Bool, home: String) {
        func url(_ name: String) -> URL? { directory?.appendingPathComponent(name) }
        self.directory = directory
        self.persists = persists
        ledger = SessionLedger(fileURL: url(SessionLedger.fileName), persists: persists, home: home)
        scanner = SessionTokenScanner(fileURL: url(SessionTokenScanner.fileName), persists: persists)
        recorder = UsageHistoryRecorder(fileURL: url(UsageHistoryRecorder.fileName), persists: persists)
        summaries = SessionSummaryStore(fileURL: url(SessionSummaryStore.fileName), persists: persists)
        memory = CloudSyncMemory(fileURL: url(CloudSyncMemory.fileName), persists: persists)
        folderLogins = CloudFolderLogins(fileURL: url(CloudFolderLogins.fileName), persists: persists)
    }

    /// `CloudInstallSecret`, read (or made) the first time a project key is
    /// needed, which only a sync pass does (signed in, with sync on): never
    /// at launch. Kept in the folder when persisted, else made for this run
    /// only (as it is when the folder can't be written).
    func secret() -> Data {
        lock.withLock {
            if let madeSecret { return madeSecret }
            var made: Data?
            if persists, let directory { made = CloudInstallSecret.load(directory: directory) }
            let secret = made ?? CloudInstallSecret.random()
            madeSecret = secret
            return secret
        }
    }

    func saveNow() {
        ledger.saveNow()
        scanner.saveNow()
        recorder.saveNow()
        summaries.saveNow()
        memory.saveNow()
        folderLogins.saveNow()
    }
}

// MARK: - A pass

/// One sync pass's work off the main actor: the backfill, the transcripts'
/// totals, what changed since it was sent, and the batches. Pure apart from
/// the stores it is given.
nonisolated enum CloudSyncPass {
    /// A backfilled session whose transcript moved in the last half hour may
    /// still be running.
    static let backfillOpenWindow: TimeInterval = 30 * 60
    /// New transcripts the backfill reads per pass (the first pass on a
    /// long history spreads over a few).
    static let backfillFilesPerPass = 200
    /// Requests a pass sends at most, back to back; the rest go in the next
    /// pass, 30 seconds on. Well under the website's burst (12 a user,
    /// shared by all of the user's Macs), so a catch-up isn't refused.
    static let maxRequestsPerPass = 5

    nonisolated struct Input: Sendable {
        var accounts: [CloudAccountInfo]
        /// Nil: no backfill this pass.
        var backfillFolders: [CloudBackfill.Folder]?
        var includeSummaries: Bool
        var device: CloudSyncRequest.Device
        var now: Date
        /// What `~` means in a transcript's working directory.
        var home: String
        var maxSessionsPerRequest = CloudContract.Limit.sessions
        var maxRequests = CloudSyncPass.maxRequestsPerPass
    }

    nonisolated struct Batch: Sendable {
        var request: CloudSyncRequest
        /// By ledger entry key.
        var records: [String: CloudSyncMemory.Record]
        var readings: [RecordedUsageReading]

        /// The batch with no summary in it (summaries were turned off
        /// during the pass): the website keeps what it has, and what was
        /// sent is remembered without them.
        func withoutSummaries() -> Batch {
            var copy = self
            for index in copy.request.sessions.indices { copy.request.sessions[index].summary = nil }
            for key in copy.records.keys { copy.records[key]?.summary = nil }
            return copy
        }
    }

    nonisolated struct Prepared: Sendable {
        var batches: [Batch]
        /// Changed sessions waiting to be sent (all of them, not only this pass's).
        var sessionCount: Int
        var usageCount: Int
        /// More than `maxRequests` were needed: the rest go next pass.
        var hasMore: Bool
        var backfilled: Int
    }

    static func prepare(_ input: Input, stores: CloudStores) -> Prepared {
        let allowed = Dictionary(input.accounts.map { ($0.accountKey, $0) }, uniquingKeysWith: { first, _ in first })
        var backfilled = 0
        if let folders = input.backfillFolders {
            // Transcripts Claude Code deleted since (their totals were sent).
            stores.scanner.pruneMissingFiles()
            backfilled = backfill(folders: folders, accounts: allowed, stores: stores, home: input.home, now: input.now)
        }

        var changed: [(CloudSyncRequest.Session, CloudSyncMemory.Record)] = []
        // Oldest first: an original session claims its responses before a
        // fork that copied them.
        let entries = stores.ledger.entries()
            .filter { allowed[$0.accountKey] != nil }
            .sorted { lhs, rhs in lhs.startedAt != rhs.startedAt ? lhs.startedAt < rhs.startedAt : lhs.key < rhs.key }
        for entry in entries {
            // Sent as ended, from a transcript that hasn't moved since, and
            // no new summary: nothing can have changed (a stat, no read).
            if let sent = stores.memory.sent(entry.key), sent.ended, entry.endedAt != nil, let stamp = sent.transcript,
               let path = entry.transcriptPath, TranscriptStamp.of(path) == stamp,
               !hasNewSummary(entry.key, sent: sent, stores: stores, includeSummaries: input.includeSummaries) {
                continue
            }
            // Ended with no response of its account in it (so never sent),
            // read since its transcript last moved: nothing to report.
            if stores.memory.sent(entry.key) == nil, entry.endedAt != nil, let path = entry.transcriptPath,
               let cached = stores.scanner.cachedSummary(of: entry.sessionId), cached.part(for: entry.accountKey).messageCount == 0,
               TranscriptStamp.of(path) == TranscriptStamp(bytes: cached.transcriptBytes, modified: cached.transcriptModified) {
                continue
            }
            guard let built = payload(for: entry, stores: stores, includeSummaries: input.includeSummaries, now: input.now)
            else { continue }
            let record = Self.record(for: built.session, transcript: built.transcript)
            if stores.memory.needsSending(entry.key, record) {
                changed.append((built.session, record))
            } else {
                stores.memory.noteTranscript(entry.key, record.transcript)
            }
        }
        stores.scanner.save()

        // Readings of accounts that may not be mentioned (any more) go.
        stores.recorder.discard { allowed[$0.accountKey] == nil }
        let readings = stores.recorder.pending()
        let all = CloudBatcher.batches(sessions: changed, readings: readings, accounts: allowed, device: input.device,
                                       maxSessions: input.maxSessionsPerRequest)
        return Prepared(batches: Array(all.prefix(input.maxRequests)), sessionCount: changed.count,
                        usageCount: readings.count, hasMore: all.count > input.maxRequests, backfilled: backfilled)
    }

    /// A summary made since the session was last sent (and summaries are on).
    static func hasNewSummary(_ key: String, sent: CloudSyncMemory.Record, stores: CloudStores,
                              includeSummaries: Bool) -> Bool {
        guard includeSummaries, let summary = stores.summaries.summary(for: key) else { return false }
        return summaryHash(summary) != sent.summary
    }

    /// The hash a summary is remembered by once sent (`record(for:)`'s). Pure.
    static func summaryHash(_ summary: SessionSummaryStore.Entry) -> String? {
        (try? CloudJSON.makeEncoder().encode(summary.contract)).map(CloudKeys.sha256Hex)
    }

    /// Whether the website this Mac is signed in to has the summary (it
    /// was sent with its session, as it is now).
    static func wasSent(_ summary: SessionSummaryStore.Entry, key: String, stores: CloudStores) -> Bool {
        guard let sent = stores.memory.sent(key)?.summary else { return false }
        return summaryHash(summary) == sent
    }

    /// The session (this entry's account's part of it) as the website gets
    /// it, from the ledger and its transcripts, read again now (an
    /// unchanged file costs a stat), and the transcript it came from. Nil
    /// when that account has no response in it (nothing to report).
    static func payload(for entry: CloudLedgerEntry, stores: CloudStores, includeSummaries: Bool,
                        now: Date) -> (session: CloudSyncRequest.Session, transcript: TranscriptStamp?)? {
        var path = entry.transcriptPath
        if path == nil, let configDir = entry.configDir {
            // Seen only in the session registry so far: look for it.
            path = TranscriptLocator.searchTranscript(sessionId: entry.sessionId, configDir: configDir)
        }
        let owners = stores.ledger.owners(of: entry.sessionId)
        var totals: SessionTokenSummary?
        if let path {
            totals = stores.scanner.scan(sessionId: entry.sessionId, transcriptPath: path, owners: owners)
            if totals != nil, entry.transcriptPath == nil {
                stores.ledger.refine(entry.key, lastActivityAt: nil, title: nil, transcriptPath: path)
            }
        }
        if totals == nil { totals = stores.scanner.cachedSummary(of: entry.sessionId) }
        guard let totals else { return nil }
        let part = totals.part(for: entry.accountKey)
        guard part.messageCount > 0 else { return nil }
        if let last = part.lastTimestamp {
            // Found on disk still going, quiet since: it ended.
            let ends = entry.origin == .backfill && entry.endedAt == nil && now.timeIntervalSince(last) > backfillOpenWindow
            stores.ledger.refine(entry.key, lastActivityAt: last, title: nil, transcriptPath: nil, endedAt: ends ? last : nil)
        }

        // The transcript's first line is when the session began; the ledger's
        // time is when the app first saw it running.
        let startedAt = part.firstTimestamp ?? entry.startedAt
        // Likewise the last line is its last activity (the app may have
        // first seen it, idle, long after). An end is never before it: a
        // session found gone after capture paused ends at its last line.
        let lastActivityAt = max(part.lastTimestamp ?? entry.lastActivityAt, startedAt)
        let endedAt = (stores.ledger.entry(key: entry.key)?.endedAt ?? entry.endedAt).map { max($0, lastActivityAt) }
        var models = part.models
        if models.isEmpty, let model = entry.model, !model.isEmpty { models = [model] }
        var source = entry.source
        if source == .other { source = CloudSessionSource.from(entrypoint: totals.entrypoint) }
        // A session more than one account ran: the status line's cost is the
        // whole process's (a resumed session starts from the total it had),
        // which can't be divided between them, so no part carries one. Its
        // tokens are split exactly, by who ran it when. Split means another
        // owner (nobody included) has responses in it: a stretch of nobody
        // with none (a session placed a moment after it started) takes
        // nothing from it.
        let isSplit = Self.isSplit(totals, accountKey: entry.accountKey)
        let session = CloudSyncRequest.Session(
            accountKey: entry.accountKey,
            sessionId: entry.sessionId,
            project: CloudSyncRequest.Project(key: CloudKeys.projectKey(accountKey: entry.accountKey, path: entry.projectPath,
                                                                       secret: stores.secret()),
                                              name: entry.projectName),
            title: entry.title ?? totals.title,
            source: source,
            models: models,
            startedAt: startedAt,
            lastActivityAt: lastActivityAt,
            endedAt: endedAt,
            messageCount: part.messageCount,
            tokens: part.tokens.contract,
            costUsd: isSplit ? nil : entry.costUsd,
            summary: includeSummaries ? stores.summaries.summary(for: entry.key)?.contract : nil
        )
        return (session, TranscriptStamp(bytes: totals.transcriptBytes, modified: totals.transcriptModified))
    }

    /// Whether anyone but `accountKey` (nobody included) made responses in
    /// the session, as the transcript was counted by its owners. Pure.
    static func isSplit(_ totals: SessionTokenSummary, accountKey: String) -> Bool {
        totals.parts.contains { $0.key != accountKey && $0.value.messageCount > 0 }
    }

    /// What is remembered of a sent session: hashes of its payload without
    /// its summary and of the summary, whether it had ended, and the
    /// transcript it was built from. Pure.
    static func record(for session: CloudSyncRequest.Session, transcript: TranscriptStamp? = nil) -> CloudSyncMemory.Record {
        let encoder = CloudJSON.makeEncoder()
        var base = session
        base.summary = nil
        let baseHash = (try? encoder.encode(base)).map(CloudKeys.sha256Hex) ?? UUID().uuidString
        let summaryHash = session.summary.flatMap { try? encoder.encode($0) }.map(CloudKeys.sha256Hex)
        return CloudSyncMemory.Record(base: baseHash, summary: summaryHash, ended: session.endedAt != nil,
                                      transcript: transcript)
    }

    /// Add the sessions found on disk in folders only one account uses,
    /// begun after the folder was first seen signed in as that account.
    /// Oldest first (by their first own line, then by file creation), so an
    /// original claims its responses before a copy resumed or forked from
    /// it. Returns how many were added.
    static func backfill(folders: [CloudBackfill.Folder], accounts: [String: CloudAccountInfo],
                         stores: CloudStores, home: String, now: Date) -> Int {
        struct Found {
            var sessionId: String
            var path: String
            var root: CloudBackfill.Root
            var first: Date
            var born: Date?
        }
        var found: [Found] = []
        for root in CloudBackfill.roots(folders: folders) where accounts[root.accountKey] != nil {
            for (sessionId, path) in SessionTokenScanner.sessionFiles(inProjectsRoot: root.projects) {
                guard !stores.ledger.knows(sessionId),
                      let first = stores.scanner.firstTimestamp(ofTranscript: path, sessionId: sessionId),
                      first > root.signedInSince else { continue }
                found.append(Found(sessionId: sessionId, path: path, root: root, first: first,
                                   born: SessionTokenScanner.birthTime(of: path)))
            }
        }
        found.sort { lhs, rhs in
            if lhs.first != rhs.first { return lhs.first < rhs.first }
            let left = lhs.born ?? .distantFuture, right = rhs.born ?? .distantFuture
            return left != right ? left < right : lhs.path < rhs.path
        }

        var added: [CloudLedgerEntry] = []
        var budget = backfillFilesPerPass
        for candidate in found {
            guard budget > 0 else { break }
            let root = candidate.root
            guard let account = accounts[root.accountKey] else { continue }
            // Only transcripts never read count: one looked at before
            // (and left out) is a stat to check again.
            if stores.scanner.cachedSummary(of: candidate.sessionId) == nil { budget -= 1 }
            let owners = [SessionOwner(from: nil, accountKey: root.accountKey)]
            guard let totals = stores.scanner.scan(sessionId: candidate.sessionId, transcriptPath: candidate.path, owners: owners),
                  totals.messageCount > 0, let cwd = totals.cwd, !cwd.isEmpty,
                  let first = totals.firstTimestamp, first > root.signedInSince,
                  let last = totals.lastTimestamp else { continue }
            let source = CloudSessionSource.from(entrypoint: totals.entrypoint)
            // Claude Desktop runs its sessions as whoever it is signed in as,
            // not as the folder: only its record of a running one tells whose.
            if source == .desktop || DesktopHostedSessions.isDesktopHosted(entrypoint: totals.entrypoint) { continue }
            added.append(CloudLedgerEntry(
                sessionId: candidate.sessionId,
                identityId: root.identityId,
                accountKey: root.accountKey,
                projectName: CloudKeys.projectName(forCwd: cwd),
                projectPath: CloudKeys.projectPath(forCwd: cwd, home: home),
                transcriptPath: candidate.path,
                configDir: root.configDir,
                source: source,
                startedAt: first,
                lastActivityAt: last,
                endedAt: now.timeIntervalSince(last) > backfillOpenWindow ? last : nil,
                model: totals.models.first,
                costUsd: nil,
                title: totals.title,
                origin: .backfill
            ))
        }
        guard !added.isEmpty else { return 0 }
        return stores.ledger.record(backfill: added, accounts: [:])
    }

    /// Ended sessions (each account's part) of the allowed accounts that
    /// could be summarised, with their responses so far: only those that
    /// ended after `since` (when summaries were last turned on).
    static func summaryCandidates(accounts: [CloudAccountInfo], stores: CloudStores,
                                  since: Date?) -> [SessionSummaryStore.Candidate] {
        guard let since else { return [] }
        let allowed = Dictionary(accounts.map { ($0.accountKey, $0) }, uniquingKeysWith: { first, _ in first })
        return stores.ledger.entries().compactMap { entry in
            guard let account = allowed[entry.accountKey], let endedAt = entry.endedAt, endedAt >= since,
                  let path = entry.transcriptPath,
                  let totals = stores.scanner.cachedSummary(of: entry.sessionId) else { return nil }
            return SessionSummaryStore.Candidate(key: entry.key, sessionId: entry.sessionId, identityId: account.identityId,
                                                 accountKey: entry.accountKey, transcriptPath: path,
                                                 messageCount: totals.part(for: entry.accountKey).messageCount,
                                                 endedAt: endedAt)
        }
    }
}

/// Splits what is to be sent into requests the website takes: at most 200
/// sessions, 500 readings and 50 accounts each, every account a session or
/// reading names listed in its request. Pure.
nonisolated enum CloudBatcher {
    static func batches(sessions: [(CloudSyncRequest.Session, CloudSyncMemory.Record)],
                        readings: [RecordedUsageReading],
                        accounts: [String: CloudAccountInfo],
                        device: CloudSyncRequest.Device,
                        maxSessions: Int = CloudContract.Limit.sessions,
                        maxUsage: Int = CloudContract.Limit.usage,
                        maxAccounts: Int = CloudContract.Limit.accounts) -> [CloudSyncPass.Batch] {
        var batches: [CloudSyncPass.Batch] = []
        var current = CloudSyncPass.Batch(request: CloudSyncRequest(device: device, accounts: [], sessions: [], usage: []),
                                          records: [:], readings: [])
        var keys: Set<String> = []

        func close() {
            if !current.request.sessions.isEmpty || !current.request.usage.isEmpty {
                current.request.accounts = keys.sorted().compactMap { accounts[$0]?.contract }
                batches.append(current)
            }
            current = CloudSyncPass.Batch(request: CloudSyncRequest(device: device, accounts: [], sessions: [], usage: []),
                                          records: [:], readings: [])
            keys = []
        }
        func makeRoom(for key: String, isSession: Bool) {
            let full = isSession ? current.request.sessions.count >= maxSessions : current.request.usage.count >= maxUsage
            if full || (!keys.contains(key) && keys.count >= maxAccounts) { close() }
            keys.insert(key)
        }

        for (session, record) in sessions where accounts[session.accountKey] != nil {
            makeRoom(for: session.accountKey, isSession: true)
            current.request.sessions.append(session)
            current.records[CloudLedgerEntry.key(sessionId: session.sessionId, accountKey: session.accountKey)] = record
        }
        for reading in readings where accounts[reading.accountKey] != nil {
            makeRoom(for: reading.accountKey, isSession: false)
            current.request.usage.append(reading.contract)
            current.readings.append(reading)
        }
        close()
        return batches
    }
}

// MARK: - The service

@MainActor
final class CloudSync: ObservableObject {
    static let shared = CloudSync()

    private static var logger: Logger { EngineLog.logger("CloudSync") }

    // MARK: Tuning

    nonisolated static let syncInterval: TimeInterval = 5 * 60
    nonisolated static let tickInterval: TimeInterval = 20
    /// After a session ends or a summary is written.
    nonisolated static let soonDelay: TimeInterval = 30
    nonisolated static let initialBackoff: TimeInterval = 30
    nonisolated static let maxBackoff: TimeInterval = 30 * 60
    nonisolated static let backfillInterval: TimeInterval = 6 * 60 * 60
    /// A rate-limited summary pauses them all this long.
    nonisolated static let summaryPause: TimeInterval = 30 * 60
    /// A summary whose folder has no one signed in as its account waits this long.
    nonisolated static let noFolderWait: TimeInterval = 60 * 60
    /// An account whose 5-hour window is at least this used (percent) gets
    /// no summaries until it comes down: they would eat into real work.
    nonisolated static let summaryUsageCeiling: Double = 80

    // MARK: Dependencies

    struct Dependencies {
        /// Where the files live; nil keeps everything in memory.
        var directory: URL?
        var persists: Bool
        /// A sealed run: fixture state only.
        var sealed: Bool
        var settings: ClaudeControlSettings.Store
        var transport: any CloudTransport
        var sessionStore: any CloudSessionStoring
        var summaryRunner: SessionSummarizer.Runner
        var clock: @Sendable () -> Date
        var home: String
        var appVersion: String
        var deviceName: String
        /// `AGENTNOTCH_WEB_URL`.
        var websiteOverride: String?
        /// This run may launch Claude Code for a summary.
        var summariesAllowed: @Sendable () -> Bool

        /// The app's: the engine's folder, the real network (refused unless
        /// bootstrapped and live), Claude Code for summaries.
        static func live() -> Dependencies {
            let sealed = AppIdentity.isSealed
            let home = AppIdentity.homeDirectory
            return Dependencies(
                directory: sealed ? nil : AppIdentity.supportDirectory,
                persists: !sealed,
                sealed: sealed,
                settings: ClaudeControlSettings.store,
                transport: URLSessionCloudTransport.shared,
                sessionStore: CloudSessionFileStore(),
                summaryRunner: SessionSummarizer.runClaudeCode,
                clock: { Date() },
                home: home,
                appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0",
                deviceName: CloudSync.computerName(),
                websiteOverride: sealed ? nil : DevFlags.webURLOverride,
                summariesAllowed: { CloudSync.summariesAllowedByDefault }
            )
        }
    }

    /// Whether this run may launch Claude Code for summaries: bootstrapped,
    /// live, allowed to launch it at all, and not a `--no-install` dev run
    /// (which would summarise the same sessions as the app beside it).
    nonisolated static var summariesAllowedByDefault: Bool {
        AppIdentity.isFrozen && !AppIdentity.isSealed && !DevFlags.probesDisabled && !DevFlags.installsDisabled
    }

    /// The Mac's name as Sharing shows it ("Paras's MacBook Pro").
    nonisolated static func computerName() -> String {
        if let name = SCDynamicStoreCopyComputerName(nil, nil) as String?, !name.isEmpty { return name }
        return ProcessInfo.processInfo.hostName
    }

    // MARK: State

    @Published private(set) var state = ClaudeCloudState()

    private let makeDependencies: @MainActor () -> Dependencies
    private var deps: Dependencies?
    private(set) var stores: CloudStores?
    private var auth: CloudAuth?
    private var environment: (any CloudSyncEnvironment)?
    private var started = false
    private var tickTask: Task<Void, Never>?
    /// The summary running beside the schedule (cancelled when summaries
    /// or sync are switched off, on sign-out and on quit).
    private(set) var summaryTask: Task<Void, Never>?
    private var usageSubscription: AnyCancellable?

    private var authState: ClaudeCloudState.Auth = .signedOut
    /// Bumped by every sign-in and sign-out: a look at the saved session
    /// (or a sign-in) that started before one is out of date when it ends.
    private var authGeneration = 0
    private var dashboardURL: URL?
    private var lastError: String?
    private var isSyncing = false
    private var isSummarizing = false
    /// Bumped when summaries (or sync) are turned off: a run still going is dropped.
    private var summaryGeneration = 0
    /// Bumped when sync is turned off or the user signed out: a pass in
    /// flight sends nothing more.
    private var syncGeneration = 0
    private var nextSyncAt: Date?
    /// After a failure: no sync before this (the backoff, or the website's
    /// Retry-After), whatever else asks for one sooner.
    private var notBefore: Date?
    private var failures = 0
    private var lastBackfillAt: Date?
    private var maxSessionsPerRequest = CloudContract.Limit.sessions
    private var summariesPausedUntil: Date?
    private var pendingSessions = 0
    private var lastLiveIDs: Set<String> = []
    /// The hub reported running sessions since capture last (re)started:
    /// until it has, `lastLiveIDs` says nothing about what ended.
    private var liveObservedSinceResume = false

    init(dependencies: @escaping @MainActor () -> Dependencies = { Dependencies.live() }) {
        makeDependencies = dependencies
    }

    // MARK: Lifecycle

    /// Start: load the saved session and state, follow `usage` readings,
    /// and (with `runsLoop`) tick on a timer. A sealed run only shows the
    /// fixture state.
    func start(environment: (any CloudSyncEnvironment)? = nil,
               usage: AnyPublisher<UsageObservation, Never>? = nil,
               runsLoop: Bool = true) {
        guard !started else { return }
        started = true
        let deps = makeDependencies()
        self.deps = deps
        if deps.sealed {
            Self.logger.notice("Sealed: the website's state is a fixture; nothing is sent")
            state = ClaudeCloudState.sealedFixture(now: deps.clock())
            return
        }
        let stores = CloudStores(directory: deps.directory, persists: deps.persists, home: deps.home)
        self.stores = stores
        // Summaries turned on before they kept a date: from now on.
        if deps.settings.cloudSummariesEnabled, stores.summaries.enabledAt == nil {
            stores.summaries.noteEnabled(at: deps.clock())
        }
        auth = CloudAuth(transport: deps.transport, store: deps.sessionStore, clock: deps.clock)
        self.environment = environment ?? LiveCloudEnvironment()
        dashboardURL = stores.memory.dashboardUrl.flatMap(CloudWebsite.validatedLink)
        usageSubscription = usage?
            .receive(on: DispatchQueue.main)
            .sink { [weak self] observation in self?.record(observation) }
        publish()
        Task { await self.restoreSession() }
        if runsLoop {
            tickTask = Task { [weak self] in
                while !Task.isCancelled {
                    await self?.tick()
                    try? await Task.sleep(for: .seconds(Self.tickInterval))
                }
            }
        }
    }

    func stop() {
        guard started else { return }
        started = false
        tickTask?.cancel()
        tickTask = nil
        // A summary still running is stopped, not left behind.
        summaryTask?.cancel()
        summaryTask = nil
        usageSubscription = nil
        stores?.saveNow()
    }

    /// The saved session, if it is for the website set now (another
    /// website's is set aside, not deleted: signing in replaces it).
    func restoreSession() async {
        guard let auth, let deps, !deps.sealed else { return }
        let generation = authGeneration
        let session = await auth.currentSession()
        guard generation == authGeneration else { return }
        guard let session else {
            authState = .signedOut
            publish()
            return
        }
        guard let website = effectiveWebsite, session.websiteURL == website.absoluteString else {
            await auth.setAside()
            authState = .signedOut
            publish()
            return
        }
        authState = .signedIn(email: session.email)
        publish()
    }

    // MARK: Settings

    private var settings: ClaudeControlSettings.Store {
        deps?.settings ?? ClaudeControlSettings.store
    }

    /// The website in use: `AGENTNOTCH_WEB_URL`, else the one the user typed.
    var effectiveWebsite: URL? {
        if let override = deps?.websiteOverride, let url = CloudWebsite.validated(override) { return url }
        return CloudWebsite.validated(settings.cloudWebsiteURL)
    }

    /// Set (or, with nil or blank, clear) the website's address. False when
    /// it isn't one the app accepts (https, or http to this Mac). Another
    /// website turns sync and summaries off (they were agreed to for the old
    /// one) and signs out of the old one, a sign-in in progress included.
    @discardableResult
    func setWebsite(_ text: String?) async -> Bool {
        guard let deps, !deps.sealed else { return false }
        let trimmed = text?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let newValue: String?
        if trimmed.isEmpty {
            newValue = nil
        } else {
            guard let url = CloudWebsite.validated(trimmed) else { return false }
            newValue = url.absoluteString
        }
        let before = effectiveWebsite
        settings.cloudWebsiteURL = newValue
        if effectiveWebsite != before {
            turnSwitchesOff()
            switch authState {
            case .signedIn, .signingIn: await signOut()
            case .signedOut, .error: break
            }
        }
        lastError = nil
        publish()
        return true
    }

    func setSyncEnabled(_ enabled: Bool) {
        guard let deps, !deps.sealed else { return }
        settings.cloudSyncEnabled = enabled
        if enabled {
            nextSyncAt = nil
            failures = 0
        } else {
            stopSyncing()
        }
        publish()
    }

    func setSummariesEnabled(_ enabled: Bool) {
        guard let deps, !deps.sealed else { return }
        settings.cloudSummariesEnabled = enabled
        if enabled {
            // Sessions that ended before now are never summarised.
            stores?.summaries.noteEnabled(at: now)
        } else {
            stopSummarizing()
            forgetUnsentSummaries()
        }
        publish()
    }

    /// Sync and summaries off, and whatever they were doing stopped.
    private func turnSwitchesOff() {
        settings.cloudSyncEnabled = false
        settings.cloudSummariesEnabled = false
        stopSyncing()
        forgetUnsentSummaries()
    }

    /// Summaries are off: the ones the website this Mac is signed in to
    /// doesn't have (made, never sent to it) are deleted here. Those it has
    /// stay there (the website keeps what was sent) and here.
    private func forgetUnsentSummaries() {
        guard let stores else { return }
        let removed = stores.summaries.removeAll { key, summary in
            CloudSyncPass.wasSent(summary, key: key, stores: stores)
        }
        if removed > 0 { Self.logger.info("Deleted \(removed) summary(ies) that were never sent") }
    }

    /// Capture and uploads stop: a pass in flight sends nothing more, a
    /// summary in flight is stopped, the readings kept only to be sent go,
    /// and what capture saw no longer dates the end of a session (it may
    /// end during the gap: then it ends at its last activity).
    private func stopSyncing() {
        syncGeneration += 1
        stopSummarizing()
        stores?.ledger.forgetRunState()
        lastLiveIDs = []
        liveObservedSinceResume = false
        stores?.recorder.clear()
        pendingSessions = 0
    }

    /// A summary in flight is dropped and its `claude` stopped.
    private func stopSummarizing() {
        summaryGeneration += 1
        summaryTask?.cancel()
    }

    // MARK: Signing in

    /// Google sign-in through the website's Supabase project; `browser` is
    /// the host's (an ASWebAuthenticationSession). True when signed in. A
    /// sign-in is bound to the website it started with: if that changed
    /// meanwhile (or the user signed out), its result is thrown away and its
    /// session ended on Supabase. A new sign-in starts with sync and
    /// summaries off, for the user to turn on.
    @discardableResult
    func signIn(presentingBrowser browser: @escaping ClaudeCloudBrowser) async -> Bool {
        guard let deps, !deps.sealed, let auth else { return false }
        guard let website = effectiveWebsite else {
            lastError = CloudAPIError.invalidWebsite.errorDescription
            publish()
            return false
        }
        if case .signingIn = authState { return false }
        authGeneration += 1
        let generation = authGeneration
        authState = .signingIn
        lastError = nil
        publish()
        /// Still the sign-in the user is waiting for, for the same website.
        func isCurrent() -> Bool {
            started && generation == authGeneration && effectiveWebsite?.absoluteString == website.absoluteString
        }
        do {
            let api = makeAPI(website: website)
            let config = try await api.config()
            guard isCurrent() else { return false }
            guard config.redirectUrl == CloudContract.redirectURL else {
                throw CloudAuthError.badConfig("redirectUrl is \(config.redirectUrl)")
            }
            let session = try await auth.signIn(config: config, website: website.absoluteString, presentBrowser: browser)
            guard isCurrent() else {
                Self.logger.notice("The website changed during sign-in: that sign-in is thrown away")
                await auth.revoke(session)
                return false
            }
            await auth.adopt(session)
            var email = session.email
            var userId = session.userId
            var dashboard = config.dashboardUrl
            if let me = try? await api.me() {
                email = me.user.email ?? email
                userId = me.user.id
                dashboard = me.dashboardUrl
            }
            // Signed out, or the website changed, while asking: whoever did
            // that ended this session already.
            guard isCurrent() else { return false }
            dashboardURL = CloudWebsite.validatedLink(dashboard)
            stores?.memory.adopt(userId: userId, website: website.absoluteString, dashboardUrl: dashboardURL?.absoluteString)
            // Consent is per sign-in: the user turns sync on for this one.
            turnSwitchesOff()
            authState = .signedIn(email: email)
            nextSyncAt = nil
            notBefore = nil
            failures = 0
            publish()
            return true
        } catch {
            guard isCurrent() else { return false }
            if case CloudAuthError.cancelled = error {
                authState = .signedOut
            } else {
                let message = Self.describe(error)
                authState = .error(message)
                lastError = message
                Self.logger.error("Sign-in failed: \(message, privacy: .public)")
            }
            publish()
            return false
        }
    }

    /// Sign out of the website (this Mac only). Sync and summaries go off,
    /// and what waits to be sent goes.
    func signOut() async {
        guard let deps, !deps.sealed, let auth else { return }
        authGeneration += 1
        turnSwitchesOff()
        await auth.signOut()
        authState = .signedOut
        dashboardURL = nil
        publish()
    }

    // MARK: Feeding

    /// Signed in, sync on: sessions are captured and readings recorded.
    private var isCapturing: Bool { canUpload }

    /// The hub's running sessions it attributed for certain (see
    /// `ClaudeControlHub.feedCloud`), those whose account it can't tell now
    /// (`unsure`), those it hasn't placed yet (`waiting`: neither counted
    /// nor paused), and every running session's id. Only while signed in
    /// with sync on; only allowed accounts, each keyed as `accounts()` keys
    /// it. A session the ledger knows that runs as no allowed account now
    /// (unsure, or an account the website may not hear of) counts for none.
    func observeLive(_ live: [LiveSessionObservation], unsure: Set<String> = [], waiting: Set<String> = [],
                     liveIDs: Set<String>) {
        guard isCapturing, let stores, let environment else { return }
        lastLiveIDs = liveIDs.union(waiting)
        liveObservedSinceResume = true
        let accounts = environment.accounts()
        let byIdentity = Dictionary(accounts.map { ($0.identityId, $0) }, uniquingKeysWith: { first, _ in first })
        let kept: [LiveSessionObservation] = live.compactMap { observation in
            guard let account = byIdentity[observation.identityId] else { return nil }
            var keyed = observation
            keyed.accountKey = account.accountKey
            return keyed
        }
        let ledgerAccounts = Dictionary(kept.compactMap { observation in
            byIdentity[observation.identityId].map { ($0.accountKey, $0.ledgerAccount) }
        }, uniquingKeysWith: { first, _ in first })
        let ended = stores.ledger.observe(live: kept, liveIDs: liveIDs, unsure: unsure, waiting: waiting,
                                          accounts: ledgerAccounts, now: now)
        if !ended.isEmpty { syncSoon() }
    }

    /// A usage reading `UsageStore` took in. Only while signed in with sync
    /// on; only allowed accounts.
    func record(_ observation: UsageObservation) {
        guard isCapturing, let stores, let environment else { return }
        guard let account = environment.accounts().first(where: { $0.identityId == observation.identityId }) else { return }
        let kept = stores.recorder.record(RecordedUsageReading(accountKey: account.accountKey, identityId: account.identityId,
                                                               source: observation.source, observedAt: observation.observedAt,
                                                               windows: observation.windows))
        if kept { publish() }
    }

    // MARK: Schedule

    /// The regular pass: note who is signed in where, end sessions gone for
    /// a minute, sync when due, summarise when allowed.
    func tick() async {
        guard started, let stores else { return }
        // Local only, whether or not sync is on: the backfill needs to know
        // since when each folder has been signed in as its account.
        if let logins = environment?.folderLogins() { stores.folderLogins.observe(logins, now: now) }
        if isCapturing, liveObservedSinceResume {
            let ended = stores.ledger.settle(liveIDs: lastLiveIDs, now: now)
            if !ended.isEmpty { syncSoon() }
        }
        // A summary (up to 90 s) runs beside the schedule, never in its way.
        if canSummarize, !isSummarizing, summaryTask == nil {
            summaryTask = Task { [weak self] in
                await self?.summarizeNext()
                self?.summaryTask = nil
            }
        }
        if canUpload, !isSyncing, (nextSyncAt ?? .distantPast) <= now, (notBefore ?? .distantPast) <= now {
            await syncNow()
        }
    }

    private var now: Date { deps?.clock() ?? Date() }

    /// Sync within `soonDelay` (unless one is due sooner), never before a
    /// failure's backoff or the website's Retry-After ends.
    private func syncSoon() {
        var soon = now.addingTimeInterval(Self.soonDelay)
        if let notBefore, notBefore > soon { soon = notBefore }
        if let next = nextSyncAt, next <= soon { return }
        nextSyncAt = soon
    }

    /// Signed in, sync on, a website: uploads may happen.
    var canUpload: Bool {
        guard started, let deps, !deps.sealed, settings.cloudSyncEnabled, effectiveWebsite != nil else { return false }
        if case .signedIn = authState { return true }
        return false
    }

    /// Uploads may happen, summaries are on, and this run may launch Claude Code.
    var canSummarize: Bool {
        guard canUpload, let deps, settings.cloudSummariesEnabled, deps.summariesAllowed() else { return false }
        if let environment, environment.isLaunchingClaude { return false }
        return (summariesPausedUntil ?? .distantPast) <= now
    }

    // MARK: Syncing

    /// One sync pass now: what changed since it was last sent, in batches.
    /// Turning sync off (or signing out) during it stops it before the next
    /// batch; turning summaries off leaves them out of the batches still to go.
    /// A pass belongs to the sign-in and website it started with: when
    /// either changed while a request was out (signed out, in again, or to
    /// another website), what comes back is dropped. Nothing is marked sent
    /// and no failure is held against the new sign-in.
    func syncNow() async {
        guard canUpload, !isSyncing, let stores, let deps, let environment, let website = effectiveWebsite else { return }
        isSyncing = true
        publish()
        defer {
            isSyncing = false
            publish()
        }
        let generation = syncGeneration
        let passSignIn = authGeneration
        /// Still the sign-in and website the pass started with.
        func isBound() -> Bool {
            passSignIn == authGeneration && effectiveWebsite?.absoluteString == website.absoluteString
        }
        let passStart = now
        let backfillFolders: [CloudBackfill.Folder]?
        if lastBackfillAt.map({ passStart.timeIntervalSince($0) >= Self.backfillInterval }) ?? true {
            backfillFolders = environment.backfillFolders().map { folder in
                var dated = folder
                dated.signedInSince = stores.folderLogins.since(folder: folder.configDir, login: folder.login)
                return dated
            }
            lastBackfillAt = passStart
        } else {
            backfillFolders = nil
        }
        let input = CloudSyncPass.Input(
            accounts: environment.accounts(),
            backfillFolders: backfillFolders,
            includeSummaries: settings.cloudSummariesEnabled,
            device: CloudSyncRequest.Device(id: settings.cloudDeviceId, name: deps.deviceName, appVersion: deps.appVersion),
            now: passStart,
            home: deps.home,
            maxSessionsPerRequest: maxSessionsPerRequest
        )
        let prepared = await Task.detached(priority: .utility) {
            CloudSyncPass.prepare(input, stores: stores)
        }.value
        guard isBound() else { return }
        pendingSessions = prepared.sessionCount
        publish()

        let api = makeAPI(website: website)
        for var batch in prepared.batches {
            guard generation == syncGeneration, isBound(), canUpload else {
                Self.logger.info("Sync was switched off (or the sign-in changed) during a pass: the rest isn't sent")
                return
            }
            if !settings.cloudSummariesEnabled { batch = batch.withoutSummaries() }
            do {
                _ = try await api.sync(batch.request)
                guard isBound() else {
                    Self.logger.notice("The sign-in or website changed during a sync: its answer is dropped")
                    return
                }
                stores.memory.markSent(batch.records, at: now)
                stores.recorder.markSent(batch.readings)
                pendingSessions = max(0, pendingSessions - batch.records.count)
            } catch {
                guard isBound() else {
                    Self.logger.notice("The sign-in or website changed during a sync: its failure is dropped")
                    return
                }
                await handleSyncFailure(error)
                return
            }
        }
        guard generation == syncGeneration, isBound() else { return }
        stores.memory.noteSynced(at: now)
        failures = 0
        notBefore = nil
        lastError = nil
        nextSyncAt = now.addingTimeInterval(prepared.hasMore ? Self.soonDelay : Self.syncInterval)
        Self.logger.info("Synced \(prepared.batches.count) request(s); \(prepared.backfilled) session(s) backfilled")
    }

    private func handleSyncFailure(_ error: Error) async {
        let now = self.now
        switch error {
        case CloudAuthError.signedOut, CloudAuthError.otherWebsite, CloudAPIError.notSignedIn:
            // Supabase refused the refresh token (the session is gone), or it
            // belongs to another website: signed out, and the switches with it.
            authGeneration += 1
            turnSwitchesOff()
            authState = .signedOut
            dashboardURL = nil
            lastError = "Signed out of the website. Sign in again."
        case CloudAPIError.server(let status, _, let message, let retryAfter):
            if status == 413, maxSessionsPerRequest > 10 {
                maxSessionsPerRequest = max(10, maxSessionsPerRequest / 2)
                nextSyncAt = now.addingTimeInterval(5)
            } else {
                failures += 1
                if status == 401 {
                    // Refused even after a refresh the sign-in service
                    // accepted: the website's own check failed (its key set
                    // can't be fetched, say). The session is kept; try later.
                    lastError = "The website didn't accept this Mac's sign-in. Trying again later."
                } else {
                    lastError = message ?? CloudAPIError.server(status: status, code: nil, message: nil, retryAfter: nil).errorDescription
                }
                backOff(until: now.addingTimeInterval(max(Self.backoff(afterFailures: failures), retryAfter ?? 0)))
            }
        default:
            failures += 1
            lastError = Self.describe(error)
            backOff(until: now.addingTimeInterval(Self.backoff(afterFailures: failures)))
        }
        Self.logger.error("Sync failed: \(self.lastError ?? "unknown", privacy: .public)")
    }

    private func backOff(until date: Date) {
        notBefore = date
        nextSyncAt = date
    }

    /// 30 s, doubling per failure, up to 30 minutes. Pure.
    nonisolated static func backoff(afterFailures failures: Int) -> TimeInterval {
        guard failures > 0 else { return 0 }
        return min(initialBackoff * pow(2, Double(min(failures - 1, 16))), maxBackoff)
    }

    private func makeAPI(website: URL) -> CloudAPI {
        CloudAPI(website: website, transport: deps?.transport ?? URLSessionCloudTransport.shared, auth: auth,
                 appVersion: deps?.appVersion ?? "0")
    }

    // MARK: Summaries

    /// Summarise the next due session, if any (one at a time): one that
    /// ended after summaries were turned on, of an account whose 5-hour
    /// window is under `summaryUsageCeiling`, run in a folder of the account
    /// that ran it, from its own part of the conversation only.
    func summarizeNext() async {
        guard canSummarize, !isSummarizing, let stores, let deps, let environment else { return }
        isSummarizing = true
        defer { isSummarizing = false }
        let generation = summaryGeneration
        let passStart = now
        let accounts = environment.accounts().filter { account in
            (environment.sessionUtilization(identityId: account.identityId) ?? 0) < Self.summaryUsageCeiling
        }
        let candidate = await Task.detached(priority: .utility) {
            stores.summaries.nextCandidate(
                from: CloudSyncPass.summaryCandidates(accounts: accounts, stores: stores, since: stores.summaries.enabledAt),
                now: passStart)
        }.value
        guard let candidate, generation == summaryGeneration else { return }

        guard let folder = await environment.summaryFolder(forIdentity: candidate.identityId) else {
            stores.summaries.recordFailure(key: candidate.key, reason: "No folder is signed in as this account now",
                                           at: passStart, minimumWait: Self.noFolderWait)
            return
        }
        let stretches = SessionOwners.stretches(of: candidate.accountKey, in: stores.ledger.owners(of: candidate.sessionId))
        let excerpt = await Task.detached(priority: .utility) {
            SessionExcerpt.build(transcriptPath: candidate.transcriptPath, sessionId: candidate.sessionId, stretches: stretches)
        }.value
        guard let excerpt else {
            stores.summaries.recordFailure(key: candidate.key, reason: "No conversation to summarise",
                                           at: passStart, minimumWait: SessionSummaryStore.maxRetry)
            return
        }
        // Switched off (or signed out) while it was being prepared: nothing is launched.
        guard generation == summaryGeneration, canSummarize, !Task.isCancelled else { return }
        stores.summaries.noteRun(at: passStart)
        let workingDirectory = (deps.directory ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true))
            .appendingPathComponent("session-summary", isDirectory: true)
        let request = SessionSummarizer.Request(sessionId: candidate.sessionId, identityId: candidate.identityId,
                                                input: SessionSummarizer.input(excerpt: excerpt),
                                                configDirEnv: folder.configDirEnv, configDirs: folder.configDirs,
                                                workingDirectory: workingDirectory)
        let outcome = await deps.summaryRunner(request)
        // Turned off meanwhile: whatever came back isn't wanted.
        guard generation == summaryGeneration, settings.cloudSummariesEnabled, !Task.isCancelled else { return }
        guard await environment.summaryFolderStillRuns(folder, identityId: candidate.identityId) else {
            stores.summaries.recordFailure(key: candidate.key, reason: "The folder changed accounts during the summary",
                                           at: now)
            return
        }
        switch outcome {
        case .summary(let summary):
            stores.summaries.record(key: candidate.key, summary: summary, messageCount: candidate.messageCount, at: now)
            syncSoon()
        case .rateLimited:
            summariesPausedUntil = now.addingTimeInterval(Self.summaryPause)
            stores.summaries.recordFailure(key: candidate.key, reason: "Rate limited", at: now,
                                           minimumWait: Self.summaryPause)
        case .unavailable(let reason):
            stores.summaries.recordFailure(key: candidate.key, reason: reason, at: now, minimumWait: 6 * 60 * 60)
        case .failed(let reason):
            stores.summaries.recordFailure(key: candidate.key, reason: reason, at: now)
        }
        publish()
    }

    // MARK: Publishing

    private func publish() {
        guard let deps, !deps.sealed else { return }
        var next = ClaudeCloudState()
        next.websiteURL = effectiveWebsite?.absoluteString
        next.websiteIsOverridden = deps.websiteOverride.flatMap(CloudWebsite.validated) != nil
        next.auth = authState
        next.syncEnabled = settings.cloudSyncEnabled
        next.summariesEnabled = settings.cloudSummariesEnabled
        next.summariesAvailable = deps.summariesAllowed()
        next.isSyncing = isSyncing
        next.lastSyncAt = stores?.memory.lastSyncAt
        next.lastError = lastError
        next.pendingSessions = settings.cloudSyncEnabled ? pendingSessions : 0
        next.pendingUsage = stores?.recorder.pendingCount ?? 0
        next.summarizedSessions = stores?.summaries.count ?? 0
        next.dashboardURL = dashboardURL
        if state != next { state = next }
    }

    /// A short, user-facing reason.
    nonisolated static func describe(_ error: Error) -> String {
        if let error = error as? LocalizedError, let text = error.errorDescription { return text }
        return error.localizedDescription
    }
}
