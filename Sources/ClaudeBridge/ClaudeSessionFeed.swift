import ClaudeControl
import Combine
import Foundation

/// Claude sessions on the rings and in the hover card (design §5): the hub's
/// sessions, at most every 400 ms, handed to `fleet.setSessions` for each
/// Claude ring whose rows changed. Codenotch's `ActivitySummary` then gives
/// each ring one arc (waiting over working over done), the hover card its
/// session rows, and the menu bar and the phone link the same, read-only.
///
/// - A ring switched off in the notch gets no rows.
/// - A session whose account the hub does not know yet shows on the default
///   ring, `claude`, rather than nowhere.
/// - A ring that goes away is emptied, so its sessions do not linger in the
///   menu bar or on the phone.
///
/// `ActivityCoordinator` is not involved: it stays upstream's, for the other
/// agents. Claude's rows never reach `announceCompletions` either (U5); the
/// chime and peek are `ClaudeAttentionReactions`'.
///
/// Fork-only file. Owned by WP-C.
@MainActor
final class ClaudeSessionFeed {
    static let throttle: TimeInterval = 0.4
    /// A working session's row says how long its tasks have left ("3/7 ·
    /// ~4m left"), which moves with the clock as well as with the hub; the
    /// panel's list ticks as often. Rows that read the same aren't sent.
    static let clockInterval: TimeInterval = 30

    private weak var hub: ClaudeControlHub?
    private weak var fleet: NotchFleet?
    private let state: ClaudeNotchState
    private let isShown: (String) -> Bool
    private var cancellables = Set<AnyCancellable>()
    /// What each ring was last given, so only changes go out: every
    /// `setSessions` animates every notch.
    private var sent: [String: [AgentSession]] = [:]
    private var lastRun = Date.distantPast
    private var pending: DispatchWorkItem?

    /// `isShown` answers whether a ring is switched on in the notch
    /// (Codenotch's connected state). `state` is told the same routing, so the
    /// badges, the folded pill and the reactions count what the rows show.
    init(hub: ClaudeControlHub, fleet: NotchFleet, state: ClaudeNotchState,
         isShown: @escaping (String) -> Bool) {
        self.hub = hub
        self.fleet = fleet
        self.state = state
        self.isShown = isShown
    }

    /// Follow the hub, and anything else that changes which rows a ring
    /// gets (`changes`: the connected set, the ring list).
    func start(alsoOn changes: AnyPublisher<Void, Never>) {
        guard let hub else { return }
        Publishers.Merge4(
            hub.$sessions.map { _ in () }.eraseToAnyPublisher(),
            hub.$accounts.map { _ in () }.eraseToAnyPublisher(),
            changes,
            Timer.publish(every: Self.clockInterval, on: .main, in: .common)
                .autoconnect()
                .map { _ in () }
                .eraseToAnyPublisher()
        )
        // `@Published` fires before the value lands; the update runs a turn
        // later at the earliest.
        .receive(on: DispatchQueue.main)
        .sink { [weak self] in self?.setNeedsUpdate() }
        .store(in: &cancellables)
        setNeedsUpdate()
    }

    func stop() {
        cancellables.removeAll()
        pending?.cancel()
        pending = nil
    }

    /// Run soon: at once if the last run was long enough ago, otherwise when
    /// the throttle allows. Changes in between are folded into that one run.
    private func setNeedsUpdate() {
        guard pending == nil else { return }
        let wait = max(0, lastRun.addingTimeInterval(Self.throttle).timeIntervalSinceNow)
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated {
                self?.pending = nil
                self?.update()
            }
        }
        pending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + wait, execute: work)
    }

    private func update() {
        guard let hub, let fleet else { return }
        lastRun = Date()
        let rings = ClaudeProviderSync.ringIDs(hub.accounts)
        let sessionRings = hub.sessions.map(\.ringID)
        let shown = Self.shownRings(rings: rings, sessionRings: sessionRings, isShown: isShown)
        // The hub hands rows only to rings it is told are shown (and stops
        // reading usage for the rest), so it hears first.
        hub.setShownRings(shown)
        state.route(rings: rings, shown: shown)
        let rows = Self.rows(
            rings: rings,
            sessionRings: sessionRings,
            previous: Array(sent.keys),
            isShown: isShown,
            rows: { hub.activityRows(ringID: $0, now: Date()) }
        )
        for (ringID, sessions) in rows.sorted(by: { $0.key < $1.key }) where sent[ringID] != sessions {
            sent[ringID] = sessions
            fleet.setSessions(providerID: ringID, sessions: sessions)
        }
        for ringID in sent.keys where rows[ringID] == nil {
            sent[ringID] = nil
        }
    }

    // MARK: - Rules (pure)

    /// Session ring ids with no account behind them yet, in first-seen order.
    static func orphans(rings: [String], sessionRings: [String]) -> [String] {
        let known = Set(rings)
        var orphans: [String] = []
        for ring in sessionRings where !known.contains(ring) && !orphans.contains(ring) {
            orphans.append(ring)
        }
        return orphans
    }

    /// The ring ids the hub may hand rows to: the rings switched on, and the
    /// accounts not known yet when the default ring (which shows them) is on.
    static func shownRings(rings: [String], sessionRings: [String], isShown: (String) -> Bool) -> Set<String> {
        var shown = Set(rings.filter(isShown))
        if shown.contains(ClaudeRingIdentity.defaultRingID) {
            shown.formUnion(Self.orphans(rings: rings, sessionRings: sessionRings))
        }
        return shown
    }

    /// Every ring's rows: the rings of the known accounts, plus sessions of
    /// accounts not known yet folded into the default ring, plus an empty
    /// list for every ring that had rows last time and has no ring now.
    ///
    /// `rows` is the hub's projection for one ring id.
    static func rows(
        rings: [String],
        sessionRings: [String],
        previous: [String],
        isShown: (String) -> Bool,
        rows: (String) -> [ClaudeActivityRow]
    ) -> [String: [AgentSession]] {
        let known = Set(rings)
        let defaultRing = ClaudeRingIdentity.defaultRingID
        let orphans = Self.orphans(rings: rings, sessionRings: sessionRings)
        var result: [String: [AgentSession]] = [:]
        for ring in known {
            guard isShown(ring) else { result[ring] = []; continue }
            var ringRows = rows(ring)
            if ring == defaultRing {
                for orphan in orphans { ringRows += rows(orphan) }
            }
            result[ring] = ringRows.map(agentSession)
        }
        // With no default ring to take them, sessions of unknown accounts
        // show nowhere — but a ring that is gone still has to be emptied.
        for ring in previous where result[ring] == nil {
            result[ring] = []
        }
        return result
    }

    /// A hover-card row as Codenotch's `AgentSession`, field for field.
    static func agentSession(_ row: ClaudeActivityRow) -> AgentSession {
        let state: AgentSession.State
        switch row.state {
        case .busy: state = .busy
        case .waiting: state = .waiting
        case .success: state = .success
        case .idle: state = .idle
        }
        return AgentSession(
            id: row.id,
            name: row.name,
            detail: row.detail,
            state: state,
            waitingFor: row.waitingFor,
            since: row.since,
            processID: row.pid
        )
    }
}
