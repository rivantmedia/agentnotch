import Foundation
import Testing
@testable import ClaudeControl

extension ParallelProfilesHome {
    /// The manifest with `created` apart from `stores` (the rest adopted).
    func writeManifest(stores: [String], created: [String]) throws {
        try writeJSON(".claude-windows/.manifest.json", [
            "stores": stores.map { path($0) }, "created": created.map { path($0) },
            "customOAuth": false, "defaultConfigDir": NSNull(),
        ])
    }

    /// Claude Parallel Profiles mirrors `email`'s account into `~/.claude`:
    /// it rewrites `~/.claude.json`'s email and keeps its `accountUuid`.
    func mirrorIntoDefault(keepingUuid uuid: String, email: String, at date: Date) throws {
        try writeJSON(".claude.json", login(uuid: uuid, email: email))
        try FileManager.default.setAttributes([.modificationDate: date], ofItemAtPath: path(".claude.json"))
    }

    func setModified(_ relative: String, _ date: Date) throws {
        try FileManager.default.setAttributes([.modificationDate: date], ofItemAtPath: path(relative))
    }
}

/// A status line update from a folder, for a session.
@MainActor
private func statusLine(_ session: String, folder: String, env: String?, five: Double, resetIn: TimeInterval,
                        at: Date = Date()) -> StatusLineUpdate {
    StatusLineUpdate(sessionId: session, transcriptPath: nil, configDirEnv: env, accountId: folder, receivedAt: at,
                     fiveHour: UsageWindow(utilization: five, resetsAt: Date().addingTimeInterval(resetIn),
                                           duration: UsageWindow.sessionDuration),
                     sevenDay: nil, contextUsedPercent: nil, contextWindowSize: nil, modelId: nil,
                     modelDisplayName: nil, costUSD: nil, sessionName: nil, claudeCodeVersion: nil)
}

// MARK: - UX-1, PP-C4, S6: adopted profiles, unreadable manifests

@MainActor
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
struct PPFix_AdoptedProfileTests {
    typealias Home = ParallelProfilesHome
    let defaults = TestDefaults()
    let createdStores = [".claude-claude", ".claude-paras", ".claude-paras-rivant-in"]

    private func manager(_ registry: AccountRegistry) -> AccountHookManager {
        AccountHookManager(registry: registry, settings: defaults.store, environment: AccountHookManager.Environment(
            installsDisabled: { false }, isRunning: { _ in false }, python: { "python3" },
            binaryVersions: { _ in [ClaudeCodeVersion(major: 2, minor: 1, patch: 280)] }, forgetBinary: {},
            hookScript: { EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket) },
            statusLineScript: { EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket) },
            observesWorkspace: false))
    }

    /// The user's own terminal profile, signed in, with settings of its own.
    private func addWorkProfile(_ fake: Home) throws {
        try fake.mkdir(".claude-work/projects")
        try fake.writeJSON(".claude-work/.claude.json", fake.login(uuid: "u-work", email: "me@work.dev"))
        try fake.write(".claude-work/settings.json", "{\n  \"model\": \"opus\"\n}\n")
    }

    @Test func anAdoptedProfileIsARunFolderAndAMadeStoreIsAStore() throws {
        let fake = Home("fix-adopted-classify")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        try addWorkProfile(fake)
        try fake.writeManifest(stores: createdStores + [".claude-work"], created: createdStores)

        let layout = ConfigDirClassifier.classify(ConfigDirClassifier.readSnapshot(home: fake.home, isAlive: { _ in false }))
        #expect(layout.kind(of: fake.path(".claude-work")) == .run)
        #expect(layout.adoptedByExtension == [fake.path(".claude-work")])
        #expect(layout.stores == createdStores.map(fake.path))
        #expect(!HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude-work"), home: fake.home))
        // A made store without its marker is still a store (listed in `created`).
        try FileManager.default.removeItem(atPath: fake.path(".claude-paras/" + ParallelProfiles.storeMarkerName))
        #expect(HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude-paras"), home: fake.home))

        // It has hooks to get and a usage check to run.
        let registry = fake.registry()
        let work = try #require(registry.identities.first { $0.email == "me@work.dev" })
        #expect(work.runDirs.map(\.configDir) == [fake.path(".claude-work")] && work.storeDirs.isEmpty)
        #expect(UsageProbePlanner.probeFolder(runDirs: work.runDirs, activity: [nil], home: fake.home,
                                              prefersOwnFolders: true)?.configDir == fake.path(".claude-work"))
        #expect(work.terminalLaunchCommand?.contains(".claude-work") == true)
        let summary = ClaudeHostProjections.account(identity: work, hookStatuses: [:],
                                                    adopted: registry.layout.adoptedByExtension, home: fake.home)
        #expect(summary.adoptedDirs == [fake.path(".claude-work")])
    }

    /// The user's own profile keeps our hooks when the extension adopts it.
    @Test func hooksSurviveTheExtensionAdoptingTheProfile() async throws {
        let fake = Home("fix-adopted-hooks")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout(vibeNotch: false)
        try addWorkProfile(fake)
        let registry = fake.registry()
        let manager = manager(registry)
        manager.grantConsent(takeOverFromVibeNotch: false)
        await manager.waitUntilIdle()
        #expect(HookInstaller.readStatus(configDir: fake.path(".claude-work")).hooksInstalled)

        // The extension adopts it: `stores`, not `created`, no marker.
        try fake.writeManifest(stores: createdStores + [".claude-work"], created: createdStores)
        await registry.discoverNow()
        manager.installAll()
        await manager.waitUntilIdle()
        #expect(registry.layout.kind(of: fake.path(".claude-work")) == .run)
        let status = HookInstaller.readStatus(configDir: fake.path(".claude-work"))
        #expect(status.hooksInstalled && status.statusLineInstalled)
        #expect(!FileManager.default.fileExists(atPath: fake.path(".claude-work/" + ParallelProfiles.storeMarkerName)))
    }

    /// A manifest caught mid-write never turns a store into a run folder.
    @Test func anUnreadableManifestKeepsTheStoresStores() async throws {
        let fake = Home("fix-manifest-unreadable")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false, markers: false)
        let registry = fake.registry()
        #expect(registry.layout.kind(of: fake.path(".claude-paras")) == .store)

        try fake.write(".claude-windows/.manifest.json", "{\"stores\": [\"" + fake.path(".claude-paras"))
        await registry.discoverNow()
        #expect(registry.layout.kind(of: fake.path(".claude-paras")) == .store)
        #expect(registry.layout.kind(of: fake.path(".claude-claude")) == .store)
        #expect(HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude-paras"), home: fake.home))
        // Without the last classification they would have been run folders.
        let fresh = ConfigDirClassifier.classify(ConfigDirClassifier.readSnapshot(home: fake.home, isAlive: { _ in false }))
        #expect(fresh.kind(of: fake.path(".claude-paras")) == .run)
        // ~/.claude and the windows are never refused.
        #expect(!HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude"), home: fake.home))
        #expect(!HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude-windows/801f9dd51396"), home: fake.home))
    }
}

