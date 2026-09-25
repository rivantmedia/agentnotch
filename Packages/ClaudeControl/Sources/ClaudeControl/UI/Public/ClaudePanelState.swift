//
//  ClaudePanelState.swift
//  ClaudeControl
//
//  What the sessions panel is showing, shared between the panel's SwiftUI
//  content (this package) and the window that hosts it (the app's
//  ClaudePanelController): routes, back, close, the keyboard selection, the
//  per-session drafts, a "mark all reviewed" waiting out its undo, and the
//  content height the window sizes itself to.
//

import Combine
import CoreGraphics
import Foundation

@MainActor
public final class ClaudePanelState: ObservableObject {
    public init(route: ClaudePanelRoute) {
        self.route = route
        if case .sessions(let ringID) = route {
            ringFilter = ringID
        }
    }

    /// The list (optionally filtered to one ring), one session's chat, or setup.
    @Published public var route: ClaudePanelRoute {
        didSet { routeChanged(from: oldValue) }
    }
    /// The ring the list is filtered to; nil shows every account.
    @Published public var ringFilter: String?
    /// The header's "Keep open" pin: an outside click does not close the panel.
    @Published public var isPinned: Bool = false
    /// The width the content lays out at, set by the window (list or chat width).
    @Published public var contentWidth: CGFloat = 440
    /// The height the content would like at `contentWidth`, reported by the
    /// views and kept within [220, `effectiveMaxContentHeight`].
    @Published public private(set) var idealContentHeight: CGFloat = ClaudePanelState.minimumContentHeight
    /// Whether the panel is on screen. The window sets it; the chat marks a
    /// session reviewed when it finishes only while this is true, and a
    /// "mark all reviewed" waiting out its undo is carried out on close.
    @Published public var isPresented: Bool = true {
        didSet {
            if !isPresented { commitPendingReview() }
        }
    }
    /// A row to point out: a banner click or an auto-open lands on the list
    /// with this session highlighted and selected, so its inline answer is
    /// one keystroke away.
    @Published public var highlightedSessionId: String? {
        didSet {
            if let highlightedSessionId { selectedSessionId = highlightedSessionId }
        }
    }
    /// The tallest the window can make the content (its placement's
    /// `maxContentHeight`). Nil uses the design's cap for the mode (680 for
    /// the list, 780 for the chat).
    @Published public var maxContentHeight: CGFloat? {
        didSet { applyContentHeight() }
    }

    /// The list or the chat, for the window's width and height limits.
    public var mode: ClaudePanelMode {
        if case .session = route { return .chat }
        return .list
    }

    /// The cap `idealContentHeight` is held under.
    public var effectiveMaxContentHeight: CGFloat {
        max(Self.minimumContentHeight, maxContentHeight ?? ClaudePanelGeometry.heightCap(mode))
    }

    /// Esc: back from the chat to the list, else close. For the window's
    /// `cancelOperation` when no view in the panel took the key.
    public func escape() {
        back()
    }

    // MARK: - Callbacks (set by the window)

    /// Close the panel (Esc in the list, the close button).
    public var onClose: () -> Void = {}
    /// Make the panel key (a text field was clicked, a key action needs it).
    public var onRequestKey: () -> Void = {}
    /// Show Codenotch's settings at the Claude Code pane.
    public var onOpenSettings: () -> Void = {}
    /// A session's terminal was brought to the front.
    public var onJumped: () -> Void = {}

    // MARK: - Package state

    /// Smallest height the panel content is given.
    nonisolated static let minimumContentHeight: CGFloat = ClaudePanelGeometry.minimumHeight

    /// The row the keyboard acts on.
    @Published var selectedSessionId: String?
    /// Sections the user folded or unfolded (bucket → folded).
    @Published var sectionFolds: [AttentionBucket: Bool] = [:]
    /// What was typed into each session's composer and not sent yet.
    @Published var drafts: [String: String] = [:]
    /// "Marked 3 reviewed · Undo", while it can still be undone.
    @Published private(set) var pendingReview: PendingReview?
    /// The setup card was answered in this panel; hide it before the engine
    /// republishes its setup state.
    @Published var didAnswerSetup = false
    /// The chat's composer has keyboard focus (plain keys are typing).
    @Published var isComposerFocused = false
    /// Which requests can be answered yet (see `AnswerGate`).
    @Published private(set) var answerGate = AnswerGate()
    /// Bumped the moment a waiting request becomes answerable, so the list
    /// and the chat (which both observe this state) draw its buttons live
    /// even when nothing else about the session changes meanwhile.
    @Published private(set) var armingTick = 0

    /// A "mark all reviewed" waiting out its undo window. Nothing is marked
    /// until it commits, so undo loses nothing.
    struct PendingReview: Equatable {
        let sessionIds: [String]
        let deadline: Date
        /// The click: the commit marks them reviewed as of then (BHV-9).
        var clickedAt: Date { deadline.addingTimeInterval(-ClaudePanelState.undoWindow) }
    }

    /// How long "Mark all reviewed" can be undone.
    nonisolated static let undoWindow: TimeInterval = 5

    private var pendingCommit: (([String], Date) -> Void)?
    private var pendingTask: Task<Void, Never>?
    private var armingTask: Task<Void, Never>?

    /// Show one session's chat.
    func showChat(sessionId: String) {
        selectedSessionId = sessionId
        route = .session(id: sessionId)
    }

