//
//  HookEventPipeline.swift
//  ClaudeControl
//
//  The single, ordered path from the hook socket, the session registry, the
//  interrupt watcher and permission outcomes into SessionStore. Swift does
//  not order separate Tasks reaching an actor (a PostToolUse processed after
//  Stop left sessions stuck in "processing"), so every input goes through
//  one AsyncStream with one consumer that awaits each event before the next.
//
//  The consumer never waits for the main thread. Main-thread side effects
//  (interrupt watchers, the status line and account-sighting buses) are
//  handed to the main queue in order (`DispatchQueue.main.async` is FIFO), so
//  a busy main thread — an open panel rendering a burst — can't slow the
//  store down, and a SessionEnd's side effect still can't overtake the
//  side effect of an earlier event.
//

import Combine
import Foundation
import os.log

nonisolated private var logger: Logger { EngineLog.logger("Pipeline") }

nonisolated final class HookEventPipeline: @unchecked Sendable {
    /// Anything that changes session state from outside the UI.
    enum Input: Sendable {
        case socket(HookSocketMessage)
        case registry(configDir: String, entries: [SessionRegistryEntry])
        /// A pending permission's hook went away.
        case permissionFailed(sessionId: String, toolUseId: String)
        /// Something that must keep its place among hook events: a
        /// permission's outcome after the answer was written, an interrupt.
        case session(SessionEvent)
    }

    /// Side effects of processed inputs. Called on the consumer, in order,
    /// and must not block: the live ones hop to the main queue.
    struct Effects: Sendable {
        /// After a hook event was applied to the store.
        var afterHook: @Sendable (HookEvent) -> Void
        /// A status line update (for the usage store).
        var statusLine: @Sendable (StatusLineUpdate) -> Void
        /// An account seen in a hook or status line (at most once a minute per session).
        var sighting: @Sendable (AccountSighting) -> Void

        static let none = Effects(afterHook: { _ in }, statusLine: { _ in }, sighting: { _ in })
    }

    /// A backlog this deep is logged (at most once a minute).
    static let backlogWarningThreshold = 256

    private let store: SessionStore
    /// Guards everything below.
    private let lock = NSLock()
    /// The current stream. A consumer that is cancelled ends its stream for
    /// good (AsyncStream finishes on cancellation), so `stop` starts a fresh
    /// one for the next `start`.
    private var stream: AsyncStream<Input>
    private var continuation: AsyncStream<Input>.Continuation
    /// Bumped by `stop`; a consumer of an older stream no longer counts.
    private var generation = 0
    private var consumer: Task<Void, Never>?
    private var yielded = 0
    private var processed = 0
    private var lastBacklogWarning = Date.distantPast

    init(store: SessionStore = .shared) {
        self.store = store
        (stream, continuation) = AsyncStream.makeStream(of: Input.self, bufferingPolicy: .unbounded)
    }

    /// Thread-safe; inputs are processed in the order they are yielded.
    /// Inputs yielded while stopped wait for the next `start`.
    func yield(_ input: Input) {
        lock.lock()
        yielded += 1
        let backlog = yielded - processed
        let warn = backlog >= Self.backlogWarningThreshold && Date().timeIntervalSince(lastBacklogWarning) > 60
        if warn { lastBacklogWarning = Date() }
        // Under the lock: the stream's order is the counters' order, and a
        // concurrent `stop` can't swap the stream in between.
        continuation.yield(input)
        lock.unlock()
        if warn {
            logger.warning("Session event backlog: \(backlog) events waiting")
        }
    }

    /// Inputs yielded but not yet applied.
    var backlog: Int {
        lock.lock()
        defer { lock.unlock() }
        return yielded - processed
    }

    /// Starts the single consumer (idempotent; again after `stop`).
    func start(effects: Effects) {
        lock.lock()
        defer { lock.unlock() }
        guard consumer == nil else { return }
        let stream = self.stream
        let store = self.store
        let generation = self.generation
        consumer = Task.detached(priority: .userInitiated) { [weak self] in
            var sightings = SightingThrottle()
            for await input in stream {
                await Self.handle(input, store: store, sightings: &sightings, effects: effects)
                self?.markProcessed(generation: generation)
            }
        }
    }

    /// Stops the consumer. What it hadn't applied yet is dropped; a later
    /// `start` reads a new stream (the cancelled one is over for good).
    func stop() {
        lock.lock()
        defer { lock.unlock() }
        guard let consumer else { return }
        consumer.cancel()
        self.consumer = nil
        continuation.finish()
        (stream, continuation) = AsyncStream.makeStream(of: Input.self, bufferingPolicy: .unbounded)
        generation += 1
        yielded = 0
        processed = 0
    }

    private func markProcessed(generation: Int) {
        lock.lock()
        if generation == self.generation {
            processed += 1
        }
        lock.unlock()
    }

    private static func handle(
        _ input: Input,
        store: SessionStore,
        sightings: inout SightingThrottle,
        effects: Effects
    ) async {
        switch input {
        case .socket(.hook(let event)):
            if let sighting = sightings.sighting(
                sessionId: event.sessionId,
                transcriptPath: event.transcriptPath,
                configDirEnv: event.configDirEnv
            ) {
                publish(sighting, effects: effects)
            }
            await store.process(.hookReceived(event))
            effects.afterHook(event)

        case .socket(.statusLine(let message)):
            if let sighting = sightings.sighting(
                sessionId: message.sessionId,
                transcriptPath: message.transcriptPath,
                configDirEnv: message.configDirEnv
            ) {
                publish(sighting, effects: effects)
            }
            effects.statusLine(message.update)
            await store.process(.statusLineReceived(message))

        case .registry(let configDir, let entries):
            await store.process(.registrySnapshot(configDir: configDir, entries: entries))

        case .permissionFailed(let sessionId, let toolUseId):
            await store.process(.permissionSocketFailed(sessionId: sessionId, toolUseId: toolUseId))

        case .session(let event):
            await store.process(event)
        }
    }

    private static func publish(_ sighting: AccountSighting, effects: Effects) {
        effects.sighting(sighting)
        logger.debug("Account sighting \(AccountPaths.shortName(forConfigDir: sighting.configDir), privacy: .public) from \(sighting.sessionId.prefix(8), privacy: .public)")
    }
}

/// Emits at most one AccountSighting per session per interval (and at once
/// when a session's account changes).
nonisolated struct SightingThrottle: Sendable {
    static let interval: TimeInterval = 60

    private var lastSent: [String: (configDir: String, at: Date)] = [:]

    init() {}

    mutating func sighting(
        sessionId: String,
        transcriptPath: String?,
        configDirEnv: String?,
        now: Date = Date()
    ) -> AccountSighting? {
        let hasTranscript = !(transcriptPath ?? "").isEmpty
        let hasEnv = !(configDirEnv ?? "").isEmpty
        guard hasTranscript || hasEnv else { return nil }

        let configDir = SessionFilter.configDir(transcriptPath: transcriptPath, configDirEnv: configDirEnv)
        if let last = lastSent[sessionId], last.configDir == configDir, now.timeIntervalSince(last.at) < Self.interval {
            return nil
        }
        lastSent[sessionId] = (configDir, now)
        if lastSent.count > 512 {
            lastSent = lastSent.filter { now.timeIntervalSince($0.value.at) < Self.interval }
        }
        return AccountSighting(
            configDir: configDir,
            configDirEnv: hasEnv ? configDirEnv : nil,
            sessionId: sessionId,
            at: now
        )
    }
}
