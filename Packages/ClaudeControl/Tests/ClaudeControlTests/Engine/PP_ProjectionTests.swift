import Foundation
import Testing
@testable import ClaudeControl

/// What the app is handed: one account per identity with its folders, and
/// the per-folder rings of earlier versions moved over.
@MainActor
@Suite(.serialized)
struct PP_ProjectionTests {
    typealias Home = ParallelProfilesHome

    @Test func summariesCarryTheAccountsFolders() throws {
        let fake = Home("summary")
        defer { fake.cleanUp() }
        try fake.buildUserLayout(vibeNotch: false)
        let registry = fake.registry()
        let summaries = registry.identities.map {
            ClaudeHostProjections.account(identity: $0, hookStatuses: [:], home: fake.home)
        }
        #expect(summaries.map(\.email) == [Home.biios, Home.paras])
        let paras = summaries[1]
        #expect(paras.id == "uuid:" + Home.parasUUID)
        #expect(paras.configDir == fake.path(".claude"))
        #expect(paras.isDefault)
        #expect(paras.launchCommand == "claude")
        #expect(paras.windowCount == 2)
        #expect(paras.runDirs == [".claude", ".claude-windows/801f9dd51396", ".claude-windows/b9fbb9ecd7cb"].map(fake.path))
        #expect(paras.storeDirs == [".claude-paras", ".claude-paras-rivant-in"].map(fake.path))
        #expect(paras.configDirs.count == 5)
        #expect(paras.planName == "Max 20x")
        #expect(paras.formerRingIDs.first == "claude")
        #expect(paras.formerRingIDs.contains("claude-paras") && paras.formerRingIDs.contains("claude-paras-rivant-in"))
        let biios = summaries[0]
        #expect(!biios.isDefault)
        #expect(biios.formerRingIDs.contains("claude-claude"))
        // A VS Code window's working copy is that window's: no terminal
        // command points into it (UX-3, PP-C6).
        #expect(!biios.hasTerminalLaunch)
        #expect(!biios.launchCommand.contains(".claude-windows"))
        #expect(paras.hasTerminalLaunch)

        // No ring id is shared, and the shared history's is retired.
        #expect(Set(summaries.map(\.ringID)).count == 2)
        let retired = ClaudeControlHub.retiredRingIDs(current: Set(summaries.map(\.ringID)), registry: registry, home: fake.home)
        #expect(retired.contains("claude-shared"))
        #expect(retired.contains("claude") && retired.contains("claude-claude"))
        #expect(!retired.contains(paras.ringID))
    }

    @Test func hooksAreCountedOverRunFoldersOnly() {
        let run = ["/h/.claude", "/h/.claude-windows/a", "/h/.claude-windows/b"].map { ClaudeAccount(configDir: $0) }
        let identity = ClaudeIdentityAccount(id: "uuid:u", ringID: "claude-acct-u", runDirs: run,
                                             storeDirs: [ClaudeAccount(configDir: "/h/.claude-u", kind: .store)],
                                             customLabel: nil, colorIndex: 0, isHidden: false)
        var installed = AccountHookStatus()
        installed.hooksInstalled = true
        installed.statusLineInstalled = true
        var spvn = AccountHookStatus()
        spvn.superpoweredVibeNotchHooksPresent = true
        let hooks = ClaudeHostProjections.hookStatus(identity: identity, statuses: [
            "/h/.claude": installed, "/h/.claude-windows/a": installed, "/h/.claude-windows/b": AccountHookStatus(),
            "/h/.claude-u": spvn,
        ])
        #expect(!hooks.hooksInstalled)
        #expect(hooks.folderCount == 3 && hooks.installedFolderCount == 2)
        #expect(hooks.superpoweredVibeNotchHooksPresent)   // in its store
    }

