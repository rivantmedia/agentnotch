//
//  ReviewStateStore.swift
//  ClaudeControl
//
//  Persists the review queue ("Claude finished, you haven't looked yet") and
//  failed turns in `<supportDirectory>/review-state.json`, so they survive app
//  restarts: when a session is rediscovered (hook, registry, status line) its
//  completion, review and failure are restored.
//
//  It also keeps a heartbeat, `lastAliveAt`, updated while the app runs: a
//  turn whose reply is newer than the previous run's heartbeat finished
//  while nobody was watching (see SessionStore.inferCompletion).
//
//  Completions and failures are written at once (a crash or SIGTERM right
//  after one must not lose it); review marks are debounced. Writes are
//  atomic; entries untouched for 7 days are pruned. The file of Superpowered
//  Vibe Notch (a bare `{sessionId: record}` object) is read as well.
//

import Foundation
import os.log

nonisolated private var logger: Logger { EngineLog.logger("ReviewState") }

/// Review-relevant state of one session.
nonisolated struct ReviewRecord: Codable, Equatable, Sendable {
    var completedAt: Date?
    var reviewedAt: Date?
    var lastAssistantMessage: String?
    /// The last turn failed (StopFailure): humanized error, raw code, when.
    var stopError: String?
    var stopErrorCode: String?
    var failedAt: Date?
    /// Last time the record changed; drives pruning.
    var updatedAt: Date

    init(
        completedAt: Date? = nil,
        reviewedAt: Date? = nil,
        lastAssistantMessage: String? = nil,
        stopError: String? = nil,
        stopErrorCode: String? = nil,
        failedAt: Date? = nil,
        updatedAt: Date
    ) {
        self.completedAt = completedAt
        self.reviewedAt = reviewedAt
        self.lastAssistantMessage = lastAssistantMessage
        self.stopError = stopError
        self.stopErrorCode = stopErrorCode
        self.failedAt = failedAt
        self.updatedAt = updatedAt
    }

    /// Nothing worth keeping.
    var isEmpty: Bool {
        completedAt == nil && reviewedAt == nil && lastAssistantMessage == nil && stopError == nil
    }

    /// Same content, ignoring when it was written.
    func hasSameContent(as other: ReviewRecord) -> Bool {
        var copy = other
        copy.updatedAt = updatedAt
        return copy == self
    }
}

