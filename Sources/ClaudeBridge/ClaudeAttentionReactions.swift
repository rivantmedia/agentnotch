import AppKit
import ClaudeControl
import Combine
import Foundation

/// What the notch does when a Claude session starts needing you or finishes
/// (design §6), carried out with Codenotch's own sounds, peek and panel:
///
/// - **Chime and peek.** `ClaudeAttentionPolicy` decides per transition —
///   nothing at all while you are looking at that session's terminal; the
///   blocked or finished sound (Codenotch's "Play a sound" and its two sound
///   choices); the panel opened by itself where `autoOpen` allows, otherwise a
///   peek (Codenotch's "Open the notch when a session ends" and its duration).
///   Transitions arriving together are one burst: one chime, one peek.
///   Upstream's own announcer never sees a Claude session (U5).
/// - **Dock badge.** The needs-you count on the Dock icon, when that is on:
///   every session, like the panel.
/// - **Hold open.** While a session the notch shows needs you,
///   `ClaudeNotchHold` keeps the notches open that cannot show it folded (the
///   setting decides which). A session on a ring switched off in the notch
///   holds nothing open: there would be no amber ring to see.
///
/// Fork-only file. Owned by WP-C.
@MainActor
final class ClaudeAttentionReactions {
    private weak var hub: ClaudeControlHub?
    private weak var fleet: NotchFleet?
    private weak var preferences: Preferences?
    private let state: ClaudeNotchState
    private let settings: ClaudeNotchSettings
    /// Sealed runs log the chime instead of playing it.
    private let playsSounds: Bool
    private let log: (String) -> Void
    private var cancellables = Set<AnyCancellable>()
    private var burst = ClaudeAttentionPolicy.Burst()
    private var closeWork: DispatchWorkItem?
    private var dockLabel: String?
    private var holding: (on: Bool, policy: HoldOpenPolicy)?

    /// The last peek offered, so a click on it can be told from a click on a
    /// hover-card row (see `ClaudeBridge.focusSession`).
    private(set) var lastPeek: (pid: pid_t, until: Date)?

    init(hub: ClaudeControlHub, fleet: NotchFleet, preferences: Preferences,
         state: ClaudeNotchState, settings: ClaudeNotchSettings, playsSounds: Bool,
         log: @escaping (String) -> Void = { _ in }) {
        self.hub = hub
        self.fleet = fleet
        self.preferences = preferences
        self.state = state
        self.settings = settings
        self.playsSounds = playsSounds
        self.log = log
    }

    func start() {
        guard let hub else { return }
        hub.transitions
            .receive(on: DispatchQueue.main)
            .sink { [weak self] in self?.handle($0) }
            .store(in: &cancellables)

        // Counts and the settings that act on them, a turn after either
        // changes (`@Published` fires before the value lands). The Dock
        // counts every session; the hold only those the notch shows.
        Publishers.CombineLatest(hub.$totalCounts, settings.$dockBadge)
            .receive(on: DispatchQueue.main)
            .sink { [weak self] total, dockBadge in
                self?.updateDockBadge(needsYou: total.needsYou, enabled: dockBadge)
            }
            .store(in: &cancellables)
        Publishers.CombineLatest(state.$totalCounts, settings.$holdOpen)
            .receive(on: DispatchQueue.main)
            .sink { [weak self] notch, holdOpen in
                self?.updateHold(needsYou: notch.needsYou, policy: holdOpen)
            }
            .store(in: &cancellables)
    }

    func stop() {
        cancellables.removeAll()
        closeWork?.cancel()
        closeWork = nil
        if dockLabel != nil { NSApp.dockTile.badgeLabel = nil }
    }

    // MARK: - Transitions

    private func handle(_ transition: ClaudeAttentionTransition) {
        // Resolutions make no sound; the panel closing after an answered
        // prompt is the panel's own business.
        guard transition.kind != .resolved else { return }
        let session = transition.session
        Log.sessions.info("claude \(String(describing: transition.kind), privacy: .public): \(session.id, privacy: .public) on \(session.ringID, privacy: .public)")
        log("transition \(transition.kind) \(session.id) on \(session.ringID)")
        if let due = burst.begin(at: Date()) {
            scheduleClose(at: due)
        }
        Task { @MainActor [weak self] in
            guard let self else { return }
            let context = await self.context(for: session)
            self.log("context \(session.id): focused=\(context.terminalFocused) terminal-visible=\(context.anyTerminalVisible) full-screen=\(context.fullScreen) panel-open=\(context.panelOpen) ring-shown=\(context.ringShown)")
            self.burst.finish(ClaudeAttentionPolicy.decide(transition, context: context))
            self.closeBurstIfDue()
        }
    }

