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
    /// The processes status lines name are made up: their start times come
    /// from `processStarts`, never from the kernel.
    func makeStore(desktopOn: Bool = true, waitLimit: TimeInterval = 30,
                   processStarts: [Int: Date] = [:], hookPids: [String: Int] = [:]) -> UsageStore {
        UsageStore(
            registry: registry,
            configReader: ClaudeGlobalConfigReader(),
            stateStore: stateStore,
            externalSource: desktop,
            readsExternalUsage: { desktopOn },
            probeRunner: { [probe] request in await probe.run(request) },
            automaticProbes: true,
            refreshWaitLimit: waitLimit,
            sessionStartedAt: { _ in nil },
            processStartedAt: { processStarts[$0] },
            sessionProcessId: { hookPids[$0] }
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

// MARK: - Early resets

/// Anthropic resets the weekly limit early and keeps its reset time: what is
/// read after the reset must replace what was read before it, whichever
/// source is higher.
@Suite(.serialized)
struct A3_EarlyResetTests {
    private func line(_ fixture: UsageFixture, weekly: Double, process: Int, session: String? = nil,
                      at: Date) -> StatusLineUpdate {
        StatusLineUpdate(
            sessionId: session ?? "s\(process)", transcriptPath: nil, configDirEnv: fixture.configDir.path,
            accountId: fixture.configDir.path, receivedAt: at, fiveHour: nil, sevenDay: week(fixture, weekly),
            contextUsedPercent: nil, contextWindowSize: nil, modelId: nil, modelDisplayName: nil,
            costUSD: nil, sessionName: nil, claudeCodeVersion: nil, processId: process)
    }

    /// The fixture's weekly window (`UsageFixture.body` resets it in 3 days).
    private func week(_ fixture: UsageFixture, _ utilization: Double) -> UsageWindow {
        UsageWindow(utilization: utilization, resetsAt: fixture.now.addingTimeInterval(3 * 86400), duration: UsageWindow.weeklyDuration)
    }

    @Test func aCheckAfterTheResetReplacesTheStatusLinesOldNumbers() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        let store = fixture.makeStore(desktopOn: false)
        var seen: [UsageObservation] = []
        let subscription = store.observations.sink { seen.append($0) }
        defer { subscription.cancel() }

        // Before the reset: 62% of the week, from a terminal session.
        store.ingest(line(fixture, weekly: 62, process: 101, at: Date().addingTimeInterval(-600)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 62)

        // The reset; the next check reads 3%, same reset time.
        fixture.probe.answer(.usage(ParsedUsage(sevenDay: week(fixture, 3))))
        await store.refresh(accountId: fixture.id, reason: .forced)
        #expect(fixture.probe.count == 1)
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 3)

        // The session, idle, re-renders its old numbers: still 3%, and
        // nothing for the history.
        seen.removeAll()
        store.ingest(line(fixture, weekly: 62, process: 101, at: Date()))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 3)
        #expect(seen.isEmpty)

        // Its next response after the reset: shown and recorded, though the
        // process said more before.
        store.ingest(line(fixture, weekly: 4, process: 101, at: Date().addingTimeInterval(1)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 4)
        let recorded = try #require(seen.first { $0.source == .statusLine })
        #expect(recorded.windows.first { $0.id == UsageRingWindows.weeklyID }?.utilization == 4)
    }

    /// Without any check: a session working through the reset brings the
    /// ring down, and another session's older numbers don't bring it back.
    @Test func aWorkingSessionBringsTheRingDownByItself() throws {
        let fixture = try UsageFixture()
        let store = fixture.makeStore(desktopOn: false)
        let start = Date().addingTimeInterval(-900)
        store.ingest(line(fixture, weekly: 60, process: 101, at: start))
        store.ingest(line(fixture, weekly: 62, process: 202, at: start.addingTimeInterval(10)))
        store.ingest(line(fixture, weekly: 60, process: 101, at: start.addingTimeInterval(20)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 62)

        store.ingest(line(fixture, weekly: 3, process: 101, at: start.addingTimeInterval(600)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 3)
        store.ingest(line(fixture, weekly: 62, process: 202, at: start.addingTimeInterval(700)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 3)
        // A new session in the same process (`/clear`) is still that process.
        store.ingest(line(fixture, weekly: 62, process: 202, session: "after-clear", at: start.addingTimeInterval(800)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 3)
    }

    /// After a relaunch the reading kept from before the reset shows until
    /// something known to be newer arrives: a session that started after it.
    @Test func aSessionStartedAfterTheReadingReplacesIt() async throws {
        let fixture = try UsageFixture()
        // Claude Code's own cache, from before the reset: 10% of the week.
        try fixture.writeGlobalConfig(cachedSession: 20, fetchedAt: fixture.now.addingTimeInterval(-3600))
        let starts = [101: fixture.now.addingTimeInterval(-7200), 202: fixture.now.addingTimeInterval(-1800)]
        let store = UsageStore(registry: fixture.registry, configReader: ClaudeGlobalConfigReader(),
                               stateStore: fixture.stateStore, externalSource: nil, readsExternalUsage: { false },
                               probeRunner: { _ in .failed("no probes here") }, automaticProbes: false,
                               sessionStartedAt: { _ in nil }, processStartedAt: { starts[$0] },
                               sessionProcessId: { _ in nil })
        await store.pollCycle()
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 10)

        // A session older than that reading can't tell which came first.
        store.ingest(line(fixture, weekly: 3, process: 101, session: "old", at: Date()))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 10)
        // One started after it can: Claude Code has no rate limits before a
        // process's first response.
        store.ingest(line(fixture, weekly: 3, process: 202, session: "new", at: Date()))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 3)
    }

    /// The history gets what the ring shows: another process's lagging,
    /// lower numbers are not sent, and neither is a stale first report.
    @Test func onlyWhatTheRingShowsIsRecorded() throws {
        let fixture = try UsageFixture()
        let store = fixture.makeStore(desktopOn: false)
        var seen: [UsageObservation] = []
        let subscription = store.observations.sink { seen.append($0) }
        defer { subscription.cancel() }
        let start = Date().addingTimeInterval(-600)

        store.ingest(line(fixture, weekly: 45, process: 101, at: start))
        #expect(seen.count == 1)
        // Another process, its numbers a little behind: not shown, not sent.
        store.ingest(line(fixture, weekly: 43, process: 202, at: start.addingTimeInterval(5)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 45)
        #expect(seen.count == 1)
        // Shown again when it moves ahead.
        store.ingest(line(fixture, weekly: 46, process: 202, at: start.addingTimeInterval(10)))
        #expect(seen.last?.windows.first?.utilization == 46)
    }

    /// A relaunch keeps what each running process said, so an idle one
    /// re-rendering its pre-reset numbers is still a repeat.
    @Test func aRelaunchRemembersWhatEachProcessSaid() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        let started = Date().addingTimeInterval(-3600)
        let store = fixture.makeStore(desktopOn: false, processStarts: [101: started])
        store.ingest(line(fixture, weekly: 62, process: 101, at: Date().addingTimeInterval(-600)))
        fixture.probe.answer(.usage(ParsedUsage(sevenDay: week(fixture, 3))))
        await store.refresh(accountId: fixture.id, reason: .forced)
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 3)
        store.saveStateNow()

        // The next run, the same process still running: a repeat, not news.
        let next = fixture.makeStore(desktopOn: false, processStarts: [101: started])
        var seen: [UsageObservation] = []
        let subscription = next.observations.sink { seen.append($0) }
        defer { subscription.cancel() }
        next.restoreState()
        #expect(next.usage[fixture.id]?.sevenDay?.utilization == 3)
        next.ingest(line(fixture, weekly: 62, process: 101, at: Date()))
        #expect(next.usage[fixture.id]?.sevenDay?.utilization == 3)
        #expect(seen.isEmpty)

        // A new process that got the same pid is another process: its line is news.
        let reused = fixture.makeStore(desktopOn: false, processStarts: [101: started.addingTimeInterval(30)])
        let reusedSubscription = reused.observations.sink { seen.append($0) }
        defer { reusedSubscription.cancel() }
        reused.restoreState()
        reused.ingest(line(fixture, weekly: 62, process: 101, at: Date()))
        #expect(seen.contains { $0.source == .statusLine })
    }

    /// The reading that brought the ring down comes back after a relaunch
    /// even when its process has ended since, rather than the older
    /// snapshot from before the reset.
    @Test func anEndedProcesssReadingOutlivesARelaunch() async throws {
        let fixture = try UsageFixture()
        // Claude Code's own cache, from before the reset: 10% of the week.
        try fixture.writeGlobalConfig(cachedSession: 20, fetchedAt: fixture.now.addingTimeInterval(-3600))
        func makeStore(_ starts: [Int: Date]) -> UsageStore {
            UsageStore(registry: fixture.registry, configReader: ClaudeGlobalConfigReader(), stateStore: fixture.stateStore,
                       externalSource: nil, readsExternalUsage: { false }, probeRunner: { _ in .failed("no probes here") },
                       automaticProbes: false, sessionStartedAt: { _ in nil }, processStartedAt: { starts[$0] },
                       sessionProcessId: { _ in nil })
        }
        let store = makeStore([101: fixture.now.addingTimeInterval(-1800)])
        await store.pollCycle()
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 10)
        store.ingest(line(fixture, weekly: 3, process: 101, at: Date()))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 3)
        store.saveStateNow()

        // The next run; the process has ended.
        let next = makeStore([:])
        next.restoreState()
        #expect(next.usage[fixture.id]?.sevenDay?.utilization == 3)
        await next.pollCycle()
        #expect(next.usage[fixture.id]?.sevenDay?.utilization == 3)
    }

    @Test func savedRecordsComeBackOnlyToTheirAccountsFoldersWithinAWeek() throws {
        let fixture = try UsageFixture()
        let side = try fixture.addSideAccount()
        let folder = AccountPaths.normalize(fixture.configDir.path)
        let key = "pid:101@1700000000"
        func record(_ weekly: Double, age: TimeInterval) -> UsageStore.StatusLineReadings {
            let at = Date().addingTimeInterval(-age)
            return UsageStore.StatusLineReadings(lastReportAt: at, fiveHour: nil, sevenDay: UsageStore.Reading(week(fixture, weekly), at: at))
        }
        func restored(_ line: UsageState.StatusLine) -> Double? {
            var state = UsageState()
            state.accounts[fixture.id] = UsageState.Account(statusLines: [line])
            fixture.stateStore.saveNow(state)
            let store = fixture.makeStore(desktopOn: false)
            store.restoreState()
            return store.usage[fixture.id]?.sevenDay?.utilization
        }
        #expect(restored(.init(folder: folder, key: key, readings: record(30, age: 60))) == 30)
        // Not heard from for a week: its window has reset since.
        #expect(restored(.init(folder: folder, key: key, readings: record(30, age: UsageStore.statusLineRetention + 60))) == nil)
        // A folder another account holds now.
        #expect(restored(.init(folder: AccountPaths.normalize(side.configDir), key: key, readings: record(30, age: 60))) == nil)
        // A key without the process's start could be any process's.
        #expect(restored(.init(folder: folder, key: "pid:101", readings: record(30, age: 60))) == nil)
    }

    @Test func aProcessNotHeardFromForAWeekIsForgotten() async throws {
        let fixture = try UsageFixture()
        var now = Date()
        let store = UsageStore(registry: fixture.registry, configReader: ClaudeGlobalConfigReader(), stateStore: fixture.stateStore,
                               externalSource: nil, readsExternalUsage: { false }, probeRunner: { _ in .failed("no probes here") },
                               automaticProbes: false, clock: { now }, sessionStartedAt: { _ in nil },
                               processStartedAt: { _ in nil }, sessionProcessId: { _ in nil })
        store.ingest(line(fixture, weekly: 30, process: 101, at: now))
        await store.pollCycle()
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 30)
        now = now.addingTimeInterval(UsageStore.statusLineRetention + 60)
        await store.pollCycle()
        #expect(store.usage[fixture.id] == nil)
    }

    /// A line that is news for its process but that the check outranks is
    /// not recorded: the history gets what the ring shows.
    @Test func aLineTheCheckOutranksIsNotRecorded() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        let store = fixture.makeStore(desktopOn: false)
        var seen: [UsageObservation] = []
        let subscription = store.observations.sink { seen.append($0) }
        defer { subscription.cancel() }
        fixture.probe.answer(.usage(ParsedUsage(sevenDay: week(fixture, 42))))
        await store.refresh(accountId: fixture.id, reason: .forced)
        let t = Date()
        store.ingest(line(fixture, weekly: 42, process: 101, at: t))
        seen.removeAll()

        // A process's own small step back (a late response): not news.
        store.ingest(line(fixture, weekly: 40, process: 101, at: t.addingTimeInterval(1)))
        // Another process's numbers, a little behind: news for it, not shown.
        store.ingest(line(fixture, weekly: 40, process: 202, at: t.addingTimeInterval(2)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 42)
        #expect(!seen.contains { $0.source == .statusLine })

        // A reset-sized drop from a process that reported after both: shown and recorded.
        store.ingest(line(fixture, weekly: 40, process: 101, at: t.addingTimeInterval(2.5)))
        store.ingest(line(fixture, weekly: 30, process: 101, at: t.addingTimeInterval(3)))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 30)
        #expect(seen.contains { $0.source == .statusLine && $0.windows.first?.utilization == 30 })
    }

    /// Claude Desktop's reading is dated by the server's clock, to the
    /// second: it is not taken for newer than a line from the minute before it.
    @Test func claudeDesktopsDateAllowsForClockSkew() async throws {
        for (lineAge, shown) in [(30.0, 62.0), (120.0, 10.0)] {
            let fixture = try UsageFixture()
            try fixture.writeGlobalConfig()
            let observed = Date().addingTimeInterval(-10)
            fixture.desktop.set(fixture.desktopReading(session: 20, observedAt: observed))
            let store = fixture.makeStore(desktopOn: true)
            store.ingest(line(fixture, weekly: 62, process: 101, at: observed.addingTimeInterval(-lineAge)))
            await store.pollCycle()
            try await fixture.settle(store)
            #expect(store.usage[fixture.id]?.sevenDay?.utilization == shown, "line \(lineAge) s before Desktop's reading")
        }
    }

    /// A check's answer came some time after it was launched: a line that
    /// arrived while it ran is not known to be older than the answer.
    @Test func aLineThatArrivesDuringACheckIsNotOlderThanItsAnswer() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        let store = fixture.makeStore(desktopOn: false)
        fixture.probe.answer(.usage(ParsedUsage(sevenDay: week(fixture, 3))))
        fixture.probe.hold()
        let refresh = Task { await store.refresh(accountId: fixture.id, reason: .forced) }
        for _ in 0..<250 where fixture.probe.count == 0 {
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(fixture.probe.count == 1)
        store.ingest(line(fixture, weekly: 62, process: 101, at: Date()))
        try await Task.sleep(for: .milliseconds(20))
        fixture.probe.release()
        await refresh.value
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 62)
    }

    /// A status line's slow save doesn't hold back the quick one a check asks for.
    @Test func aChecksSaveDoesntWaitBehindAStatusLines() async throws {
        let fixture = try UsageFixture()
        try fixture.writeGlobalConfig()
        fixture.probe.answer(.usage(ParsedUsage(sevenDay: week(fixture, 31))))
        let store = fixture.makeStore(desktopOn: false, processStarts: [101: Date().addingTimeInterval(-60)])
        store.start()
        defer { store.stop() }
        store.ingest(line(fixture, weekly: 30, process: 101, at: Date()))
        await store.refresh(accountId: fixture.id, reason: .forced)
        for _ in 0..<50 where fixture.stateStore.load().accounts[fixture.id]?.lastProbeAt == nil {
            try await Task.sleep(for: .milliseconds(100))
        }
        #expect(fixture.stateStore.load().accounts[fixture.id]?.lastProbeAt != nil)
    }

    /// A pid the session's hooks contradict isn't trusted: the line counts
    /// for the session, with nothing known of when it was taken.
    @Test func aPidTheHooksContradictIsNotTrusted() throws {
        let fixture = try UsageFixture()
        let started = Date().addingTimeInterval(-60)
        let store = fixture.makeStore(desktopOn: false, processStarts: [101: started], hookPids: ["s101": 777])
        let snapshotTime = Date().addingTimeInterval(-120)
        // An older, higher reading from another process.
        store.ingest(line(fixture, weekly: 62, process: 202, at: snapshotTime))
        // Trusted, this process's start would make its first report newer than that.
        store.ingest(line(fixture, weekly: 3, process: 101, at: Date()))
        #expect(store.usage[fixture.id]?.sevenDay?.utilization == 62)
        let trusting = fixture.makeStore(desktopOn: false, processStarts: [101: started])
        trusting.ingest(line(fixture, weekly: 62, process: 202, at: snapshotTime))
        trusting.ingest(line(fixture, weekly: 3, process: 101, at: Date()))
        #expect(trusting.usage[fixture.id]?.sevenDay?.utilization == 3)
    }
}
