//
//  AnswerGate.swift
//  ClaudeControl
//
//  Keeps one click from answering two requests. Claude Code queues parallel
//  permission requests in one session, and the next one takes the row's
//  place (same buttons, same spot) the moment the first is answered. So:
//
//  - a request can be answered once from the panel: a double-click, or a
//    click and ⌘⏎, sends one answer;
//  - a request that has just appeared can't be answered for a moment, so a
//    click meant for the request it replaced never lands on it.
//
//  Pure: the clock comes in, the panel keeps the value.
//

import Foundation

nonisolated struct AnswerGate: Equatable, Sendable {
    /// How long a newly shown request waits before it can be answered.
    static let armDelay: TimeInterval = 0.35

    /// How long an answered request is remembered after its answer. The
    /// engine resolves a request within moments; this only has to outlast a
    /// request lingering on screen after it was answered, so leaving the
    /// list (for a chat) and coming back can't arm it again.
    static let answeredMemory: TimeInterval = 600

    /// When each request on screen was first shown.
    private(set) var firstShown: [String: Date] = [:]
    /// Requests already answered from the panel, and when.
    private(set) var answered: [String: Date] = [:]

    /// Record the requests on screen now. New ones start their wait; ones no
    /// longer shown are forgotten, except that an answered request stays
    /// answered (for `answeredMemory`) wherever it shows up again.
    mutating func noteShown<S: Sequence>(_ toolUseIds: S, now: Date) where S.Element == String {
        let shown = Set(toolUseIds)
        firstShown = firstShown.filter { shown.contains($0.key) }
        answered = answered.filter { now.timeIntervalSince($0.value) < Self.answeredMemory }
        for id in shown where firstShown[id] == nil {
            firstShown[id] = now
        }
    }

    /// Record requests as shown long ago: snapshots, and requests already on
    /// screen when the panel opened (they were visible elsewhere first).
    mutating func noteShownArmed<S: Sequence>(_ toolUseIds: S) where S.Element == String {
        for id in toolUseIds where firstShown[id] == nil {
            firstShown[id] = .distantPast
        }
    }

    /// Whether `toolUseId` can be answered at `now`.
    func isArmed(_ toolUseId: String, now: Date) -> Bool {
        guard answered[toolUseId] == nil, let shown = firstShown[toolUseId] else { return false }
        return now.timeIntervalSince(shown) >= Self.armDelay
    }

    /// Take the one answer `toolUseId` gets. False when it was answered
    /// already, isn't on screen, or appeared too recently.
    mutating func claim(_ toolUseId: String, now: Date) -> Bool {
        guard isArmed(toolUseId, now: now) else { return false }
        answered[toolUseId] = now
        return true
    }

    /// When the next request on screen becomes answerable, if one is waiting.
    func nextArming(after now: Date) -> Date? {
        firstShown.filter { answered[$0.key] == nil }.values
            .map { $0.addingTimeInterval(Self.armDelay) }
            .filter { $0 > now }
            .min()
    }
}
