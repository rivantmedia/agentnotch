import Foundation
import Testing
@testable import ClaudeControl

// MARK: - Fixture

/// A throwaway home with one signed-in account, a stand-in probe (never
/// `claude`) and a stand-in Claude Desktop cache. Nothing outside the
/// temporary folder is read or written.
@MainActor
private final class UsageFixture {
    let root: URL
    let configDir: URL
    let account: ClaudeAccount
    let registry: AccountRegistry
    let stateStore: UsageStateStore
    let probe = ProbeStub()
    let desktop = DesktopStub()
    let now = Date()

    init() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("agentnotch-usage-\(UUID().uuidString)")
        configDir = root.appendingPathComponent("home/.claude-work", isDirectory: true)
        try FileManager.default.createDirectory(at: configDir, withIntermediateDirectories: true)
        account = ClaudeAccount(configDir: configDir.path, configDirEnv: configDir.path, email: "me@x.dev", accountUuid: "acc-1")
        registry = AccountRegistry(home: root.appendingPathComponent("home").path,
                                   storeURL: root.appendingPathComponent("support/accounts.json"),
                                   configReader: ClaudeGlobalConfigReader())
        registry.replaceAllWithFixtures([account])
        stateStore = UsageStateStore(directory: root.appendingPathComponent("support", isDirectory: true))
    }

    deinit {
        try? FileManager.default.removeItem(at: root)
    }

    /// The account's id in the store: its identity (usage is kept per
    /// signed-in identity, whatever folders it lives in).
    var id: String { "uuid:acc-1" }
    static let sideID = "uuid:acc-2"

    /// `waitLimit` is generous by default: a refresh returns once its probe
    /// answered, and on a busy machine running the whole suite the fake
    /// probe's hops can take longer than a few seconds. Tests of the limit
    /// itself pass their own.
    func makeStore(desktopOn: Bool = true, waitLimit: TimeInterval = 30) -> UsageStore {
        UsageStore(
            registry: registry,
            configReader: ClaudeGlobalConfigReader(),
            stateStore: stateStore,
            externalSource: desktop,
            readsExternalUsage: { desktopOn },
            probeRunner: { [probe] request in await probe.run(request) },
            automaticProbes: true,
            refreshWaitLimit: waitLimit
        )
    }

    /// `.claude.json`: signed in, in organization `org-1`, optionally with
    /// Claude Code's cached usage.
    func writeGlobalConfig(cachedSession: Double? = nil, fetchedAt: Date? = nil) throws {
        var json: [String: Any] = [
            "oauthAccount": ["accountUuid": "acc-1", "emailAddress": "me@x.dev", "organizationUuid": "org-1"],
        ]
        if let cachedSession, let fetchedAt {
            json["cachedUsageUtilization"] = [
                "accountUuid": "acc-1",
                "fetchedAtMs": fetchedAt.timeIntervalSince1970 * 1000,
                "utilization": body(session: cachedSession),
            ]
        }
        let data = try JSONSerialization.data(withJSONObject: json)
        try data.write(to: configDir.appendingPathComponent(".claude.json"))
    }

    /// A second signed-in account (`~/.claude-side`), next to the first.
    func addSideAccount() throws -> ClaudeAccount {
        let sideDir = root.appendingPathComponent("home/.claude-side", isDirectory: true)
        try FileManager.default.createDirectory(at: sideDir, withIntermediateDirectories: true)
        try JSONSerialization.data(withJSONObject: ["oauthAccount": ["accountUuid": "acc-2", "emailAddress": "me@y.dev"]])
            .write(to: sideDir.appendingPathComponent(".claude.json"))
        let side = ClaudeAccount(configDir: sideDir.path, configDirEnv: sideDir.path, email: "me@y.dev", accountUuid: "acc-2")
        registry.replaceAllWithFixtures([account, side])
        return side
    }

    /// A usage body with a session and a weekly window.
    func body(session: Double) -> [String: Any] {
        [
            "five_hour": ["utilization": session, "resets_at": iso(now.addingTimeInterval(3 * 3600))],
            "seven_day": ["utilization": 10, "resets_at": iso(now.addingTimeInterval(3 * 86400))],
        ]
    }

    func parsed(session: Double, seeded: Bool = false) -> ParsedUsage {
        var usage = UsageParser.parseUsageBody(body(session: session))
        usage.isPossiblySeeded = seeded
        return usage
    }

    func desktopReading(session: Double, observedAt: Date) -> ClaudeExternalUsageReading {
        ClaudeExternalUsageReading(windows: [
            .init(id: "session", usedFraction: session / 100, resetsAt: now.addingTimeInterval(3 * 3600), duration: 18_000),
            .init(id: "weekly_all", usedFraction: 0.1, resetsAt: now.addingTimeInterval(3 * 86400), duration: 604_800),
        ], observedAt: observedAt)
    }

    private func iso(_ date: Date) -> String {
        date.formatted(.iso8601)
    }

    /// Until the running probe (if any) has finished.
    func settle(_ store: UsageStore) async throws {
        for _ in 0..<250 where store.isProbing {
            try await Task.sleep(for: .milliseconds(20))
        }
    }
}

