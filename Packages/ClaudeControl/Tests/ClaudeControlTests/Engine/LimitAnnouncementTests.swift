import Combine
import Foundation
import Testing
@testable import ClaudeControl

/// `LimitAnnouncements` behind a reference, so a claim can sit in `#expect`.
private final class Gate {
    var state = LimitAnnouncements()
    var incidents: [String: [LimitAnnouncements.Incident]] { state.incidents }

    func claim(_ channel: LimitAnnouncements.Channel, ring: String, window: String, resetsAt: Date?, now: Date,
               alsoMarking extra: Set<LimitAnnouncements.Channel> = []) -> Bool {
        state.claim(channel, ring: ring, window: window, resetsAt: resetsAt, now: now, alsoMarking: extra)
    }

    func prune(now: Date) { state.prune(now: now) }
    func learn(ring: String, window: String, resetsAt: Date?, now: Date) {
        state.learn(ring: ring, window: window, resetsAt: resetsAt, now: now)
    }
    func end(ring: String, window: String) { state.end(ring: ring, window: window) }
}

/// A usage limit is announced once per account until it lifts: not again on
/// every retry, wake-up or /loop tick that fails on it, nor for another
/// session stopped by it, a session reopened with its old failure, a relaunch,
/// or a reset time that two usage sources round differently.
struct LimitAnnouncementRuleTests {
    let now = Date(timeIntervalSince1970: 1_800_000_000)
    var resets: Date { now.addingTimeInterval(3 * 3600) }

