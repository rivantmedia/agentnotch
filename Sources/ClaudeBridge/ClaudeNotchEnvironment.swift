import ClaudeControl
import Combine
import SwiftUI

/// Where a cell is drawn, for the Claude badges: which edge (so they sit on
/// the side facing the screen) and whether it is the compact strip beside the
/// camera (dots instead of counts). Set by U8(c)/(d) in NotchRootView.
///
/// Fork-only file. Owned by WP-C.
struct ClaudeCellContext: Equatable {
    var edge: NotchEdge = .right
    var compact = false
}

private struct ClaudeCellContextKey: EnvironmentKey {
    static let defaultValue = ClaudeCellContext()
}

private struct ActivitySuccessSettlesKey: EnvironmentKey {
    static let defaultValue = false
}

private struct ClaudeStillFrameKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    var claudeCellContext: ClaudeCellContext {
        get { self[ClaudeCellContextKey.self] }
        set { self[ClaudeCellContextKey.self] = newValue }
    }

    /// Whether a `.success` activity arc stops pulsing and stays steady (U7).
    /// False everywhere upstream draws, so every other provider pulses as
    /// before; `ClaudeRingDecoration` sets it for Claude rings 90 s after
    /// their newest completion.
    var activitySuccessSettles: Bool {
        get { self[ActivitySuccessSettlesKey.self] }
        set { self[ActivitySuccessSettlesKey.self] = newValue }
    }

    /// Drawn for a still picture (`ClaudeNotchSnapshots`): the fork's own
    /// animations show their resting frame rather than wherever they are.
    var claudeStillFrame: Bool {
        get { self[ClaudeStillFrameKey.self] }
        set { self[ClaudeStillFrameKey.self] = newValue }
    }
}

extension NotchEdge {
    /// The same edge in ClaudeControl's geometry.
    var claudeEdge: ClaudePanelEdge {
        switch self {
        case .right: return .right
        case .left: return .left
        case .top: return .top
        case .bottom: return .bottom
        }
    }
}

/// What the rings and the folded notch draw from the hub, and nothing else:
/// attention counts and the "just finished" deadlines. Views observe this
/// rather than the hub itself, which republishes on every session event —
/// most of which change nothing a ring shows.
///
/// Everything here is as the notch shows it
/// (`ClaudeAttentionPolicy.notchValues`): a ring switched off in the notch
/// counts for nothing, and a session whose account is not known yet counts
/// on the default ring, beside its row. So the folded pill's dots and the
/// hold never stand for a session no ring shows, and the default ring's
/// badges and settle agree with the rows folded into it. `ClaudeSessionFeed`
/// says which rings those are (`route`); until it has, the hub's values pass
/// through as they are.
///
/// Filled by `ClaudeBridge.attach` (and by the snapshot renderer). Until then
/// `isAttached` is false and every Claude ring draws exactly as upstream's
/// would.
@MainActor
final class ClaudeNotchState: ObservableObject {
    static let shared = ClaudeNotchState()

    @Published private(set) var isAttached = false
    /// Per ring in the notch.
    @Published private(set) var ringCounts: [String: ClaudeAttentionCounts] = [:]
    /// Every ring in the notch together: the folded pill, the hold.
    @Published private(set) var totalCounts: ClaudeAttentionCounts = .zero
    @Published private(set) var freshSuccessUntil: [String: Date] = [:]

    /// The hub's values, by the ring id of each session's account.
    private var hubCounts: [String: ClaudeAttentionCounts] = [:]
    private var hubFresh: [String: Date] = [:]
    /// The accounts' rings, and the ring ids the notch shows (see
    /// `ClaudeSessionFeed.shownRings`). Nil until the feed has run.
    private var routing: (rings: [String], shown: Set<String>)?

    private var cancellables = Set<AnyCancellable>()

