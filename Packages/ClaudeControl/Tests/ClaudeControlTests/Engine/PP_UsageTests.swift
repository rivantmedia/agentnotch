import Foundation
import Testing
@testable import ClaudeControl

/// Usage per account: every folder's cached snapshot counts, the check runs
/// once, in a folder Claude Code runs in, never in a store.
@MainActor
@Suite(.serialized)
struct PP_UsageTests {
    typealias Home = ParallelProfilesHome

    /// Stands in for `claude -p` / `get_usage`, recording where it ran.
    final class Probe: @unchecked Sendable {
        let lock = NSLock()
        var requests: [UsageStore.ProbeRequest] = []
        func run(_ request: UsageStore.ProbeRequest) async -> UsageProbe.Outcome {
            lock.withLock { requests.append(request) }
            return .usage(UsageParser.parseUsageBody([
                "five_hour": ["utilization": 40, "resets_at": "2099-01-01T00:00:00Z"],
                "seven_day": ["utilization": 50, "resets_at": "2099-01-05T00:00:00Z"],
            ]))
        }
    }

    private func store(_ registry: AccountRegistry, probe: Probe, support: String) -> UsageStore {
        UsageStore(registry: registry, configReader: ClaudeGlobalConfigReader(),
                   stateStore: UsageStateStore(directory: URL(fileURLWithPath: support, isDirectory: true)),
                   externalSource: nil, readsExternalUsage: { false },
                   probeRunner: { await probe.run($0) }, automaticProbes: true)
    }