    @Test func aLimitIsAnnouncedOncePerChannelUntilItsWindowResets() {
        let gate = Gate()
        #expect(gate.claim(.notification, ring: "work", window: "session", resetsAt: resets, now: now))
        // The retry five minutes later, the /loop tick after that.
        #expect(!gate.claim(.notification, ring: "work", window: "session", resetsAt: resets,
                            now: now.addingTimeInterval(300)))
        #expect(!gate.claim(.notification, ring: "work", window: "session", resetsAt: resets,
                            now: now.addingTimeInterval(3600)))
        // The chime and peek are a channel of their own: once too.
        #expect(gate.claim(.reaction, ring: "work", window: "session", resetsAt: resets, now: now.addingTimeInterval(1)))
        #expect(!gate.claim(.reaction, ring: "work", window: "session", resetsAt: resets,
                            now: now.addingTimeInterval(400)))
        // The window reset, and the next one ran out: a new limit.
        let next = resets.addingTimeInterval(5 * 3600)
        #expect(gate.claim(.notification, ring: "work", window: "session", resetsAt: next,
                           now: resets.addingTimeInterval(60)))
    }

    @Test func accountsAndWindowsAreApart() {
        let gate = Gate()
        #expect(gate.claim(.notification, ring: "work", window: "session", resetsAt: resets, now: now))
        #expect(gate.claim(.notification, ring: "personal", window: "session", resetsAt: resets, now: now))
        // The week running out too is news of its own.
        #expect(gate.claim(.notification, ring: "work", window: "weekly",
                           resetsAt: now.addingTimeInterval(4 * 86_400), now: now))
    }

    /// A turn fails on the limit before the readings say which window ran
    /// out: that incident covers whichever window is named next, and takes
    /// its reset time.
    @Test func aLimitOfAnUnknownWindowLearnsItsWindow() {
        let gate = Gate()
        #expect(gate.claim(.notification, ring: "work", window: LimitAnnouncements.unknownWindow,
                           resetsAt: nil, now: now))
        // Codenotch's "limit reached" for the 5-hour window, seconds later.
        #expect(!gate.claim(.notification, ring: "work", window: "session", resetsAt: resets,
                            now: now.addingTimeInterval(5)))
        #expect(gate.incidents["work"] == [
            .init(window: "session", resetsAt: resets, startedAt: now, channels: [.notification]),
        ])
        // It lasts until that reset now, not the unknown lifetime.
        #expect(!gate.claim(.notification, ring: "work", window: "session", resetsAt: resets,
                            now: now.addingTimeInterval(2 * 3600)))
        // A failure that names no window is covered by a known incident.
        #expect(!gate.claim(.notification, ring: "work", window: LimitAnnouncements.unknownWindow,
                            resetsAt: nil, now: now.addingTimeInterval(2 * 3600)))
    }

    @Test func aLimitNoReadingShowsLastsAnHour() {
        let gate = Gate()
        let unknown = LimitAnnouncements.unknownWindow
        #expect(gate.claim(.notification, ring: "work", window: unknown, resetsAt: nil, now: now))
        #expect(!gate.claim(.notification, ring: "work", window: unknown, resetsAt: nil,
                            now: now.addingTimeInterval(59 * 60)))
        #expect(gate.claim(.notification, ring: "work", window: unknown, resetsAt: nil,
                           now: now.addingTimeInterval(LimitAnnouncements.unknownLifetime + 1)))
        // A reset time already past says nothing about how long it lasts.
        let other = Gate()
        #expect(other.claim(.notification, ring: "work", window: "session",
                            resetsAt: now.addingTimeInterval(-60), now: now))
        #expect(other.incidents["work"]?.first?.resetsAt == nil)
    }

    /// Codenotch's card brings its own sound: it takes the chime and peek
    /// with it, and a banner after it stays away.
    @Test func aNotificationCanTakeTheReactionWithIt() {
        let gate = Gate()
        #expect(gate.claim(.notification, ring: "work", window: "session", resetsAt: resets, now: now,
                           alsoMarking: [.reaction]))
        #expect(!gate.claim(.reaction, ring: "work", window: "session", resetsAt: resets, now: now))
        #expect(!gate.claim(.notification, ring: "work", window: "session", resetsAt: resets, now: now))
    }

    /// A failure that came before any reading named a window stands for the
    /// next window named only while the readings may still be catching up;
    /// a window that runs out later is a limit of its own.
    @Test func anUnknownLimitAbsorbsOnlyWhileTheReadingsCatchUp() {
        let gate = Gate()
        #expect(gate.claim(.notification, ring: "work", window: LimitAnnouncements.unknownWindow,
                           resetsAt: nil, now: now))
        let later = now.addingTimeInterval(LimitAnnouncements.unknownAbsorbsFor + 60)
        #expect(gate.claim(.notification, ring: "work", window: "session", resetsAt: resets.addingTimeInterval(3600),
                           now: later))
        #expect(!gate.claim(.notification, ring: "work", window: "session", resetsAt: resets.addingTimeInterval(3600),
                            now: later.addingTimeInterval(60)))
    }

    /// The readings name the window of a limit announced before they could:
    /// it lasts until that window resets, not an hour.
    @Test func theReadingsTeachAnUnknownLimitItsWindow() {
        let gate = Gate()
        let unknown = LimitAnnouncements.unknownWindow
        #expect(gate.claim(.notification, ring: "work", window: unknown, resetsAt: nil, now: now))
        #expect(gate.claim(.reaction, ring: "work", window: unknown, resetsAt: nil, now: now))
        gate.learn(ring: "work", window: "session", resetsAt: resets, now: now.addingTimeInterval(300))
        // Past the unknown lifetime, before the reset: still the same limit.
        let retry = now.addingTimeInterval(LimitAnnouncements.unknownLifetime + 20 * 60)
        #expect(!gate.claim(.notification, ring: "work", window: "session", resetsAt: resets, now: retry))
        #expect(!gate.claim(.reaction, ring: "work", window: unknown, resetsAt: nil, now: retry))
        // Too late to learn: a reading long after the failure names a limit of its own.
        let other = Gate()
        #expect(other.claim(.notification, ring: "work", window: unknown, resetsAt: nil, now: now))
        other.learn(ring: "work", window: "session", resetsAt: resets,
                    now: now.addingTimeInterval(LimitAnnouncements.unknownAbsorbsFor + 60))
        #expect(other.incidents["work"]?.map(\.window) == [unknown])
    }

    /// An early reset (a reset credit, `/limit-reset`) keeps the reset time:
    /// using the window up again is announced again.
    @Test func anEarlyResetMakesTheNextHitNews() {
        let gate = Gate()
        let friday = now.addingTimeInterval(4 * 86_400)
        #expect(gate.claim(.notification, ring: "work", window: "weekly", resetsAt: friday, now: now))
        #expect(!gate.claim(.notification, ring: "work", window: "weekly", resetsAt: friday, now: now))
        gate.end(ring: "work", window: "weekly")
        #expect(gate.claim(.notification, ring: "work", window: "weekly", resetsAt: friday,
                           now: now.addingTimeInterval(86_400)))
    }

    /// Codenotch's card with its sound off leaves the chime to the session.
    @Test func aSilentCardLeavesTheChime() {
        let gate = Gate()
        #expect(gate.claim(.notification, ring: "work", window: "session", resetsAt: resets, now: now, alsoMarking: []))
        #expect(gate.claim(.reaction, ring: "work", window: "session", resetsAt: resets, now: now))
    }

    @Test func expiredIncidentsArePruned() {
        let gate = Gate()
        _ = gate.claim(.notification, ring: "work", window: "session", resetsAt: resets, now: now)
        gate.prune(now: resets)
        #expect(gate.incidents.isEmpty)
    }
}