// MARK: - PP-C2: ~/.claude attributed by when the session started

@MainActor
@Suite(.serialized)
struct PPFix_AttributionTests {
    typealias Home = ParallelProfilesHome

    @Test func theTimelineTellsWhoTheFolderRanAsAndWhenItCantTell() {
        let t0 = Date(timeIntervalSince1970: 1_000_000)
        var timeline = FolderIdentityTimeline()
        let first = timeline.observe("uuid:p", rawUuid: "p", modifiedAt: t0, at: t0.addingTimeInterval(10))
        let same = timeline.observe("uuid:p", rawUuid: "p", modifiedAt: t0.addingTimeInterval(15), at: t0.addingTimeInterval(20))
        // The extension mirrors b in: same UUID, another email.
        let mirrored = timeline.observe("uuid:b", rawUuid: "p", modifiedAt: t0.addingTimeInterval(25), at: t0.addingTimeInterval(30))
        #expect(first && !same && mirrored)
        #expect(timeline.identity(at: t0.addingTimeInterval(-1)) == .unknown)
        #expect(timeline.identity(at: t0.addingTimeInterval(5)) == .identity("uuid:p"))
        #expect(timeline.identity(at: t0.addingTimeInterval(22)) == .switching)
        #expect(timeline.identity(at: t0.addingTimeInterval(26)) == .identity("uuid:b"))
        // A real login (the file's UUID changed) may take running sessions with it.
        timeline.observe("uuid:x", rawUuid: "x", modifiedAt: t0.addingTimeInterval(40), at: t0.addingTimeInterval(45))
        #expect(timeline.identity(at: t0.addingTimeInterval(5)) == .switching)
        #expect(timeline.identity(at: t0.addingTimeInterval(41)) == .identity("uuid:x"))

        // After a relaunch, a write nobody watched leaves the time between open.
        var resumed = FolderIdentityTimeline()
        resumed.observe("uuid:p", rawUuid: "p", modifiedAt: t0, at: t0.addingTimeInterval(10))
        resumed.observe("uuid:p", rawUuid: "p", modifiedAt: t0.addingTimeInterval(100), at: t0.addingTimeInterval(200), resumed: true)
        #expect(resumed.identity(at: t0.addingTimeInterval(50)) == .switching)
        #expect(resumed.identity(at: t0.addingTimeInterval(150)) == .identity("uuid:p"))

        // Only the mirrored ~/.claude is attributed by time; others by who they name now.
        #expect(FolderAttribution.attribute(current: "uuid:b", timeline: timeline, startedAt: t0.addingTimeInterval(5), mirrored: false) == .known("uuid:b"))
        #expect(FolderAttribution.attribute(current: "uuid:b", timeline: timeline, startedAt: nil, mirrored: true) == .unsure(current: "uuid:b"))
    }

