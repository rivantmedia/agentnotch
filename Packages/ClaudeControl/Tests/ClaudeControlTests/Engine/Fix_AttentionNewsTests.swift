import Combine
import Foundation
import Testing
@testable import ClaudeControl

/// CS-3: one rule (`AttentionNews`) for what a change announces, and both
/// streams (the hub's for chimes/peeks/auto-open, the tracker's for banners)
/// agree on it, first-seen sessions included.
struct Fix_AttentionNewsTests {
    typealias Kind = AttentionNews.Kind
    let launch = Date(timeIntervalSince1970: 1_800_000_000)

    private func kinds(_ from: SessionAttention?, _ to: SessionAttention?, quiet: Bool = false,
                       completedAt: Date? = nil) -> [Kind] {
        AttentionNews.kinds(from: from, to: to, isQuietCompletion: quiet,
                            completedAt: completedAt ?? launch.addingTimeInterval(60), launchedAt: launch)
    }

    /// The table.
    @Test func theRule() {
        let bash = SessionAttention.needsInput(.permission(tool: "Bash"))
        let edit = SessionAttention.needsInput(.permission(tool: "Edit"))
        #expect(kinds(.working, bash) == [.needsInput])
        #expect(kinds(nil, bash) == [.needsInput])                          // first seen, waiting
        #expect(kinds(bash, edit) == [.needsInput])                         // another request
        #expect(kinds(bash, bash) == [])
        #expect(kinds(bash, .working) == [.resolved])
        #expect(kinds(bash, nil) == [.resolved])                            // went away
        #expect(kinds(bash, .readyForReview) == [.resolved, .readyForReview])
        #expect(kinds(.working, .readyForReview) == [.readyForReview])
        #expect(kinds(nil, .readyForReview) == [.readyForReview])           // first seen, finished now
        #expect(kinds(.working, .readyForReview, quiet: true) == [])
        #expect(kinds(.working, .readyForReview, completedAt: launch.addingTimeInterval(-1)) == [])
        #expect(kinds(.readyForReview, .idle) == [.resolved])
        #expect(kinds(.readyForReview, nil) == [.resolved])
        #expect(kinds(nil, .working) == [])
        #expect(kinds(.idle, .working) == [])
        #expect(kinds(.working, nil) == [])
    }

    private func state(_ id: String, _ attention: SessionAttention) -> SessionState {
        var state = SessionState(sessionId: id, cwd: "/tmp/\(id)")
        switch attention {
        case .working: state.phase = .processing
        case .readyForReview:
            state.phase = .waitingForInput
            state.completedAt = launch.addingTimeInterval(120)
        case .needsInput(let reason):
            state.phase = .processing
            state.needsInputReason = reason
        case .idle: state.phase = .idle
        }
        return state
    }

    private func snapshot(_ state: SessionState) -> ClaudeControlHub.AttentionSnapshot {
        let summary = ClaudeHostProjections.session(state, home: "/Users/x", hostApp: nil, limitHit: nil, now: launch)
        return .init(attention: state.attention, summary: summary, completedAt: state.completedAt,
                     isQuietCompletion: state.completionIsQuiet)
    }

    /// The same changes, after the baseline, give the same announcements on
    /// both streams: a session first seen already waiting, and one that
    /// changes inside what used to be the hub-only window.
    @MainActor
    @Test func bothStreamsAgree() {
        let tracker = AttentionTracker(launchedAt: launch)
        var banners: [String] = []
        let subscription = tracker.transitions.sink { change in
            if change.becameNeedsInput { banners.append("\(change.session.sessionId):needsInput") }
            if change.becameReadyForReview { banners.append("\(change.session.sessionId):readyForReview") }
        }
        defer { subscription.cancel() }
        tracker.initialScanCompleted(now: launch)

        let first = [state("a", .working), state("b", .working)]
        let second = [state("a", .needsInput(.question)), state("b", .readyForReview),
                      state("new", .needsInput(.permission(tool: "Bash")))]
        let afterBaseline = launch.addingTimeInterval(AttentionTracker.settleInterval + 1)
        tracker.update(first, now: afterBaseline)
        tracker.update(second, now: afterBaseline)

        let previous = Dictionary(uniqueKeysWithValues: first.map { ($0.sessionId, snapshot($0)) })
        let hub = ClaudeControlHub.transitions(previous: previous, current: second.map(snapshot), launchedAt: launch)
            .filter { $0.kind != .resolved }
            .map { "\($0.session.id):\($0.kind)" }
        #expect(Set(hub) == Set(banners))
        #expect(Set(hub) == ["a:needsInput", "b:readyForReview", "new:needsInput"])
    }
}