/// The record survives a relaunch, in the engine's own folder only.
@MainActor
struct LimitAnnouncementStoreTests {
    let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func folder() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("agentnotch-limits-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    @Test func aRelaunchDoesNotAnnounceTheSameLimitAgain() throws {
        let dir = try folder()
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent(LimitAnnouncementStore.fileName)
        let resets = now.addingTimeInterval(3600)

        let first = LimitAnnouncementStore(fileURL: file, createsFolder: false, now: now)
        #expect(first.claim(.notification, ring: "work", window: .session, resetsAt: resets, now: now))
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600)

        let second = LimitAnnouncementStore(fileURL: file, createsFolder: false, now: now.addingTimeInterval(60))
        #expect(!second.claim(.notification, ring: "work", window: .session, resetsAt: resets,
                              now: now.addingTimeInterval(60)))
        // Once the window has reset, the next launch has nothing left.
        let later = LimitAnnouncementStore(fileURL: file, createsFolder: false, now: resets.addingTimeInterval(1))
        #expect(later.state.incidents.isEmpty)
    }

    @Test func anUnreadableFileStartsAfresh() throws {
        let dir = try folder()
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent(LimitAnnouncementStore.fileName)
        try Data("not json".utf8).write(to: file)
        let store = LimitAnnouncementStore(fileURL: file, createsFolder: false, now: now)
        #expect(store.state == LimitAnnouncements())
        #expect(store.claim(.notification, ring: "work", window: nil, resetsAt: nil, now: now))
    }

    @Test func aSealedRunKeepsItInMemory() throws {
        let dir = try folder()
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent(LimitAnnouncementStore.fileName)
        let store = LimitAnnouncementStore(fileURL: file, persists: false, createsFolder: false, now: now)
        #expect(store.claim(.notification, ring: "work", window: .weekly, resetsAt: nil, now: now))
        #expect(!FileManager.default.fileExists(atPath: file.path))
    }
}

