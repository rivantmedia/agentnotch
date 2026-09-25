//
//  FolderIdentityTimeline.swift
//  ClaudeControl
//
//  Who a config folder ran as, over time, so a session is attributed to the
//  account it started as rather than to whoever the folder names now.
//
//  Claude Parallel Profiles mirrors the focused VS Code window's account
//  into `~/.claude` (rewriting `~/.claude.json`) every time another window
//  is focused. A Claude Code process that is already running keeps the
//  account it started with. So a terminal session in `~/.claude` that
//  started as Rivant keeps running as Rivant after the extension mirrored
//  Biios in, and its status line's rate limits are Rivant's.
//
//  The registry observes `~/.claude`'s identity whenever it reads it, with
//  the modification time of the `.claude.json` it read and the file's own
//  `accountUuid` (which a mirror keeps and a `/login` replaces). A change
//  is known to have happened after the last look that still showed the old
//  identity and no later than the write the new one was read from; a
//  process that started in between can't be attributed (`switching`), one
//  that started before the timeline begins neither (`unknown`), and after a
//  real login (the UUID itself changed) no earlier process can be either: a
//  `/login` typed in a running session moves that session to the new
//  account. Other folders change hands only when their VS Code window
//  switches account (and reloads) or someone signs in there, so they are
//  attributed to whoever they name now. Pure.
//

import Foundation

nonisolated struct FolderIdentityTimeline: Codable, Equatable, Sendable {
    struct Span: Codable, Equatable, Sendable {
        /// The identity id (`uuid:…`, `email:…`); nil when nobody was signed in.
        var identity: String?
        /// The file's own `accountUuid` (not corrected for a mirror).
        var rawUuid: String?
        /// It began with a real login (the file's UUID changed), not a
        /// mirror: processes that started earlier may have moved with it.
        var isLogin = false
        /// Held for certain from here: the `.claude.json` write it was first
        /// read from (or the look itself when the time is unknown).
        var from: Date
        /// The span before it was last seen at this time: the switch
        /// happened between this and `from`. Nil for the first span.
        var after: Date?
        /// The last look that still showed it.
        var lastSeen: Date
    }

    enum Answer: Equatable, Sendable {
        /// The folder ran as this identity then (nil: nobody signed in).
        case identity(String?)
        /// It changed hands around then; which side is unknown.
        case switching
        /// Before anything was recorded.
        case unknown
    }

    /// Spans kept per folder (oldest dropped first).
    static let maxSpans = 24

    private(set) var spans: [Span] = []

    init(spans: [Span] = []) {
        self.spans = spans
    }

    /// The identity the folder names now, if it was ever observed.
    var current: String?? { spans.last.map(\.identity) }

    /// Record a look at the folder: it names `identity`, read from a
    /// `.claude.json` last written at `modifiedAt`.
    ///
    /// - Parameter resumed: the first look after a relaunch. The folder
    ///   wasn't watched meanwhile: if its file was written since the last
    ///   look, the same identity may still have changed hands and back, so
    ///   the time in between is left open.
    /// - Returns: whether a new span started (worth saving).
    @discardableResult
    mutating func observe(_ identity: String?, rawUuid: String? = nil, modifiedAt: Date?, at now: Date,
                          resumed: Bool = false) -> Bool {
        let rawUuid = rawUuid?.lowercased()
        guard let last = spans.last else {
            spans.append(Span(identity: identity, rawUuid: rawUuid, from: min(modifiedAt ?? now, now), after: nil, lastSeen: now))
            return true
        }
        let written = modifiedAt ?? now
        let unwatchedWrite = resumed && written > last.lastSeen
        if last.identity == identity, last.rawUuid == rawUuid, !unwatchedWrite {
            spans[spans.count - 1].lastSeen = max(last.lastSeen, now)
            return false
        }
        spans.append(Span(identity: identity, rawUuid: rawUuid, isLogin: last.rawUuid != rawUuid,
                          from: min(max(written, last.lastSeen), now), after: last.lastSeen, lastSeen: now))
        if spans.count > Self.maxSpans { spans.removeFirst(spans.count - Self.maxSpans) }
        return true
    }

    /// Who the folder ran as at `date` (a process's start time).
    func identity(at date: Date) -> Answer {
        guard let first = spans.first, date >= first.from else { return .unknown }
        for index in spans.indices.reversed() {
            let span = spans[index]
            if date >= span.from {
                // A later login may have taken this process with it.
                return spans[(index + 1)...].contains(where: \.isLogin) ? .switching : .identity(span.identity)
            }
            if let after = span.after, date > after { return .switching }
        }
        return .unknown
    }
}

/// Which account a session in a folder runs as.
nonisolated enum FolderAttribution: Equatable, Sendable {
    /// Known: the identity id (nil: nobody signed in, or a folder the
    /// registry hasn't grouped yet).
    case known(String?)
    /// Can't be told: `~/.claude` while Claude Parallel Profiles mirrors
    /// accounts into it, for a process that started around a switch (or
    /// before anything was recorded). `current` is who it names now.
    case unsure(current: String?)

    /// The identity to show the session under: the known one, else the
    /// folder's current one.
    var bestGuess: String? {
        switch self {
        case .known(let identity): return identity
        case .unsure(let current): return current
        }
    }

    /// Pure: attribute a session in a folder, started at `startedAt`.
    ///
    /// - Parameters:
    ///   - current: the identity the folder names now.
    ///   - mirrored: the folder is `~/.claude` and Claude Parallel Profiles
    ///     mirrors accounts into it: a guess would be wrong on every focus
    ///     switch, so anything uncertain is `unsure`. Other folders are
    ///     attributed to whoever they name now.
    static func attribute(current: String?, timeline: FolderIdentityTimeline?, startedAt: Date?,
                          mirrored: Bool) -> FolderAttribution {
        guard mirrored else { return .known(current) }
        guard let startedAt, let timeline else { return .unsure(current: current) }
        switch timeline.identity(at: startedAt) {
        case .identity(let identity?): return .known(identity)
        case .identity(nil), .switching, .unknown: return .unsure(current: current)
        }
    }
}