    /// The finder's scenario: a Rivant terminal session in ~/.claude keeps
    /// ticking after the extension mirrored Biios in. Its later-resetting
    /// reading must not land on (and pin) Biios's ring.
    @Test func aMirrorDoesntHandATerminalSessionsLimitsToTheOtherAccount() async throws {
        let fake = Home("fix-attribution")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let now = Date()
        try fake.setModified(".claude.json", now.addingTimeInterval(-3600))
        let starts: [String: Date] = ["rivant-terminal": now.addingTimeInterval(-1800)]
        var lateStart: Date?
        let registry = fake.registry()
        let store = UsageStore(registry: registry, configReader: ClaudeGlobalConfigReader(),
                               stateStore: UsageStateStore(directory: URL(fileURLWithPath: fake.support, isDirectory: true)),
                               externalSource: nil, readsExternalUsage: { false },
                               probeRunner: { _ in .failed("no probes here") }, automaticProbes: false,
                               sessionStartedAt: { id in id == "biios-terminal" ? lateStart : starts[id] })
        let parasId = "uuid:" + Home.parasUUID
        let biiosId = "uuid:" + Home.biiosUUID
        let window = fake.path(".claude-windows/1bf3e8f92b11")
        let main = fake.path(".claude")

        store.ingest(statusLine("biios-window", folder: window, env: window, five: 10, resetIn: 3600))
        #expect(store.usage[biiosId]?.fiveHour?.utilization == 10)

        // The extension mirrors Biios into ~/.claude.
        try fake.mirrorIntoDefault(keepingUuid: Home.parasUUID, email: Home.biios, at: Date())
        await registry.refreshIdentities()
        #expect(registry.identity(forFolderId: main)?.id == biiosId)

        // Rivant's terminal session (started before) ticks: 70%, reset in 4 h.
        store.ingest(statusLine("rivant-terminal", folder: main, env: nil, five: 70, resetIn: 4 * 3600))
        #expect(store.usage[biiosId]?.fiveHour?.utilization == 10)
        #expect(store.usage[parasId]?.fiveHour?.utilization == 70)
        // Biios's own next tick in the same window shows.
        store.ingest(statusLine("biios-window", folder: window, env: window, five: 12, resetIn: 3600))
        #expect(store.usage[biiosId]?.fiveHour?.utilization == 12)
        // A ~/.claude session nobody can place is left out.
        store.ingest(statusLine("who-knows", folder: main, env: nil, five: 99, resetIn: 5 * 3600))
        #expect(store.usage[biiosId]?.fiveHour?.utilization == 12)
        #expect(store.usage[parasId]?.fiveHour?.utilization == 70)

        // A terminal started after the mirror is Biios's; its reading counts
        // until Biios's own folders report again.
        try await Task.sleep(for: .milliseconds(1100))
        lateStart = Date()
        store.ingest(statusLine("biios-terminal", folder: main, env: nil, five: 30, resetIn: 3 * 3600))
        #expect(store.usage[biiosId]?.fiveHour?.utilization == 30)
        store.ingest(statusLine("biios-window", folder: window, env: window, five: 13, resetIn: 3600))
        #expect(store.usage[biiosId]?.fiveHour?.utilization == 13)
    }

    /// The combination rule alone: a reading through a mirrored ~/.claude
    /// counts only until the account's own folders report after it.
    @Test func ownFoldersReplaceAReadingThatCameThroughTheDefault() {
        let now = Date()
        func reading(_ value: Double, reset: TimeInterval, at: TimeInterval) -> UsageStore.Reading {
            (UsageWindow(utilization: value, resetsAt: now.addingTimeInterval(reset), duration: UsageWindow.sessionDuration),
             now.addingTimeInterval(at))
        }
        let viaDefault = reading(70, reset: 4 * 3600, at: 10)
        let own = reading(12, reset: 3600, at: 20)
        #expect(UsageStore.combinedStatus(["/h/.claude": viaDefault, "/h/w": own], defaultFolder: "/h/.claude", mirrorsDefault: true)?.window.utilization == 12)
        #expect(UsageStore.combinedStatus(["/h/.claude": reading(70, reset: 4 * 3600, at: 30), "/h/w": own],
                                          defaultFolder: "/h/.claude", mirrorsDefault: true)?.window.utilization == 70)
        // Without the extension, the most current reading wins, as always.
        #expect(UsageStore.combinedStatus(["/h/.claude": viaDefault, "/h/w": own], defaultFolder: "/h/.claude", mirrorsDefault: false)?.window.utilization == 70)
    }

    /// The hub and the socket server read the same rule off the main thread.
    @Test func anUntrackedAccountsSessionIsToldApartByWhenItStarted() {
        let t0 = Date(timeIntervalSince1970: 2_000_000)
        var snapshot = FolderRings.Snapshot()
        snapshot.defaultFolder = "/h/.claude"
        snapshot.mirrorsDefault = true
        snapshot.identityOfFolder = ["/h/.claude": "uuid:b", "/h/w": "uuid:b"]
        snapshot.untrackedIdentities = ["uuid:b"]
        snapshot.untrackedFolders = ["/h/.claude", "/h/w"]
        snapshot.defaultTimeline.observe("uuid:p", rawUuid: "p", modifiedAt: t0, at: t0.addingTimeInterval(10))
        snapshot.defaultTimeline.observe("uuid:b", rawUuid: "p", modifiedAt: t0.addingTimeInterval(20), at: t0.addingTimeInterval(30))
        // Started as the tracked account: not held back.
        #expect(!snapshot.isUntracked(folder: "/h/.claude", startedAt: t0.addingTimeInterval(5)))
        // Started as the untracked one, or can't tell: its prompts go to the terminal.
        #expect(snapshot.isUntracked(folder: "/h/.claude", startedAt: t0.addingTimeInterval(25)))
        #expect(snapshot.isUntracked(folder: "/h/.claude", startedAt: nil))
        #expect(snapshot.isUntracked(folder: "/h/w", startedAt: nil))
        #expect(!snapshot.isUntracked(folder: "/h/other", startedAt: nil))
    }
}

// MARK: - PP-C5, UX-4: tracking doesn't follow the mirror

@MainActor
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
struct PPFix_MirrorTrackingTests {
    typealias Home = ParallelProfilesHome
    let defaults = TestDefaults()