/// A failed turn read back from review-state.json (the session reopened,
/// resumed, or seen again after a relaunch) shows, but isn't news.
struct RestoredFailureTests {
    private let account: TemporaryAccount
    private let transcript: String
    let launch = Date(timeIntervalSince1970: 1_800_000_000)

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-restored-failure")
        transcript = account.transcript("s1")
    }

    private func event(_ name: String, at date: Date, stopError: String? = nil) -> SessionEvent {
        .hookReceived(HookEvent(sessionId: "s1", event: name, status: "waiting_for_input", cwd: "/tmp/proj",
                                transcriptPath: transcript, attended: true, entrypoint: "cli",
                                stopError: stopError, source: name == "UserPromptSubmit" ? "user" : nil,
                                prompt: name == "UserPromptSubmit" ? "go" : nil, receivedAt: date))
    }

    @Test func theRule() {
        let limited = SessionAttention.needsInput(.error("Rate limited"))
        func kinds(_ from: SessionAttention?, restored: Bool) -> [AttentionNews.Kind] {
            AttentionNews.kinds(from: from, to: limited, isQuietCompletion: false, completedAt: nil,
                                launchedAt: launch, failureIsRestored: restored)
        }
        #expect(kinds(nil, restored: true) == [])
        #expect(kinds(.working, restored: true) == [])
        #expect(kinds(.needsInput(.question), restored: true) == [])
        #expect(kinds(.readyForReview, restored: true) == [.resolved])
        // Seen happen: news as before.
        #expect(kinds(nil, restored: false) == [.needsInput])
        #expect(kinds(.working, restored: false) == [.needsInput])
        // Only failures: anything else restored is still news.
        #expect(AttentionNews.kinds(from: nil, to: .needsInput(.question), isQuietCompletion: false,
                                    completedAt: nil, launchedAt: launch, failureIsRestored: true) == [.needsInput])
    }

    @MainActor
    @Test func aSessionSeenAgainKeepsItsFailureButNotAsNews() async throws {
        let reviews = ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false)
        let first = SessionStore.forTests(reviewStore: reviews, effects: .none, completionTiming: .immediate)
        let t0 = Date()
        await first.process(event("UserPromptSubmit", at: t0))
        await first.process(event("StopFailure", at: t0.addingTimeInterval(1), stopError: "rate_limit"))
        let live = try #require(await first.session(for: "s1"))
        #expect(live.hasFailedTurn && !live.stopErrorIsRestored)
        reviews.flush()

        // Seen again (the next launch, or a resumed session): restored.
        let second = SessionStore.forTests(
            reviewStore: ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            effects: .none, completionTiming: .immediate)
        await second.process(.registrySnapshot(configDir: account.configDir.path, entries: [
            SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", status: "idle",
                                 waitingFor: nil, statusUpdatedAt: t0.addingTimeInterval(3)),
        ]))
        let restored = try #require(await second.session(for: "s1"))
        #expect(restored.hasFailedTurn && restored.stopErrorIsRestored)

        // The tracker, past its baseline, says nothing about it.
        let tracker = AttentionTracker(launchedAt: launch)
        var news: [AttentionTransition] = []
        let subscription = tracker.transitions.sink { news.append($0) }
        defer { subscription.cancel() }
        let after = launch.addingTimeInterval(AttentionTracker.settleInterval + 1)
        tracker.initialScanCompleted(now: launch)
        tracker.update([restored], now: after)
        #expect(news.isEmpty)

        // The same session tried again and failed again, live: news, and no
        // longer restored.
        await second.process(event("UserPromptSubmit", at: t0.addingTimeInterval(10)))
        tracker.update([try #require(await second.session(for: "s1"))], now: after.addingTimeInterval(1))
        await second.process(event("StopFailure", at: t0.addingTimeInterval(11), stopError: "rate_limit"))
        let again = try #require(await second.session(for: "s1"))
        #expect(again.hasFailedTurn && !again.stopErrorIsRestored)
        tracker.update([again], now: after.addingTimeInterval(2))
        #expect(news.filter(\.becameNeedsInput).map(\.to) == [again.attention])
    }
}

/// Which window a limit is announced under, and when the readings say it
/// lifted early.
struct LimitWindowTests {
    let now = Date(timeIntervalSince1970: 1_800_000_000)