/// Stands in for `claude -p` / `get_usage`. `hold()` keeps the next
/// answers back until `release()`.
private final class ProbeStub: @unchecked Sendable {
    private let lock = NSLock()
    private var outcome: UsageProbe.Outcome = .failed("unset")
    private var holding = false
    private var waiting: [CheckedContinuation<Void, Never>] = []
    private(set) var requests: [UsageStore.ProbeRequest] = []

    var count: Int { lock.withLock { requests.count } }

    func answer(_ outcome: UsageProbe.Outcome) {
        lock.withLock { self.outcome = outcome }
    }

    func hold() {
        lock.withLock { holding = true }
    }

    func release() {
        let resumed = lock.withLock { () -> [CheckedContinuation<Void, Never>] in
            holding = false
            defer { waiting.removeAll() }
            return waiting
        }
        resumed.forEach { $0.resume() }
    }

    func run(_ request: UsageStore.ProbeRequest) async -> UsageProbe.Outcome {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            let held = lock.withLock { () -> Bool in
                requests.append(request)
                if holding { waiting.append(continuation) }
                return holding
            }
            if !held { continuation.resume() }
        }
        return lock.withLock { outcome }
    }
}

/// Stands in for Claude Desktop's cache.
private final class DesktopStub: ClaudeExternalUsageSource, @unchecked Sendable {
    private let lock = NSLock()
    private var reading: ClaudeExternalUsageReading?
    private(set) var organizations: [String] = []

    var count: Int { lock.withLock { organizations.count } }

    func set(_ reading: ClaudeExternalUsageReading?) {
        lock.withLock { self.reading = reading }
    }

    func reading(organizationUuid: String, now: Date) async -> ClaudeExternalUsageReading? {
        lock.withLock {
            organizations.append(organizationUuid)
            return reading
        }
    }
}

// MARK: - Claude Desktop merge

@MainActor
@Suite(.serialized)
struct A3_ExternalUsageMergeTests {
    @Test func aNewerDesktopReadingWinsAndHoldsOffTheProbe() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig(cachedSession: 20, fetchedAt: fixture.now.addingTimeInterval(-20 * 60))
        fixture.desktop.set(fixture.desktopReading(session: 50, observedAt: fixture.now.addingTimeInterval(-10)))
        fixture.probe.answer(.usage(fixture.parsed(session: 99)))
        let store = fixture.makeStore()

        await store.pollCycle()
        try await fixture.settle(store)