    /// Follow the hub. Only real changes republish.
    func follow(_ hub: ClaudeControlHub) {
        cancellables.removeAll()
        isAttached = true
        hub.$ringCounts
            .removeDuplicates()
            .sink { [weak self] in
                self?.hubCounts = $0
                self?.republish()
            }
            .store(in: &cancellables)
        hub.$freshSuccessUntil
            .removeDuplicates()
            .sink { [weak self] in
                self?.hubFresh = $0
                self?.republish()
            }
            .store(in: &cancellables)
    }

    /// Which ring ids are accounts' rings, and which the notch shows.
    func route(rings: [String], shown: Set<String>) {
        if let routing, routing.rings == rings, routing.shown == shown { return }
        routing = (rings, shown)
        republish()
    }

    /// Whether the notch shows the sessions of `ringID` (on its own ring, or
    /// on the default one for an account not known yet).
    func isShown(_ ringID: String) -> Bool {
        routing?.shown.contains(ringID) ?? true
    }

    /// Fixed values, for rendering snapshots without a running hub.
    func show(ringCounts: [String: ClaudeAttentionCounts], freshSuccessUntil: [String: Date]) {
        cancellables.removeAll()
        isAttached = true
        routing = nil
        hubCounts = ringCounts
        hubFresh = freshSuccessUntil
        republish()
    }

    private func republish() {
        let counts = ClaudeAttentionPolicy.notchValues(
            hubCounts, rings: routing?.rings ?? [], shown: routing?.shown, combine: ClaudeAttentionPolicy.combined)
        let fresh = ClaudeAttentionPolicy.notchValues(
            hubFresh, rings: routing?.rings ?? [], shown: routing?.shown, combine: max)
        let total = ClaudeAttentionPolicy.total(counts.values)
        if ringCounts != counts { ringCounts = counts }
        if totalCounts != total { totalCounts = total }
        if freshSuccessUntil != fresh { freshSuccessUntil = fresh }
    }

    /// Whether the green arc of `ringID` has stopped pulsing: nothing on it
    /// finished within the last 90 s.
    func settles(_ ringID: String, now: Date) -> Bool {
        freshSuccessUntil[ringID].map { now >= $0 } ?? true
    }
}

/// The ClaudeControl settings the notch surface draws or reacts with, as
/// published values: the rings, the folded notch and the reactions redraw as
/// soon as one of them is changed in Settings, not at the next session event.
///
/// ClaudeControl keeps them in UserDefaults (`claudeControl.*`); any write to
/// the defaults re-reads them, and only a real change republishes.
@MainActor
final class ClaudeNotchSettings: ObservableObject {
    static let shared = ClaudeNotchSettings()

    @Published private(set) var ringBadges = ClaudeControlSettings.ringBadges
    @Published private(set) var restingMarks = ClaudeControlSettings.restingMarks
    @Published private(set) var dockBadge = ClaudeControlSettings.dockBadge
    @Published private(set) var holdOpen = ClaudeControlSettings.holdOpenWhileNeedsYou
    @Published private(set) var autoOpen = ClaudeControlSettings.autoOpen

    private var observer: NSObjectProtocol?

    private init() {
        observer = NotificationCenter.default.addObserver(
            forName: UserDefaults.didChangeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.reload() }
        }
    }

    private func reload() {
        if ringBadges != ClaudeControlSettings.ringBadges { ringBadges = ClaudeControlSettings.ringBadges }
        if restingMarks != ClaudeControlSettings.restingMarks { restingMarks = ClaudeControlSettings.restingMarks }
        if dockBadge != ClaudeControlSettings.dockBadge { dockBadge = ClaudeControlSettings.dockBadge }
        if holdOpen != ClaudeControlSettings.holdOpenWhileNeedsYou { holdOpen = ClaudeControlSettings.holdOpenWhileNeedsYou }
        if autoOpen != ClaudeControlSettings.autoOpen { autoOpen = ClaudeControlSettings.autoOpen }
    }
}
