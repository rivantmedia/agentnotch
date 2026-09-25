import Combine
import Foundation
import Testing
@testable import ClaudeControl

/// Usage readings over time, with where they came from, until the website has them.
struct UsageHistoryRecorderTests {
    static func reading(_ source: CloudUsageSource = .probe, at seconds: TimeInterval, session: Double = 10,
                        weekly: Double? = nil, key: String = CloudFixture.accountKey) -> RecordedUsageReading {
        var windows: [CloudSyncRequest.UsageWindow] = [.init(id: "session", utilization: session, resetsAt: nil)]
        if let weekly { windows.append(.init(id: "weekly_all", utilization: weekly, resetsAt: nil)) }
        return RecordedUsageReading(accountKey: key, identityId: CloudFixture.identityId, source: source,
                                    observedAt: CloudFixture.base.addingTimeInterval(seconds), windows: windows)
    }

    @Test func aReadingIsKeptOnceAndItsMomentsWindowsMerge() {
        let recorder = UsageHistoryRecorder(fileURL: nil, persists: false)
        #expect(recorder.record(Self.reading(at: 0)))
        #expect(!recorder.record(Self.reading(at: 0)))
        // The same moment with another window: merged into it.
        #expect(recorder.record(Self.reading(at: 0, weekly: 30)))
        #expect(recorder.pendingCount == 1)
        #expect(recorder.pending().first?.windows.map(\.id) == ["session", "weekly_all"])
        // Older than the last taken: dropped.
        #expect(recorder.record(Self.reading(at: 900, session: 12)))
        #expect(!recorder.record(Self.reading(at: 600, session: 11)))
        #expect(recorder.pendingCount == 2)
        #expect(!recorder.record(RecordedUsageReading(accountKey: CloudFixture.accountKey, identityId: CloudFixture.identityId,
                                                      source: .probe, observedAt: CloudFixture.base.addingTimeInterval(2000),
                                                      windows: [])))
    }

    @Test func repeatsWithinTenMinutesAreDropped() {
        let recorder = UsageHistoryRecorder(fileURL: nil, persists: false)
        #expect(recorder.record(Self.reading(at: 0, session: 10)))
        #expect(!recorder.record(Self.reading(at: 300, session: 10)))
        #expect(recorder.record(Self.reading(at: 301, session: 11)))
        #expect(recorder.record(Self.reading(at: 301 + 600, session: 11)))
        #expect(recorder.pendingCount == 3)
    }

    @Test func claudeDesktopAndClaudeJsonAreSeparateSources() {
        let recorder = UsageHistoryRecorder(fileURL: nil, persists: false)
        #expect(recorder.record(Self.reading(.desktop, at: 0)))
        #expect(recorder.record(Self.reading(.claudeJson, at: 0)))
        #expect(recorder.record(Self.reading(.statusLine, at: 0)))
        #expect(Set(recorder.pending().map(\.source)) == [.desktop, .claudeJson, .statusLine])
        #expect(recorder.pending().map(\.contract.source) == [.desktop, .claudeJson, .statusLine])
    }

    @Test func theOutboxIsBoundedAndEmptiesAsTheWebsiteTakesIt() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-usage"))
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent(UsageHistoryRecorder.fileName)
        let recorder = UsageHistoryRecorder(fileURL: file, persists: true)
        for index in 0..<(UsageHistoryRecorder.capacity + 5) {
            recorder.record(Self.reading(at: TimeInterval(index), session: Double(index)))
        }
        #expect(recorder.pendingCount == UsageHistoryRecorder.capacity)
        #expect(recorder.pending().first?.windows.first?.utilization == 5)
        recorder.markSent(recorder.pending(limit: 100))
        #expect(recorder.pendingCount == UsageHistoryRecorder.capacity - 100)
        recorder.discard { $0.windows.first!.utilization < 1000 }
        recorder.saveNow()
        #expect(CloudFiles.permissions(of: file) == 0o600)
        #expect(UsageHistoryRecorder(fileURL: file, persists: true).pendingCount == UsageHistoryRecorder.capacity - 995)
        recorder.clear()
        recorder.saveNow()
        #expect(UsageHistoryRecorder(fileURL: file, persists: true).pendingCount == 0)
    }

    @Test func aSnapshotsWindowsAsRecorded() {
        let usage = AccountUsage(
            accountId: CloudFixture.identityId,
            fiveHour: UsageWindow(utilization: 40, resetsAt: CloudFixture.base, duration: UsageWindow.sessionDuration),
            sevenDay: UsageWindow(utilization: 60, resetsAt: nil, duration: UsageWindow.weeklyDuration),
            scoped: [ScopedUsage(name: "Opus", window: UsageWindow(utilization: 5, resetsAt: nil, duration: UsageWindow.weeklyDuration)),
                     ScopedUsage(name: String(repeating: "Very Long Model ", count: 8),
                                 window: UsageWindow(utilization: 1, resetsAt: nil, duration: UsageWindow.weeklyDuration))],
            extraUsage: ExtraUsage(isEnabled: true, monthlyLimit: 5000, usedCredits: 1250, utilization: nil, currency: "USD"),
            source: .cache, updatedAt: CloudFixture.base)
        let windows = UsageObservation.windows(from: usage)
        #expect(windows.map(\.id).prefix(3) == ["session", "weekly_all", "weekly_opus"])
        #expect(windows.allSatisfy { CloudKeys.isWindowID($0.id) })
        #expect(windows.last == .init(id: "extra_usage", utilization: 25, resetsAt: nil))
    }

    /// `UsageStore` says where each reading came from: Claude Desktop's
    /// cache and Claude Code's `.claude.json` are told apart.
    @MainActor
    @Test func theUsageStoreTellsClaudeDesktopFromClaudeJson() async throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-usage-store"))
        defer { try? FileManager.default.removeItem(at: root) }
        let configDir = root.appendingPathComponent("home/.claude-work", isDirectory: true)
        try FileManager.default.createDirectory(at: configDir, withIntermediateDirectories: true)
        let now = Date()
        let body: [String: Any] = [
            "five_hour": ["utilization": 30, "resets_at": now.addingTimeInterval(3 * 3600).formatted(.iso8601)],
            "seven_day": ["utilization": 10, "resets_at": now.addingTimeInterval(3 * 86400).formatted(.iso8601)],
        ]
        let json: [String: Any] = [
            "oauthAccount": ["accountUuid": "acc-1", "emailAddress": "me@x.dev", "organizationUuid": "org-1"],
            "cachedUsageUtilization": ["accountUuid": "acc-1", "fetchedAtMs": (now.timeIntervalSince1970 - 120) * 1000,
                                       "utilization": body],
        ]
        try JSONSerialization.data(withJSONObject: json).write(to: configDir.appendingPathComponent(".claude.json"))
        let account = ClaudeAccount(configDir: configDir.path, configDirEnv: configDir.path, email: "me@x.dev", accountUuid: "acc-1")
        let registry = AccountRegistry(home: root.appendingPathComponent("home").path,
                                       storeURL: root.appendingPathComponent("support/accounts.json"),
                                       configReader: ClaudeGlobalConfigReader())
        registry.replaceAllWithFixtures([account])
        let desktop = DesktopReadings(ClaudeExternalUsageReading(windows: [
            .init(id: "session", usedFraction: 0.35, resetsAt: now.addingTimeInterval(3 * 3600), duration: 18_000),
            .init(id: "weekly_all", usedFraction: 0.12, resetsAt: now.addingTimeInterval(3 * 86400), duration: 604_800),
        ], observedAt: now.addingTimeInterval(-30)))
        let store = UsageStore(registry: registry, configReader: ClaudeGlobalConfigReader(),
                               stateStore: UsageStateStore(directory: root.appendingPathComponent("support", isDirectory: true)),
                               externalSource: desktop, readsExternalUsage: { true },
                               probeRunner: { _ in .failed("no probes in tests") }, automaticProbes: false)
        var seen: [UsageObservation] = []
        let subscription = store.observations.sink { seen.append($0) }
        defer { subscription.cancel() }
        await store.pollCycle()

        let cached = try #require(seen.first { $0.source == .claudeJson })
        #expect(cached.identityId == "uuid:acc-1")
        #expect(cached.windows.first { $0.id == "session" }?.utilization == 30)
        let fromDesktop = try #require(seen.first { $0.source == .desktop })
        #expect(fromDesktop.windows.first { $0.id == "session" }?.utilization == 35)
        #expect(abs(fromDesktop.observedAt.timeIntervalSince(now.addingTimeInterval(-30))) < 1)
    }
}

/// Claude Desktop's cache, standing in.
private nonisolated struct DesktopReadings: ClaudeExternalUsageSource {
    let value: ClaudeExternalUsageReading
    init(_ value: ClaudeExternalUsageReading) { self.value = value }
    func reading(organizationUuid: String, now: Date) async -> ClaudeExternalUsageReading? {
        organizationUuid == "org-1" ? value : nil
    }
}