    private func manager(_ registry: AccountRegistry) -> AccountHookManager {
        AccountHookManager(registry: registry, settings: defaults.store, environment: AccountHookManager.Environment(
            installsDisabled: { false }, isRunning: { _ in false }, python: { "python3" },
            binaryVersions: { _ in [ClaudeCodeVersion(major: 2, minor: 1, patch: 280)] }, forgetBinary: {},
            hookScript: { EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket) },
            statusLineScript: { EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket) },
            observesWorkspace: false))
    }

    private func backups(_ fake: Home) -> [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: fake.path(".claude"))) ?? [])
            .filter { $0.hasPrefix("settings.json.") }.sorted()
    }

    @Test func mirroringAnUntrackedAccountInAndOutLeavesSettingsAlone() async throws {
        let fake = Home("fix-mirror-untracked")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let manager = manager(registry)
        manager.grantConsent(takeOverFromVibeNotch: false)
        await manager.waitUntilIdle()
        let biiosId = "uuid:" + Home.biiosUUID
        manager.setTracked(accountId: biiosId, false)
        await manager.waitUntilIdle()
        let settings = fake.path(".claude/settings.json")
        let before = try Data(contentsOf: URL(fileURLWithPath: settings))
        let backupsBefore = backups(fake)

        for (email, holder) in [(Home.biios, biiosId), (Home.paras, "uuid:" + Home.parasUUID), (Home.biios, biiosId)] {
            try fake.mirrorIntoDefault(keepingUuid: Home.parasUUID, email: email, at: Date())
            await registry.refreshIdentities()
            #expect(registry.identity(forFolderId: fake.path(".claude"))?.id == holder)
            manager.installAll()
            await manager.waitUntilIdle()
            #expect(try Data(contentsOf: URL(fileURLWithPath: settings)) == before)
            #expect(backups(fake) == backupsBefore)
            #expect(HookInstaller.readStatus(configDir: fake.path(".claude")).hooksInstalled)
        }
        // The untracked account's window still has none.
        #expect(!HookInstaller.readStatus(configDir: fake.path(".claude-windows/1bf3e8f92b11")).hooksRegistered)
    }

    /// "Forget…" doesn't come and go with the focused window.
    @Test func anExtensionAccountCanBeForgottenWhoeverHoldsTheDefault() throws {
        let fake = Home("fix-forget")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        try fake.mirrorIntoDefault(keepingUuid: Home.parasUUID, email: Home.biios, at: Date())
        let registry = fake.registry()
        let biios = try #require(registry.identity(id: "uuid:" + Home.biiosUUID))
        #expect(biios.includesDefault && biios.canBeForgotten)
        let paras = try #require(registry.identity(id: "uuid:" + Home.parasUUID))
        #expect(paras.canBeForgotten)
        // Without the extension, the account ~/.claude runs as stays.
        let plain = ClaudeIdentityAccount(id: "uuid:x", ringID: "claude-acct-x", runDirs: [ClaudeAccount(configDir: fake.path(".claude"))],
                                          storeDirs: [], customLabel: nil, colorIndex: 0, isHidden: false, includesDefault: true)
        #expect(!plain.canBeForgotten)
    }
}

// MARK: - PP-C1, S5, PP-C8: migration follows ~/.claude's own UUID

@MainActor
@Suite(.serialized)
struct PPFix_MigrationTests {
    typealias Home = ParallelProfilesHome

    /// The real half-mirrored state: ~/.claude.json names paras's UUID with
    /// Biios's email. Rivant gets `claude`'s choices, Biios `claude-claude`'s.
    @Test func theDefaultsOldRingGoesToItsOwnerNotTheMirroredAccount() throws {
        let fake = Home("fix-migration")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        try fake.mirrorIntoDefault(keepingUuid: Home.parasUUID, email: Home.biios, at: Date())
        let registry = fake.registry()
        let parasId = "uuid:" + Home.parasUUID
        #expect(registry.identity(forFolderId: fake.path(".claude"))?.id == "uuid:" + Home.biiosUUID)
        #expect(registry.defaultOwner == parasId)

        let owner = ClaudeControlHub.defaultRingOwner(registry)
        let accounts = registry.identities.map {
            ClaudeHostProjections.account(identity: $0, hookStatuses: [:], defaultRing: owner, home: fake.home)
        }
        let paras = try #require(accounts.first { $0.id == parasId })
        let biios = try #require(accounts.first { $0.id != parasId })
        #expect(paras.formerRingIDs.first == "claude")
        #expect(!biios.formerRingIDs.contains("claude"))

        let stored = ClaudeRingMigration.Stored(
            nicknames: ["claude": "Main", "claude-claude": "Side"],
            seen: ["claude", "claude-shared", "claude-claude", "claude-paras", "claude-paras-rivant-in"],
            connected: ["claude", "claude-shared", "claude-claude", "claude-paras-rivant-in"],   // claude-paras off
            order: ["claude", "claude-shared", "claude-claude", "claude-paras", "claude-paras-rivant-in"],
            muted: ["claude"], menuBar: ["claude-claude", "codex"])
        let plan = ClaudeRingMigration.plan(accounts: accounts, retired: ["claude-shared"], stored: stored)
        #expect(plan.nicknames[paras.ringID] == .some("Main"))
        #expect(plan.nicknames[biios.ringID] == .some("Side"))
        // On when any of its old rings was (claude-paras was off).
        #expect(plan.connected[paras.ringID] ?? true)
        #expect(plan.order?.first == paras.ringID)
        // Muted alerts and the menu-bar choice move too; old ids go (PP-C8).
        #expect(plan.muted[paras.ringID] == true && plan.muted["claude"] == false)
        #expect(plan.muted[biios.ringID] == nil)
        #expect(plan.menuBar[biios.ringID] == true && plan.menuBar["claude-claude"] == false)
        #expect(plan.menuBar[paras.ringID] == nil)
    }

    /// When ~/.claude's own UUID names nobody known, its ring still moves,
    /// but only when no other old ring of the account has a choice.
    @Test func anUncertainOldRingRanksLast() {
        var account = ClaudeAccountSummary(id: "uuid:b", ringID: "claude-acct-b", configDir: "/h", label: "B",
                                           isDefault: true, launchCommand: "claude")
        account.formerRingIDs = ["claude", "claude-claude"]
        account.uncertainFormerRingIDs = ["claude"]
        let stored = ClaudeRingMigration.Stored(nicknames: ["claude": "Main", "claude-claude": "Side"],
                                                seen: ["claude", "claude-claude"], connected: ["claude-claude"],
                                                order: ["claude", "claude-claude"])
        let plan = ClaudeRingMigration.plan(accounts: [account], retired: [], stored: stored)
        #expect(plan.nicknames["claude-acct-b"] == .some("Side"))
        #expect(plan.connected["claude-acct-b"] ?? true)
    }

