import Foundation
import Testing
@testable import ClaudeControl

/// Discovery and bookkeeping against a throwaway home folder.
@MainActor
@Suite(.serialized)
struct AccountRegistryTests {
    let home: String
    let store: URL

    init() throws {
        home = TestPaths.temporaryPath("registry")
        store = URL(fileURLWithPath: home + "-support/accounts.json")
    }

    private func mkdir(_ relative: String = "") throws {
        try FileManager.default.createDirectory(atPath: home + "/" + relative, withIntermediateDirectories: true)
    }

    private func touch(_ relative: String, _ contents: String = "{}") throws {
        try FileManager.default.createDirectory(atPath: ((home + "/" + relative) as NSString).deletingLastPathComponent,
                                                withIntermediateDirectories: true)
        try Data(contents.utf8).write(to: URL(fileURLWithPath: home + "/" + relative))
    }

    private func signIn(_ relative: String, email: String, uuid: String = UUID().uuidString, org: String? = nil) throws {
        let organization = org.map { #","organizationName":"\#($0)","organizationUuid":"org-\#($0)""# } ?? ""
        try touch(relative, #"{"numStartups":3,"oauthAccount":{"accountUuid":"\#(uuid)","emailAddress":"\#(email)"\#(organization)}}"#)
    }

    private func cleanUp() {
        try? FileManager.default.removeItem(atPath: home)
        try? FileManager.default.removeItem(atPath: home + "-support")
    }

    private func registry(extra: [String] = []) -> AccountRegistry {
        AccountRegistry(home: home, storeURL: store, configReader: ClaudeGlobalConfigReader(), extraConfigDirs: extra)
    }

    // MARK: - Discovery rules

    /// Only folders clearly in use are added by themselves: ~/.claude, and
    /// look-alikes that are signed in or have session files. History alone,
    /// a backup's name, ~/.config/claude and foreign folders are not.
    @Test func discoveryRules() throws {
        defer { cleanUp() }
        try mkdir(".claude")                            // default: always, if it exists
        try mkdir(".claude-work/projects")              // signed in
        try signIn(".claude-work/.claude.json", email: "me@work.com")
        try mkdir(".claude_personal/sessions")          // a live session registry
        try touch(".claude_personal/sessions/\(getpid()).json", #"{"pid":\#(getpid())}"#)
        try mkdir(".claude-history/projects")           // history, no login, no sessions: suggest
        try mkdir(".claude-fresh")                      // only an empty .claude.json: suggest
        try touch(".claude-fresh/.claude.json")
        try mkdir(".claude-backup/projects")            // a backup copy, even signed in: suggest
        try signIn(".claude-backup/.claude.json", email: "me@work.com")
        try mkdir(".claude-old-2025/sessions")          // a dated copy: suggest
        try touch(".claude-old-2025/sessions/1.json")
        try mkdir(".claude-server-commander")           // an MCP server's folder: nothing
        try touch(".claude-server-commander/config.json")
        try mkdir(".claude-mem")                        // a look-alike with a config but no login: suggest at most
        try touch(".claude-mem/.claude.json", #"{"oauthAccount":{}}"#)
        try mkdir(".config/claude/projects")            // XDG-style: only via a live session
        try signIn(".config/claude/.claude.json", email: "me@x.com")
        try mkdir(".claudette/projects")                // not our prefix
        try touch(".claude.json")                       // a file, not a dir
        try touch(".claude-notes.json")                 // a file with our prefix

        let found = AccountRegistry.discover(home: home)
        #expect(found.accounts == [home + "/.claude", home + "/.claude-work", home + "/.claude_personal"])
        let suggested = Dictionary(uniqueKeysWithValues: found.suggestions.map { ($0.configDir, $0.reason) })
        #expect(suggested == [
            home + "/.claude-history": .found,
            home + "/.claude-fresh": .found,
            home + "/.claude-mem": .found,
            home + "/.claude-backup": .looksLikeBackup,
            home + "/.claude-old-2025": .looksLikeBackup,
        ])
        #expect(AccountRegistry.discoverConfigDirs(home: home) == found.accounts)
    }

    @Test func discoveryWithoutDefaultDir() throws {
        defer { cleanUp() }
        try mkdir(".claude-work/sessions")
        try touch(".claude-work/sessions/1.json")
        #expect(AccountRegistry.discoverConfigDirs(home: home) == [home + "/.claude-work"])
    }

    @Test func extraConfigDirsAreAddedButNeverHome() throws {
        defer { cleanUp() }
        try mkdir("elsewhere/claude-profile")
        let found = AccountRegistry.discover(home: home, extraDirs: [home + "/elsewhere/claude-profile", home, home + "/missing"])
        #expect(found.accounts == [home + "/elsewhere/claude-profile"])
    }

    @Test(arguments: [
        (".claude-backup", true), (".claude_bak", true), (".claude-old", true), (".claude-copy-2", true),
        (".claude-2024-01-01", true), (".claude-20250101", true), (".claude-work.bak", true), (".claude-orig", true),
        (".claude-work", false), (".claude-acme", false), (".claude_personal", false), (".claude-team2", false),
        (".claude-mem", false), (".claude-client-42", false),
    ])
    func backupNames(name: String, isBackup: Bool) {
        #expect(AccountRegistry.looksLikeBackup(name) == isBackup)
    }

    @Test func loginDetectionReadsOnlyForAnAccountObject() throws {
        defer { cleanUp() }
        try signIn("a.json", email: "me@x.com")
        try touch("b.json", #"{"oauthAccount": null}"#)
        try touch("c.json", #"{"oauthAccount" : { }}"#)
        try touch("d.json", #"{"projects":{"x":{"note":"oauthAccount"}}}"#)
        try touch("e.json", #"{"x":1,"oauthAccount" :{"emailAddress":"a@b.c"}}"#)
        #expect(AccountRegistry.hasLogin(globalConfigAt: home + "/a.json"))
        #expect(!AccountRegistry.hasLogin(globalConfigAt: home + "/b.json"))
        #expect(!AccountRegistry.hasLogin(globalConfigAt: home + "/c.json"))
        #expect(!AccountRegistry.hasLogin(globalConfigAt: home + "/d.json"))
        #expect(AccountRegistry.hasLogin(globalConfigAt: home + "/e.json"))
        #expect(!AccountRegistry.hasLogin(globalConfigAt: home + "/missing.json"))
    }

    @Test func discoverAddsAccountsWithStableColoursAndSuggestsTheRest() async throws {
        defer { cleanUp() }
        try mkdir(".claude/projects")
        try mkdir(".claude-work/projects")
        try signIn(".claude-work/.claude.json", email: "me@work.com")
        try mkdir(".claude-alt/sessions")
        try touch(".claude-alt/sessions/\(getpid()).json")
        try mkdir(".claude-copy/projects")

        let registry = registry()
        await registry.discoverNow()

        #expect(registry.accounts.count == 3)
        #expect(registry.accounts.first?.configDir == home + "/.claude")
        #expect(registry.isDefault(registry.accounts[0]))
        #expect(registry.accounts[0].configDirEnv == nil)
        #expect(Set(registry.accounts.map(\.colorIndex)) == [0, 1, 2])
        #expect(registry.accounts.allSatisfy { $0.source == .discovered })
        #expect(registry.suggestions == [AccountFolderSuggestion(configDir: home + "/.claude-copy", reason: .looksLikeBackup)])

        let work = try #require(registry.account(forConfigDir: home + "/.claude-work/"))
        #expect(work.configDirEnv == home + "/.claude-work")

        // A rescan changes nothing.
        let before = registry.accounts
        await registry.discoverNow()
        #expect(registry.accounts == before)

        // Accepting a suggestion adds it; dismissing one hides it for good.
        try registry.acceptSuggestion(home + "/.claude-copy")
        #expect(registry.account(forConfigDir: home + "/.claude-copy")?.source == .manual)
        #expect(registry.suggestions.isEmpty)
    }

    @Test func dismissedSuggestionsStayDismissed() async throws {
        defer { cleanUp() }
        try mkdir(".claude-history/projects")
        let registry = registry()
        await registry.discoverNow()
        #expect(registry.suggestions.map(\.configDir) == [home + "/.claude-history"])
        registry.dismissSuggestion(home + "/.claude-history")
        #expect(registry.suggestions.isEmpty)
        registry.saveNow()

        let reloaded = self.registry()
        await reloaded.discoverNow()
        #expect(reloaded.suggestions.isEmpty)
        #expect(reloaded.accounts.isEmpty)
    }

    // MARK: - Identity

    @Test func identityFromGlobalConfig() async throws {
        defer { cleanUp() }
        try mkdir(".claude-work/projects")
        try touch(".claude-work/.claude.json", """
        {"oauthAccount":{"accountUuid":"u-1","emailAddress":"me@company.com","displayName":"Me",
          "organizationName":"Company","organizationUuid":"org-9","organizationType":"claude_team","organizationRateLimitTier":"default_claude_team"},
         "projects":{}}
        """)
        let registry = registry()
        await registry.discoverNow()

        let account = try #require(registry.account(forConfigDir: home + "/.claude-work"))
        #expect(account.email == "me@company.com")
        #expect(account.accountUuid == "u-1")
        #expect(account.organizationName == "Company")
        #expect(account.organizationUuid == "org-9")
        #expect(account.subscriptionType == "team")
        #expect(account.rateLimitTier == "default_claude_team")
        #expect(account.label == "Claude Company")

        // Signing out clears it.
        try touch(".claude-work/.claude.json", #"{"projects":{},"numStartups":3}"#)
        await registry.refreshIdentities()
        let signedOut = try #require(registry.account(forConfigDir: home + "/.claude-work"))
        #expect(signedOut.email == nil)
        #expect(signedOut.accountUuid == nil)
        #expect(signedOut.organizationUuid == nil)
        #expect(signedOut.subscriptionType == nil)
        #expect(signedOut.label == "Claude (work)")
    }

    @Test func defaultAccountReadsHomeClaudeJson() throws {
        defer { cleanUp() }
        let account = ClaudeAccount(configDir: home + "/.claude")
        #expect(AccountRegistry.globalConfigPath(for: account, home: home) == home + "/.claude.json")
        var explicit = account
        explicit.configDirEnv = home + "/.claude"
        #expect(AccountRegistry.globalConfigPath(for: explicit, home: home) == home + "/.claude/.claude.json")
        let work = ClaudeAccount(configDir: home + "/.claude-work", configDirEnv: home + "/.claude-work/")
        #expect(AccountRegistry.globalConfigPath(for: work, home: home) == home + "/.claude-work/.claude.json")
    }

    // MARK: - Sightings

    @Test func sightingsAddAccountsAndKeepTheRawEnv() throws {
        defer { cleanUp() }
        let registry = registry()
        let raw = home + "/.claude-work/"   // trailing slash: a different keychain item, kept verbatim
        let now = Date()
        registry.record(AccountSighting(configDir: AccountPaths.normalize(raw), configDirEnv: raw, sessionId: "s1", at: now))

        let account = try #require(registry.account(forConfigDir: raw))
        #expect(account.source == .hook)
        #expect(account.configDirEnv == raw)
        #expect(account.lastSeenAt == now)

        // Within a minute: lastSeenAt doesn't churn.
        registry.record(AccountSighting(configDir: account.configDir, configDirEnv: raw, sessionId: "s2", at: now.addingTimeInterval(10)))
        #expect(registry.account(id: account.id)?.lastSeenAt == now)
        registry.record(AccountSighting(configDir: account.configDir, configDirEnv: raw, sessionId: "s2", at: now.addingTimeInterval(90)))
        #expect(registry.account(id: account.id)?.lastSeenAt == now.addingTimeInterval(90))

        // A session without the env var on a custom dir keeps the known value.
        registry.record(AccountSighting(configDir: account.configDir, configDirEnv: nil, sessionId: "s3", at: now.addingTimeInterval(200)))
        #expect(registry.account(id: account.id)?.configDirEnv == raw)
        #expect(registry.hasLoginConflict(registry.account(id: account.id)!) == false)
    }

    /// ~/.claude used both with and without CLAUDE_CONFIG_DIR is two logins.
    /// The account stays on one of them (no flipping every minute), moving
    /// only to the one that is signed in.
    @Test func twoSpellingsOfOneFolderDoNotFlipFlop() async throws {
        defer { cleanUp() }
        try mkdir(".claude")
        try signIn(".claude.json", email: "me@personal.dev")   // the default login's file
        let registry = registry()
        let dir = home + "/.claude"

        registry.record(AccountSighting(configDir: dir, configDirEnv: dir, sessionId: "s1", at: Date()))
        #expect(registry.account(forConfigDir: dir)?.configDirEnv == dir)
        registry.record(AccountSighting(configDir: dir, configDirEnv: nil, sessionId: "s2", at: Date()))
        registry.record(AccountSighting(configDir: dir, configDirEnv: dir, sessionId: "s3", at: Date()))
        let account = try #require(registry.account(forConfigDir: dir))
        #expect(account.seenConfigDirEnvs == [dir, ""])
        #expect(registry.hasLoginConflict(account))
        // Sightings alone never flip it...
        #expect(account.configDirEnv == dir)

        // ...but the explicit spelling's ~/.claude/.claude.json has no login
        // and the default one's does, so it moves there, once.
        await registry.refreshIdentities()
        let settled = try #require(registry.account(forConfigDir: dir))
        #expect(settled.configDirEnv == nil)
        #expect(settled.email == "me@personal.dev")
        #expect(registry.isDefault(settled))
        registry.record(AccountSighting(configDir: dir, configDirEnv: dir, sessionId: "s4", at: Date()))
        #expect(registry.account(forConfigDir: dir)?.configDirEnv == nil)
    }

    /// A forgotten account is not re-added by a session; it is suggested.
    @Test func forgottenAccountsComeBackOnlyAsASuggestion() async throws {
        defer { cleanUp() }
        try mkdir(".claude-work/sessions")
        try touch(".claude-work/sessions/1.json")
        let registry = registry()
        await registry.discoverNow()
        let work = try #require(registry.account(forConfigDir: home + "/.claude-work"))
        registry.remove(id: work.id)

        registry.record(AccountSighting(configDir: work.configDir, configDirEnv: work.configDir, sessionId: "s1", at: Date()))
        #expect(registry.account(id: work.id) == nil)
        #expect(registry.suggestions == [AccountFolderSuggestion(configDir: work.configDir, reason: .seenAgain)])

        try registry.acceptSuggestion(work.configDir)
        #expect(registry.account(id: work.id) != nil)
        #expect(registry.suggestions.isEmpty)
    }

    @Test func homeIsNeverAnAccount() throws {
        defer { cleanUp() }
        let registry = registry()
        registry.record(AccountSighting(configDir: home, configDirEnv: home, sessionId: "s1", at: Date()))
        registry.record(AccountSighting(configDir: "/", configDirEnv: "/", sessionId: "s2", at: Date()))
        #expect(registry.accounts.isEmpty)
    }

    // MARK: - Adding folders

    @Test func addFolderRefusesHomeAndItsParents() throws {
        defer { cleanUp() }
        try mkdir(".claude/projects")
        try touch(".claude.json", #"{"oauthAccount":{"emailAddress":"me@x.com"}}"#)
        let registry = registry()
        #expect(throws: AccountFolderError.homeFolder) { try registry.addAccount(configDir: home) }
        #expect(throws: AccountFolderError.homeFolder) { try registry.addAccount(configDir: home + "/") }
        #expect(throws: AccountFolderError.containsAccounts) { try registry.addAccount(configDir: (home as NSString).deletingLastPathComponent) }
        #expect(throws: AccountFolderError.containsAccounts) { try registry.addAccount(configDir: "/") }
        #expect(throws: AccountFolderError.missing) { try registry.addAccount(configDir: home + "/nope") }
        #expect(throws: AccountFolderError.notAFolder) { try registry.addAccount(configDir: home + "/.claude.json") }
        // ~/.claude was found when the registry was made (it discovers at
        // once); inside it is inside that account.
        #expect(throws: AccountFolderError.insideAccount("Claude X")) { try registry.addAccount(configDir: home + "/.claude/projects") }
        #expect(registry.accounts.map(\.configDir) == [home + "/.claude"])
    }

    @Test func folderMarkersSayWhetherToAsk() throws {
        defer { cleanUp() }
        try mkdir(".claude-a/projects")
        try mkdir(".claude-b")
        try touch(".claude-b/.claude.json")
        try mkdir("somewhere")
        let registry = registry()
        #expect(try registry.checkFolder(home + "/.claude-a").isClearlyConfigDir)
        let onlyConfig = try registry.checkFolder(home + "/.claude-b")
        #expect(onlyConfig.hasGlobalConfig && !onlyConfig.isClearlyConfigDir)
        #expect(try registry.checkFolder(home + "/somewhere") == AccountFolderMarkers())
    }

    // MARK: - User actions and persistence

    @Test func renameHideRemoveAndPersist() async throws {
        defer { cleanUp() }
        try mkdir(".claude/projects")
        try mkdir(".claude-work/projects")
        try signIn(".claude-work/.claude.json", email: "me@work.com")

        let registry = registry()
        await registry.discoverNow()
        let work = try #require(registry.account(forConfigDir: home + "/.claude-work"))

        registry.rename(id: work.id, label: "  Work  ")
        #expect(registry.account(id: work.id)?.customLabel == "Work")
        #expect(registry.account(id: work.id)?.label == "Work")
        registry.setHidden(id: work.id, true)
        #expect(registry.visibleAccounts.map(\.id) == [home + "/.claude"])
        registry.saveNow()

        // A new registry on the same store restores the user's choices.
        let reloaded = self.registry()
        let restored = try #require(reloaded.account(id: work.id))
        #expect(restored.customLabel == "Work")
        #expect(restored.isHidden)
        #expect(restored.colorIndex == work.colorIndex)
        #expect(restored.configDirEnv == work.configDirEnv)

        // Removing forgets it; discovery doesn't bring it back; the folder stays.
        reloaded.rename(id: work.id, label: nil)
        #expect(reloaded.account(id: work.id)?.customLabel == nil)
        reloaded.remove(id: work.id)
        await reloaded.discoverNow()
        #expect(reloaded.account(id: work.id) == nil)
        #expect(FileManager.default.fileExists(atPath: home + "/.claude-work"))
        reloaded.saveNow()

        let again = self.registry()
        await again.discoverNow()
        #expect(again.account(id: work.id) == nil)

        // Adding it by hand brings it back.
        let added = try again.addAccount(configDir: home + "/.claude-work")
        #expect(added.source == .manual)
        #expect(again.account(id: work.id) != nil)
    }

    @Test func createAccountMakesTheFolder() throws {
        defer { cleanUp() }
        let registry = registry()
        let account = try registry.createAccount(name: "Side Project")
        let expectedDir = home + "/.claude-side-project"
        #expect(account.configDir == expectedDir)
        #expect(account.configDirEnv == expectedDir)
        #expect(account.customLabel == "Side Project")
        #expect(account.launchCommand == "CLAUDE_CONFIG_DIR='\(expectedDir)' claude")
        var isDirectory: ObjCBool = false
        #expect(FileManager.default.fileExists(atPath: expectedDir, isDirectory: &isDirectory) && isDirectory.boolValue)

        #expect(throws: AccountRegistry.CreateAccountError.invalidName) {
            try registry.createAccount(name: "  /// ")
        }
    }

    /// "New account…" never adopts another tool's folder that happens to
    /// have the same name; a real config folder is adopted.
    @Test func createAccountRefusesAForeignFolder() throws {
        defer { cleanUp() }
        try mkdir(".claude-server-commander")
        try touch(".claude-server-commander/config.json")
        try mkdir(".claude-existing/projects")
        let registry = registry()
        #expect(throws: AccountRegistry.CreateAccountError.folderExistsNotClaude("~/.claude-server-commander")) {
            try registry.createAccount(name: "Server Commander")
        }
        #expect(registry.accounts.isEmpty)
        #expect(try registry.createAccount(name: "existing").configDir == home + "/.claude-existing")
    }

    @Test func launchCommandQuotesTheFolder() {
        let odd = ClaudeAccount(configDir: "/Users/u/my \"quoted\" $HOME dir", configDirEnv: "/Users/u/it's `x` $(y)")
        #expect(odd.launchCommand == #"CLAUDE_CONFIG_DIR='/Users/u/it'\''s `x` $(y)' claude"#)
        #expect(ShellWords.words(odd.launchCommand) == ["CLAUDE_CONFIG_DIR=/Users/u/it's `x` $(y)", "claude"])
        #expect(ClaudeAccount(configDir: AccountPaths.defaultConfigDir).launchCommand == "claude")
        // A raw `~` is kept as the session spelled it (Claude Code keys the login by it).
        #expect(ClaudeAccount(configDir: "/Users/u/.claude-w", configDirEnv: "~/.claude-w").launchCommand == "CLAUDE_CONFIG_DIR='~/.claude-w' claude")
    }

    @Test func sanitizedNames() {
        #expect(AccountRegistry.sanitizedAccountName("Work") == "work")
        #expect(AccountRegistry.sanitizedAccountName("my team_2") == "my-team_2")
        #expect(AccountRegistry.sanitizedAccountName("../../etc") == "etc")
        #expect(AccountRegistry.sanitizedAccountName("émile") == "mile")
        #expect(AccountRegistry.sanitizedAccountName("   ") == nil)
    }

    @Test func colourIndexPicksTheLeastUsed() {
        #expect(AccountRegistry.nextColorIndex(used: []) == 0)
        #expect(AccountRegistry.nextColorIndex(used: [0, 1, 3]) == 2)
        #expect(AccountRegistry.nextColorIndex(used: Array(0..<8)) == 0)
        #expect(AccountRegistry.nextColorIndex(used: Array(0..<8) + [0, 1]) == 2)
        #expect(AccountRegistry.nextColorIndex(used: [9]) == 0)  // 9 wraps to 1
        #expect(AccountRegistry.nextColorIndex(used: [0, 9]) == 2)
    }

    @Test func sortingPutsDefaultFirst() {
        let accounts = [
            ClaudeAccount(configDir: home + "/.claude-zeta", configDirEnv: home + "/.claude-zeta", customLabel: "Zeta"),
            ClaudeAccount(configDir: home + "/.claude-alpha", configDirEnv: home + "/.claude-alpha", customLabel: "beta"),
            ClaudeAccount(configDir: home + "/.claude"),
        ]
        let sorted = AccountRegistry.sorted(accounts, home: home)
        #expect(sorted.map(\.configDir) == [home + "/.claude", home + "/.claude-alpha", home + "/.claude-zeta"])
    }
}