        let usage = try #require(store.usage[fixture.id])
        #expect(usage.fiveHour?.utilization == 50)
        #expect(usage.updatedAt == fixture.now.addingTimeInterval(-10))
        #expect(store.organizationUuids[fixture.id] == "org-1")
        #expect(fixture.desktop.organizations == ["org-1"])
        // Fresh from Desktop: Claude Code isn't asked.
        #expect(fixture.probe.count == 0)
    }

    @Test func anOlderDesktopReadingLoses() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig(cachedSession: 20, fetchedAt: fixture.now.addingTimeInterval(-60))
        fixture.desktop.set(fixture.desktopReading(session: 50, observedAt: fixture.now.addingTimeInterval(-600)))
        let store = fixture.makeStore()

        await store.pollCycle()
        try await fixture.settle(store)

        let usage = try #require(store.usage[fixture.id])
        #expect(usage.fiveHour?.utilization == 20)
        #expect(usage.updatedAt.timeIntervalSince(fixture.now.addingTimeInterval(-60)) < 0.01)
        #expect(fixture.probe.count == 0)
    }

    @Test func withNothingFreshTheProbeRuns() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig(cachedSession: 20, fetchedAt: fixture.now.addingTimeInterval(-20 * 60))
        fixture.desktop.set(nil)
        fixture.probe.answer(.usage(fixture.parsed(session: 33)))
        let store = fixture.makeStore()

        await store.pollCycle()
        try await fixture.settle(store)

        #expect(fixture.probe.count == 1)
        #expect(fixture.probe.requests.first?.configDirEnv == fixture.configDir.path)
        #expect(store.usage[fixture.id]?.fiveHour?.utilization == 33)
        #expect(store.usage[fixture.id]?.source == .probe)
    }

    @Test func desktopIsLeftAloneWhenSwitchedOffOrPaused() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.desktop.set(fixture.desktopReading(session: 50, observedAt: fixture.now))
        fixture.probe.answer(.usage(fixture.parsed(session: 1)))

        let off = fixture.makeStore(desktopOn: false)
        await off.pollCycle()
        try await fixture.settle(off)
        #expect(fixture.desktop.count == 0)
        // Without the Desktop setting the organization isn't read at all.
        #expect(off.organizationUuids.isEmpty)

        let paused = fixture.makeStore()
        paused.setPausedAccounts([fixture.id])
        await paused.pollCycle()
        try await fixture.settle(paused)
        #expect(fixture.desktop.count == 0)
        #expect(fixture.probe.count == 1, "only the unpaused store with Desktop off probed")
    }

    @Test func desktopPollCadence() {
        let now = Date()
        #expect(UsageStore.isExternalPollDue(lastPollAt: nil, lastWasMiss: false, now: now))
        #expect(!UsageStore.isExternalPollDue(lastPollAt: now.addingTimeInterval(-30), lastWasMiss: false, now: now))
        #expect(UsageStore.isExternalPollDue(lastPollAt: now.addingTimeInterval(-60), lastWasMiss: false, now: now))
        #expect(!UsageStore.isExternalPollDue(lastPollAt: now.addingTimeInterval(-120), lastWasMiss: true, now: now))
        #expect(UsageStore.isExternalPollDue(lastPollAt: now.addingTimeInterval(-300), lastWasMiss: true, now: now))
    }
}

// MARK: - usage-state.json

@MainActor
@Suite(.serialized)
struct A3_UsageStatePersistenceTests {
    @Test func stateRoundTripsThroughTheFile() throws {
        let fixture = try UsageFixture()
        let at = Date(timeIntervalSince1970: 1_800_000_000)
        var state = UsageState()
        state.accounts["a"] = .init(
            lastProbeAt: at,
            failureCount: 2,
            nextAttemptAt: at.addingTimeInterval(240),
            lastFullReading: AccountUsage(accountId: "a", fiveHour: UsageWindow(utilization: 12, resetsAt: at, duration: 18_000),
                                          source: .probe, updatedAt: at)
        )
        fixture.stateStore.saveNow(state)
        #expect(fixture.stateStore.load() == state)
        let attributes = try FileManager.default.attributesOfItem(atPath: fixture.stateStore.fileURL.path)
        #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600)
        // A damaged file is only a cache: start empty.
        try Data("{".utf8).write(to: fixture.stateStore.fileURL)
        #expect(fixture.stateStore.load() == UsageState())
    }