nonisolated final class ReviewStateStore: @unchecked Sendable {
    /// A sealed run keeps its fixture review marks in memory only: nothing
    /// is read from or written to its temporary support folder.
    static let shared = ReviewStateStore(
        fileURL: AppIdentity.supportDirectory.appendingPathComponent("review-state.json"),
        persists: !DevFlags.isSealed
    )

    /// Records untouched for longer than this are dropped.
    static let retention: TimeInterval = 7 * 24 * 60 * 60
    /// Longest preview kept on disk.
    static let maxMessageLength = 1500
    /// How often the running app refreshes `lastAliveAt`.
    static let heartbeatInterval: TimeInterval = 30

    /// The file's current shape.
    private struct FileContents: Codable {
        var version = 2
        var lastAliveAt: Date?
        var sessions: [String: ReviewRecord]
    }

    private let fileURL: URL
    private let writeDelay: TimeInterval
    /// Whether a missing folder is created on write. Tests say no, so a
    /// write landing after a test removed its temporary folder doesn't
    /// bring the folder back.
    private let createsFolder: Bool
    private let lock = NSLock()
    private var records: [String: ReviewRecord] = [:]
    private var lastAliveAt: Date?
    private let ioQueue = DispatchQueue(label: EngineLog.queueLabel("review-state"), qos: .utility)
    /// False: records live in memory only (sealed runs).
    private let persists: Bool
    private var pendingWrite: DispatchWorkItem?
    private var heartbeat: DispatchSourceTimer?

    /// The previous run's last heartbeat, read at launch (nil on a first
    /// run, or from a file without one).
    let previousRunAliveAt: Date?

    init(fileURL: URL, writeDelay: TimeInterval = 1.0, now: Date = Date(), createsFolder: Bool = true,
         persists: Bool = true) {
        self.fileURL = fileURL
        self.writeDelay = writeDelay
        self.createsFolder = createsFolder
        self.persists = persists
        let loaded = persists ? Self.load(from: fileURL) : (sessions: [:], lastAliveAt: nil)
        self.records = Self.prune(loaded.sessions, now: now)
        self.lastAliveAt = loaded.lastAliveAt
        self.previousRunAliveAt = loaded.lastAliveAt
    }

    // MARK: - Access

    func record(for sessionId: String) -> ReviewRecord? {
        lock.lock()
        defer { lock.unlock() }
        return records[sessionId]
    }

    /// Stores the session's record if its content changed. `urgent` writes
    /// right away (completions, failures); otherwise the write is debounced.
    func update(sessionId: String, record: ReviewRecord, urgent: Bool = false) {
        var record = record
        record.lastAssistantMessage = record.lastAssistantMessage.map { String($0.prefix(Self.maxMessageLength)) }
        lock.lock()
        if let existing = records[sessionId], existing.hasSameContent(as: record) {
            lock.unlock()
            return
        }
        if record.isEmpty {
            records.removeValue(forKey: sessionId)
        } else {
            records[sessionId] = record
        }
        lock.unlock()
        if urgent {
            writeSoon(after: 0)
        } else {
            writeSoon(after: writeDelay)
        }
    }

    /// Stores completion, review and preview (no failure).
    func update(sessionId: String, completedAt: Date?, reviewedAt: Date?, lastAssistantMessage: String?, now: Date = Date()) {
        update(
            sessionId: sessionId,
            record: ReviewRecord(completedAt: completedAt, reviewedAt: reviewedAt, lastAssistantMessage: lastAssistantMessage, updatedAt: now)
        )
    }

    /// Writes immediately (app termination, tests).
    func flush() {
        ioQueue.sync {
            pendingWrite?.cancel()
            pendingWrite = nil
            writeNow()
        }
    }

    // MARK: - Heartbeat

    /// Refreshes `lastAliveAt` now and every `heartbeatInterval` until
    /// `stopHeartbeat` (which writes a last one).
    func startHeartbeat(interval: TimeInterval = heartbeatInterval) {
        ioQueue.async { [self] in
            guard heartbeat == nil else { return }
            let timer = DispatchSource.makeTimerSource(queue: ioQueue)
            timer.schedule(deadline: .now(), repeating: interval, leeway: .seconds(2))
            timer.setEventHandler { [weak self] in
                self?.beat()
            }
            heartbeat = timer
            timer.resume()
        }
    }

    func stopHeartbeat() {
        ioQueue.sync {
            heartbeat?.cancel()
            heartbeat = nil
            beat()
        }
    }

    /// Must run on `ioQueue`.
    private func beat() {
        lock.lock()
        lastAliveAt = Date()
        lock.unlock()
        pendingWrite?.cancel()
        pendingWrite = nil
        writeNow()
    }

    // MARK: - Persistence

    private func writeSoon(after delay: TimeInterval) {
        ioQueue.async { [self] in
            pendingWrite?.cancel()
            let work = DispatchWorkItem { [weak self] in
                self?.writeNow()
            }
            pendingWrite = work
            if delay <= 0 {
                work.perform()
            } else {
                ioQueue.asyncAfter(deadline: .now() + delay, execute: work)
            }
        }
    }

    /// Must run on `ioQueue`.
    private func writeNow() {
        guard persists else { return }
        lock.lock()
        records = Self.prune(records, now: Date())
        let contents = FileContents(lastAliveAt: lastAliveAt, sessions: records)
        lock.unlock()

        do {
            let data = try Self.encoder.encode(contents)
            if createsFolder {
                try FileManager.default.createDirectory(
                    at: fileURL.deletingLastPathComponent(),
                    withIntermediateDirectories: true,
                    attributes: [.posixPermissions: 0o700]
                )
            } else if !FileManager.default.fileExists(atPath: fileURL.deletingLastPathComponent().path) {
                return
            }
            try data.write(to: fileURL, options: .atomic)
            // It holds the last lines of Claude's replies: owner only.
            try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: fileURL.path)
        } catch {
            logger.error("Failed to write review state: \(error.localizedDescription, privacy: .public)")
        }
    }

    private static func load(from url: URL) -> (sessions: [String: ReviewRecord], lastAliveAt: Date?) {
        guard let data = try? Data(contentsOf: url) else { return ([:], nil) }
        if let contents = try? decoder.decode(FileContents.self, from: data) {
            return (contents.sessions, contents.lastAliveAt)
        }
        do {
            // Superpowered Vibe Notch's shape: the records alone.
            return (try decoder.decode([String: ReviewRecord].self, from: data), nil)
        } catch {
            logger.warning("Ignoring unreadable review state: \(error.localizedDescription, privacy: .public)")
            return ([:], nil)
        }
    }

    static func prune(_ records: [String: ReviewRecord], now: Date) -> [String: ReviewRecord] {
        let cutoff = now.addingTimeInterval(-retention)
        return records.filter { $0.value.updatedAt >= cutoff }
    }

    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        encoder.outputFormatting = [.sortedKeys]
        return encoder
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        return decoder
    }()
}