    /// Sessions in a VS Code window land on the window's account's ring.
    @Test func sessionsRunInAWindowAreOnItsAccountsRing() {
        let home = "/h"
        FolderRings.set(["/h/.claude-windows/b": "claude-acct-b", "/h/.claude": "claude-acct-a"])
        defer { FolderRings.set([:]) }
        var session = SessionState(sessionId: "s", cwd: "/tmp")
        session.accountId = "/h/.claude-windows/b"
        #expect(ClaudeHostProjections.ringID(for: session, home: home) == "claude-acct-b")
        session.accountId = "/h/.claude-other"
        #expect(ClaudeHostProjections.ringID(for: session, home: home) == "claude-other")
        let accounts = [
            ClaudeAccountSummary(id: "uuid:b", ringID: "claude-acct-b", configDir: "/h/.claude-windows/b", label: "B",
                                 isDefault: false, launchCommand: "x"),
            ClaudeAccountSummary(id: "uuid:a", ringID: "claude-acct-a", configDir: "/h/.claude", label: "A",
                                 isDefault: true, launchCommand: "claude"),
        ]
        #expect(ClaudeControlHub.defaultRingID(accounts: accounts) == "claude-acct-a")
        #expect(ClaudeControlHub.ringID("claude-unknown", knownRingIDs: ["claude-acct-a"], defaultRingID: "claude-acct-a") == "claude-acct-a")
    }
}

/// Codenotch's choices for the per-folder rings move to the account rings.
struct PP_RingMigrationTests {
    private func account(_ ring: String, former: [String]) -> ClaudeAccountSummary {
        var summary = ClaudeAccountSummary(id: ring, ringID: ring, configDir: "/h", label: ring, isDefault: false, launchCommand: "claude")
        summary.formerRingIDs = former
        return summary
    }

    /// The user's rings: `claude`, `claude-shared`, `claude-claude`,
    /// `claude-paras`, `claude-paras-rivant-in`.
    @Test func theUsersRingsBecomeTwo() {
        let paras = account("claude-acct-p", former: ["claude", "claude-dir-aaaaaaaa", "claude-paras", "claude-paras-rivant-in"])
        let biios = account("claude-acct-b", former: ["claude-dir-bbbbbbbb", "claude-claude"])
        let stored = ClaudeRingMigration.Stored(
            nicknames: ["claude-paras": "Work Max", "claude": "Main", "claude-shared": "Shared", "codex": "Codex!"],
            seen: ["claude", "claude-shared", "claude-claude", "claude-paras", "claude-paras-rivant-in", "codex",
                   "claude-acct-p", "claude-acct-b"],
            connected: ["claude", "claude-shared", "claude-paras", "codex", "claude-acct-p", "claude-acct-b"],
            order: ["codex", "claude-paras", "claude", "claude-shared", "claude-claude", "claude-paras-rivant-in", "cursor"],
            archived: ["claude-paras", "claude", "claude-claude", "claude-shared", "codex"]
        )
        let plan = ClaudeRingMigration.plan(accounts: [paras, biios], retired: ["claude-shared", "claude-windows"], stored: stored)
        // First by the user's order: claude-paras sits before claude.
        #expect(plan.nicknames["claude-acct-p"] == .some("Work Max"))
        #expect(plan.nicknames["claude-acct-b"] == nil)
        #expect(plan.nicknames["claude-paras"] == .some(nil))
        #expect(plan.nicknames["claude"] == .some(nil))
        #expect(plan.nicknames["claude-shared"] == .some(nil))
        #expect(plan.nicknames["codex"] == nil)
        // claude-claude was switched off: the biios ring is too.
        #expect(plan.connected["claude-acct-b"] == false)
        #expect(plan.connected["claude-acct-p"] == nil)
        // The rings take their first old ring's place; the rest are gone.
        #expect(plan.order == ["codex", "claude-acct-p", "claude-acct-b", "cursor"])
        #expect(plan.archiveMoves == ["claude-paras": "claude-acct-p", "claude-claude": "claude-acct-b"])
        #expect(plan.archiveDrops == ["claude", "claude-shared"])
        #expect(plan.migrated == ["claude", "claude-dir-aaaaaaaa", "claude-paras", "claude-paras-rivant-in",
                                  "claude-dir-bbbbbbbb", "claude-claude", "claude-shared", "claude-windows"])

        // Once migrated, a later choice on the new ring stands.
        var after = stored
        after.migrated = plan.migrated
        after.order = plan.order ?? []
        after.connected.remove("claude-acct-p")
        #expect(ClaudeRingMigration.plan(accounts: [paras, biios], retired: ["claude-shared"], stored: after).isEmpty)
    }

    @Test func aNewRingKeepsItsOwnNickname() {
        let ring = account("claude-acct-p", former: ["claude"])
        let stored = ClaudeRingMigration.Stored(nicknames: ["claude-acct-p": "Mine", "claude": "Old"], seen: ["claude"],
                                                connected: ["claude"], order: ["claude"])
        let plan = ClaudeRingMigration.plan(accounts: [ring], retired: [], stored: stored)
        #expect(plan.nicknames["claude-acct-p"] == nil)
        #expect(plan.nicknames["claude"] == .some(nil))
        #expect(plan.connected["claude-acct-p"] == true)   // never seen: set from its old ring
        #expect(plan.order == ["claude-acct-p"])
    }
}

