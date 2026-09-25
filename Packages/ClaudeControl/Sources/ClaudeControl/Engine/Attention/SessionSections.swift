//
//  SessionSections.swift
//  ClaudeControl
//
//  How the sessions panel groups and orders sessions, when a section folds
//  and when rows go compact, and the counts its attention strip shows. Pure
//  functions of the session list so the rules are unit-tested and identical
//  in the panel and the snapshots.
//

import Foundation

// MARK: - Counts

/// How many sessions sit in each attention bucket.
nonisolated struct AttentionCounts: Equatable, Sendable {
    /// Every session in "Needs you", failed turns included.
    var needsInput = 0
    var readyForReview = 0
    var working = 0
    var idle = 0
    /// The failed turns among `needsInput`: they wait on the user (retry,
    /// switch account, sign in) but there is nothing to answer, so the strip
    /// counts them apart, in the critical colour.
    var failed = 0

    init(needsInput: Int = 0, readyForReview: Int = 0, working: Int = 0, idle: Int = 0, failed: Int = 0) {
        self.needsInput = needsInput
        self.readyForReview = readyForReview
        self.working = working
        self.idle = idle
        self.failed = failed
    }

    init(_ sessions: [SessionState]) {
        for session in sessions {
            switch session.attention {
            case .needsInput(let reason):
                needsInput += 1
                if reason.isError { failed += 1 }
            case .readyForReview: readyForReview += 1
            case .working: working += 1
            case .idle: idle += 1
            }
        }
    }

    /// Sessions waiting on an answer (needs you, not failed).
    var answerable: Int { needsInput - failed }

    var total: Int { needsInput + readyForReview + working + idle }
}

// MARK: - Sections

/// One attention bucket's sessions, in display order.
nonisolated struct SessionSection: Identifiable, Equatable, Sendable {
    let bucket: AttentionBucket
    var sessions: [SessionState]

    var id: Int { bucket.rawValue }
}