    @Test func restoringDropsGoneAccountsAndAncientReadings() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        var state = UsageState()
        state.accounts["kept"] = .init(lastProbeAt: now, lastFullReading: AccountUsage(accountId: "kept", source: .probe, updatedAt: now))
        state.accounts["old"] = .init(lastFullReading: AccountUsage(accountId: "old", source: .probe, updatedAt: now.addingTimeInterval(-9 * 86400)))
        state.accounts["gone"] = .init(lastProbeAt: now)
        let restored = state.restored(knownAccountIds: ["kept", "old"], now: now)
        #expect(Set(restored.accounts.keys) == ["kept"])
        #expect(state.restored(knownAccountIds: nil, now: now).accounts["gone"] != nil)
    }

    @Test func aRelaunchNeitherProbesEarlyNorStartsEmpty() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.probe.answer(.usage(fixture.parsed(session: 70)))
        let id = fixture.id
        // The last run probed a minute ago; its reading is 20 minutes old.
        var state = UsageState()
        let old = AccountUsage(accountId: id, fiveHour: UsageWindow(utilization: 44, resetsAt: fixture.now.addingTimeInterval(3600), duration: 18_000),
                               source: .probe, updatedAt: fixture.now.addingTimeInterval(-20 * 60))
        state.accounts[id] = .init(lastProbeAt: fixture.now.addingTimeInterval(-60), lastFullReading: old)
        fixture.stateStore.saveNow(state)

        let store = fixture.makeStore(desktopOn: false)
        store.restoreState()
        #expect(store.usage[id]?.fiveHour?.utilization == 44)
        await store.pollCycle()
        try await fixture.settle(store)
        #expect(fixture.probe.count == 0)
    }

    @Test func aBackoffSurvivesARelaunch() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.probe.answer(.usage(fixture.parsed(session: 70)))
        let id = fixture.id
        var state = UsageState()
        state.accounts[id] = .init(lastProbeAt: fixture.now.addingTimeInterval(-3600), failureCount: 3,
                                   nextAttemptAt: fixture.now.addingTimeInterval(600))
        fixture.stateStore.saveNow(state)

        let store = fixture.makeStore(desktopOn: false)
        store.restoreState()
        await store.pollCycle()
        try await fixture.settle(store)
        #expect(fixture.probe.count == 0)
    }

    @Test func probesAreRecordedForTheNextRun() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.probe.answer(.usage(fixture.parsed(session: 61)))
        let store = fixture.makeStore(desktopOn: false)
        await store.refresh(accountId: fixture.id, reason: .forced)
        #expect(fixture.probe.count == 1)
        store.saveStateNow()

        let saved = try #require(fixture.stateStore.load().accounts[fixture.id])
        #expect(saved.lastProbeAt != nil)
        #expect(saved.failureCount == 0)
        #expect(saved.lastFullReading?.fiveHour?.utilization == 61)
    }
}

// MARK: - Refresh policy, paused rings, seeded answers

