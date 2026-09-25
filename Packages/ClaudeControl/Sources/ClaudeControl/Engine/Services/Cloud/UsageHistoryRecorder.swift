//
//  UsageHistoryRecorder.swift
//  ClaudeControl
//
//  Usage-limit readings over time, for sync. `UsageStore` keeps only the
//  latest reading per account; this keeps each reading it takes in, with
//  where it came from, until the website has it:
//  - `probe`: Claude Code's `get_usage`;
//  - `statusLine`: a live session's `rate_limits` (5-hour and weekly only);
//  - `claudeJson`: Claude Code's cached copy in `.claude.json`;
//  - `desktop`: Claude Desktop's HTTP cache.
//  Windows: `session`, `weekly_all`, `weekly_<model>`, `extra_usage`.
//
//  A reading is dated when Claude Code or Claude Desktop took it. Readings
//  are deduplicated by (account, source, observedAt) with windows merged by
//  id; a reading no newer than the last one taken from the same account and
//  source is dropped, and so is one that repeats the previous values within
//  ten minutes (caches are re-read every 20 seconds). The outbox is bounded:
//  the oldest go first.
//

import Foundation

/// A reading `UsageStore` took in.
nonisolated struct UsageObservation: Equatable, Sendable {
    /// The identity id (or a folder id not grouped yet: dropped later).
    var identityId: String
    var source: CloudUsageSource
    var observedAt: Date
    var windows: [CloudSyncRequest.UsageWindow]

    /// The windows of a full snapshot, as recorded (utilization as read,
    /// not reset to 0 for a window whose reset has passed since). Pure.
    static func windows(from usage: AccountUsage) -> [CloudSyncRequest.UsageWindow] {
        var windows: [CloudSyncRequest.UsageWindow] = []
        func add(_ id: String, _ window: UsageWindow?) {
            guard let window, window.utilization.isFinite, let id = CloudKeys.contractWindowID(id) else { return }
            windows.append(.init(id: id, utilization: max(0, window.utilization), resetsAt: window.resetsAt))
        }
        add(UsageRingWindows.sessionID, usage.fiveHour)
        add(UsageRingWindows.weeklyID, usage.sevenDay)
        var seen: Set<String> = [UsageRingWindows.sessionID, UsageRingWindows.weeklyID]
        for scoped in usage.scoped {
            let id = UsageRingWindows.scopedID(forModel: scoped.name)
            guard seen.insert(id).inserted else { continue }
            add(id, scoped.window)
        }
        if let extra = usage.extraUsage, extra.isEnabled {
            let utilization: Double?
            if let value = extra.utilization {
                utilization = value
            } else if let limit = extra.monthlyLimit, let used = extra.usedCredits, limit > 0 {
                utilization = used / limit * 100
            } else {
                utilization = nil
            }
            if let utilization, utilization.isFinite {
                windows.append(.init(id: UsageRingWindows.extraUsageID, utilization: utilization, resetsAt: nil))
            }
        }
        return windows
    }
}

/// A reading waiting to be sent.
nonisolated struct RecordedUsageReading: Codable, Equatable, Sendable {
    var accountKey: String
    var identityId: String
    var source: CloudUsageSource
    var observedAt: Date
    var windows: [CloudSyncRequest.UsageWindow]

    /// (account, source, observedAt to the millisecond).
    var dedupeKey: String {
        "\(accountKey)|\(source.rawValue)|\(Int64((observedAt.timeIntervalSince1970 * 1000).rounded()))"
    }

    var contract: CloudSyncRequest.UsageReading {
        CloudSyncRequest.UsageReading(accountKey: accountKey, source: source, observedAt: observedAt, windows: windows)
    }
}

nonisolated final class UsageHistoryRecorder: @unchecked Sendable {
    static let fileName = "cloud-usage-outbox.json"
    /// Readings kept waiting at most.
    static let capacity = 5_000
    /// A reading repeating the last values is dropped within this long.
    static let repeatWindow: TimeInterval = 10 * 60

    nonisolated struct Contents: Codable, Equatable, Sendable {
        var version = 1
        var pending: [RecordedUsageReading] = []
        /// Per account and source: the newest reading taken in.
        var last: [String: RecordedUsageReading] = [:]
    }

    private let lock = NSLock()
    private var contents: Contents
    private let file: CloudStateFile<Contents>

    init(fileURL: URL?, persists: Bool) {
        file = CloudStateFile(url: fileURL, persists: persists, label: "cloud-usage")
        contents = file.load() ?? Contents()
    }

    var pendingCount: Int { lock.withLock { contents.pending.count } }

    /// Take a reading in. Returns whether it was kept (new, or merged into
    /// a waiting one with new windows).
    @discardableResult
    func record(_ reading: RecordedUsageReading) -> Bool {
        guard !reading.windows.isEmpty else { return false }
        return lock.withLock {
            let stream = "\(reading.accountKey)|\(reading.source.rawValue)"
            if let last = contents.last[stream] {
                if reading.dedupeKey == last.dedupeKey {
                    return merge(reading)
                }
                if reading.observedAt < last.observedAt { return false }
                if Self.sameValues(reading.windows, last.windows),
                   reading.observedAt.timeIntervalSince(last.observedAt) < Self.repeatWindow {
                    return false
                }
            }
            contents.last[stream] = reading
            contents.pending.append(reading)
            if contents.pending.count > Self.capacity {
                contents.pending.removeFirst(contents.pending.count - Self.capacity)
            }
            file.save(contents)
            return true
        }
    }

    /// Lock held: add the windows a waiting reading of the same moment lacks.
    private func merge(_ reading: RecordedUsageReading) -> Bool {
        guard let index = contents.pending.lastIndex(where: { $0.dedupeKey == reading.dedupeKey }) else { return false }
        let known = Set(contents.pending[index].windows.map(\.id))
        let added = reading.windows.filter { !known.contains($0.id) }
        guard !added.isEmpty else { return false }
        contents.pending[index].windows += added
        file.save(contents)
        return true
    }

    private static func sameValues(_ lhs: [CloudSyncRequest.UsageWindow], _ rhs: [CloudSyncRequest.UsageWindow]) -> Bool {
        Set(lhs) == Set(rhs)
    }

    /// The oldest waiting readings, at most `limit`.
    func pending(limit: Int = .max) -> [RecordedUsageReading] {
        lock.withLock { Array(contents.pending.prefix(limit)) }
    }

    /// The website has these: stop sending them.
    func markSent(_ readings: [RecordedUsageReading]) {
        let keys = Set(readings.map(\.dedupeKey))
        lock.withLock {
            let before = contents.pending.count
            contents.pending.removeAll { keys.contains($0.dedupeKey) }
            if contents.pending.count != before { file.save(contents) }
        }
    }

    /// Drop what waits for accounts that must not be sent (forgotten, switched off).
    func discard(where shouldDrop: (RecordedUsageReading) -> Bool) {
        lock.withLock {
            let before = contents.pending.count
            contents.pending.removeAll(where: shouldDrop)
            if contents.pending.count != before { file.save(contents) }
        }
    }

    /// Forget every waiting reading (sync switched off, signed out): they
    /// were kept only to be sent.
    func clear() {
        lock.withLock {
            guard !contents.pending.isEmpty || !contents.last.isEmpty else { return }
            contents = Contents()
            file.save(contents)
        }
    }

    func saveNow() {
        let snapshot = lock.withLock { contents }
        file.saveNow(snapshot)
    }
}
