//
//  JSONLInterruptWatcher.swift
//  ClaudeIsland
//
//  Watches JSONL files for interrupt patterns in real-time
//  Uses file system events to detect interrupts faster than hook polling
//

import Foundation
import os.log

/// Logger for interrupt watcher
nonisolated private var logger: Logger { EngineLog.logger("Interrupt") }

protocol JSONLInterruptWatcherDelegate: AnyObject, Sendable {
    /// Called on the watcher's queue; `at` is when the interrupt line was read.
    nonisolated func didDetectInterrupt(sessionId: String, at: Date)
}

/// Watches a session's JSONL file for interrupt patterns in real-time
/// Uses DispatchSource for immediate detection when new lines are written.
/// All mutable state lives on `queue`.
nonisolated final class JSONLInterruptWatcher: @unchecked Sendable {
    private var fileHandle: FileHandle?
    private var source: DispatchSourceFileSystemObject?
    private var lastOffset: UInt64 = 0
    private var isStopped = false
    private var openAttempts = 0
    private let sessionId: String
    let filePath: String
    /// Utility: scanning appended lines must not compete with the UI.
    private let queue = DispatchQueue(label: EngineLog.queueLabel("interruptwatcher"), qos: .utility)

    weak var delegate: JSONLInterruptWatcherDelegate?

    /// Patterns that indicate an interrupt occurred
    /// We check for is_error:true combined with interrupt content
    static let interruptContentPatterns = [
        "Interrupted by user",
        "interrupted by user",
        "user doesn't want to proceed",
        "[Request interrupted by user"
    ]

    /// `filePath` is the session's transcript (the hook's `transcript_path`).
    init(sessionId: String, filePath: String) {
        self.sessionId = sessionId
        self.filePath = filePath
    }

    /// Start watching the JSONL file for interrupts
    func start() {
        queue.async { [weak self] in
            self?.startWatching()
        }
    }

    /// A new session's transcript appears with its first message, which is
    /// written after the UserPromptSubmit hook that starts the watcher.
    private static let maxOpenAttempts = 30

    private func startWatching() {
        stopInternal()
        guard !isStopped else { return }

        guard FileManager.default.fileExists(atPath: filePath),
              let handle = FileHandle(forReadingAtPath: filePath) else {
            openAttempts += 1
            guard openAttempts < Self.maxOpenAttempts else {
                logger.warning("Failed to open file: \(self.filePath, privacy: .public)")
                return
            }
            queue.asyncAfter(deadline: .now() + 1) { [weak self] in
                guard let self, !self.isStopped, self.source == nil else { return }
                self.startWatching()
            }
            return
        }

        fileHandle = handle

        do {
            lastOffset = try handle.seekToEnd()
        } catch {
            logger.error("Failed to seek to end: \(error.localizedDescription, privacy: .public)")
            return
        }

        let fd = handle.fileDescriptor
        let newSource = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd,
            eventMask: [.write, .extend],
            queue: queue
        )

        newSource.setEventHandler { [weak self] in
            self?.checkForInterrupt()
        }

        newSource.setCancelHandler { [weak self] in
            try? self?.fileHandle?.close()
            self?.fileHandle = nil
        }

        source = newSource
        newSource.resume()

        logger.debug("Started watching: \(self.sessionId.prefix(8), privacy: .public)...")
    }

    private func checkForInterrupt() {
        guard let handle = fileHandle else { return }

        let currentSize: UInt64
        do {
            currentSize = try handle.seekToEnd()
        } catch {
            return
        }

        guard currentSize > lastOffset else { return }

        do {
            try handle.seek(toOffset: lastOffset)
        } catch {
            return
        }

        guard let newData = try? handle.readToEnd(),
              let lastNewline = newData.lastIndex(of: UInt8(ascii: "\n")) else {
            return
        }
        // Only complete lines; a half-written one is read again next time.
        let complete = newData[newData.startIndex...lastNewline]
        lastOffset += UInt64(complete.count)

        var detected = false
        TranscriptLineReader.forEachLine(in: complete) { line in
            if !detected && Self.isInterruptLine(line) {
                detected = true
            }
        }
        guard detected else { return }
        logger.info("Detected interrupt in session: \(self.sessionId.prefix(8), privacy: .public)")
        delegate?.didDetectInterrupt(sessionId: sessionId, at: Date())
    }

    private static let userMarker = Data("\"type\":\"user\"".utf8)
    private static let toolResultMarker = Data("\"tool_result\"".utf8)
    private static let errorMarker = Data("\"is_error\":true".utf8)
    private static let interruptedMarker = Data("\"interrupted\":true".utf8)
    private static let requestInterruptedMarker = Data("[Request interrupted by user".utf8)
    private static let contentMarkers = interruptContentPatterns.map { Data($0.utf8) }

    /// Byte-level check of one transcript line (no decoding).
    static func isInterruptLine(_ line: Data) -> Bool {
        if line.range(of: userMarker) != nil, line.range(of: requestInterruptedMarker) != nil {
            return true
        }
        guard line.range(of: toolResultMarker) != nil else { return false }
        if line.range(of: errorMarker) != nil, contentMarkers.contains(where: { line.range(of: $0) != nil }) {
            return true
        }
        // Bash results carry `"interrupted":true` in toolUseResult; only trust it
        // on tool result lines, not anywhere in arbitrary content.
        return line.range(of: interruptedMarker) != nil
    }

    /// Stop watching
    func stop() {
        queue.async { [weak self] in
            self?.isStopped = true
            self?.stopInternal()
        }
    }

    private func stopInternal() {
        if source != nil {
            logger.debug("Stopped watching: \(self.sessionId.prefix(8), privacy: .public)...")
        }
        source?.cancel()
        source = nil
        // fileHandle closed by cancel handler
    }

    deinit {
        source?.cancel()
    }
}