    /// accounts.json from before identities: ~/.claude's name and colour go
    /// to its owner; Biios takes claude-claude's (untracked).
    @Test func savedFolderChoicesGoToTheDefaultsOwner() throws {
        let fake = Home("fix-migration-prefs")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        try fake.mirrorIntoDefault(keepingUuid: Home.parasUUID, email: Home.biios, at: Date())
        let saved: [String: Any] = [
            "version": 1, "removedIds": [],
            "accounts": [
                ["id": fake.path(".claude"), "configDir": fake.path(".claude"), "customLabel": "Work (Rivant)",
                 "colorIndex": 3, "isHidden": false, "source": "discovered"],
                ["id": fake.path(".claude-claude"), "configDir": fake.path(".claude-claude"), "configDirEnv": fake.path(".claude-claude"),
                 "colorIndex": 5, "isHidden": true, "source": "discovered"],
            ],
        ]
        try FileManager.default.createDirectory(atPath: fake.support, withIntermediateDirectories: true)
        try JSONSerialization.data(withJSONObject: saved).write(to: URL(fileURLWithPath: fake.support + "/accounts.json"))
        let registry = fake.registry()
        let paras = try #require(registry.identity(id: "uuid:" + Home.parasUUID))
        let biios = try #require(registry.identity(id: "uuid:" + Home.biiosUUID))
        #expect(paras.customLabel == "Work (Rivant)" && paras.colorIndex == 3 && !paras.isHidden)
        #expect(biios.customLabel == nil && biios.isHidden)
    }

    /// The last reading saved for ~/.claude is its owner's, under its id.
    @Test func theDefaultsSavedReadingGoesToItsOwner() throws {
        let fake = Home("fix-migration-usage")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        try fake.mirrorIntoDefault(keepingUuid: Home.parasUUID, email: Home.biios, at: Date())
        let registry = fake.registry()
        let directory = URL(fileURLWithPath: fake.support, isDirectory: true)
        var state = UsageState()
        state.accounts[fake.path(".claude")] = UsageState.Account(lastFullReading: AccountUsage(
            accountId: fake.path(".claude"),
            fiveHour: UsageWindow(utilization: 66, resetsAt: Date().addingTimeInterval(3600), duration: UsageWindow.sessionDuration),
            source: .probe, updatedAt: Date()))
        UsageStateStore(directory: directory).saveNow(state)
        let store = UsageStore(registry: registry, configReader: ClaudeGlobalConfigReader(), stateStore: UsageStateStore(directory: directory),
                               externalSource: nil, readsExternalUsage: { false }, probeRunner: { _ in .failed("none") },
                               automaticProbes: false)
        store.restoreState()
        let parasId = "uuid:" + Home.parasUUID
        #expect(store.usage[parasId]?.fiveHour?.utilization == 66)
        #expect(store.usage[parasId]?.accountId == parasId)
        #expect(store.usage["uuid:" + Home.biiosUUID]?.fiveHour?.utilization != 66)
    }
}

// MARK: - PP-C3, S6: where and when the usage check runs

@MainActor
@Suite(.serialized)
struct PPFix_ProbeTests {
    typealias Home = ParallelProfilesHome

    final class Probe: @unchecked Sendable {
        let lock = NSLock()
        var requests: [UsageStore.ProbeRequest] = []
        var during: (@Sendable () -> Void)?
        func run(_ request: UsageStore.ProbeRequest) async -> UsageProbe.Outcome {
            lock.withLock { requests.append(request) }
            during?()
            return .usage(UsageParser.parseUsageBody([
                "five_hour": ["utilization": 40, "resets_at": "2099-01-01T00:00:00Z"],
            ]))
        }
    }

    private func store(_ registry: AccountRegistry, probe: Probe, support: String) -> UsageStore {
        UsageStore(registry: registry, configReader: ClaudeGlobalConfigReader(),
                   stateStore: UsageStateStore(directory: URL(fileURLWithPath: support, isDirectory: true)),
                   externalSource: nil, readsExternalUsage: { false },
                   probeRunner: { await probe.run($0) }, automaticProbes: true)
    }