    /// A model's week used up days ago doesn't stand in for today's 5-hour
    /// limit: a failed turn is announced under the window that stops every model.
    @Test func theFiveHourOrWeeklyWindowComesBeforeAModelsOwn() {
        let opus = ScopedUsage(name: "Opus", window: UsageWindow(utilization: 100, resetsAt: now.addingTimeInterval(4 * 86_400),
                                                                 duration: UsageWindow.weeklyDuration))
        var usage = AccountUsage(accountId: "a",
                                 fiveHour: UsageWindow(utilization: 100, resetsAt: now.addingTimeInterval(7200),
                                                       duration: UsageWindow.sessionDuration),
                                 sevenDay: UsageWindow(utilization: 60, resetsAt: now.addingTimeInterval(3 * 86_400),
                                                       duration: UsageWindow.weeklyDuration),
                                 scoped: [opus], source: .probe, updatedAt: now)
        #expect(usage.limitHit(now: now)?.window == .scoped("Opus"))
        #expect(usage.announcedLimitHit(now: now) == UsageLimitHit(window: .session, resetsAt: now.addingTimeInterval(7200)))
        // Only a model's own window used up: that one.
        usage.fiveHour?.utilization = 40
        #expect(usage.announcedLimitHit(now: now)?.window == .scoped("Opus"))
        // Both general windows: the one that lifts last.
        usage.fiveHour?.utilization = 100
        usage.sevenDay?.utilization = 100
        #expect(usage.announcedLimitHit(now: now)?.window == .weekly)
    }

    @Test func aFreshReadingUnderTheLimitLiftsItsWindow() {
        func reading(session: Double, weekly: Double, updated: Date?) -> ClaudeRingReading {
            ClaudeRingReading(windows: [
                .init(id: "session", usedFraction: session, resetsAt: now.addingTimeInterval(3600), duration: 18_000),
                .init(id: "weekly_all", usedFraction: weekly, resetsAt: now.addingTimeInterval(86_400), duration: 604_800),
                .init(id: "weekly_opus", usedFraction: 0.1, resetsAt: now.addingTimeInterval(86_400), duration: 604_800),
            ], updatedAt: updated, status: .ok)
        }
        #expect(ClaudeControlHub.liftedLimitWindows(reading(session: 0.2, weekly: 1, updated: now), now: now) == [.session])
        #expect(ClaudeControlHub.liftedLimitWindows(reading(session: 0.96, weekly: 0.5, updated: now), now: now) == [.weekly])
        // A stale reading, or none yet, proves nothing.
        let stale = now.addingTimeInterval(-2 * ClaudeRingReading.staleAfter)
        #expect(ClaudeControlHub.liftedLimitWindows(reading(session: 0.2, weekly: 0.2, updated: stale), now: now).isEmpty)
        #expect(ClaudeControlHub.liftedLimitWindows(reading(session: 0.2, weekly: 0.2, updated: nil), now: now).isEmpty)
    }
}

/// The hub's chime and peek for a turn stopped by the limit: once per
/// account and limit.
struct LimitReactionTests {
    let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func snapshot(_ id: String, ring: String, _ attention: SessionAttention,
                          limitHit: UsageLimitHit? = nil) -> ClaudeControlHub.AttentionSnapshot {
        var state = SessionState(sessionId: id, cwd: "/tmp/\(id)")
        state.phase = .processing
        state.needsInputReason = attention.needsInputReason
        var summary = ClaudeHostProjections.session(state, home: "/Users/x", hostApp: nil, limitHit: limitHit, now: now)
        summary.ringID = ring
        return .init(attention: attention, summary: summary, limitHit: limitHit)
    }