nonisolated enum SessionSections {
    /// Idle sessions beyond this many fold into a one-line summary.
    static let idleCollapseThreshold = 3
    /// Above this many rows drawn (folded sections don't count), rows that
    /// don't need an answer go to one line, so 25 sessions fit without a
    /// scroll hunt.
    static let compactThreshold = 8

    /// Whether a section starts folded: only a long idle list does; the user
    /// folds the others with their headers.
    static func isCollapsedByDefault(_ bucket: AttentionBucket, count: Int) -> Bool {
        bucket == .idle && count > idleCollapseThreshold
    }

    /// Whether `bucket` is folded, given the user's choices (bucket → folded).
    static func isCollapsed(_ bucket: AttentionBucket, count: Int, overrides: [AttentionBucket: Bool]) -> Bool {
        // Needs-you never folds: it is what the panel is for.
        guard bucket != .needsInput else { return false }
        return overrides[bucket] ?? isCollapsedByDefault(bucket, count: count)
    }

    /// Whether rows go to one line when `sessionCount` rows are drawn.
    static func isCompact(sessionCount: Int) -> Bool {
        sessionCount > compactThreshold
    }

    /// The one line a folded section shows: its first titles, then how many
    /// more. "Write migration tests, Investigate the flaky CI job and 1 more".
    static func collapsedSummary(_ section: SessionSection, maxTitles: Int = 2) -> String {
        let titles = section.sessions.prefix(maxTitles).map(\.displayTitle)
        let rest = section.sessions.count - titles.count
        let list = titles.joined(separator: ", ")
        return rest > 0 ? "\(list) and \(rest) more" : list
    }

    /// Non-empty sections in bucket order (needs you, review, working, idle).
    ///
    /// Within a section: needs-you answerable requests before failed turns,
    /// each oldest waiting first; review newest completion first; working
    /// longest-running first; idle most recent first. Ties by session id so
    /// equal keys never swap between renders.
    ///
    /// - Parameter previousOrder: The session ids as currently displayed.
    ///   When given (the pointer is over the list), sessions keep that
    ///   relative order inside their section and newcomers go to the end of
    ///   theirs, so a row never moves under the pointer. Sessions still
    ///   change section as their attention changes.
    static func build(_ sessions: [SessionState], preservingOrder previousOrder: [String]? = nil) -> [SessionSection] {
        var grouped: [AttentionBucket: [SessionState]] = [:]
        for session in sessions {
            grouped[session.attention.bucket, default: []].append(session)
        }

        let previousIndex: [String: Int]? = previousOrder.map { order in
            var index: [String: Int] = [:]
            for (position, id) in order.enumerated() where index[id] == nil {
                index[id] = position
            }
            return index
        }

        return AttentionBucket.allCases.compactMap { bucket in
            guard let members = grouped[bucket], !members.isEmpty else { return nil }
            var ordered = members.sorted { naturallyPrecedes($0, $1, in: bucket) }
            if let previousIndex {
                ordered = preserve(ordered, previousIndex: previousIndex)
            }
            return SessionSection(bucket: bucket, sessions: ordered)
        }
    }

    /// Flat display order of a section list.
    static func order(of sections: [SessionSection]) -> [String] {
        sections.flatMap { $0.sessions.map(\.sessionId) }
    }

    /// When the session started waiting on the user: the pending approval's
    /// arrival, else the last hook event (the one that blocked it).
    static func waitingSince(_ session: SessionState) -> Date {
        session.activePermission?.receivedAt ?? session.lastActivity
    }

    // MARK: Ordering

    static func naturallyPrecedes(_ a: SessionState, _ b: SessionState, in bucket: AttentionBucket) -> Bool {
        switch bucket {
        case .needsInput:
            // A prompt you can answer outranks a failed turn (the engine's
            // `sortRank`): nothing gets pushed below the fold by sessions
            // that only need a retry.
            let rankA = a.attention.needsInputReason?.sortRank ?? 0
            let rankB = b.attention.needsInputReason?.sortRank ?? 0
            if rankA != rankB { return rankA < rankB }
            let waitA = waitingSince(a), waitB = waitingSince(b)
            if waitA != waitB { return waitA < waitB }
        case .readyForReview:
            if let result = compareDescending(a.completedAt, b.completedAt) { return result }
        case .working:
            if let result = compareAscending(a.turnStartedAt, b.turnStartedAt) { return result }
        case .idle:
            if a.lastActivity != b.lastActivity { return a.lastActivity > b.lastActivity }
        }
        return a.sessionId < b.sessionId
    }

    /// Newest first; a missing date sorts last. Nil when equal.
    private static func compareDescending(_ a: Date?, _ b: Date?) -> Bool? {
        switch (a, b) {
        case let (a?, b?): return a == b ? nil : a > b
        case (.some, nil): return true
        case (nil, .some): return false
        case (nil, nil): return nil
        }
    }

    /// Oldest first; a missing date sorts last. Nil when equal.
    private static func compareAscending(_ a: Date?, _ b: Date?) -> Bool? {
        switch (a, b) {
        case let (a?, b?): return a == b ? nil : a < b
        case (.some, nil): return true
        case (nil, .some): return false
        case (nil, nil): return nil
        }
    }

    /// Known sessions in their previous relative order, then the newcomers in
    /// their natural order.
    private static func preserve(_ ordered: [SessionState], previousIndex: [String: Int]) -> [SessionState] {
        let known = ordered
            .filter { previousIndex[$0.sessionId] != nil }
            .sorted { previousIndex[$0.sessionId]! < previousIndex[$1.sessionId]! }
        let newcomers = ordered.filter { previousIndex[$0.sessionId] == nil }
        return known + newcomers
    }
}

// MARK: - Section Titles

extension AttentionBucket {
    /// Section header label.
    var sectionTitle: String {
        switch self {
        case .needsInput: return "Needs you"
        case .readyForReview: return "Ready for review"
        case .working: return "Working"
        case .idle: return "Idle"
        }
    }
}