    @Test func withTheExtensionTheWindowsFolderComesFirst() {
        let main = ClaudeAccount(configDir: "/h/.claude")
        let window = ClaudeAccount(configDir: "/h/.claude-windows/801f9dd51396", configDirEnv: "/h/.claude-windows/801f9dd51396")
        let now = Date()
        #expect(UsageProbePlanner.probeFolder(runDirs: [main, window], activity: [now, now.addingTimeInterval(-600)], home: "/h",
                                              prefersOwnFolders: true)?.id == window.id)
        #expect(UsageProbePlanner.probeFolder(runDirs: [main], activity: [now], home: "/h", prefersOwnFolders: true)?.id == main.id)
    }

    /// A mirror during the check swaps ~/.claude's token: the answer would be
    /// the other account's, so it is thrown away (no failure, no backoff).
    @Test func anAnswerFromAFolderThatChangedHandsIsDiscarded() async throws {
        let fake = Home("fix-probe-discard")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        // Rivant runs only in ~/.claude (no window open).
        for id in ["801f9dd51396", "b9fbb9ecd7cb"] { try FileManager.default.removeItem(atPath: fake.path(".claude-windows/" + id)) }
        let registry = fake.registry()
        let probe = Probe()
        // The extension mirrors Biios in while the check runs.
        var mirrored = fake.login(uuid: Home.parasUUID, email: Home.biios)
        mirrored.removeValue(forKey: "projects")
        let bytes = try JSONSerialization.data(withJSONObject: mirrored)
        let file = URL(fileURLWithPath: fake.path(".claude.json"))
        probe.during = { try? bytes.write(to: file) }
        let store = store(registry, probe: probe, support: fake.support)
        let parasId = "uuid:" + Home.parasUUID
        await store.refresh(accountId: parasId, reason: .forced)
        #expect(probe.lock.withLock { probe.requests.count } == 1)
        #expect(store.usage[parasId]?.source != .probe)
        #expect(store.fetchState[parasId] == .idle)
    }

    /// Right before it runs: a folder that turned out to be a store is refused.
    @Test func aFolderThatBecameAStoreIsNeverChecked() async throws {
        let fake = Home("fix-probe-store")
        defer { fake.cleanUp() }
        try fake.mkdir(".claude/projects")
        try fake.writeJSON(".claude.json", fake.login(uuid: "u-me", email: "me@x.dev"))
        try fake.mkdir(".claude-solo/projects")
        try fake.writeJSON(".claude-solo/.claude.json", fake.login(uuid: "u-solo", email: "solo@x.dev"))
        let registry = fake.registry()
        let solo = try #require(registry.identities.first { $0.email == "solo@x.dev" })
        try fake.write(".claude-solo/" + ParallelProfiles.storeMarkerName, "")
        let probe = Probe()
        let store = store(registry, probe: probe, support: fake.support)
        await store.refresh(accountId: solo.id, reason: .forced)
        #expect(probe.lock.withLock { probe.requests }.isEmpty)
        #expect(store.fetchState[solo.id] == UsageStore.noRunFolder)
    }
}

// MARK: - S2: a yes given to an earlier build

@MainActor
@Suite(.serialized)
struct PPFix_ConsentScopeTests {
    typealias Home = ParallelProfilesHome
    let defaults = TestDefaults()

    @Test func anOlderYesNamesTheNewFoldersOnce() throws {
        let fake = Home("fix-scope")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let settings = defaults.store
        settings.hookConsent = true
        settings.hooksEnabled = true
        let manager = AccountHookManager(registry: registry, settings: settings, environment: AccountHookManager.Environment(
            installsDisabled: { true }, isRunning: { _ in false }, python: { "python3" }, binaryVersions: { _ in [] },
            forgetBinary: {}, hookScript: { nil }, statusLineScript: { nil }, observesWorkspace: false))
        let windows = ["1bf3e8f92b11", "801f9dd51396", "b9fbb9ecd7cb"].map { fake.path(".claude-windows/" + $0) }
        #expect(manager.foldersBeyondConsent.sorted() == windows)
        #expect(manager.setupState(isSealed: false, socketError: nil).newInstallFolders.count == 3)
        manager.acknowledgeInstallScope()
        #expect(manager.foldersBeyondConsent.isEmpty)
        #expect(settings.hookConsentScope == AccountHookManager.installScope)

        // A yes given now covers them: nothing to say.
        settings.hookConsentScope = 0
        settings.hookConsent = nil
        #expect(manager.foldersBeyondConsent.isEmpty)
        #expect(manager.grantConsent(takeOverFromVibeNotch: false))
        #expect(manager.foldersBeyondConsent.isEmpty)
    }
}

// MARK: - S3, S7, S8: what earlier builds left in stores

@MainActor
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
struct PPFix_LeftoverTests {
    typealias Home = ParallelProfilesHome
    let defaults = TestDefaults()