    @Test func onlyTheFirstFailureOfALimitChimes() {
        let limited = SessionAttention.needsInput(.error(NeedsInputReason.humanizedStopError("rate_limit")))
        let hit = UsageLimitHit(window: .session, resetsAt: now.addingTimeInterval(3600))
        let snapshots = [
            snapshot("a", ring: "work", limited, limitHit: hit),
            snapshot("b", ring: "work", limited, limitHit: hit),
            snapshot("c", ring: "personal", limited),
            snapshot("d", ring: "work", .needsInput(.question)),
        ]
        let byID = Dictionary(uniqueKeysWithValues: snapshots.map { ($0.summary.id, $0) })
        let transitions = snapshots.map { ClaudeAttentionTransition(kind: .needsInput, session: $0.summary) }
        let gate = Gate()
        func claim(_ ring: String, _ hit: UsageLimitHit?) -> Bool {
            gate.claim(.reaction, ring: ring, window: LimitAnnouncements.windowKey(hit?.window),
                       resetsAt: hit?.resetsAt, now: now)
        }
        let first = ClaudeControlHub.withoutRepeatedLimitReactions(transitions, snapshots: byID, claim: claim)
        // One per account; a question is never held back.
        #expect(first.map(\.session.id) == ["a", "c", "d"])
        // The retry that fails again: nothing, the question still comes through.
        let retry = ClaudeControlHub.withoutRepeatedLimitReactions(transitions, snapshots: byID, claim: claim)
        #expect(retry.map(\.session.id) == ["d"])
        // Resolutions always pass.
        let resolved = [ClaudeAttentionTransition(kind: .resolved, session: snapshots[0].summary)]
        #expect(ClaudeControlHub.withoutRepeatedLimitReactions(resolved, snapshots: byID, claim: claim).count == 1)
    }
}

/// One reset time per window, whichever source reported it last.
struct RingResetTimeTests {
    let base = Date(timeIntervalSince1970: 1_790_254_200)

    private func reading(_ windows: [(String, Date?, TimeInterval?)]) -> ClaudeRingReading {
        ClaudeRingReading(windows: windows.map { id, resetsAt, duration in
            .init(id: id, usedFraction: 1, resetsAt: resetsAt, duration: duration)
        }, status: .ok)
    }

    @Test func roundingBetweenSourcesKeepsTheFirstResetTime() {
        let previous = reading([("session", base, 18_000), ("weekly_all", base.addingTimeInterval(86_400), 604_800)])
        // The usage endpoint's fractional seconds after the status line's whole ones, and back.
        let later = reading([("session", base.addingTimeInterval(0.257626), 18_000),
                             ("weekly_all", base.addingTimeInterval(86_399.6), 604_800)])
        let kept = UsageRingWindows.keepingResetTimes(later, previous: previous)
        #expect(kept.windows.map(\.resetsAt) == [base, base.addingTimeInterval(86_400)])
        let earlier = reading([("session", base.addingTimeInterval(-0.4), 18_000)])
        #expect(UsageRingWindows.keepingResetTimes(earlier, previous: previous).windows.first?.resetsAt == base)
    }

    @Test func aNewWindowPassesThrough() {
        let previous = reading([("session", base, 18_000)])
        let next = reading([("session", base.addingTimeInterval(18_000), 18_000)])
        #expect(UsageRingWindows.keepingResetTimes(next, previous: previous) == next)
        // Nothing to keep: no previous reading, a window it didn't have, no reset time.
        #expect(UsageRingWindows.keepingResetTimes(next, previous: nil) == next)
        let other = reading([("weekly_opus", base.addingTimeInterval(0.3), 604_800)])
        #expect(UsageRingWindows.keepingResetTimes(other, previous: previous) == other)
        let unknown = reading([("session", nil, 18_000)])
        #expect(UsageRingWindows.keepingResetTimes(unknown, previous: previous) == unknown)
    }

    @Test func aWindowOfUnknownLengthUsesAMinute() {
        let previous = reading([("session", base, nil)])
        let jitter = reading([("session", base.addingTimeInterval(0.5), nil)])
        #expect(UsageRingWindows.keepingResetTimes(jitter, previous: previous).windows.first?.resetsAt == base)
        let moved = reading([("session", base.addingTimeInterval(120), nil)])
        #expect(UsageRingWindows.keepingResetTimes(moved, previous: previous) == moved)
    }
}