    private func settle(_ store: UsageStore) async throws {
        for _ in 0..<250 where store.isProbing || store.isFetching {
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    @Test func theFreshestMatchingCacheOfAnyFolderWins() {
        func config(_ uuid: String?, _ ms: Double) -> ClaudeGlobalConfig {
            ClaudeGlobalConfig(identity: nil, cachedUsage: CachedUsageSnapshot(
                fetchedAt: Date(timeIntervalSince1970: ms / 1000), accountUuid: uuid,
                usage: UsageParser.parseUsageBody(["five_hour": ["utilization": ms / 1_000_000_000, "resets_at": "2099-01-01T00:00:00Z"]])))
        }
        var identity = ClaudeIdentityAccount(id: "uuid:u-1", ringID: "claude-acct-x", accountUuid: "U-1",
                                             runDirs: [ClaudeAccount(configDir: "/h/.claude")],
                                             storeDirs: [ClaudeAccount(configDir: "/h/.claude-u", kind: .store)],
                                             customLabel: nil, colorIndex: 0, isHidden: false)
        let configs = [
            "/h/.claude": config("u-1", 1_000_000_000),
            "/h/.claude-u": config("u-1", 3_000_000_000),   // the store's is newer
            "/h/.claude-other": config("u-1", 9_000_000_000), // not this identity's folder
        ]
        #expect(UsageStore.freshestCachedUsage(identity: identity, configs: configs)?.fetchedAt == Date(timeIntervalSince1970: 3_000_000))
        // Someone else's snapshot left in a folder (a mirrored default) is ignored.
        identity.runDirs = [ClaudeAccount(configDir: "/h/.claude")]
        identity.storeDirs = []
        #expect(UsageStore.freshestCachedUsage(identity: identity, configs: ["/h/.claude": config("u-2", 5_000_000_000)]) == nil)
    }

    @Test func theCheckRunsInTheMostRecentlyActiveRunFolder() {
        let home = "/h"
        let main = ClaudeAccount(configDir: "/h/.claude")
        let window = ClaudeAccount(configDir: "/h/.claude-windows/801f9dd51396", configDirEnv: "/h/.claude-windows/801f9dd51396")
        let store = ClaudeAccount(configDir: "/h/.claude-paras", configDirEnv: "/h/.claude-paras", kind: .store)
        let now = Date()
        #expect(UsageProbePlanner.probeFolder(runDirs: [main, window], activity: [now.addingTimeInterval(-60), now], home: home)?.id == window.id)
        #expect(UsageProbePlanner.probeFolder(runDirs: [main, window], activity: [now, now.addingTimeInterval(-60)], home: home)?.id == main.id)
        #expect(UsageProbePlanner.probeFolder(runDirs: [window, main], activity: [nil, nil], home: home)?.id == main.id)
        #expect(UsageProbePlanner.probeFolder(runDirs: [window], activity: [nil], home: home)?.id == window.id)
        // Never a store, even if handed one.
        #expect(UsageProbePlanner.probeFolder(runDirs: [store], activity: [now], home: home) == nil)
        #expect(UsageProbePlanner.probeFolder(runDirs: [], activity: [], home: home) == nil)
    }

    /// Over the user's layout: one check per account, each in a run folder
    /// of its own; an account only a store holds is not checked.
    @Test func oneCheckPerAccountNeverInAStore() async throws {
        let fake = Home("usage-probe")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let probe = Probe()
        let store = store(registry, probe: probe, support: fake.support)
        let parasId = "uuid:" + Home.parasUUID
        let biiosId = "uuid:" + Home.biiosUUID

        store.refresh()
        try await Task.sleep(for: .milliseconds(200))
        try await settle(store)
        let requests = probe.lock.withLock { probe.requests }
        #expect(Set(requests.map(\.accountId)) == [parasId, biiosId])
        #expect(requests.count == 2)
        let paras = registry.identity(id: parasId)?.runDirs.map(\.configDir) ?? []
        let storeFolders = registry.accounts.filter { $0.kind == .store }.map(\.configDir)
        for request in requests {
            let folder = request.configDirEnv ?? fake.path(".claude")
            #expect(!storeFolders.contains(folder), "checked in a store: \(folder)")
            if request.accountId == parasId { #expect(paras.contains(folder)) }
            if request.accountId == biiosId { #expect(folder == fake.path(".claude-windows/1bf3e8f92b11")) }
        }
        #expect(store.usage[parasId]?.fiveHour?.utilization == 40)
        #expect(store.usage[biiosId]?.fiveHour?.utilization == 40)
    }

    @Test func anAccountOnlyAStoreHoldsShowsItsCachedUsageAndIsNotChecked() async throws {
        let fake = Home("usage-store")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        try FileManager.default.removeItem(atPath: fake.path(".claude-windows/1bf3e8f92b11"))
        let registry = fake.registry()
        let probe = Probe()
        let store = store(registry, probe: probe, support: fake.support)
        let biiosId = "uuid:" + Home.biiosUUID

        await store.pollCycle()
        // The store's cached snapshot, passive, with its date.
        #expect(store.usage[biiosId]?.source == .cache)
        #expect(store.usage[biiosId]?.updatedAt == Date(timeIntervalSince1970: 1_790_000_050))

        await store.refresh(accountId: biiosId, reason: .forced)
        try await settle(store)
        #expect(!probe.lock.withLock { probe.requests }.contains { $0.accountId == biiosId })
        #expect(store.fetchState[biiosId] == UsageStore.noRunFolder)
    }

    /// Of the account's folders' caches the newest wins, the store's included.
    @Test func cachedUsageMergesAcrossTheAccountsFolders() async throws {
        let fake = Home("usage-cache")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let store = store(registry, probe: Probe(), support: fake.support)
        await store.pollCycle()
        // Paras: ~/.claude 1_790_000_000, store 1_790_000_100, window 801f 1_790_000_200.
        #expect(store.usage["uuid:" + Home.parasUUID]?.updatedAt == Date(timeIntervalSince1970: 1_790_000_200))
        // Biios: store 1_790_000_050, window 1_790_000_300.
        #expect(store.usage["uuid:" + Home.biiosUUID]?.updatedAt == Date(timeIntervalSince1970: 1_790_000_300))
        #expect(store.usage.count == 2)
    }

    /// A status line from a window counts for the account the window runs.
    @Test func statusLinesCountForTheirWindowsAccount() throws {
        let fake = Home("usage-statusline")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let store = store(registry, probe: Probe(), support: fake.support)
        store.ingest(StatusLineUpdate(
            sessionId: "s", transcriptPath: nil, configDirEnv: fake.path(".claude-windows/1bf3e8f92b11"),
            accountId: fake.path(".claude-windows/1bf3e8f92b11"), receivedAt: Date(),
            fiveHour: UsageWindow(utilization: 77, resetsAt: Date().addingTimeInterval(3600), duration: UsageWindow.sessionDuration),
            sevenDay: nil, contextUsedPercent: nil, contextWindowSize: nil, modelId: nil, modelDisplayName: nil,
            costUSD: nil, sessionName: nil, claudeCodeVersion: nil))
        #expect(store.usage["uuid:" + Home.biiosUUID]?.fiveHour?.utilization == 77)
        #expect(store.usage[fake.path(".claude-windows/1bf3e8f92b11")] == nil)
    }
}
