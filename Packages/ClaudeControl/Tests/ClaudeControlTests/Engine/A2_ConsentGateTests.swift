import Foundation
import Testing
@testable import ClaudeControl

/// The hook manager over a throwaway home: nothing is written before the
/// user says yes, nothing while Superpowered Vibe Notch runs or its hooks
/// are still there, and untracking or forgetting takes ours out.
@MainActor
@Suite(.serialized, .enabled(if: !DevFlags.installsDisabled))
struct ConsentGateTests {
    let home: String
    let defaults = TestDefaults()
    /// Bundle ids the fake workspace says are running.
    final class Running { var ids: Set<String> = [] }
    let running = Running()

    init() throws {
        home = TestPaths.temporaryRoot("consent")
    }

    private func cleanUp() {
        try? FileManager.default.removeItem(atPath: home)
        try? FileManager.default.removeItem(atPath: home + "-support")
        defaults.remove()
    }

    private func makeAccount(_ name: String, settings: String?) throws -> String {
        let dir = home + "/" + name
        try FileManager.default.createDirectory(atPath: dir + "/sessions", withIntermediateDirectories: true)
        try Data("{}".utf8).write(to: URL(fileURLWithPath: dir + "/sessions/1.json"))
        if let settings {
            try Data(settings.utf8).write(to: URL(fileURLWithPath: dir + "/settings.json"))
        }
        return dir
    }

    private func setUp() async -> (AccountRegistry, AccountHookManager) {
        let registry = AccountRegistry(home: home, storeURL: URL(fileURLWithPath: home + "-support/accounts.json"),
                                       configReader: ClaudeGlobalConfigReader(), extraConfigDirs: [])
        await registry.discoverNow()
        let running = self.running
        let manager = AccountHookManager(registry: registry, settings: defaults.store, environment: AccountHookManager.Environment(
            installsDisabled: { false },
            isRunning: { running.ids.contains($0) },
            python: { "python3" },
            binaryVersions: { _ in [ClaudeCodeVersion(major: 2, minor: 1, patch: 280)] },
            forgetBinary: {},
            hookScript: { EmbeddedScripts.hook(socketPath: ScriptPaths.unusedSocket) },
            statusLineScript: { EmbeddedScripts.statusLine(socketPath: ScriptPaths.unusedSocket) },
            observesWorkspace: false
        ))
        return (registry, manager)
    }

