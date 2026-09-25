import Foundation
import Testing
@testable import ClaudeControl

/// Hooks go into the folders Claude Code runs in (`~/.claude`, every VS
/// Code window, standalone folders) and nowhere else; the takeover from
/// Superpowered Vibe Notch cleans every folder it wrote to.
@MainActor
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
struct PP_HooksTests {
    typealias Home = ParallelProfilesHome
    let defaults = TestDefaults()

    private func manager(_ registry: AccountRegistry) -> AccountHookManager {
        AccountHookManager(registry: registry, settings: defaults.store, environment: AccountHookManager.Environment(
            installsDisabled: { false },
            isRunning: { _ in false },
            python: { "python3" },
            binaryVersions: { _ in [ClaudeCodeVersion(major: 2, minor: 1, patch: 280)] },
            forgetBinary: {},
            hookScript: { EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket) },
            statusLineScript: { EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket) },
            observesWorkspace: false
        ))
    }

    private let runFolders = [".claude", ".claude-windows/1bf3e8f92b11", ".claude-windows/801f9dd51396", ".claude-windows/b9fbb9ecd7cb"]
    private let stores = [".claude-paras", ".claude-paras-rivant-in", ".claude-claude"]

    /// Files that must never change: logins, markers, the manifest and the
    /// shared history.
    private func untouchable(_ files: [String: Data]) -> [String: Data] {
        files.filter { key, _ in
            key.hasSuffix(".claude.json") || key.hasSuffix(ParallelProfiles.storeMarkerName)
                || key.hasSuffix(".manifest.json") || key.contains(" ->")
                || (key.hasPrefix(".claude-shared/") && !key.hasPrefix(".claude-shared/settings.json") && !key.hasPrefix(".claude-shared/hooks"))
        }
    }

    @Test func consentInstallsIntoTheFourRunFoldersOnly() async throws {
        let fake = Home("hooks-consent")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let manager = manager(registry)
        #expect(manager.settingsFilesToEdit.sorted() == runFolders.map { fake.path($0) + "/settings.json" }.sorted())
        let before = fake.files()

        #expect(manager.grantConsent(takeOverFromVibeNotch: false))
        await manager.waitUntilIdle()
        for folder in runFolders {
            let status = HookInstaller.readStatus(configDir: fake.path(folder))
            #expect(status.hooksInstalled && status.statusLineInstalled, "\(folder)")
        }
        // Stores and the shared history: not a byte changed or added.
        let after = fake.files()
        for (key, value) in after where stores.contains(where: { key.hasPrefix($0 + "/") }) || key.hasPrefix(".claude-shared") {
            #expect(before[key] == value, "\(key)")
        }
        #expect(!FileManager.default.fileExists(atPath: fake.path(".claude-paras/settings.json")))
        #expect(!FileManager.default.fileExists(atPath: fake.path(".claude-shared/hooks")))
        #expect(untouchable(after) == untouchable(before))

        // Per account: hooks in every run folder.
        let paras = try #require(registry.identities.first { $0.email == Home.paras })
        let hooks = ClaudeHostProjections.hookStatus(identity: paras, statuses: manager.status)
        #expect(hooks.hooksInstalled && hooks.folderCount == 3 && hooks.installedFolderCount == 3)
    }

    @Test func aNewWindowGetsTheHooksOnTheNextPass() async throws {
        let fake = Home("hooks-window")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let manager = manager(registry)
        manager.grantConsent(takeOverFromVibeNotch: false)
        await manager.waitUntilIdle()

        try fake.addWindow("0a1b2c3d4e5f", uuid: Home.biiosUUID, email: Home.biios)
        await registry.discoverNow()
        manager.installAll()
        await manager.waitUntilIdle()
        #expect(HookInstaller.readStatus(configDir: fake.path(".claude-windows/0a1b2c3d4e5f")).hooksInstalled)
        let biios = try #require(registry.identities.first { $0.email == Home.biios })
        #expect(ClaudeHostProjections.hookStatus(identity: biios, statuses: manager.status).installedFolderCount == 2)
    }

    @Test func untrackingAnAccountTakesOursOutOfAllItsRunFolders() async throws {
        let fake = Home("hooks-untrack")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let manager = manager(registry)
        manager.grantConsent(takeOverFromVibeNotch: false)
        await manager.waitUntilIdle()

        manager.setTracked(accountId: "uuid:" + Home.parasUUID, false)
        await manager.waitUntilIdle()
        for folder in [".claude-windows/801f9dd51396", ".claude-windows/b9fbb9ecd7cb"] {
            #expect(!HookInstaller.readStatus(configDir: fake.path(folder)).hooksRegistered, "\(folder)")
        }
        // ~/.claude is whoever the extension mirrored in last: it keeps our
        // hooks while another account is tracked (PP-C5, UX-4).
        #expect(HookInstaller.readStatus(configDir: fake.path(".claude")).hooksInstalled)
        #expect(HookInstaller.readStatus(configDir: fake.path(".claude-windows/1bf3e8f92b11")).hooksInstalled)

        // Every account untracked: nothing of ours anywhere.
        manager.setTracked(accountId: "uuid:" + Home.biiosUUID, false)
        await manager.waitUntilIdle()
        for folder in runFolders {
            #expect(!HookInstaller.readStatus(configDir: fake.path(folder)).hooksRegistered, "\(folder)")
        }
        manager.setTracked(accountId: "uuid:" + Home.biiosUUID, true)
        await manager.waitUntilIdle()
        #expect(HookInstaller.readStatus(configDir: fake.path(".claude")).hooksInstalled)

        manager.disableHooks()
        await manager.waitUntilIdle()
        #expect(!HookInstaller.readStatus(configDir: fake.path(".claude-windows/1bf3e8f92b11")).hooksRegistered)
        #expect(!HookInstaller.readStatus(configDir: fake.path(".claude")).hooksRegistered)
    }

    /// Whatever a caller thinks, nothing is installed into a store or the
    /// shared history.
    @Test func theInstallerRefusesStoresAndTheSharedHistory() throws {
        let fake = Home("hooks-refuse")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let configuration = HookInstaller.Configuration(python: "python3", version: nil, statusLineIntegration: true,
                                                        hookScript: "# hook", statusLineScript: "# status")
        #expect(HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude-paras"), home: fake.home))
        #expect(HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude-shared"), home: fake.home))
        #expect(!HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude"), home: fake.home))
        #expect(!HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude-windows/801f9dd51396"), home: fake.home))
        // A store listed only in the manifest (no marker).
        try FileManager.default.removeItem(atPath: fake.path(".claude-claude/" + ParallelProfiles.storeMarkerName))
        #expect(HookInstaller.isNeverInstallTarget(configDir: fake.path(".claude-claude"), home: fake.home))
        // The marker alone refuses whatever the home.
        #expect(HookInstaller.install(configDir: fake.path(".claude-paras"), configuration: configuration) == .notAnInstallTarget)
        #expect(!FileManager.default.fileExists(atPath: fake.path(".claude-paras/settings.json")))
        #expect(!FileManager.default.fileExists(atPath: fake.path(".claude-paras/hooks")))
    }

    /// The user's case: Superpowered Vibe Notch wrote into `~/.claude`, the
    /// three stores and `~/.claude-shared`. Taking over cleans all five.
    @Test func takingOverCleansStoresAndTheSharedHistoryToo() async throws {
        let fake = Home("hooks-takeover")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout()
        let registry = fake.registry()
        let manager = manager(registry)
        manager.refreshStatus()
        await manager.waitUntilIdle()
        let cleanup = [".claude", ".claude-claude", ".claude-paras", ".claude-paras-rivant-in", ".claude-shared"].map(fake.path)
        #expect(manager.vibeNotchCleanupDirs.sorted() == cleanup.sorted())
        let setup = manager.setupState(isSealed: false, socketError: nil)
        #expect(setup.superpoweredVibeNotchHooksFound)
        let before = fake.files()

        #expect(manager.grantConsent(takeOverFromVibeNotch: true))
        await manager.waitUntilIdle()

        // ~/.claude: its entries gone, the user's kept, the status line it
        // wrapped back byte for byte (then wrapped by ours), its scripts gone.
        let main = HookInstaller.readStatus(configDir: fake.path(".claude"))
        #expect(!main.superpoweredVibeNotchHooksPresent && main.hooksInstalled && main.statusLineInstalled)
        let saved = HookInstaller.readSavedStatusLine(at: HookInstaller.previousStatusLineURL(configDir: fake.path(".claude")))
        #expect(saved?.serialized() == TakeoverFixture.originalStatusLine.serialized())
        #expect(!FileManager.default.fileExists(atPath: fake.path(".claude/hooks/superpowered-notch-hook.py")))
        #expect(FileManager.default.fileExists(atPath: fake.path(".claude/settings.json.agentnotch.original.bak")))

        // Stores and ~/.claude-shared: back to what the extension made.
        for folder in [".claude-paras", ".claude-paras-rivant-in", ".claude-claude", ".claude-shared"] {
            #expect(!FileManager.default.fileExists(atPath: fake.path(folder + "/settings.json")), "\(folder)")
            #expect(!FileManager.default.fileExists(atPath: fake.path(folder + "/hooks")), "\(folder)")
            let leftovers = ((try? FileManager.default.contentsOfDirectory(atPath: fake.path(folder))) ?? [])
                .filter { $0.contains("superpowered") || $0.hasPrefix("settings.json") }
            #expect(leftovers.isEmpty, "\(folder): \(leftovers)")
        }
        #expect(manager.vibeNotchCleanupDirs.isEmpty)
        #expect(!manager.setupState(isSealed: false, socketError: nil).superpoweredVibeNotchHooksFound)
        // Never touched: logins, markers, the manifest, the shared history.
        #expect(untouchable(fake.files()) == untouchable(before))
        // And nothing of ours in a store.
        #expect(!FileManager.default.fileExists(atPath: fake.path(".claude-paras/hooks/agentnotch-hook.py")))
    }

    /// A store's settings.json that existed before Superpowered Vibe Notch
    /// with something of the user's in it (its backup shows that) stays.
    @Test func aSettingsFileThatPredatesItStays() async throws {
        let fake = Home("hooks-predates")
        defer { fake.cleanUp(); defaults.remove() }
        try fake.buildUserLayout()
        try fake.write(".claude-paras/settings.json.superpowered-notch-20260101-000000-000.bak", "{\"env\": {\"A\": \"1\"}}\n")
        try fake.write(".claude-claude/settings.json", TakeoverFixture.settings(configDir: fake.path(".claude-claude")))
        let registry = fake.registry()
        let manager = manager(registry)
        manager.refreshStatus()
        await manager.waitUntilIdle()
        #expect(manager.takeOver(accountIds: nil))
        await manager.waitUntilIdle()
        // `{}` now, but it held the user's settings before: kept, with its backups.
        #expect(FileManager.default.fileExists(atPath: fake.path(".claude-paras/settings.json")))
        #expect(FileManager.default.fileExists(atPath: fake.path(".claude-paras/settings.json.superpowered-notch-20260101-000000-000.bak")))
        // The user's own entries: kept (only its entries went).
        let kept = try String(contentsOfFile: fake.path(".claude-claude/settings.json"), encoding: .utf8)
        #expect(kept.contains("guard-bash.sh") && !kept.contains("superpowered-notch"))
        // Its scripts go either way.
        #expect(!VibeNotchLeftovers.hasScripts(configDir: fake.path(".claude-paras")))
    }

    /// A script another settings.json still runs is never deleted.
    @Test func aScriptStillRunElsewhereStays() throws {
        let fake = Home("hooks-referenced")
        defer { fake.cleanUp() }
        try fake.addVibeNotch(".claude-a", userContent: false)
        try fake.mkdir(".claude-b")
        try fake.write(".claude-b/settings.json", Home.vibeNotchOnlySettings(configDir: fake.path(".claude-a")))
        try Data("{}".utf8).write(to: URL(fileURLWithPath: fake.path(".claude-a/settings.json")))
        let referenced = VibeNotchLeftovers.referencedScripts(configDirs: [fake.path(".claude-a"), fake.path(".claude-b")])
        #expect(referenced.contains(fake.path(".claude-a/hooks/superpowered-notch-hook.py")))
        VibeNotchLeftovers.cleanUp(configDir: fake.path(".claude-a"), referenced: referenced, removesBlankSettings: true)
        #expect(FileManager.default.fileExists(atPath: fake.path(".claude-a/hooks/superpowered-notch-hook.py")))
    }

    @Test func theConsentCardSaysWhereInPlainWords() {
        let scope = ConsentScope(includesDefault: true, windowCount: 3, storeCount: 3, parallelProfiles: true)
        #expect(scope.folderCount == 4)
        #expect(scope.sentence == "Installs into ~/.claude and your VS Code workspaces' folders (3 now; new ones are set up automatically). Claude Parallel Profiles' account stores never get hooks.")
        #expect(ConsentScope(includesDefault: true, parallelProfiles: false).sentence == nil)
    }
}
