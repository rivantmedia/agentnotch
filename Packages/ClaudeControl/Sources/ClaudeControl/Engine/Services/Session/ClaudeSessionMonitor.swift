//
//  ClaudeSessionMonitor.swift
//  ClaudeControl
//
//  MainActor wrapper around SessionStore for UI binding: publishes the
//  sessions, and carries the user's answers (approve, deny, answer, review)
//  back to Claude Code and the store.
//
//  A single shared instance is started once by the hub and owns the whole
//  session pipeline (socket server, registry scanner, periodic checks,
//  review-state heartbeat), so recreating views never restarts or
//  duplicates it.
//
//  Answers name the exact request the user saw (its tool_use_id). A second
//  click on the same button, or a click landing just as the next queued
//  request takes the row, finds that request already answered and does
//  nothing, instead of answering a request the user never saw.
//

import AppKit
import Combine
import Foundation
import os.log

@MainActor
final class ClaudeSessionMonitor: ObservableObject {
    static let shared = ClaudeSessionMonitor()

    nonisolated static var logger: Logger { EngineLog.logger("Monitor") }

    /// All tracked sessions.
    @Published private(set) var instances: [SessionState] = []

    /// Why the hook socket isn't listening; nil when it is (or isn't started).
    @Published private(set) var socketError: String?

    private let store: SessionStore
    private let server: HookSocketServer
    nonisolated private let pipeline: HookEventPipeline
    private var cancellables = Set<AnyCancellable>()
    private var isStarted = false
    private var isPipelineStarted = false
    /// The last start or stop of the store's periodic check: each one waits
    /// for the one before, so a quick stop and start can't run backwards.
    private var periodicCheckChange: Task<Void, Never>?
    private let stateDump = SessionStateDump()
    private let sealedOverride: Bool?

    /// Sealed runs answer fixtures: no hook is waiting on a socket, so an
    /// answer changes the fixture session only.
    private var answersFixtures: Bool {
        sealedOverride ?? DevFlags.isSealed
    }

    private convenience init() {
        self.init(store: .shared, server: .shared)
    }

    /// A monitor over its own store and socket server (tests). `sealed`
    /// overrides the configuration's mode.
    init(store: SessionStore, server: HookSocketServer, sealed: Bool? = nil) {
        self.store = store
        self.server = server
        self.sealedOverride = sealed
        self.pipeline = HookEventPipeline(store: store)
        store.sessionsPublisher
            .receive(on: DispatchQueue.main)
            .sink { [weak self] sessions in
                self?.updateFromSessions(sessions)
            }
            .store(in: &cancellables)
    }

    // MARK: - Monitoring Lifecycle

    /// Starts the socket server, the ordered event pipeline, the session
    /// registry scanner, the periodic liveness check and the review-state
    /// heartbeat. Idempotent.
    func start() {
        guard !isStarted else { return }
        isStarted = true
        InterruptWatcherManager.shared.delegate = self
        startSessionPipeline()

        let pipeline = self.pipeline
        SessionRegistryScanner.shared.start(
            onSnapshot: { configDir, entries in
                pipeline.yield(.registry(configDir: configDir, entries: entries))
            },
            onInitialScan: {
                Self.onMain { AttentionTracker.shared.initialScanCompleted() }
            }
        )

        ReviewStateStore.shared.startHeartbeat()
        let store = self.store
        let previous = periodicCheckChange
        periodicCheckChange = Task {
            await previous?.value
            await store.startPeriodicStatusCheck()
        }

        SessionDevConsole.startIfEnabled()
    }

    /// The ordered pipeline and the socket server feeding it (all a test
    /// monitor needs).
    func startSessionPipeline() {
        guard !isPipelineStarted else { return }
        isPipelineStarted = true
        pipeline.start(effects: HookEventPipeline.Effects(
            afterHook: { [weak self] event in
                guard let monitor = self else { return }
                Self.onMain { monitor.afterHookProcessed(event) }
            },
            statusLine: { update in
                Self.onMain { AppEventBus.shared.statusLineUpdates.send(update) }
            },
            sighting: { sighting in
                // New accounts' session registries are worth scanning too.
                SessionRegistryScanner.shared.addConfigDir(sighting.configDir)
                Self.onMain { AppEventBus.shared.accountSightings.send(sighting) }
            }
        ))

        let pipeline = self.pipeline
        server.start(
            onMessage: { message in
                pipeline.yield(.socket(message))
            },
            onPermissionFailure: { sessionId, toolUseId in
                pipeline.yield(.permissionFailed(sessionId: sessionId, toolUseId: toolUseId))
            },
            onStatus: { [weak self] error in
                guard let monitor = self else { return }
                Self.onMain { monitor.socketError = error }
            }
        )
    }

    /// Hands an input to the ordered pipeline (tests, the dev tools).
    nonisolated func enqueue(_ input: HookEventPipeline.Input) {
        pipeline.yield(input)
    }

