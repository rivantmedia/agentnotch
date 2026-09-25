import Foundation
import Testing
@testable import ClaudeControl

/// Which account each session belonged to, remembered after it ends.
struct SessionLedgerTests {
    static func observation(_ id: String = CloudFixture.sessionA, cwd: String = "/Users/me/code/app",
                            entrypoint: String? = "claude-vscode", lastActivity: Date = CloudFixture.base,
                            cost: Double? = nil, title: String? = nil,
                            account: CloudAccountInfo = CloudFixture.account, startedAt: Date = CloudFixture.base,
                            processStartedAt: Date? = nil) -> LiveSessionObservation {
        LiveSessionObservation(sessionId: id, identityId: account.identityId, accountKey: account.accountKey,
                               cwd: cwd, transcriptPath: "/Users/me/.claude/projects/-Users-me-code-app/\(id).jsonl",
                               configDir: "/Users/me/.claude", entrypoint: entrypoint, startedAt: startedAt,
                               lastActivityAt: lastActivity, model: "claude-opus-4-5", costUsd: cost, title: title,
                               processStartedAt: processStartedAt)
    }

    @Test func capturesTheAccountAndProjectAndKeepsThem() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-ledger"))
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent(SessionLedger.fileName)
        let ledger = SessionLedger(fileURL: file, persists: true, home: "/Users/me")
        ledger.observe(live: [Self.observation(cost: 1.5, title: "Fix the notch")], liveIDs: [CloudFixture.sessionA],
                       accounts: [CloudFixture.accountKey: CloudFixture.account.ledgerAccount], now: CloudFixture.base)
        let entry = try #require(ledger.entry(CloudFixture.sessionA))
        #expect(entry.accountKey == CloudFixture.accountKey && entry.identityId == CloudFixture.identityId)
        #expect(entry.projectName == "app")
        #expect(entry.projectPath == CloudKeys.projectPath(forCwd: "/Users/me/code/app", home: "/Users/me"))
        #expect(entry.key == "\(CloudFixture.sessionA)|\(CloudFixture.accountKey)")
        #expect(entry.source == .vscode && entry.origin == .live && entry.endedAt == nil)
        #expect(entry.costUsd == 1.5 && entry.title == "Fix the notch" && entry.model == "claude-opus-4-5")