// MARK: - Interrupt Watcher Manager

/// Manages interrupt watchers for all active sessions
@MainActor
class InterruptWatcherManager {
    static let shared = InterruptWatcherManager()

    private var watchers: [String: JSONLInterruptWatcher] = [:]
    weak var delegate: JSONLInterruptWatcherDelegate?

    private init() {}

    /// Watches while a main-session turn runs; stops at its end (Stop,
    /// StopFailure) or the session's (SessionEnd). The next turn's first
    /// event starts it again.
    func apply(_ event: HookEvent) {
        if event.event == "SessionEnd" || event.endsMainTurn {
            stopWatching(sessionId: event.sessionId)
            return
        }
        guard !event.isSubagentEvent,
              event.determinePhase() == .processing,
              let transcriptPath = event.transcriptPath, !transcriptPath.isEmpty else { return }
        startWatching(sessionId: event.sessionId, transcriptPath: transcriptPath)
    }

    func startWatching(sessionId: String, transcriptPath: String) {
        if let existing = watchers[sessionId] {
            // The same file named through another config folder's link
            // (a shared history) is the same file: keep watching it.
            guard !TranscriptLocator.isSameFile(existing.filePath, transcriptPath) else { return }
            existing.stop()
        }

        let watcher = JSONLInterruptWatcher(sessionId: sessionId, filePath: transcriptPath)
        watcher.delegate = delegate
        watcher.start()
        watchers[sessionId] = watcher
    }

    /// Stop watching every session not in `sessionIds` (ended or removed).
    func stopWatching(except sessionIds: Set<String>) {
        for sessionId in watchers.keys where !sessionIds.contains(sessionId) {
            stopWatching(sessionId: sessionId)
        }
    }

    /// Stop watching a specific session
    func stopWatching(sessionId: String) {
        watchers[sessionId]?.stop()
        watchers.removeValue(forKey: sessionId)
    }

    /// Stop all watchers
    func stopAll() {
        for (_, watcher) in watchers {
            watcher.stop()
        }
        watchers.removeAll()
    }

    /// Check if we're watching a session
    func isWatching(sessionId: String) -> Bool {
        watchers[sessionId] != nil
    }
}