    /// Inputs handed to the pipeline and not applied yet.
    nonisolated var pipelineBacklog: Int {
        pipeline.backlog
    }

    func stop() {
        guard isStarted || isPipelineStarted else { return }
        let wasFullyStarted = isStarted
        isStarted = false
        isPipelineStarted = false
        server.stop()
        pipeline.stop()
        guard wasFullyStarted else { return }
        SessionRegistryScanner.shared.stop()
        InterruptWatcherManager.shared.stopAll()
        ReviewStateStore.shared.stopHeartbeat()
        ReviewStateStore.shared.flush()
        let store = self.store
        let previous = periodicCheckChange
        periodicCheckChange = Task {
            await previous?.value
            await store.stopPeriodicStatusCheck()
        }
    }

    /// Runs `body` on the main thread after everything handed over before it
    /// (the main queue is FIFO; separate Tasks are not ordered).
    nonisolated static func onMain(_ body: @escaping @MainActor @Sendable () -> Void) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated(body)
        }
    }

    /// Side effects after the store applied a hook event.
    private func afterHookProcessed(_ event: HookEvent) {
        InterruptWatcherManager.shared.apply(event)
    }

    // MARK: - Permission Handling

    /// Approves the request `toolUseId` of the session. With `alwaysAllow`,
    /// also applies Claude Code's first permission suggestion (the terminal's
    /// "Yes, and don't ask again"). Does nothing when that request is no
    /// longer pending. AskUserQuestion needs `answerQuestion`.
    func approvePermission(sessionId: String, toolUseId: String, alwaysAllow: Bool = false) {
        Task {
            guard let permission = await pendingPermission(sessionId: sessionId, toolUseId: toolUseId) else { return }
            guard permission.toolName != "AskUserQuestion" else {
                Self.logger.warning("AskUserQuestion must be answered with answerQuestion, not approved")
                return
            }
            let updatedPermissions: [AnyCodable]? = alwaysAllow
                ? permission.permissionSuggestions?.first.map { [$0] }
                : nil
            // Tools that require user interaction ignore a plain allow; an empty
            // update makes the hook echo the original input back.
            let updatedInput: [String: AnyCodable]? = Self.toolsNeedingInputEcho.contains(permission.toolName) ? [:] : nil
            await respond(
                sessionId: sessionId,
                permission: permission,
                decision: .allow,
                updatedInput: updatedInput,
                updatedPermissions: updatedPermissions
            )
        }
    }

    /// Denies the request `toolUseId`; nothing when it is no longer pending.
    func denyPermission(sessionId: String, toolUseId: String, reason: String?) {
        Task {
            guard let permission = await pendingPermission(sessionId: sessionId, toolUseId: toolUseId) else { return }
            await respond(sessionId: sessionId, permission: permission, decision: .deny, reason: reason)
        }
    }

    /// Answers the pending AskUserQuestion `toolUseId`: `answers` maps each
    /// question's text to the chosen option label (multi-select labels
    /// joined by ", "). Nothing when it is no longer pending.
    func answerQuestion(sessionId: String, toolUseId: String, answers: [String: String]) {
        Task {
            guard let permission = await pendingPermission(sessionId: sessionId, toolUseId: toolUseId),
                  permission.toolName == "AskUserQuestion" else { return }
            let answerValues: [String: Any] = answers.mapValues { $0 }
            await respond(
                sessionId: sessionId,
                permission: permission,
                decision: .allow,
                updatedInput: ["answers": AnyCodable(answerValues)]
            )
        }
    }

    /// Approves the request the UI shows for the session right now (the
    /// published state the click came from). Prefer the `toolUseId` form.
    func approvePermission(sessionId: String, alwaysAllow: Bool = false) {
        guard let toolUseId = displayedPermissionId(sessionId: sessionId) else { return }
        approvePermission(sessionId: sessionId, toolUseId: toolUseId, alwaysAllow: alwaysAllow)
    }

    /// Denies the request the UI shows for the session right now.
    func denyPermission(sessionId: String, reason: String?) {
        guard let toolUseId = displayedPermissionId(sessionId: sessionId) else { return }
        denyPermission(sessionId: sessionId, toolUseId: toolUseId, reason: reason)
    }

    /// Answers the question the UI shows for the session right now.
    func answerQuestion(sessionId: String, answers: [String: String]) {
        guard let toolUseId = displayedPermissionId(sessionId: sessionId) else { return }
        answerQuestion(sessionId: sessionId, toolUseId: toolUseId, answers: answers)
    }

    /// The request shown as active in the last published state.
    private func displayedPermissionId(sessionId: String) -> String? {
        instances.first { $0.sessionId == sessionId }?.activePermission?.toolUseId
    }

    /// Tools with `requiresUserInteraction` besides AskUserQuestion.
    private static let toolsNeedingInputEcho: Set<String> = ["ExitPlanMode"]

    private func pendingPermission(sessionId: String, toolUseId: String) async -> PermissionContext? {
        let permission = await store.session(for: sessionId)?.pendingPermission(toolUseId: toolUseId)
        if permission == nil {
            Self.logger.info("Ignoring an answer for \(toolUseId.prefix(12), privacy: .public): no longer pending")
        }
        return permission
    }

    private func respond(
        sessionId: String,
        permission: PermissionContext,
        decision: PermissionDecision,
        reason: String? = nil,
        updatedInput: [String: AnyCodable]? = nil,
        updatedPermissions: [AnyCodable]? = nil
    ) async {
        guard !answersFixtures else {
            let outcome: SessionEvent = decision == .deny
                ? .permissionDenied(sessionId: sessionId, toolUseId: permission.toolUseId, reason: reason)
                : .permissionApproved(sessionId: sessionId, toolUseId: permission.toolUseId)
            await store.process(outcome)
            return
        }
        let delivered = await server.respondToPermission(
            toolUseId: permission.toolUseId,
            decision: decision,
            reason: reason,
            updatedInput: updatedInput,
            updatedPermissions: updatedPermissions
        )
        let event: SessionEvent
        if !delivered {
            // The hook is gone: don't pretend the decision was applied.
            event = .permissionSocketFailed(sessionId: sessionId, toolUseId: permission.toolUseId)
        } else if decision == .deny {
            event = .permissionDenied(sessionId: sessionId, toolUseId: permission.toolUseId, reason: reason)
        } else {
            event = .permissionApproved(sessionId: sessionId, toolUseId: permission.toolUseId)
        }
        // In line with hook events, so a PostToolUse already queued for this
        // tool can't be overtaken.
        pipeline.yield(.session(event))
    }

    // MARK: - Review

    /// The user looked at the session (chat opened, focus button, "reviewed").
    func markReviewed(sessionId: String) {
        // The moment of the click, not of processing: a turn that completes
        // in between is still unreviewed.
        markReviewed(sessionId: sessionId, at: Date())
    }

    /// Reviewed as of `date` (the click), however much later this runs: a
    /// deferred commit (mark all reviewed's undo window) keeps a turn that
    /// completed meanwhile unreviewed.
    func markReviewed(sessionId: String, at date: Date) {
        let store = self.store
        Task {
            await store.process(.markReviewed(sessionId: sessionId, at: date))
        }
    }

    /// The Hooks switch went off (see `SessionEvent.hooksTurnedOff`).
    func hooksTurnedOff() {
        let store = self.store
        Task {
            await store.process(.hooksTurnedOff)
        }
    }

    /// A forgotten account's sessions go (see `SessionEvent.dropAccountSessions`).
    func dropSessions(ofAccount accountId: String) {
        let store = self.store
        Task {
            await store.process(.dropAccountSessions(accountId: accountId))
        }
    }

    /// The user dismissed a failed turn (see `SessionEvent.dismissFailure`).
    func dismissFailure(sessionId: String, at date: Date = Date()) {
        let store = self.store
        Task {
            await store.process(.dismissFailure(sessionId: sessionId, at: date))
        }
    }

    /// The user watched the completion at `completedAt` happen (its own
    /// terminal tab was frontmost): reviews that completion, never a later one.
    func markViewed(sessionId: String, completedAt: Date) {
        let store = self.store
        Task {
            await store.process(.markViewed(sessionId: sessionId, completedAt: completedAt))
        }
    }

    func markAllReviewed() {
        let store = self.store
        let now = Date()
        Task {
            await store.process(.markAllReviewed(at: now))
        }
    }

    // MARK: - State Update

    private func updateFromSessions(_ sessions: [SessionState]) {
        if instances != sessions {
            instances = sessions
        }

        // Sessions that disappeared (ended, process gone) stop being watched.
        let liveIds = Set(sessions.map(\.sessionId))
        InterruptWatcherManager.shared.stopWatching(except: liveIds)

        stateDump.dumpIfEnabled(sessions)
    }

    // MARK: - History (for the chat)

    /// A chat opened: read and keep the session's whole history.
    func loadHistory(sessionId: String, cwd: String) async {
        await store.process(.loadHistory(sessionId: sessionId, cwd: cwd))
    }

    /// A chat closed: the session keeps only its newest items again.
    func releaseHistory(sessionId: String) {
        let store = self.store
        Task {
            await store.process(.releaseHistory(sessionId: sessionId))
        }
    }

    /// The session as the store has it now (ahead of the next publish).
    func currentSession(_ sessionId: String) async -> SessionState? {
        await store.session(for: sessionId)
    }
}

// MARK: - Interrupt Watcher Delegate

extension ClaudeSessionMonitor: JSONLInterruptWatcherDelegate {
    nonisolated func didDetectInterrupt(sessionId: String, at date: Date) {
        // In line with hook events: a later prompt's events apply after it,
        // and the store ignores it if that prompt came first.
        pipeline.yield(.session(.interruptDetected(sessionId: sessionId, at: date)))
        Self.onMain {
            InterruptWatcherManager.shared.stopWatching(sessionId: sessionId)
        }
    }
}