    private func manager(_ registry: AccountRegistry) -> AccountHookManager {
        AccountHookManager(registry: registry, settings: defaults.store, environment: AccountHookManager.Environment(
            installsDisabled: { false }, isRunning: { _ in false }, python: { "python3" },
            binaryVersions: { _ in [ClaudeCodeVersion(major: 2, minor: 1, patch: 280)] }, forgetBinary: {},
            hookScript: { EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket) },
            statusLineScript: { EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket) },
            observesWorkspace: false))
    }

    /// What the gated build left in a folder: our hooks (installed there
    /// after its takeover), its `{}` backup, the takeover's backup of
    /// Superpowered Vibe Notch's file, and that app's scripts.
    private func leaveGatedBuildState(_ fake: Home, in folder: String) throws {
        let scratch = fake.path(".claude-windows/0000scratch00")
        try fake.mkdir(".claude-windows/0000scratch00")
        let configuration = HookInstaller.Configuration(python: "python3", version: nil, statusLineIntegration: true,
                                                        hookScript: "# hook", statusLineScript: "# status")
        #expect(HookInstaller.install(configDir: scratch, configuration: configuration) == .installed)
        let ours = try String(contentsOfFile: scratch + "/settings.json", encoding: .utf8)
            .replacingOccurrences(of: scratch, with: fake.path(folder))
        try FileManager.default.removeItem(atPath: scratch)
        try fake.write(folder + "/settings.json", ours)
        try fake.write(folder + "/hooks/superpowered-codenotch-hook.py", "# hook")
        try fake.write(folder + "/hooks/superpowered-codenotch-statusline.py", "# status")
        try fake.write(folder + "/settings.json.superpowered-codenotch-20260925-091159-860.bak", "{}")
        try fake.write(folder + "/settings.json.superpowered-codenotch-20260925-091159-980.bak", ours)
        try fake.write(folder + "/" + HookInstaller.originalBackupName, Home.vibeNotchOnlySettings(configDir: fake.path(folder)))
        try TakeoverFixture.writeVibeNotchFiles(configDir: fake.path(folder), previous: nil)
    }

    @Test func theUpgradeLeavesStoresAsTheExtensionMadeThem() async throws {
        let fake = Home("fix-leftovers")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout(vibeNotch: false)
        let cleaned = [".claude-paras", ".claude-paras-rivant-in", ".claude-claude", ".claude-shared"]
        for folder in cleaned { try leaveGatedBuildState(fake, in: folder) }
        let registry = fake.registry()
        defaults.store.hookConsent = true
        defaults.store.hooksEnabled = true
        let manager = manager(registry)
        manager.installAll()
        await manager.waitUntilIdle()
        for folder in cleaned {
            let left = ((try? FileManager.default.contentsOfDirectory(atPath: fake.path(folder))) ?? [])
                .filter { $0.hasPrefix("settings.json") || $0 == "hooks" }
            #expect(left.isEmpty, "\(folder): \(left)")
        }
        #expect(manager.vibeNotchCleanupDirs.isEmpty)
        // The markers, logins and links are untouched.
        #expect(FileManager.default.fileExists(atPath: fake.path(".claude-paras/" + ParallelProfiles.storeMarkerName)))
        #expect(FileManager.default.fileExists(atPath: fake.path(".claude-paras/.claude.json")))
    }

    /// Nothing of the user's is ever lost: a backup with their settings keeps the file.
    @Test func aBackupWithTheUsersSettingsKeepsTheFile() {
        #expect(VibeNotchLeftovers.holdsNoUserContent(Data("{}".utf8)))
        #expect(VibeNotchLeftovers.holdsNoUserContent(Data(" \n".utf8)))
        #expect(VibeNotchLeftovers.holdsNoUserContent(Data(Home.vibeNotchOnlySettings(configDir: "/h/.claude-x").utf8), home: "/h"))
        #expect(!VibeNotchLeftovers.holdsNoUserContent(Data(#"{"model": "opus"}"#.utf8)))
        #expect(!VibeNotchLeftovers.holdsNoUserContent(Data("{ nope".utf8)))
    }

    /// An unreadable settings.json may still run Superpowered Vibe Notch's
    /// scripts: they stay until it can be read (S7).
    @Test func scriptsStayWhileASettingsFileCantBeRead() async throws {
        let fake = Home("fix-leftovers-unreadable")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout(vibeNotch: false)
        try TakeoverFixture.writeVibeNotchFiles(configDir: fake.path(".claude-paras"), previous: nil)
        try fake.write(".claude-windows/b9fbb9ecd7cb/settings.json", "{ \"hooks\": ")
        #expect(VibeNotchLeftovers.referencedScriptsIfAllReadable(configDirs: [fake.path(".claude-windows/b9fbb9ecd7cb")]) == nil)
        let registry = fake.registry()
        defaults.store.hookConsent = true
        defaults.store.hooksEnabled = true
        let manager = manager(registry)
        manager.installAll()
        await manager.waitUntilIdle()
        #expect(VibeNotchLeftovers.hasScripts(configDir: fake.path(".claude-paras")))

        try fake.write(".claude-windows/b9fbb9ecd7cb/settings.json", "{}\n")
        manager.installAll()
        await manager.waitUntilIdle()
        #expect(!VibeNotchLeftovers.hasScripts(configDir: fake.path(".claude-paras")))
    }

    /// Turning off leaves no empty `hooks/` behind (S8).
    @Test func uninstallRemovesAnEmptyHooksFolder() throws {
        let fake = Home("fix-uninstall-hooks")
        defer { fake.cleanUp() }
        try fake.mkdir(".claude-windows/801f9dd51396")
        let folder = fake.path(".claude-windows/801f9dd51396")
        let configuration = HookInstaller.Configuration(python: "python3", version: nil, statusLineIntegration: true,
                                                        hookScript: "# hook", statusLineScript: "# status")
        #expect(HookInstaller.install(configDir: folder, configuration: configuration) == .installed)
        #expect(FileManager.default.fileExists(atPath: folder + "/hooks"))
        #expect(HookInstaller.uninstall(configDir: folder) == .removed)
        #expect(!FileManager.default.fileExists(atPath: folder + "/hooks"))
    }
}

// MARK: - PP-C7, UX-9, PP-C10, UX-6, UX-3

struct PPFix_GroupingTests {
    /// After the extension forgot the account whose UUID ~/.claude still
    /// names, that UUID is nowhere else: ~/.claude joins its email's owner
    /// (one ring, not a second "claude@biios.in").
    @Test func aStaleUuidOnlyTheDefaultHasIsNoSecondAccount() {
        let main = ClaudeAccount(configDir: "/h/.claude", email: "claude@biios.in", accountUuid: "u-gone")
        let window = ClaudeAccount(configDir: "/h/.claude-windows/1bf3e8f92b11", configDirEnv: "/h/.claude-windows/1bf3e8f92b11",
                                   email: "claude@biios.in", accountUuid: "u-biios")
        let mirrored = AccountIdentityGrouping.identityKeys([main, window], mirroredDefault: "/h/.claude")
        #expect(mirrored["/h/.claude"] == AccountIdentityGrouping.Resolution(key: "uuid:u-biios", corrected: true))
        let grouping = AccountIdentityGrouping.group([main, window], prefs: [:], mirrorsDefault: true, home: "/h")
        #expect(grouping.identities.count == 1)
        #expect(grouping.defaultOwner == nil)
        // Without the extension nothing mirrors into ~/.claude: left as it is.
        #expect(AccountIdentityGrouping.identityKeys([main, window])["/h/.claude"]?.key == "uuid:u-gone")
    }