    /// Back from the chat to the list (keeping the ring filter), else close.
    func back() {
        if case .session = route {
            route = .sessions(ringID: ringFilter)
        } else {
            onClose()
        }
    }

    /// Fold or unfold a section of `count` sessions.
    func toggleSection(_ bucket: AttentionBucket, count: Int) {
        let folded = SessionSections.isCollapsed(bucket, count: count, overrides: sectionFolds)
        sectionFolds[bucket] = !folded
    }

    // MARK: Answers

    /// Record the requests on screen, so new ones wait `AnswerGate.armDelay`,
    /// and redraw when the next of them becomes answerable.
    func noteShownRequests(_ toolUseIds: [String], armed: Bool = false, now: Date = Date()) {
        var gate = answerGate
        // Armed first: `noteShown` would otherwise start their wait now.
        if armed { gate.noteShownArmed(toolUseIds) }
        gate.noteShown(toolUseIds, now: now)
        if gate != answerGate { answerGate = gate }
        scheduleArming()
    }

    /// Take the one answer a request gets; false to drop the click.
    func claimAnswer(_ toolUseId: String, now: Date = Date()) -> Bool {
        var gate = answerGate
        guard gate.claim(toolUseId, now: now) else { return false }
        answerGate = gate
        return true
    }

    /// Whether the request's buttons take clicks yet.
    func isAnswerArmed(_ toolUseId: String, now: Date = Date()) -> Bool {
        answerGate.isArmed(toolUseId, now: now)
    }

    /// Bump `armingTick` when the next waiting request arms (one timer for
    /// the whole panel, re-planned whenever the requests on screen change).
    private func scheduleArming() {
        armingTask?.cancel()
        armingTask = nil
        guard let next = answerGate.nextArming(after: Date()) else { return }
        let delay = max(0.01, next.timeIntervalSinceNow + 0.01)
        armingTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled, let self else { return }
            self.armingTick &+= 1
            self.scheduleArming()
        }
    }

    // MARK: Mark all reviewed

    /// Show `sessionIds` as reviewed now and mark them for real after the
    /// undo window, or as soon as the panel closes.
    ///
    /// - Parameter schedulesCommit: False keeps it pending until undone,
    ///   committed or the panel closes (snapshots, tests).
    func markReviewedWithUndo(_ sessionIds: [String], now: Date = Date(), schedulesCommit: Bool = true,
                              commit: @escaping ([String]) -> Void) {
        markReviewedWithUndo(sessionIds, now: now, schedulesCommit: schedulesCommit, commitAt: { ids, _ in commit(ids) })
    }

    /// The same, committing with the click time rather than the moment the
    /// undo window ends: a turn that completes in those seconds stays
    /// unreviewed (BHV-9).
    func markReviewedWithUndo(_ sessionIds: [String], now: Date = Date(), schedulesCommit: Bool = true,
                              commitAt commit: @escaping ([String], Date) -> Void) {
        guard !sessionIds.isEmpty else { return }
        commitPendingReview()
        let pending = PendingReview(sessionIds: sessionIds, deadline: now.addingTimeInterval(Self.undoWindow))
        pendingReview = pending
        pendingCommit = commit
        guard schedulesCommit else { return }
        pendingTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(Self.undoWindow))
            guard !Task.isCancelled, let self, self.pendingReview == pending else { return }
            self.commitPendingReview()
        }
    }

    /// Put the sessions back in "Ready for review".
    func undoPendingReview() {
        pendingTask?.cancel()
        pendingTask = nil
        pendingCommit = nil
        pendingReview = nil
    }

    /// Mark the pending sessions reviewed now.
    func commitPendingReview() {
        pendingTask?.cancel()
        pendingTask = nil
        guard let pendingReview, let pendingCommit else { return }
        self.pendingReview = nil
        self.pendingCommit = nil
        pendingCommit(pendingReview.sessionIds, pendingReview.clickedAt)
    }

    // MARK: Height

    /// Record the content's natural height. Non-finite reports (a view laid
    /// out mid-transition) are ignored.
    func reportContentHeight(_ height: CGFloat) {
        guard height.isFinite else { return }
        naturalContentHeight = height
        applyContentHeight()
    }

    /// The last natural height reported, before clamping, so a new cap can
    /// be applied to it.
    private var naturalContentHeight: CGFloat?

    private func applyContentHeight() {
        guard let naturalContentHeight else { return }
        let clamped = Self.clampedHeight(naturalContentHeight, max: effectiveMaxContentHeight)
        if clamped != idealContentHeight {
            idealContentHeight = clamped
        }
    }

    /// `height` rounded up and held within [220, `max`].
    nonisolated static func clampedHeight(_ height: CGFloat, max maximum: CGFloat) -> CGFloat {
        min(max(minimumContentHeight, height.rounded(.up)), max(minimumContentHeight, maximum))
    }

    private func routeChanged(from old: ClaudePanelRoute) {
        // A list route names its filter every time it is set, including
        // again: a banner re-opening "all sessions" over a list the user had
        // narrowed must not leave its highlighted row filtered away.
        if case .sessions(let ringID) = route, ringFilter != ringID {
            ringFilter = ringID
        }
        guard old != route else { return }
        isComposerFocused = false
        // The other mode has its own cap; re-apply it until the new content
        // reports its own height.
        applyContentHeight()
    }
}