        ledger.saveNow()
        #expect(CloudFiles.permissions(of: file) == 0o600)
        let reloaded = SessionLedger(fileURL: file, persists: true, home: "/Users/me")
        #expect(reloaded.entry(CloudFixture.sessionA) == entry)
        #expect(reloaded.account(forKey: CloudFixture.accountKey)?.email == "me@example.com")
    }

    @Test func aSessionEndsAMinuteAfterItGoesAndComesBack() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(100))], liveIDs: [CloudFixture.sessionA],
                       accounts: [:], now: start.addingTimeInterval(100))
        // Gone: not ended at once...
        #expect(ledger.settle(liveIDs: [], now: start.addingTimeInterval(120)).isEmpty)
        #expect(ledger.entry(CloudFixture.sessionA)?.endedAt == nil)
        // ...but a minute later, dated when it went.
        #expect(ledger.settle(liveIDs: [], now: start.addingTimeInterval(190))
                == [CloudLedgerEntry.key(sessionId: CloudFixture.sessionA, accountKey: CloudFixture.accountKey)])
        #expect(ledger.entry(CloudFixture.sessionA)?.endedAt == start.addingTimeInterval(120))
        // Still running but not attributable this time: not ended.
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(300))], liveIDs: [CloudFixture.sessionA],
                       accounts: [:], now: start.addingTimeInterval(300))
        #expect(ledger.entry(CloudFixture.sessionA)?.endedAt == nil)
        ledger.observe(live: [], liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(320))
        #expect(ledger.settle(liveIDs: [CloudFixture.sessionA], now: start.addingTimeInterval(900)).isEmpty)
        #expect(ledger.entry(CloudFixture.sessionA)?.endedAt == nil)
    }

    /// Regression (review finding 22): capture paused (sync off) while a
    /// session went away: when it resumes, the session ends at its last
    /// activity, not when capture resumed.
    @Test func aSessionGoneWhileCapturePausedEndsAtItsLastActivity() {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(100))], liveIDs: [CloudFixture.sessionA],
                       accounts: [:], now: start.addingTimeInterval(100))
        ledger.forgetRunState()
        let resumed = start.addingTimeInterval(2 * 24 * 3600)
        ledger.observe(live: [], liveIDs: [], accounts: [:], now: resumed)
        #expect(ledger.settle(liveIDs: [], now: resumed.addingTimeInterval(61)) == [ledger.entry(CloudFixture.sessionA)!.key])
        #expect(ledger.entry(CloudFixture.sessionA)?.endedAt == start.addingTimeInterval(100))
    }

    /// Regression (review finding 7): a session resumed under another
    /// account is split. The first account's part ends when the second's
    /// process started; the owners say who ran it from when; each part
    /// keeps its own account.
    @Test func aSessionResumedUnderAnotherAccountIsSplit() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        let personal = CloudFixture.account, work = CloudFixture.workAccount
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(600), account: personal)],
                       liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(600))
        // Its limit hit, the window switches account and resumes the session.
        let resumedAt = start.addingTimeInterval(900)
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(960), account: work,
                                               startedAt: start.addingTimeInterval(950), processStartedAt: resumedAt)],
                       liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(960))
        let segments = ledger.segments(of: CloudFixture.sessionA)
        #expect(segments.map(\.accountKey) == [personal.accountKey, work.accountKey])
        #expect(segments[0].endedAt == resumedAt && segments[0].identityId == personal.identityId)
        #expect(segments[1].endedAt == nil && segments[1].startedAt == resumedAt && segments[1].identityId == work.identityId)
        #expect(ledger.owners(of: CloudFixture.sessionA) == [SessionOwner(from: nil, accountKey: personal.accountKey),
                                                             SessionOwner(from: resumedAt, accountKey: work.accountKey)])
        #expect(ledger.entry(CloudFixture.sessionA)?.accountKey == work.accountKey)
        // Seen under its account again: no new split.
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(1000), account: work)],
                       liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(1000))
        #expect(ledger.owners(of: CloudFixture.sessionA).count == 2)
        // Back to the first account: its part reopens, the owners go on.
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(2000), account: personal,
                                               processStartedAt: start.addingTimeInterval(1990))],
                       liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(2000))
        #expect(ledger.owners(of: CloudFixture.sessionA).map(\.accountKey) == [personal.accountKey, work.accountKey, personal.accountKey])
        #expect(ledger.entry(sessionId: CloudFixture.sessionA, accountKey: personal.accountKey)?.endedAt == nil)
        #expect(ledger.entry(sessionId: CloudFixture.sessionA, accountKey: work.accountKey)?.endedAt == start.addingTimeInterval(1990))
        #expect(ledger.count == 2)

        // Running under both at once (two windows): which one a response
        // came from can't be told, so it stays with the one it ran as.
        let both = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        both.observe(live: [Self.observation(account: personal)], liveIDs: [CloudFixture.sessionB], accounts: [:], now: start)
        both.observe(live: [Self.observation(account: personal), Self.observation(account: work)],
                     liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(10))
        #expect(both.segments(of: CloudFixture.sessionA).map(\.accountKey) == [personal.accountKey])
        #expect(both.owners(of: CloudFixture.sessionA).count == 1)
        both.observe(live: [Self.observation(CloudFixture.sessionC, account: personal),
                            Self.observation(CloudFixture.sessionC, account: work)],
                     liveIDs: [CloudFixture.sessionC], accounts: [:], now: start.addingTimeInterval(20))
        #expect(!both.knows(CloudFixture.sessionC))
    }

    @Test func ownersSayWhoRanTheSessionWhen() {
        let a = "key-a", b = "key-b"
        let t1 = CloudFixture.base, t2 = CloudFixture.base.addingTimeInterval(100)
        let owners = [SessionOwner(from: nil, accountKey: a), SessionOwner(from: t1, accountKey: b), SessionOwner(from: t2, accountKey: a)]
        #expect(SessionOwners.owner(at: nil, in: owners) == a)
        #expect(SessionOwners.owner(at: t1.addingTimeInterval(-1), in: owners) == a)
        #expect(SessionOwners.owner(at: t1, in: owners) == b)
        #expect(SessionOwners.owner(at: t2.addingTimeInterval(5), in: owners) == a)
        #expect(SessionOwners.owner(at: t1, in: []) == "")
        let single = [SessionOwner(from: nil, accountKey: a)]
        // Counted with one owner up to before the hand-over: still right.
        #expect(SessionOwners.agree(single, Array(owners.prefix(2)), through: t1.addingTimeInterval(-1)))
        #expect(!SessionOwners.agree(single, Array(owners.prefix(2)), through: t1))
        #expect(!SessionOwners.agree(single, [SessionOwner(from: nil, accountKey: b)], through: nil))
        #expect(SessionOwners.stretches(of: a, in: single) == nil)
        let stretches = SessionOwners.stretches(of: a, in: owners) ?? []
        #expect(stretches.count == 2)
        #expect(SessionOwners.contains(stretches, t1.addingTimeInterval(-1)))
        #expect(!SessionOwners.contains(stretches, t1.addingTimeInterval(50)))
        #expect(SessionOwners.contains(stretches, t2))
        #expect(!SessionOwners.contains(stretches, nil))
    }

    @Test func somethingThatIsNotASessionIsNeverCaptured() {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        var badId = Self.observation("not-a-uuid")
        badId.sessionId = "not-a-uuid"
        var badKey = Self.observation(CloudFixture.sessionB)
        badKey.accountKey = "email-account"
        let noCwd = Self.observation(CloudFixture.sessionC, cwd: "")
        ledger.observe(live: [badId, badKey, noCwd], liveIDs: [], accounts: [:], now: CloudFixture.base)
        #expect(ledger.count == 0)
    }

    @Test func liveCaptureTakesOverABackfilledSession() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        var found = SessionLedger.backfillEntry(CloudFixture.sessionA)
        found.projectName = "old"
        #expect(ledger.record(backfill: [found], accounts: [:]) == 1)
        #expect(ledger.record(backfill: [found], accounts: [:]) == 0)
        ledger.observe(live: [Self.observation()], liveIDs: [CloudFixture.sessionA], accounts: [:], now: CloudFixture.base)
        let entry = try #require(ledger.entry(CloudFixture.sessionA))
        #expect(entry.origin == .live && entry.projectName == "app" && entry.endedAt == nil)
    }

    @Test func writesAreThrottledToTheNewestValue() async throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-file"))
        defer { try? FileManager.default.removeItem(at: root) }
        let url = root.appendingPathComponent("state.json")
        let file = CloudStateFile<[String]>(url: url, persists: true, label: "test", writeDelay: 0.2)
        file.save(["first"])
        file.save(["second"])
        #expect(!FileManager.default.fileExists(atPath: url.path))
        try await Task.sleep(for: .milliseconds(700))
        #expect(file.load() == ["second"])
        #expect(CloudFiles.permissions(of: url) == 0o600)
        file.save(["third"])
        file.saveNow(["now"])
        try await Task.sleep(for: .milliseconds(500))
        #expect(file.load() == ["now"])
        // Not persisted: never touches the disk.
        let memory = CloudStateFile<[String]>(url: root.appendingPathComponent("memory.json"), persists: false, label: "test")
        memory.saveNow(["x"])
        #expect(!FileManager.default.fileExists(atPath: root.appendingPathComponent("memory.json").path))
    }

    // MARK: - Backfill roots

    @Test func onlyAFoldersOwnHistoryIsBackfilled() throws {
        let root = TestPaths.temporaryRoot("cloud-backfill")
        defer { try? FileManager.default.removeItem(atPath: root) }
        let fm = FileManager.default
        func folder(_ name: String) -> String { (root as NSString).appendingPathComponent(name) }
        // Its own history.
        try fm.createDirectory(atPath: folder(".claude-own/projects"), withIntermediateDirectories: true)
        // Claude Parallel Profiles: projects/ linked to a shared history.
        try fm.createDirectory(atPath: folder(".claude-shared/projects"), withIntermediateDirectories: true)
        try fm.createDirectory(atPath: folder(".claude-a"), withIntermediateDirectories: true)
        try fm.createSymbolicLink(atPath: folder(".claude-a/projects"), withDestinationPath: folder(".claude-shared/projects"))
        // A folder with no account to give it (not signed in, or a store).
        try fm.createDirectory(atPath: folder(".claude-store/projects"), withIntermediateDirectories: true)

        let since = CloudFixture.base
        let folders = [
            CloudBackfill.Folder(configDir: folder(".claude-own"), identityId: "uuid:a", accountKey: "key-a", login: "a", signedInSince: since),
            CloudBackfill.Folder(configDir: folder(".claude-a"), identityId: "uuid:b", accountKey: "key-b", login: "b", signedInSince: since),
            CloudBackfill.Folder(configDir: folder(".claude-shared"), identityId: nil, accountKey: nil),
            CloudBackfill.Folder(configDir: folder(".claude-store"), identityId: nil, accountKey: nil),
            CloudBackfill.Folder(configDir: folder(".claude-missing"), identityId: "uuid:c", accountKey: "key-c", login: "c", signedInSince: since),
        ]
        let roots = CloudBackfill.roots(folders: folders)
        #expect(roots.map(\.identityId) == ["uuid:a"])
        #expect(roots.first?.projects == TranscriptLocator.realPath(folder(".claude-own/projects")))
        #expect(roots.first?.signedInSince == since)
        // Since when it is that account's isn't known: nothing is read (finding 0).
        var undated = folders[0]
        undated.signedInSince = nil
        #expect(CloudBackfill.roots(folders: [undated]).isEmpty)

        // A real projects folder that another known folder links to is shared too.
        try fm.createDirectory(atPath: folder(".claude-b"), withIntermediateDirectories: true)
        try fm.createSymbolicLink(atPath: folder(".claude-b/projects"), withDestinationPath: folder(".claude-own/projects"))
        let linkedTo = CloudBackfill.roots(folders: [
            folders[0],
            CloudBackfill.Folder(configDir: folder(".claude-b"), identityId: nil, accountKey: nil),
        ])
        #expect(linkedTo.isEmpty)
        let elsewhere = CloudBackfill.roots(folders: [folders[0]], realPath: { _ in "/shared/projects" })
        #expect(elsewhere.isEmpty)
    }

    /// Regression (review finding 0): "signed in as <login> since <date>",
    /// set when the app first sees a folder's login and again when it
    /// changes; unknown before; unchanged by a sign-out.
    @Test func foldersRememberSinceWhenTheyAreSignedInAsWhom() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-logins"))
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent(CloudFolderLogins.fileName)
        let logins = CloudFolderLogins(fileURL: file, persists: true)
        let t0 = CloudFixture.base
        #expect(logins.since(folder: "/Users/me/.claude-work", login: "a") == nil)
        logins.observe(["/Users/me/.claude-work": "a"], now: t0)
        logins.observe(["/Users/me/.claude-work/": "a"], now: t0.addingTimeInterval(60))
        #expect(logins.since(folder: "/Users/me/.claude-work", login: "a") == t0)
        #expect(logins.since(folder: "/Users/me/.claude-work", login: "b") == nil)
        #expect(logins.since(folder: "/Users/me/.claude-work", login: nil) == nil)
        // Signed out for a while (left out of the logins), back as the same account.
        logins.observe([:], now: t0.addingTimeInterval(120))
        logins.observe(["/Users/me/.claude-work": "a"], now: t0.addingTimeInterval(180))
        #expect(logins.since(folder: "/Users/me/.claude-work", login: "a") == t0)
        // /login as someone else: from then on.
        logins.observe(["/Users/me/.claude-work": "b"], now: t0.addingTimeInterval(240))
        #expect(logins.since(folder: "/Users/me/.claude-work", login: "b") == t0.addingTimeInterval(240))
        #expect(logins.since(folder: "/Users/me/.claude-work", login: "a") == nil)
        logins.saveNow()
        #expect(CloudFiles.permissions(of: file) == 0o600)
        #expect(CloudFolderLogins(fileURL: file, persists: true).since(folder: "/Users/me/.claude-work", login: "b")
                == t0.addingTimeInterval(240))
        #expect(!(try String(contentsOf: file, encoding: .utf8)).contains("me@example.com"))

        // A login is who the folder's own oauthAccount names.
        let a = CloudBackfill.login(accountUuid: "U", organizationUuid: "O", email: "me@example.com")
        #expect(a == CloudBackfill.login(accountUuid: "u", organizationUuid: "o", email: "ME@example.com"))
        #expect(a != CloudBackfill.login(accountUuid: "u", organizationUuid: "other", email: "me@example.com"))
        #expect(CloudBackfill.login(accountUuid: nil, organizationUuid: nil, email: nil) == nil)
    }

    /// Only a run folder whose own login is the account's is backfilled: not
    /// a corrected (mirrored) copy, nor `~/.claude` while the extension
    /// mirrors into it.
    @Test func backfillFoldersAreRunFoldersOfTheirOwnAccount() {
        let home = "/Users/me"
        let own = ClaudeAccount(configDir: "/Users/me/.claude-own", email: "me@example.com", accountUuid: CloudFixture.accountUuid)
        let mirrored = ClaudeAccount(configDir: "/Users/me/.claude-mirror", email: "me@example.com", accountUuid: "stale")
        let store = ClaudeAccount(configDir: "/Users/me/.claude-store", email: "me@example.com",
                                  accountUuid: CloudFixture.accountUuid, kind: .store)
        let defaultFolder = ClaudeAccount(configDir: "/Users/me/.claude", email: "me@example.com", accountUuid: CloudFixture.accountUuid)
        let folders = LiveCloudEnvironment.backfillFolders(
            folders: [own, mirrored, store, defaultFolder], accounts: [CloudFixture.account],
            identityOfFolder: Dictionary(uniqueKeysWithValues: [own, mirrored, store, defaultFolder].map { ($0.id, CloudFixture.identityId) }),
            defaultFolder: AccountRegistry.defaultConfigDir(home: home),
            mirrorsDefault: true, correctedFolders: [mirrored.id], infrastructure: ["/Users/me/.claude-shared"])
        #expect(folders.filter { $0.accountKey != nil }.map(\.configDir) == [own.configDir])
        #expect(folders.first?.login == CloudBackfill.login(accountUuid: CloudFixture.accountUuid, organizationUuid: nil,
                                                             email: "me@example.com"))
        #expect(folders.last?.configDir == "/Users/me/.claude-shared" && folders.last?.login == nil)
        #expect(LiveCloudEnvironment.folderLogins([own, ClaudeAccount(configDir: "/Users/me/.claude-empty")]).keys.sorted()
                == [own.configDir])
    }

    // MARK: - From the hub

    @Test func aRunningSessionIsCapturedWithItsTitleNeverItsPrompt() throws {
        var state = SessionState(sessionId: CloudFixture.sessionA, cwd: "/Users/me/code/app", projectName: "app")
        state.transcriptPath = "/Users/me/.claude/projects/-Users-me-code-app/\(CloudFixture.sessionA).jsonl"
        state.accountId = "/Users/me/.claude"
        state.entrypoint = "cli"
        state.costUSD = 0.42
        state.model = "claude-opus-4-5"
        state.conversationInfo = ConversationInfo(summary: nil, lastMessage: nil, lastMessageRole: nil, lastToolName: nil,
                                                  firstUserMessage: "my secret prompt", lastUserMessageDate: nil)
        state.pidStartedAt = CloudFixture.base.addingTimeInterval(-30)
        let identity = ClaudeIdentityAccount(id: CloudFixture.identityId, ringID: "claude-acct-x", accountUuid: CloudFixture.accountUuid,
                                             runDirs: [], storeDirs: [], colorIndex: 0, isHidden: false)
        var observation = try #require(ClaudeControlHub.cloudObservation(state: state, identity: identity))
        #expect(observation.accountKey == CloudFixture.accountKey)
        #expect(observation.processStartedAt == CloudFixture.base.addingTimeInterval(-30))
        // An account in an organization: the organization is part of its key (finding 6).
        let work = ClaudeIdentityAccount(id: CloudFixture.workIdentityId, ringID: "claude-acct-w",
                                         organizationUuid: CloudFixture.workOrganization, accountUuid: CloudFixture.workUuid,
                                         runDirs: [], storeDirs: [], colorIndex: 0, isHidden: false)
        #expect(ClaudeControlHub.cloudObservation(state: state, identity: work)?.accountKey == CloudFixture.workAccountKey)
        #expect(observation.title == nil)
        #expect(observation.costUsd == 0.42 && observation.entrypoint == "cli" && observation.configDir == "/Users/me/.claude")

        state.applyTitle("my-project-3", source: .derivedName)
        observation = try #require(ClaudeControlHub.cloudObservation(state: state, identity: identity))
        #expect(observation.title == nil)
        state.applyTitle("Fix the notch", source: .hook)
        observation = try #require(ClaudeControlHub.cloudObservation(state: state, identity: identity))
        #expect(observation.title == "Fix the notch")

        let emailOnly = ClaudeIdentityAccount(id: "email:me@example.com", ringID: "claude-acct-y", email: "me@example.com",
                                              runDirs: [], storeDirs: [], colorIndex: 0, isHidden: false)
        #expect(ClaudeControlHub.cloudObservation(state: state, identity: emailOnly) == nil)
    }
}

extension SessionLedger {
    /// A session the backfill found, for tests.
    static func backfillEntry(_ id: String) -> CloudLedgerEntry {
        CloudLedgerEntry(sessionId: id, identityId: CloudFixture.identityId, accountKey: CloudFixture.accountKey,
                         projectName: "app", projectPath: "/x",
                         transcriptPath: nil, configDir: nil, source: .cli, startedAt: CloudFixture.base,
                         lastActivityAt: CloudFixture.base, endedAt: CloudFixture.base, model: nil, costUsd: nil,
                         title: nil, origin: .backfill)
    }
}
