//
//  AttentionNews.swift
//  ClaudeControl
//
//  The one rule for what a change in a session's attention announces, used
//  by both streams built on it (CS-3): the hub's transitions (chime, peek,
//  auto-open, "viewed") and the attention tracker's (banners). Both also
//  share the tracker's launch baseline (`AttentionTracker.isInBaseline`):
//  nothing is news until every account's registry has been read, and after
//  that a session seen for the first time is news like any other change.
//
//  - resolved: it needed input or waited for review, and now does
//    something else, or it went away (`to == nil`) while it did;
//  - needs input: it didn't need input (or wasn't known), or now needs it
//    for another reason; but not a failed turn read back from
//    review-state.json (`SessionState.stopErrorIsRestored`): a session seen
//    again (a relaunch, `--resume`, a reopened editor chat) shows its old
//    failure without announcing it again;
//  - ready for review: it wasn't ready for review, the completion isn't
//    quiet (see `SessionState.completionIsQuiet`), and it didn't finish
//    before this launch (restored, or inferred from a transcript that ended
//    while the app was down).
//

import Foundation

nonisolated enum AttentionNews {
    enum Kind: Equatable, Sendable {
        case needsInput
        case readyForReview
        case resolved
    }

    /// What `from` → `to` announces, in order (resolved first, so "the wait
    /// is over" is never implied). Pure.
    static func kinds(
        from: SessionAttention?,
        to: SessionAttention?,
        isQuietCompletion: Bool,
        completedAt: Date?,
        launchedAt: Date?,
        failureIsRestored: Bool = false
    ) -> [Kind] {
        var kinds: [Kind] = []
        if let from, from.bucket == .needsInput || from.bucket == .readyForReview, to?.bucket != from.bucket {
            kinds.append(.resolved)
        }
        guard let to else { return kinds }
        switch (from, to) {
        case (.needsInput(let was)?, .needsInput(let reason)):
            if was != reason, !(failureIsRestored && reason.isError) { kinds.append(.needsInput) }
        case (_, .needsInput(let reason)):
            if !(failureIsRestored && reason.isError) { kinds.append(.needsInput) }
        case (.readyForReview?, .readyForReview):
            break
        case (_, .readyForReview):
            if isQuietCompletion { break }
            if let launchedAt, let completedAt, completedAt < launchedAt { break }
            kinds.append(.readyForReview)
        default:
            break
        }
        return kinds
    }
}
