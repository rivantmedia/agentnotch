import Foundation
import Testing
@testable import ClaudeControl

/// One account per signed-in identity, however many folders it lives in.
@MainActor
@Suite(.serialized)
struct PP_IdentityTests {
    typealias Home = ParallelProfilesHome

    private func runDirs(_ identity: ClaudeIdentityAccount?) -> [String] { identity?.runDirs.map(\.configDir) ?? [] }
    private func storeDirs(_ identity: ClaudeIdentityAccount?) -> [String] { identity?.storeDirs.map(\.configDir) ?? [] }

    @Test func theUsersMacHasExactlyTwoAccounts() throws {
        let fake = Home("two")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        let registry = fake.registry()

        #expect(registry.identities.count == 2)
        let paras = registry.identities.first { $0.email == Home.paras }
        let biios = registry.identities.first { $0.email == Home.biios }
        #expect(runDirs(paras) == [".claude", ".claude-windows/801f9dd51396", ".claude-windows/b9fbb9ecd7cb"].map(fake.path))
        #expect(storeDirs(paras) == [".claude-paras", ".claude-paras-rivant-in"].map(fake.path))
        #expect(runDirs(biios) == [fake.path(".claude-windows/1bf3e8f92b11")])
        #expect(storeDirs(biios) == [fake.path(".claude-claude")])
        // Codenotch's naming rule, no folder suffix: the names don't collide.
        #expect(paras?.label == "Claude Rivant")
        #expect(biios?.label == "Claude Biios")
        // By name, whichever one ~/.claude runs as.
        #expect(registry.identities.map(\.label) == ["Claude Biios", "Claude Rivant"])
        #expect(paras?.includesDefault == true && biios?.includesDefault == false)
        #expect(paras?.windowDirs.count == 2 && biios?.windowDirs.count == 1)
        // Ring ids: from the account's UUID alone, Codenotch's Claude family.
        #expect(paras?.ringID == ClaudeRingIdentity.ringID(accountKey: Home.parasUUID))
        #expect(paras?.ringID.hasPrefix("claude-acct-") == true && paras?.ringID.count == "claude-acct-".count + 12)
        #expect(ClaudeRingIdentity.isClaudeRing(paras?.ringID ?? ""))
        // No phantom account for the shared history, nothing unsigned.
        #expect(registry.unsignedFolders.isEmpty)
        #expect(!registry.accounts.contains { $0.configDir == fake.path(".claude-shared") })
        #expect(registry.infrastructureDirs == [".claude-shared", ".claude-windows"].map(fake.path))
        #expect(registry.layout.extensionDetected)
        // Stores are folders of the account, but never run folders.
        #expect(registry.accounts.filter { $0.kind == .store }.map(\.configDir)
                == [".claude-claude", ".claude-paras", ".claude-paras-rivant-in"].map(fake.path).sorted())
        #expect(registry.identity(forFolderId: fake.path(".claude-windows/1bf3e8f92b11"))?.email == Home.biios)
        #expect(registry.identityId(for: fake.path(".claude-paras")) == "uuid:" + Home.parasUUID)
    }

    @Test func aWindowThatSwitchedAccountMovesToTheOtherAccount() async throws {
        let fake = Home("switch")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        let registry = fake.registry()
        try fake.writeJSON(".claude-windows/b9fbb9ecd7cb/.claude.json", fake.login(uuid: Home.biiosUUID, email: Home.biios))
        await registry.refreshIdentities()
        let biios = registry.identities.first { $0.email == Home.biios }
        #expect(runDirs(biios) == [".claude-windows/1bf3e8f92b11", ".claude-windows/b9fbb9ecd7cb"].map(fake.path))
        #expect(registry.identities.count == 2)
    }

    /// The extension mirrors the last-used account into ~/.claude.
    @Test func aDefaultMirroredToTheOtherAccountJoinsIt() async throws {
        let fake = Home("mirror")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        let registry = fake.registry()
        try fake.writeJSON(".claude.json", fake.login(uuid: Home.biiosUUID, email: Home.biios))
        await registry.refreshIdentities()
        let paras = registry.identities.first { $0.email == Home.paras }
        let biios = registry.identities.first { $0.email == Home.biios }
        #expect(runDirs(biios) == [".claude", ".claude-windows/1bf3e8f92b11"].map(fake.path))
        #expect(runDirs(paras) == [".claude-windows/801f9dd51396", ".claude-windows/b9fbb9ecd7cb"].map(fake.path))
        // The rings keep their places.
        #expect(registry.identities.map(\.email) == [Home.biios, Home.paras])
    }

    /// The mirror rewrites the email and keeps the rest of `oauthAccount`:
    /// the UUID left behind belongs to the other person, and the email wins.
    @Test func aHalfMirroredDefaultFollowsItsEmail() async throws {
        let fake = Home("halfmirror")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        let registry = fake.registry()
        try fake.writeJSON(".claude.json", fake.login(uuid: Home.parasUUID, email: Home.biios))
        await registry.refreshIdentities()
        #expect(registry.identities.count == 2)
        let biios = registry.identities.first { $0.email == Home.biios }
        #expect(runDirs(biios).first == fake.path(".claude"))
        #expect(biios?.accountUuid == Home.biiosUUID)

        let keys = AccountIdentityGrouping.identityKeys(registry.accounts)
        #expect(keys[fake.path(".claude")] == .init(key: "uuid:" + Home.biiosUUID, corrected: true))
        #expect(keys[fake.path(".claude-paras")] == .init(key: "uuid:" + Home.parasUUID, corrected: false))
    }

    @Test func anAccountOnlyItsStoreHoldsStillHasARingButRunsNowhere() async throws {
        let fake = Home("storeonly")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        try FileManager.default.removeItem(atPath: fake.path(".claude-windows/1bf3e8f92b11"))
        let registry = fake.registry()
        let biios = registry.identities.first { $0.email == Home.biios }
        #expect(biios != nil)
        #expect(runDirs(biios).isEmpty)
        #expect(storeDirs(biios) == [fake.path(".claude-claude")])
        #expect(UsageProbePlanner.probeFolder(runDirs: biios?.runDirs ?? [], activity: [], home: fake.home) == nil)
    }

    @Test func aNewWindowJoinsItsAccountAndAClosedOneLeaves() async throws {
        let fake = Home("newwindow")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        let registry = fake.registry()
        try fake.addWindow("0a1b2c3d4e5f", uuid: Home.biiosUUID, email: Home.biios)
        await registry.discoverNow()
        #expect(runDirs(registry.identities.first { $0.email == Home.biios }).contains(fake.path(".claude-windows/0a1b2c3d4e5f")))
        // The extension deleted it (uninstalled, say): it goes by itself.
        try FileManager.default.removeItem(atPath: fake.path(".claude-windows/0a1b2c3d4e5f"))
        await registry.discoverNow()
        #expect(registry.account(forConfigDir: fake.path(".claude-windows/0a1b2c3d4e5f")) == nil)
    }

    /// Only a folder added by hand keeps "Run … then /login"; others that
    /// nobody signed in to are listed, with no ring.
    @Test func unsignedFolders() async throws {
        let fake = Home("unsigned")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        try fake.mkdir(".claude-fresh/projects")
        try fake.write(".claude-windows/0a1b2c3d4e5f/.claude.json", "{}")
        let registry = fake.registry()
        let added = try registry.addAccount(configDir: fake.path(".claude-fresh"))
        #expect(registry.identities.count == 3)
        let manual = try #require(registry.identities.first { $0.id == "dir:" + added.id })
        #expect(manual.isStandaloneUnsigned)
        #expect(manual.ringID == "claude-fresh")
        #expect(registry.unsignedFolders.map(\.configDir) == [fake.path(".claude-windows/0a1b2c3d4e5f")])
    }

    /// With nobody signed in anywhere, `~/.claude` still has its ring.
    @Test func aLoneUnsignedDefaultKeepsItsRing() throws {
        let fake = Home("lonely")
        defer { fake.cleanUp() }
        try fake.mkdir(".claude/projects")
        let registry = fake.registry()
        #expect(registry.identities.map(\.ringID) == ["claude"])
        #expect(registry.identities.first?.isSignedIn == false)
    }

    /// Without the extension, two folders with one login are one account.
    @Test func plainFoldersWithOneLoginAreOneAccount() throws {
        let fake = Home("plaingroup")
        defer { fake.cleanUp() }
        try fake.mkdir(".claude/projects")
        try fake.writeJSON(".claude.json", fake.login(uuid: "u-1", email: "me@x.dev"))
        try fake.mkdir(".claude-personal/projects")
        try fake.writeJSON(".claude-personal/.claude.json", fake.login(uuid: "u-1", email: "me@x.dev"))
        try fake.mkdir(".claude-work/projects")
        try fake.writeJSON(".claude-work/.claude.json", fake.login(uuid: "u-2", email: "me@work.dev"))
        let registry = fake.registry()
        #expect(registry.identities.count == 2)
        #expect(runDirs(registry.identities.first { $0.email == "me@x.dev" }) == [fake.path(".claude"), fake.path(".claude-personal")])
        #expect(registry.identities.allSatisfy { $0.storeDirs.isEmpty })
        #expect(!registry.layout.extensionDetected)
    }

    /// accounts.json from before (a row per folder): name, colour and
    /// tracking go to the identity from its first folder, then persist.
    @Test func perFolderChoicesBecomePerIdentityAndPersist() throws {
        let fake = Home("prefs")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let old = """
        {"version": 1, "removedIds": [], "accounts": [
          {"id": "\(fake.path(".claude"))", "configDir": "\(fake.path(".claude"))", "customLabel": "Paras", "colorIndex": 3, "isHidden": false, "source": "discovered"},
          {"id": "\(fake.path(".claude-shared"))", "configDir": "\(fake.path(".claude-shared"))", "colorIndex": 1, "isHidden": false, "source": "discovered"},
          {"id": "\(fake.path(".claude-claude"))", "configDir": "\(fake.path(".claude-claude"))", "configDirEnv": "\(fake.path(".claude-claude"))", "colorIndex": 3, "isHidden": true, "source": "discovered"},
          {"id": "\(fake.path(".claude-paras"))", "configDir": "\(fake.path(".claude-paras"))", "configDirEnv": "\(fake.path(".claude-paras"))", "customLabel": "Store name", "colorIndex": 5, "isHidden": true, "source": "discovered"}
        ]}
        """
        try FileManager.default.createDirectory(atPath: fake.support, withIntermediateDirectories: true)
        try Data(old.utf8).write(to: URL(fileURLWithPath: fake.support + "/accounts.json"))
        let registry = fake.registry()
        let paras = try #require(registry.identities.first { $0.email == Home.paras })
        let biios = try #require(registry.identities.first { $0.email == Home.biios })
        #expect(paras.label == "Paras")          // ~/.claude's, the first folder
        #expect(paras.colorIndex == 3)
        #expect(!paras.isHidden)
        #expect(biios.isHidden)                  // ~/.claude-claude's choice
        #expect(biios.colorIndex != 3)           // the colour was taken
        // Every folder follows its identity's tracking, windows included.
        let biiosFolders = registry.accounts.filter { biios.folderIds.contains($0.id) }
        let parasFolders = registry.accounts.filter { paras.folderIds.contains($0.id) }
        #expect(biiosFolders.count == 2 && biiosFolders.allSatisfy(\.isHidden))
        #expect(parasFolders.count == 5 && !parasFolders.contains(where: \.isHidden))
        // The shared history is no account, whatever the file said.
        #expect(registry.account(forConfigDir: fake.path(".claude-shared")) == nil)

        registry.saveNow()
        let saved = try String(contentsOfFile: fake.support + "/accounts.json", encoding: .utf8)
        #expect(saved.contains("\"version\" : 2"))
        #expect(saved.contains("uuid:" + Home.parasUUID))
        let again = fake.registry()
        #expect(again.identities.first { $0.email == Home.paras }?.label == "Paras")
        #expect(again.identities.first { $0.email == Home.biios }?.isHidden == true)
    }

    @Test func trackingAndForgettingApplyToTheWholeIdentity() async throws {
        let fake = Home("track")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let biiosId = "uuid:" + Home.biiosUUID
        registry.setHidden(id: biiosId, true)
        // A window opened later for it is untracked too.
        try fake.addWindow("0a1b2c3d4e5f", uuid: Home.biiosUUID, email: Home.biios)
        await registry.discoverNow()
        #expect(registry.account(forConfigDir: fake.path(".claude-windows/0a1b2c3d4e5f"))?.isHidden == true)
        registry.setHidden(id: fake.path(".claude-windows/1bf3e8f92b11"), false)   // a folder of it: the identity
        #expect(registry.identity(id: biiosId)?.isHidden == false)

        registry.remove(id: biiosId)
        #expect(registry.identity(id: biiosId) == nil)
        #expect(registry.identities.count == 1)
        #expect(registry.isForgotten(fake.path(".claude-windows/1bf3e8f92b11")))
        #expect(registry.isForgotten(biiosId))
        // Its store stays on disk, untouched.
        #expect(FileManager.default.fileExists(atPath: fake.path(".claude-claude/.claude.json")))
        // Adding one of its folders back brings the account back.
        _ = try registry.addAccount(configDir: fake.path(".claude-windows/1bf3e8f92b11"))
        #expect(registry.identity(id: biiosId) != nil)
    }

    @Test func theSharedHistoryCannotBeAddedByHand() throws {
        let fake = Home("addshared")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        #expect(throws: AccountFolderError.infrastructure) { try registry.addAccount(configDir: fake.path(".claude-shared")) }
        #expect(throws: AccountFolderError.infrastructure) { try registry.addAccount(configDir: fake.path(".claude-windows")) }
        #expect(registry.identities.count == 2)
    }

    @Test func identityKeysPreferTheUUIDAndJoinEmailOnlyLogins() {
        var a = ClaudeAccount(configDir: "/h/.claude-a", email: "Me@X.dev", accountUuid: "U-1")
        a.kind = .store
        let b = ClaudeAccount(configDir: "/h/.claude-b", email: "me@x.dev")
        let c = ClaudeAccount(configDir: "/h/.claude-c", email: "other@x.dev")
        let d = ClaudeAccount(configDir: "/h/.claude-d")
        let keys = AccountIdentityGrouping.identityKeys([a, b, c, d])
        #expect(keys["/h/.claude-a"]?.key == "uuid:u-1")
        #expect(keys["/h/.claude-b"]?.key == "uuid:u-1")
        #expect(keys["/h/.claude-c"]?.key == "email:other@x.dev")
        #expect(keys["/h/.claude-d"] == nil)
        // Two logins with one email but two UUIDs (two organizations) stay apart.
        let e = ClaudeAccount(configDir: "/h/.claude-e", email: "me@x.dev", accountUuid: "u-9")
        #expect(AccountIdentityGrouping.identityKeys([a, e])["/h/.claude-e"]?.key == "uuid:u-9")
    }
}
