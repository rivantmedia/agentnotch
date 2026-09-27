//
//  UsageStateStore.swift
//  ClaudeControl
//
//  What the usage store must remember across launches, in
//  `<support>/usage-state.json`: per account, when Claude Code was last
//  asked (so a relaunch doesn't probe early), the failure backoff (so a
//  rate-limited account stays paused), the last full reading (so the
//  rings don't start empty), and what each running Claude Code process last
//  said in its status line (so a relaunch doesn't take its old numbers for
//  news). Nothing in it is a secret; the file is still written 0600 like
//  everything else in the engine's folder.
//

import Foundation
import os.log

/// The persisted part of the usage store's state. Pure data.
nonisolated struct UsageState: Codable, Equatable, Sendable {
    nonisolated struct Account: Codable, Equatable, Sendable {
        var lastProbeAt: Date?
        /// Consecutive failed or rate-limited probes.
        var failureCount: Int = 0
        /// No scheduled probe before this.
        var nextAttemptAt: Date?
        /// The newest full reading (probe, `.claude.json` or Claude Desktop).
        var lastFullReading: AccountUsage?
        /// The status line records of the account's Claude Code processes.
        var statusLines: [StatusLine]?

        var isEmpty: Bool {
            lastProbeAt == nil && failureCount == 0 && nextAttemptAt == nil && lastFullReading == nil
                && (statusLines ?? []).isEmpty
        }
    }

    /// One Claude Code process's status line record, under its key (pid and
    /// start time), for the next run (see `UsageStore.restoreStatusLines`).
    nonisolated struct StatusLine: Codable, Equatable, Sendable {
        var folder: String
        var key: String
        var readings: UsageStore.StatusLineReadings
    }

    static let currentVersion = 1

    var version: Int = UsageState.currentVersion
    var accounts: [String: Account] = [:]

    /// Readings older than this aren't worth restoring: every window they
    /// describe has reset since.
    static let maxRestoredReadingAge: TimeInterval = 8 * 24 * 60 * 60

    /// The state as a new run should start from it: accounts that are gone
    /// dropped (when `knownAccountIds` is given), and readings too old to
    /// describe any current window dropped. Pure.
    func restored(knownAccountIds: Set<String>?, now: Date) -> UsageState {
        var copy = self
        if let knownAccountIds {
            copy.accounts = accounts.filter { knownAccountIds.contains($0.key) }
        }
        for (id, account) in copy.accounts {
            if let reading = account.lastFullReading,
               now.timeIntervalSince(reading.updatedAt) > Self.maxRestoredReadingAge {
                copy.accounts[id]?.lastFullReading = nil
            }
            if copy.accounts[id]?.isEmpty == true {
                copy.accounts.removeValue(forKey: id)
            }
        }
        return copy
    }
}

/// Reads and writes `usage-state.json`. Writes are atomic and happen off the
/// caller's actor, newest state last.
nonisolated final class UsageStateStore: @unchecked Sendable {
    static let fileName = "usage-state.json"

    private static var logger: Logger { EngineLog.logger("UsageState") }

    let fileURL: URL
    private let queue: DispatchQueue

    init(directory: URL) {
        fileURL = directory.appendingPathComponent(Self.fileName)
        queue = DispatchQueue(label: EngineLog.queueLabel("usage-state"), qos: .utility)
    }

    /// The saved state; empty when there is none or it can't be read (a
    /// file from a newer version, or damaged: it is only a cache).
    func load() -> UsageState {
        guard let data = try? Data(contentsOf: fileURL) else { return UsageState() }
        do {
            let state = try Self.decoder.decode(UsageState.self, from: data)
            guard state.version <= UsageState.currentVersion else {
                Self.logger.notice("usage-state.json is from a newer version; starting fresh")
                return UsageState()
            }
            return state
        } catch {
            Self.logger.error("usage-state.json can't be read; starting fresh: \(error.localizedDescription, privacy: .public)")
            return UsageState()
        }
    }

    /// Write `state` in the background. Saves are ordered, so the last call wins.
    func save(_ state: UsageState) {
        let url = fileURL
        queue.async {
            Self.write(state, to: url)
        }
    }

    /// Write now, on the calling thread (quitting, tests).
    func saveNow(_ state: UsageState) {
        queue.sync {
            Self.write(state, to: fileURL)
        }
    }

    private static func write(_ state: UsageState, to url: URL) {
        do {
            let data = try encoder.encode(state)
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700]
            )
            try data.write(to: url, options: [.atomic])
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        } catch {
            logger.error("Saving usage-state.json failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.sortedKeys, .prettyPrinted]
        return encoder
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}