    /// Everything the policy weighs besides the transition. The focus check
    /// may ask the terminal app over Apple events (in osascript, off the main
    /// thread); the visibility check reads the window list
    /// (`CGWindowListCopyWindowInfo`) on the main actor, a few milliseconds
    /// per attention burst (CS-10). Both are awaited here.
    private func context(for session: ClaudeSessionSummary) async -> ClaudeAttentionPolicy.Context {
        let focused = await hub?.isTerminalFocused(sessionId: session.id) ?? false
        let visible = await hub?.isAnyTerminalVisible() ?? false
        return ClaudeAttentionPolicy.Context(
            autoOpen: settings.autoOpen,
            chimes: preferences?.sessionEndSound ?? false,
            peeks: preferences?.announceSessionEnd ?? false,
            terminalFocused: focused,
            anyTerminalVisible: visible,
            fullScreen: FullScreenDetector.isFullScreenAppFrontmost(on: NSScreen.main),
            panelOpen: ClaudePanelController.shared.isOpen,
            // Shown on its own ring, or on the default one for an account
            // not known yet: the feed's routing, not the connected set alone.
            ringShown: state.isShown(session.ringID)
        )
    }

    private func scheduleClose(at due: Date) {
        closeWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated { self?.closeBurstIfDue() }
        }
        closeWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + max(0, due.timeIntervalSinceNow), execute: work)
    }

    private func closeBurstIfDue() {
        guard let decisions = burst.close(at: Date()) else { return }
        closeWork?.cancel()
        closeWork = nil
        let summary = decisions.isEmpty ? "nothing" : decisions.map(Self.describe).joined(separator: ", ")
        log("burst: \(summary)")
        guard !decisions.isEmpty else { return }
        Log.sessions.notice("claude attention burst: \(summary, privacy: .public)")
        for decision in decisions { perform(decision) }
    }

    private func perform(_ decision: ClaudeAttentionPolicy.Decision) {
        guard let preferences else { return }
        switch decision {
        case .chime(let sound):
            let name = sound == .needsInput ? preferences.sessionBlockedSoundName : preferences.sessionEndSoundName
            if playsSounds {
                SessionChime.play(name)
            } else {
                log("chime \(name): not played in a sealed run")
            }
        case .autoOpen(let sessionID, _):
            ClaudePanelController.shared.open(.session(id: sessionID), reason: .auto)
        case .peek(let pid, _):
            let duration = preferences.peekDuration.seconds
            if let pid { lastPeek = (pid_t(pid), Date().addingTimeInterval(duration + Self.peekClickGrace)) }
            fleet?.peek(for: duration, focusing: pid.map { pid_t($0) })
        }
    }

    /// How long after a peek ends a click still counts as answering it — the
    /// grace Codenotch's own peek gives (`NotchWindowController.focusGrace`).
    static let peekClickGrace: TimeInterval = 2

    /// Whether a click on `pid` now answers the last peek.
    func answersPeek(pid: pid_t, now: Date = Date()) -> Bool {
        guard let lastPeek, lastPeek.pid == pid, now < lastPeek.until else { return false }
        return true
    }

    private static func describe(_ decision: ClaudeAttentionPolicy.Decision) -> String {
        switch decision {
        case .chime(let sound): return "chime \(sound == .needsInput ? "needs-input" : "finished")"
        case .autoOpen(let id, _): return "auto-open \(id)"
        case .peek(let pid, _): return "peek pid \(pid.map(String.init) ?? "none")"
        }
    }

    // MARK: - Dock badge and hold

    private func updateDockBadge(needsYou: Int, enabled: Bool) {
        let label = ClaudeAttentionPolicy.dockBadgeLabel(needsYou: needsYou, enabled: enabled)
        guard label != dockLabel else { return }
        dockLabel = label
        NSApp.dockTile.badgeLabel = label
        log("dock badge: \(label ?? "none")")
    }

    private func updateHold(needsYou: Int, policy: HoldOpenPolicy) {
        let on = needsYou > 0
        if let holding, holding.on == on, holding.policy == policy { return }
        holding = (on, policy)
        ClaudeNotchHold.shared.setNeedsYou(on, policy: Self.holdPolicy(policy))
        log("hold open while needed: \(on) (\(policy.rawValue))")
    }

    static func holdPolicy(_ policy: HoldOpenPolicy) -> ClaudeHoldPolicy {
        switch policy {
        case .auto: return .auto
        case .always: return .always
        case .never: return .never
        }
    }
}
