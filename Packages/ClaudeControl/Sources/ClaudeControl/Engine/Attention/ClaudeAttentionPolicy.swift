//
//  ClaudeAttentionPolicy.swift
//  ClaudeControl
//
//  What the notch does when a Claude session starts needing you or finishes
//  work to review: chime, open the sessions panel by itself, or peek the
//  notch (design §6). Pure, so the whole matrix is tested; the app's
//  `ClaudeAttentionReactions` gathers the context and carries the decisions
//  out with Codenotch's own sounds and peek.
//
//  Also the other small rules the notch surface shares: where a click on a
//  hover-card session row goes, the Dock badge, and the marks on the folded
//  notch.
//
//  Owned by WP-C.
//

import Foundation

public nonisolated enum ClaudeAttentionPolicy {
    // MARK: - Reactions

    /// Everything a reaction depends on besides the transition itself.
    public struct Context: Hashable, Sendable {
        /// When the panel may open by itself.
        public var autoOpen: AutoOpenPolicy
        /// Codenotch's "Play a sound" for sessions (`sessionEndSound`).
        public var chimes: Bool
        /// Codenotch's "Open the notch when a session ends" (`announceSessionEnd`).
        public var peeks: Bool
        /// The session's own terminal (its tab, where that can be told) is frontmost.
        public var terminalFocused: Bool
        /// Some terminal or editor window is on screen on the current Space.
        public var anyTerminalVisible: Bool
        /// The frontmost app is full screen.
        public var fullScreen: Bool
        /// The sessions panel is open.
        public var panelOpen: Bool
        /// The session's ring is in the notch. A peek of a notch that does not
        /// show the session would only point at nothing.
        public var ringShown: Bool

        public init(autoOpen: AutoOpenPolicy, chimes: Bool, peeks: Bool, terminalFocused: Bool,
                    anyTerminalVisible: Bool, fullScreen: Bool, panelOpen: Bool, ringShown: Bool = true) {
            self.autoOpen = autoOpen
            self.chimes = chimes
            self.peeks = peeks
            self.terminalFocused = terminalFocused
            self.anyTerminalVisible = anyTerminalVisible
            self.fullScreen = fullScreen
            self.panelOpen = panelOpen
            self.ringShown = ringShown
        }
    }

    /// Which of Codenotch's two session sounds.
    public enum Sound: Hashable, Sendable {
        /// `sessionBlockedSoundName`: you are the hold-up.
        case needsInput
        /// `sessionEndSoundName`: a turn is done.
        case finished
    }

    public enum Decision: Hashable, Sendable {
        case chime(Sound)
        /// Open the panel at this session, without taking key focus.
        case autoOpen(sessionID: String, kind: ClaudeAttentionTransition.Kind)
        /// Open the notch for Codenotch's peek duration; a click jumps to `pid`.
        case peek(pid: Int32?, kind: ClaudeAttentionTransition.Kind)
    }

    /// Transitions this close together are one burst: one chime, one peek.
    /// See `Burst`.
    public static let burstWindow: TimeInterval = 0.5

    /// What to do for one transition.
    ///
    /// - Looking at that session's terminal already: nothing at all.
    /// - Needs input: the blocked chime; then open the panel by itself when
    ///   `autoOpen` allows it, no terminal is on screen, nothing is full
    ///   screen and the panel is closed; otherwise peek.
    /// - A failed turn (rate limit, overload, sign-in; `.needsInput` of kind
    ///   `.error`): one soft cue — the finished chime and a peek — never the
    ///   blocked chime or an auto-open for something that can't be answered
    ///   from the panel (GUX-2, BHV-2).
    /// - Ready for review: the finished chime; auto-open only for
    ///   `.needsInputOrDone`, otherwise peek.
    /// - Resolved: nothing (the panel closing after an answered prompt is the
    ///   panel's own business).
    /// - Never a peek while the panel is open, or for a ring that is not shown.
    public static func decide(_ transition: ClaudeAttentionTransition, context: Context) -> [Decision] {
        guard !context.terminalFocused else { return [] }
        let sound: Sound
        let opensBySelf: Bool
        switch transition.kind {
        case .needsInput where transition.isFailure:
            sound = .finished
            opensBySelf = false
        case .needsInput:
            sound = .needsInput
            opensBySelf = context.autoOpen != .never
        case .readyForReview:
            sound = .finished
            opensBySelf = context.autoOpen == .needsInputOrDone
        case .resolved:
            return []
        }

        var decisions: [Decision] = []
        if context.chimes { decisions.append(.chime(sound)) }
        if opensBySelf, !context.anyTerminalVisible, !context.fullScreen, !context.panelOpen {
            decisions.append(.autoOpen(sessionID: transition.session.id, kind: transition.kind))
        } else if context.peeks, !context.panelOpen, context.ringShown {
            decisions.append(.peek(pid: transition.session.pid, kind: transition.kind))
        }
        return decisions
    }

    /// One burst's decisions, in the order they arrived, as what is actually
    /// done: at most one chime (the blocked one wins), and at most one of
    /// auto-open or peek. Opening the panel makes a peek pointless, so any
    /// auto-open wins over every peek. Within either, a needs-input session
    /// outranks a finished one, and the newest of equals is the one offered.
    public static func merge(_ decisions: [Decision]) -> [Decision] {
        var sound: Sound?
        var open: (sessionID: String, kind: ClaudeAttentionTransition.Kind)?
        var peek: (pid: Int32?, kind: ClaudeAttentionTransition.Kind)?
        for decision in decisions {
            switch decision {
            case .chime(let next):
                if sound != .needsInput { sound = next }
            case .autoOpen(let sessionID, let kind):
                if open.map({ rank(kind) >= rank($0.kind) }) ?? true { open = (sessionID, kind) }
            case .peek(let pid, let kind):
                if peek.map({ rank(kind) >= rank($0.kind) }) ?? true { peek = (pid, kind) }
            }
        }
        var merged: [Decision] = []
        if let sound { merged.append(.chime(sound)) }
        if let open {
            merged.append(.autoOpen(sessionID: open.sessionID, kind: open.kind))
        } else if let peek {
            merged.append(.peek(pid: peek.pid, kind: peek.kind))
        }
        return merged
    }

    private static func rank(_ kind: ClaudeAttentionTransition.Kind) -> Int {
        switch kind {
        case .needsInput: return 2
        case .readyForReview: return 1
        case .resolved: return 0
        }
    }

    /// Transitions that arrive together, gathered into one reaction.
    ///
    /// A burst opens with its first transition and closes `burstWindow`
    /// later, but never while one of its transitions is still being decided:
    /// deciding waits on the terminal checks, which can outlast the window,
    /// and a decision landing after the close would be a second chime.
    ///
    /// ```
    /// let due = burst.begin(at: now)     // schedule `close` at `due`, if any
    /// burst.finish(decide(transition))   // when its context is in
    /// burst.close(at: now)               // at `due`, and after each finish
    /// ```
    public struct Burst: Sendable {
        public private(set) var openedAt: Date?
        /// Transitions begun and not yet finished.
        public private(set) var pending = 0
        public private(set) var decisions: [Decision] = []

        public init() {}

        /// A transition arrived and is being decided. Returns when its burst
        /// is due to close if this transition opened it, nil if it joined one
        /// already open.
        public mutating func begin(at now: Date) -> Date? {
            pending += 1
            guard openedAt == nil else { return nil }
            openedAt = now
            return now.addingTimeInterval(ClaudeAttentionPolicy.burstWindow)
        }

        /// One transition's decisions are in.
        public mutating func finish(_ made: [Decision]) {
            pending = max(0, pending - 1)
            decisions += made
        }

        /// The burst's merged decisions, once it is over: its window has
        /// passed and nothing in it is still being decided. Nil until then.
        /// Closing starts afresh, so the next transition opens a new burst.
        public mutating func close(at now: Date) -> [Decision]? {
            guard let openedAt, pending == 0,
                  now >= openedAt.addingTimeInterval(ClaudeAttentionPolicy.burstWindow) else { return nil }
            let merged = ClaudeAttentionPolicy.merge(decisions)
            self = Burst()
            return merged
        }
    }

    // MARK: - Hover-card session rows

    public enum SessionClickTarget: Hashable, Sendable {
        /// The sessions panel, at that session, with key focus.
        case panel
        /// The session's terminal (which marks it reviewed).
        case terminal
    }

    /// Where a click on a session row in a ring's hover card goes. `.smart`
    /// sends a session that needs you to the panel, where it can be answered,
    /// and everything else to its terminal.
    public static func sessionClick(_ attention: ClaudeAttention, setting: SessionClickAction) -> SessionClickTarget {
        switch setting {
        case .panel: return .panel
        case .terminal: return .terminal
        case .smart:
            if case .needsInput = attention { return .panel }
            return .terminal
        }
    }

    // MARK: - What the notch counts

    /// Per-ring values (attention counts, "just finished" deadlines) as the
    /// notch shows them, so a badge, a folded dot or a held-open notch never
    /// stands for a session the notch does not show:
    ///
    /// - only rings in `shown` count: a ring switched off in the notch has no
    ///   ring to badge, and its sessions have no row;
    /// - a ring id that is no account's (a session whose account is not known
    ///   yet) counts on the default ring, which is where the session feed puts
    ///   its rows — and `shown` lists it only while the default ring is on.
    ///
    /// With `shown` nil (nothing has said yet which rings are shown) every
    /// value passes through unchanged.
    public static func notchValues<Value>(
        _ byRing: [String: Value],
        rings: [String],
        shown: Set<String>?,
        combine: (Value, Value) -> Value
    ) -> [String: Value] {
        guard let shown else { return byRing }
        let accountRings = Set(rings)
        var result: [String: Value] = [:]
        for (ring, value) in byRing where shown.contains(ring) {
            let target = accountRings.contains(ring) ? ring : ClaudeRingIdentity.defaultRingID
            result[target] = result[target].map { combine($0, value) } ?? value
        }
        return result
    }

    /// Two sets of counts added together.
    public static func combined(_ a: ClaudeAttentionCounts, _ b: ClaudeAttentionCounts) -> ClaudeAttentionCounts {
        ClaudeAttentionCounts(needsYou: a.needsYou + b.needsYou, review: a.review + b.review,
                              working: a.working + b.working, idle: a.idle + b.idle, failed: a.failed + b.failed)
    }

    /// Every ring's counts added together.
    public static func total<S: Sequence>(_ counts: S) -> ClaudeAttentionCounts where S.Element == ClaudeAttentionCounts {
        counts.reduce(.zero, combined)
    }

    // MARK: - Dock and folded notch

    /// The Dock icon's badge: the needs-you count, when that setting is on.
    public static func dockBadgeLabel(needsYou: Int, enabled: Bool) -> String? {
        guard enabled, needsYou > 0 else { return nil }
        return String(needsYou)
    }

    /// One dot on the folded notch.
    public enum RestingMark: Hashable, Sendable, CaseIterable {
        case needsYou, review, working
    }

    /// The dots the folded notch shows, in drawing order: amber for needs
    /// you, green for review, white for working. At most one of each.
    public static func restingMarks(_ counts: ClaudeAttentionCounts) -> [RestingMark] {
        var marks: [RestingMark] = []
        if counts.needsYou > 0 { marks.append(.needsYou) }
        if counts.review > 0 { marks.append(.review) }
        if counts.working > 0 { marks.append(.working) }
        return marks
    }

    /// How many half-cycles the amber dot breathes before resting bright.
    /// Odd, so an autoreversing animation towards "bright" ends there.
    public static let restingBreaths = 7
}