    /// Every file under `home`, with its bytes.
    private func snapshot() -> [String: Data] {
        var files: [String: Data] = [:]
        let enumerator = FileManager.default.enumerator(atPath: home)
        while let relative = enumerator?.nextObject() as? String {
            let path = home + "/" + relative
            var isDirectory: ObjCBool = false
            if FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory), !isDirectory.boolValue {
                files[relative] = FileManager.default.contents(atPath: path)
            } else {
                files[relative + "/"] = Data()
            }
        }
        return files
    }

    @Test func nothingIsWrittenBeforeConsent() async throws {
        defer { cleanUp() }
        _ = try makeAccount(".claude", settings: realisticSettings)
        _ = try makeAccount(".claude-work", settings: nil)
        let (registry, manager) = await setUp()
        let before = snapshot()

        manager.start()
        await manager.waitUntilIdle()
        manager.installAll()
        await manager.waitUntilIdle()

        #expect(snapshot() == before)
        #expect(registry.accounts.count == 2)
        #expect(manager.status.count == 2)
        #expect(manager.setupState(isSealed: false, socketError: nil).needsHookConsent)
        #expect(manager.settingsFilesToEdit == registry.accounts.map { $0.configDir + "/settings.json" })

        // "Not now" is remembered and still writes nothing.
        manager.declineConsent()
        manager.installAll()
        await manager.waitUntilIdle()
        #expect(snapshot() == before)
        #expect(defaults.store.hookConsent == false)
        #expect(!manager.setupState(isSealed: false, socketError: nil).needsHookConsent)
        manager.stop()
    }

    @Test func consentInstallsIntoEveryTrackedAccount() async throws {
        defer { cleanUp() }
        let main = try makeAccount(".claude", settings: realisticSettings)
        let work = try makeAccount(".claude-work", settings: nil)
        let (_, manager) = await setUp()

        #expect(manager.grantConsent())
        await manager.waitUntilIdle()

        for dir in [main, work] {
            let status = HookInstaller.readStatus(configDir: dir)
            #expect(status.hooksInstalled && status.statusLineInstalled, "\(dir)")
        }
        #expect(Set(manager.lastChangedAccounts) == [main, work])
        #expect(manager.detectedVersion == ClaudeCodeVersion(major: 2, minor: 1, patch: 280))
        #expect(!manager.setupState(isSealed: false, socketError: nil).needsHookConsent)
        #expect(manager.setupState(isSealed: false, socketError: nil).legacyHooksFound)   // Vibe Notch's, in main
        // GUX-8: they're Vibe Notch's (which stay), not Superpowered Vibe Notch's.
        #expect(manager.setupState(isSealed: false, socketError: nil).vibeNotchHooksFound)
        #expect(!manager.setupState(isSealed: false, socketError: nil).superpoweredVibeNotchHooksFound)
        #expect(manager.setupState(isSealed: true, socketError: nil) == ClaudeSetupState())
    }

    @Test func nothingIsInstalledWhileSuperpoweredVibeNotchRuns() async throws {
        defer { cleanUp() }
        _ = try makeAccount(".claude", settings: realisticSettings)
        let (_, manager) = await setUp()
        running.ids = [AppIdentity.vibeNotchBundleIdentifier]
        let before = snapshot()

        #expect(!manager.grantConsent())
        await manager.waitUntilIdle()
        #expect(snapshot() == before)
        #expect(defaults.store.hookConsent == nil)
        #expect(manager.setupState(isSealed: false, socketError: nil).vibeNotchRunning)

        // Consent given earlier: the pass still writes nothing while it runs.
        defaults.store.hookConsent = true
        manager.installAll()
        await manager.waitUntilIdle()
        #expect(snapshot() == before)
        #expect(!manager.takeOver(accountIds: nil))
    }

    /// Removing its hooks from one account is a takeover of that account:
    /// refused while it runs, like the takeover of all of them. Other kinds
    /// are not held back.
    @Test func removingItsHooksWaitsUntilItQuits() async throws {
        defer { cleanUp() }
        let dir = try makeAccount(".claude", settings: TakeoverFixture.settings(configDir: home + "/.claude"))
        try TakeoverFixture.writeVibeNotchFiles(configDir: dir)
        let (_, manager) = await setUp()
        defaults.store.hookConsent = true
        running.ids = [AppIdentity.vibeNotchBundleIdentifier]
        let before = snapshot()

        #expect(!manager.removeLegacyHooks(accountId: dir, kind: .superpoweredVibeNotch))
        await manager.waitUntilIdle()
        #expect(snapshot() == before)
        #expect(manager.removeLegacyHooks(accountId: dir, kind: .vibeNotch))
        await manager.waitUntilIdle()

        running.ids = []
        #expect(manager.removeLegacyHooks(accountId: dir, kind: .superpoweredVibeNotch))
        await manager.waitUntilIdle()
        let status = HookInstaller.readStatus(configDir: dir)
        #expect(!status.superpoweredVibeNotchHooksPresent)
        #expect(status.hooksInstalled && status.statusLineInstalled)
    }

    /// Superpowered Vibe Notch's hooks are taken over (its status line put
    /// back exactly from its saved copy), never stacked on.
    @Test func superpoweredVibeNotchHooksAreTakenOverNotStacked() async throws {
        defer { cleanUp() }
        let dir = try makeAccount(".claude", settings: TakeoverFixture.settings(configDir: home + "/.claude"))
        try TakeoverFixture.writeVibeNotchFiles(configDir: dir)
        let (_, manager) = await setUp()

        // Consent without takeover: the account is skipped, not stacked on.
        #expect(manager.grantConsent(takeOverFromVibeNotch: false))
        await manager.waitUntilIdle()
        var status = HookInstaller.readStatus(configDir: dir)
        #expect(status.superpoweredVibeNotchHooksPresent)
        #expect(!status.hooksRegistered)
        #expect(manager.accountsToTakeOver.map(\.configDir) == [dir])
        #expect(manager.setupState(isSealed: false, socketError: nil).legacyHooksFound)
        #expect(manager.setupState(isSealed: false, socketError: nil).superpoweredVibeNotchHooksFound)

        // Take over: its entries go, the original status line comes back and
        // is wrapped by ours, and our hooks go in.
        #expect(manager.takeOver(accountIds: nil))
        await manager.waitUntilIdle()
        status = HookInstaller.readStatus(configDir: dir)
        #expect(!status.superpoweredVibeNotchHooksPresent)
        #expect(status.hooksInstalled && status.statusLineInstalled)
        let saved = HookInstaller.readSavedStatusLine(at: HookInstaller.previousStatusLineURL(configDir: dir))
        #expect(saved?.serialized() == TakeoverFixture.originalStatusLine.serialized())
        #expect(manager.accountsToTakeOver.isEmpty)
        // Its scripts go once no settings.json runs them (its saved status
        // line too); the settings.json of a run folder always stays.
        #expect(!FileManager.default.fileExists(atPath: dir + "/hooks/superpowered-notch-hook.py"))
        #expect(!FileManager.default.fileExists(atPath: dir + "/hooks/superpowered-notch-statusline.py"))
        #expect(FileManager.default.fileExists(atPath: dir + "/settings.json"))
    }

    @Test func untrackingAndForgettingTakeOurHooksOut() async throws {
        defer { cleanUp() }
        let main = try makeAccount(".claude", settings: #"{"model":"opus"}"#)
        let work = try makeAccount(".claude-work", settings: #"{"statusLine":{"type":"command","command":"echo hi"}}"#)
        let (registry, manager) = await setUp()
        #expect(manager.grantConsent())
        await manager.waitUntilIdle()
        #expect(HookInstaller.readStatus(configDir: work).hooksInstalled)

        // Untrack: ours come out, the status line is restored.
        manager.setTracked(accountId: work, false)
        await manager.waitUntilIdle()
        var status = HookInstaller.readStatus(configDir: work)
        #expect(!status.hooksRegistered && !status.statusLineInstalled)
        let workSettings = try OrderedJSON.parse(Data(contentsOf: URL(fileURLWithPath: work + "/settings.json")))
        #expect(workSettings["statusLine"]?["command"]?.stringValue == "echo hi")
        // Another pass leaves it alone.
        manager.installAll()
        await manager.waitUntilIdle()
        #expect(!HookInstaller.readStatus(configDir: work).hooksRegistered)

        // Track again: back in. Forget: out, and gone from the registry for good.
        manager.setTracked(accountId: work, true)
        await manager.waitUntilIdle()
        #expect(HookInstaller.readStatus(configDir: work).hooksInstalled)
        manager.forget(accountId: work)
        await manager.waitUntilIdle()
        status = HookInstaller.readStatus(configDir: work)
        #expect(!status.hooksRegistered)
        #expect(registry.account(id: work) == nil)
        #expect(manager.status[work] == nil)
        #expect(HookInstaller.readStatus(configDir: main).hooksInstalled)

        await registry.discoverNow()
        #expect(registry.account(id: work) == nil)
    }

    /// A status line from an older Claude Code rewrites the hooks without the
    /// events it doesn't know.
    @Test func anOlderClientLowersTheRegisteredEvents() async throws {
        defer { cleanUp() }
        let dir = try makeAccount(".claude", settings: nil)
        let (_, manager) = await setUp()
        #expect(manager.grantConsent())
        await manager.waitUntilIdle()
        func events() throws -> Set<String> {
            let json = try OrderedJSON.parse(Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json")))
            return Set(json["hooks"]?.members?.map(\.key) ?? [])
        }
        #expect(try events().contains("PermissionDenied"))

        manager.noteVersion(ClaudeCodeVersion(major: 2, minor: 1, patch: 50), accountId: dir)
        await manager.waitUntilIdle()
        let lowered = try events()
        #expect(!lowered.contains("PermissionDenied"))
        #expect(!lowered.contains("TaskCreated"))
        #expect(lowered.contains("TaskCompleted"))

        // A newer one changes nothing.
        let before = try Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json"))
        manager.noteVersion(ClaudeCodeVersion(major: 2, minor: 1, patch: 300), accountId: dir)
        await manager.waitUntilIdle()
        #expect(try Data(contentsOf: URL(fileURLWithPath: dir + "/settings.json")) == before)
    }
}

/// Sealed runs never touch real folders or settings from the setup actions.
@MainActor
struct SealedSetupTests {
    let hub = ClaudeControlHub(configuration: .sealed(appDisplayName: "T", bundleIdentifier: "com.example.sealed-setup"))

    @Test func setupActionsDoNothingWhenSealed() {
        #expect(hub.currentSetupState() == ClaudeSetupState())
        #expect(!hub.grantHookConsent())
        #expect(!hub.takeOverFromVibeNotch())
        #expect(hub.settingsFilesToEdit.isEmpty)
        #expect(hub.accountsToTakeOver.isEmpty)
        #expect(hub.folderSuggestions.isEmpty)
        #expect(hub.followSetupChanges() == nil)
        #expect(throws: AccountFolderError.unavailableWhenSealed) { try hub.checkFolder(NSHomeDirectory()) }
        #expect(throws: AccountFolderError.unavailableWhenSealed) { try hub.addExistingFolder(NSHomeDirectory() + "/.claude") }
        #expect(throws: AccountFolderError.unavailableWhenSealed) { try hub.createAccount(name: "x") }
    }
}