@MainActor
@Suite(.serialized)
struct A3_UsageRefreshPolicyTests {
    @Test func aRingClickProbesOnlyStaleData() async throws {
        let fixture = try UsageFixture()
        fixture.probe.answer(.usage(fixture.parsed(session: 5)))

        // 60 s old: fresh enough.
        try fixture.writeGlobalConfig(cachedSession: 20, fetchedAt: fixture.now.addingTimeInterval(-60))
        let fresh = fixture.makeStore(desktopOn: false)
        await fresh.refresh(accountId: fixture.id, reason: .ringClick)
        #expect(fixture.probe.count == 0)

        // 130 s old: asked, and the call returns with the answer in.
        let other = try UsageFixture()
        other.probe.answer(.usage(other.parsed(session: 5)))
        try other.writeGlobalConfig(cachedSession: 20, fetchedAt: other.now.addingTimeInterval(-130))
        let stale = other.makeStore(desktopOn: false)
        await stale.refresh(accountId: other.id, reason: .ringClick)
        #expect(other.probe.count == 1)
        #expect(stale.usage[other.id]?.fiveHour?.utilization == 5)
        #expect(stale.fetchState[other.id] == .idle)
    }

    @Test func forcedRefreshesAreSpacedByAMinute() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.probe.answer(.usage(fixture.parsed(session: 5)))
        let store = fixture.makeStore(desktopOn: false)
        await store.refresh(accountId: fixture.id, reason: .forced)
        await store.refresh(accountId: fixture.id, reason: .forced)
        #expect(fixture.probe.count == 1)
    }

    @Test func aRingClickRespectsTheBackoffAndForcedClearsIt() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.probe.answer(.rateLimited)
        var state = UsageState()
        state.accounts[fixture.id] = .init(lastProbeAt: fixture.now.addingTimeInterval(-600), failureCount: 1,
                                                   nextAttemptAt: fixture.now.addingTimeInterval(300))
        fixture.stateStore.saveNow(state)
        let store = fixture.makeStore(desktopOn: false)
        store.restoreState()

        await store.refresh(accountId: fixture.id, reason: .ringClick)
        #expect(fixture.probe.count == 0)
        await store.refresh(accountId: fixture.id, reason: .forced)
        #expect(fixture.probe.count == 1)
        // Rate limited again: paused, and said so in words that aren't the account's own limit.
        guard case .failed(let text)? = store.fetchState[fixture.id] else {
            Issue.record("expected a failure state")
            return
        }
        #expect(text.hasPrefix("Usage check paused (too many requests), retrying in"))
    }

    @Test func refreshWaitsAtMostTheLimit() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.probe.answer(.usage(fixture.parsed(session: 5)))
        fixture.probe.hold()
        let store = fixture.makeStore(desktopOn: false, waitLimit: 0.3)
        // Returns although Claude Code hasn't answered.
        await store.refresh(accountId: fixture.id, reason: .forced)
        #expect(store.isProbing)
        #expect(store.usage[fixture.id] == nil)
        fixture.probe.release()
        try await fixture.settle(store)
        #expect(store.usage[fixture.id]?.fiveHour?.utilization == 5)
    }

    @Test func pausedAccountsAreNotProbedUntilShownAgain() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.probe.answer(.usage(fixture.parsed(session: 5)))
        let store = fixture.makeStore(desktopOn: false)
        store.setPausedAccounts([fixture.id])
        await store.pollCycle()
        try await fixture.settle(store)
        #expect(fixture.probe.count == 0)
        store.setPausedAccounts([])
        await store.pollCycle()
        try await fixture.settle(store)
        #expect(fixture.probe.count == 1)
    }

    /// Switching a ring off drops its queued request: no probe later, no
    /// "checking…" left behind, and whoever waits on it is let go.
    @Test func pausingARingDropsItsQueuedRequest() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        _ = try fixture.addSideAccount()
        fixture.probe.answer(.usage(fixture.parsed(session: 5)))
        fixture.probe.hold()
        let store = fixture.makeStore(desktopOn: false, waitLimit: 30)

        // Both asked for; the first is being probed, the second waits its turn.
        store.refresh()
        for _ in 0..<250 where fixture.probe.count == 0 {
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(fixture.probe.count == 1)
        #expect(store.fetchState[UsageFixture.sideID] == .fetching)
        let started = Date()
        let waiter = Task { await store.refresh(accountId: UsageFixture.sideID, reason: .forced) }
        try await Task.sleep(for: .milliseconds(300))

        store.setPausedAccounts([UsageFixture.sideID])
        await waiter.value
        #expect(Date().timeIntervalSince(started) < 10)
        #expect(store.fetchState[UsageFixture.sideID] == .idle)
        #expect(store.fetchState.filter { $0.value == .fetching }.map(\.key) == [fixture.id])

        fixture.probe.release()
        try await fixture.settle(store)
        #expect(fixture.probe.count == 1)
        #expect(fixture.probe.requests.map(\.accountId) == [fixture.id])
        #expect(!store.isFetching)

        // Asked for while off: nothing reaches Claude Code.
        await store.refresh(accountId: UsageFixture.sideID, reason: .forced)
        #expect(fixture.probe.count == 1)
        #expect(store.fetchState[UsageFixture.sideID] == .idle)
    }

    /// Quitting: nothing new reaches Claude Code after `stop()`, not even
    /// the request queued behind the probe that is still running.
    @Test func nothingIsProbedAfterStop() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        _ = try fixture.addSideAccount()
        fixture.probe.answer(.usage(fixture.parsed(session: 5)))
        fixture.probe.hold()
        let store = fixture.makeStore(desktopOn: false)
        store.refresh()
        for _ in 0..<250 where fixture.probe.count == 0 {
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(fixture.probe.count == 1)

        store.stop()
        #expect(store.fetchState[UsageFixture.sideID] != .fetching)
        fixture.probe.release()
        try await fixture.settle(store)
        try await Task.sleep(for: .milliseconds(100))
        #expect(fixture.probe.count == 1)
        #expect(!store.isProbing && !store.isFetching)
    }

    @Test func aSeededAnswerKeepsItsRealDate() async throws {
        let fixture = try UsageFixture()
        let fetchedAt = fixture.now.addingTimeInterval(-40 * 60)
        // Claude Code's fallback: the same numbers as its 40-minute-old cache.
        try fixture.writeGlobalConfig(cachedSession: 20, fetchedAt: fetchedAt)
        fixture.probe.answer(.usage(fixture.parsed(session: 20, seeded: true)))
        let store = fixture.makeStore(desktopOn: false)
        await store.refresh(accountId: fixture.id, reason: .forced)

        #expect(fixture.probe.count == 1)
        let usage = try #require(store.usage[fixture.id])
        #expect(abs(usage.updatedAt.timeIntervalSince(fetchedAt)) < 0.01, "not stamped as fetched now")
        #expect(usage.isStale(now: Date()))
        guard case .failed(let text)? = store.fetchState[fixture.id] else {
            Issue.record("a seeded answer counts as rate limited")
            return
        }
        #expect(text.contains("too many requests"))
    }

    @Test func probeAnswersAreDatedHonestly() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let fixture = UsageBodies(now: now)
        typealias Store = UsageStore
        // A normal answer: fetched now.
        let fresh = Store.interpretProbeAnswer(fixture.parsed(20), accountId: "a", now: now, cachedCopy: nil)
        #expect(fresh.snapshot?.updatedAt == now && fresh.snapshot?.source == .probe && !fresh.rateLimited)
        // Seeded, matching a 40-minute-old copy: that date, and back off.
        let copy = CachedUsageSnapshot(fetchedAt: now.addingTimeInterval(-2400), accountUuid: "x", usage: fixture.parsed(20))
        let seeded = Store.interpretProbeAnswer(fixture.parsed(20, seeded: true), accountId: "a", now: now, cachedCopy: copy)
        #expect(seeded.snapshot?.updatedAt == now.addingTimeInterval(-2400) && seeded.rateLimited)
        // Seeded but the copy is Claude Code's fresh cache: fine.
        let recent = CachedUsageSnapshot(fetchedAt: now.addingTimeInterval(-30), accountUuid: "x", usage: fixture.parsed(20))
        #expect(!Store.interpretProbeAnswer(fixture.parsed(20, seeded: true), accountId: "a", now: now, cachedCopy: recent).rateLimited)
        // Seeded and nothing to date it by: dropped.
        let other = CachedUsageSnapshot(fetchedAt: now, accountUuid: "x", usage: fixture.parsed(77))
        let undated = Store.interpretProbeAnswer(fixture.parsed(20, seeded: true), accountId: "a", now: now, cachedCopy: other)
        #expect(undated.snapshot == nil && undated.rateLimited)
        #expect(Store.interpretProbeAnswer(fixture.parsed(20, seeded: true), accountId: "a", now: now, cachedCopy: nil).snapshot == nil)
    }

    @Test func getUsageWithoutLimitsMayBeSeeded() throws {
        func answer(_ rateLimits: [String: Any]) throws -> ParsedUsage {
            guard case .usage(let usage) = UsageParser.parseGetUsageResponse(["rate_limits_available": true, "rate_limits": rateLimits]) else {
                throw CancellationError()
            }
            return usage
        }
        let window: [String: Any] = ["utilization": 5, "resets_at": "2026-09-24T12:50:00Z"]
        #expect(try answer(["five_hour": window]).isPossiblySeeded)
        #expect(try !answer(["five_hour": window, "limits": [["kind": "session", "percent": 5]]]).isPossiblySeeded)
        #expect(try !answer(["five_hour": window, "limits": []]).isPossiblySeeded)
    }

    @Test func pausedText() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        #expect(UsageStore.pausedText(retryAt: now.addingTimeInterval(300), now: now)
            == "Usage check paused (too many requests), retrying in 5 min")
        #expect(UsageStore.pausedText(retryAt: now.addingTimeInterval(61), now: now).hasSuffix("in 2 min"))
        #expect(UsageStore.pausedText(retryAt: now, now: now).hasSuffix("in 1 min"))
    }

    @Test func scheduledProbesPickTheStalestDueAccount() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let next = UsageStore.nextScheduledProbe(
            candidates: ["fresh", "stale", "staler", "signedOut", "paused", "backingOff", "justProbed"],
            interval: 300,
            now: now,
            signedIn: ["fresh": true, "stale": true, "staler": true, "signedOut": false, "paused": true,
                       "backingOff": true, "justProbed": true],
            paused: ["paused"],
            nextAttemptAt: ["backingOff": now.addingTimeInterval(60)],
            lastProbeAt: ["justProbed": now.addingTimeInterval(-60)],
            newestDataAt: ["fresh": now.addingTimeInterval(-10), "stale": now.addingTimeInterval(-600),
                           "staler": now.addingTimeInterval(-900), "paused": .distantPast, "backingOff": .distantPast,
                           "justProbed": .distantPast, "signedOut": .distantPast],
            newestFullAt: ["fresh": now.addingTimeInterval(-10)]
        )
        #expect(next == "staler")
        // Fresh 5h/7d from a status line, but no full snapshot for 15 minutes: probe for the scoped limits.
        let scoped = UsageStore.nextScheduledProbe(
            candidates: ["a"], interval: 300, now: now, signedIn: ["a": true], paused: [], nextAttemptAt: [:],
            lastProbeAt: [:], newestDataAt: ["a": now.addingTimeInterval(-5)], newestFullAt: ["a": now.addingTimeInterval(-16 * 60)]
        )
        #expect(scoped == "a")
    }
}

/// Usage bodies for the pure tests.
private struct UsageBodies {
    let now: Date

    func parsed(_ session: Double, seeded: Bool = false) -> ParsedUsage {
        ParsedUsage(
            fiveHour: UsageWindow(utilization: session, resetsAt: now.addingTimeInterval(3600), duration: 18_000),
            sevenDay: UsageWindow(utilization: 10, resetsAt: now.addingTimeInterval(86400), duration: 604_800),
            isPossiblySeeded: seeded
        )
    }
}