/// The inspection tool: the same rules, strictly read-only.
@MainActor
@Suite(.serialized)
struct PP_InspectionTests {
    typealias Home = ParallelProfilesHome

    @Test func theUsersLayoutInspectsAsTwoAccountsAndChangesNothing() throws {
        let fake = Home("inspect")
        defer { fake.cleanUp() }
        try fake.buildUserLayout()
        let before = fake.files()
        let result = ClaudeAccountInspection.inspect(home: fake.home)
        #expect(fake.files() == before)

        #expect(result.extensionDetected)
        #expect(result.accounts.count == 2)
        let paras = try #require(result.accounts.last)
        #expect(paras.email == Home.paras && paras.label == "Claude Rivant")
        #expect(paras.runDirs == ["~/.claude", "~/.claude-windows/801f9dd51396", "~/.claude-windows/b9fbb9ecd7cb"].map { relative($0, fake) })
        #expect(paras.storeDirs == ["~/.claude-paras", "~/.claude-paras-rivant-in"].map { relative($0, fake) })
        let biios = try #require(result.accounts.first)
        #expect(biios.email == Home.biios)
        #expect(biios.runDirs == [relative("~/.claude-windows/1bf3e8f92b11", fake)])
        #expect(biios.storeDirs == [relative("~/.claude-claude", fake)])
        #expect(biios.probeFolder == relative("~/.claude-windows/1bf3e8f92b11", fake))
        #expect(result.infrastructure == ["~/.claude-shared", "~/.claude-windows"].map { relative($0, fake) })
        #expect(result.installTargets.sorted() == ["~/.claude", "~/.claude-windows/1bf3e8f92b11", "~/.claude-windows/801f9dd51396",
                                                   "~/.claude-windows/b9fbb9ecd7cb"].map { relative($0, fake) }.sorted())
        #expect(result.vibeNotchCleanupTargets.sorted() == ["~/.claude", "~/.claude-claude", "~/.claude-paras",
                                                            "~/.claude-paras-rivant-in", "~/.claude-shared"].map { relative($0, fake) }.sorted())
        #expect(result.ringMoves.contains { $0.old == "claude-shared" && $0.new == nil })
        #expect(result.ringMoves.contains { $0.old == "claude-claude" && $0.new == biios.ringID })
        let report = ClaudeAccountInspection.report(home: fake.home)
        #expect(report.contains("Accounts: 2"))
        #expect(!report.contains("utilization"))
    }

    /// Only `oauthAccount` and two fields of `cachedUsageUtilization` are
    /// taken from `.claude.json`.
    @Test func onlyTheAllowedFieldsAreRead() throws {
        let data = Data(#"{"primaryApiKey":"sk-x","oauthAccount":{"accountUuid":"u","emailAddress":"e@x.dev"},"cachedUsageUtilization":{"accountUuid":"u","fetchedAtMs":1790000000000,"utilization":{"five_hour":{"utilization":3}}}}"#.utf8)
        let fake = Home("inspect-fields")
        defer { fake.cleanUp() }
        try data.write(to: URL(fileURLWithPath: fake.path("x.json")))
        let login = try #require(ClaudeAccountInspection.readLogin(at: fake.path("x.json")))
        #expect(login.identity?.email == "e@x.dev")
        #expect(login.cachedUsageAccountUuid == "u")
        #expect(login.cachedUsageFetchedAt == Date(timeIntervalSince1970: 1_790_000_000))
    }

    /// `~/…` for a path in the fake home, as the report writes it.
    private func relative(_ tilde: String, _ fake: Home) -> String {
        tilde
    }
}