    /// One login in two organizations is two accounts, each with its own
    /// quota; a mirrored file's stale organization doesn't split anything.
    @Test func oneLoginInTwoOrganizationsIsTwoAccounts() {
        var personal = ClaudeAccount(configDir: "/h/.claude-me", configDirEnv: "/h/.claude-me", email: "me@x.dev", accountUuid: "u-1")
        personal.organizationUuid = "org-personal"
        var team = ClaudeAccount(configDir: "/h/.claude-team", configDirEnv: "/h/.claude-team", email: "me@x.dev", accountUuid: "u-1")
        team.organizationUuid = "org-team"
        let keys = AccountIdentityGrouping.identityKeys([personal, team])
        #expect(keys["/h/.claude-me"]?.key == "uuid:u-1/org-personal")
        #expect(keys["/h/.claude-team"]?.key == "uuid:u-1/org-team")
        let grouping = AccountIdentityGrouping.group([personal, team], prefs: [:], home: "/h")
        #expect(grouping.identities.count == 2)
        #expect(Set(grouping.identities.map(\.ringID)).count == 2)
        #expect(grouping.identities.allSatisfy { $0.accountUuid == "u-1" })
        #expect(Set(grouping.identities.compactMap(\.organizationScope)) == ["org-personal", "org-team"])
        // A mirrored ~/.claude keeps a stale UUID and organization: it joins
        // its email's owner, in the organization most of its folders have.
        var mirrored = ClaudeAccount(configDir: "/h/.claude", email: "me@x.dev", accountUuid: "u-gone")
        mirrored.organizationUuid = "org-stale"
        let withMirror = AccountIdentityGrouping.identityKeys([personal, team, mirrored], mirroredDefault: "/h/.claude")
        #expect(withMirror["/h/.claude"]?.corrected == true)
        #expect(withMirror["/h/.claude"]?.key == "uuid:u-1/org-personal")
        #expect(AccountIdentityGrouping.group([personal, team, mirrored], prefs: [:], mirrorsDefault: true, home: "/h").identities.count == 2)
        // One organization (however many folders): one account, the ring id as always.
        var again = team
        again.organizationUuid = "org-personal"
        #expect(AccountIdentityGrouping.identityKeys([personal, again])["/h/.claude-team"]?.key == "uuid:u-1")
    }

    /// A process names its config folder by a resolved path (a linked
    /// folder): it still goes to the folder the registry knows (PP-C10).
    @Test func aResolvedConfigFolderMapsToItsAlias() {
        let entry = SessionRegistryEntry(json: ["pid": 4242, "sessionId": "s-1", "cwd": "/h/repo", "startedAt": 1_790_000_000_000])
        let entries = [entry].compactMap { $0 }
        let result = SessionRegistryScanner.attribute(
            entries, aliases: ["/h/.claude-work"], isShared: true, defaultDir: "/h/.claude",
            configDirOfProcess: { _ in .set("/h/dotfiles/claude-work") },
            resolve: { $0 == "/h/.claude-work" ? "/h/dotfiles/claude-work" : $0 })
        #expect(result["/h/.claude-work"]?.count == entries.count)
        #expect(result["/h/dotfiles/claude-work"] == nil)
    }

    /// A VS Code workspace's folder is named after its project (UX-6).
    @Test func aWindowFolderIsNamedAfterItsProject() {
        #expect(WindowFolderNames.windowId(forWorkspace: "/Users/paras/Documents/GitHub/@paraswtf/superpowered-vibe-notch") == "801f9dd51396")
        let names = WindowFolderNames.names(windowDirs: ["/Users/paras/.claude-windows/801f9dd51396", "/Users/paras/.claude-windows/b9fbb9ecd7cb"],
                                            paths: ["/Users/paras/Documents/GitHub/@paraswtf/superpowered-vibe-notch/Sources/App"],
                                            home: "/Users/paras")
        #expect(names == ["/Users/paras/.claude-windows/801f9dd51396": "superpowered-vibe-notch"])
        #expect(WindowFolderNames.label("/Users/paras/.claude-windows/801f9dd51396", display: "~/x", names: names) == "VS Code · superpowered-vibe-notch")
        #expect(WindowFolderNames.label("/Users/paras/.claude-windows/b9fbb9ecd7cb", display: "~/y", names: names) == "~/y")
    }

    /// No terminal command points into a VS Code window's working copy or a
    /// store (UX-3, PP-C6).
    @Test func onlyATerminalFolderGetsALaunchCommand() {
        func identity(run: [ClaudeAccount], stores: [ClaudeAccount] = [], windows: [String] = [], includesDefault: Bool = false) -> ClaudeIdentityAccount {
            ClaudeIdentityAccount(id: "uuid:x", ringID: "claude-acct-x", runDirs: run, storeDirs: stores, customLabel: nil,
                                  colorIndex: 0, isHidden: false, includesDefault: includesDefault, windowDirIds: windows)
        }
        let main = ClaudeAccount(configDir: "/h/.claude")
        let window = ClaudeAccount(configDir: "/h/.claude-windows/1bf3e8f92b11", configDirEnv: "/h/.claude-windows/1bf3e8f92b11")
        let work = ClaudeAccount(configDir: "/h/.claude-work", configDirEnv: "/h/.claude-work")
        let store = ClaudeAccount(configDir: "/h/.claude-claude", configDirEnv: "/h/.claude-claude", kind: .store)
        #expect(identity(run: [main, window], windows: [window.id], includesDefault: true).terminalLaunchCommand == "claude")
        #expect(identity(run: [window], stores: [store], windows: [window.id]).terminalLaunchCommand == nil)
        #expect(identity(run: [], stores: [store]).terminalLaunchCommand == nil)
        #expect(identity(run: [window, work], windows: [window.id]).terminalLaunchCommand?.contains(".claude-work") == true)
    }
}
