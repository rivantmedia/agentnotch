import Foundation
import Testing
@testable import ClaudeControl

/// Sessions Claude Desktop hosts (M3): what marks them in the registry, and
/// whose they are, from Desktop's own record of each (a stat, never opened).
/// Temporary folders only.
@MainActor
struct DesktopHostedSessionsTests {
    static let hostId = "local_0123abcd-89ef-4a5b-8c6d-001122334455"
    static let accountA = CloudFixture.accountUuid
    static let organizationA = "5e4d3c2b-1a09-4f8e-9d7c-6b5a49382716"
    static let accountW = CloudFixture.workUuid
    static let organizationW = CloudFixture.workOrganization

    static var candidates: [DesktopHostedSessions.Candidate] {
        [.init(identityId: CloudFixture.identityId, accountUuid: accountA, organizationUuid: organizationA),
         .init(identityId: CloudFixture.workIdentityId, accountUuid: accountW, organizationUuid: organizationW)]
    }

    /// Removed with the suite instance.
    private let home: TemporaryAccount
    let root: URL

    init() throws {
        home = try TemporaryAccount(prefix: "agentnotch-desktop")
        root = URL(fileURLWithPath: DesktopHostedSessions.root(home: home.root.path))
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    /// Desktop's record of the session, under the account and organization.
    @discardableResult
    func record(account: String, organization: String, id: String = Self.hostId) throws -> URL {
        let folder = root.appendingPathComponent(account).appendingPathComponent(organization)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let file = folder.appendingPathComponent("\(id).json")
        try Data(#"{"secret":"never read"}"#.utf8).write(to: file)
        return file
    }

    // MARK: - The registry

    @Test func theRegistryEntryCarriesDesktopsSessionId() throws {
        let entry = try #require(SessionRegistryEntry(json: [
            "pid": 4242, "sessionId": CloudFixture.sessionA, "entrypoint": "claude-desktop", "hostSessionId": Self.hostId,
        ]))
        #expect(entry.hostSessionId == Self.hostId && entry.isDesktopHosted)
        for entrypoint in ["claude-desktop-3p", "local-agent"] {
            #expect(SessionRegistryEntry(json: ["pid": 1, "sessionId": "s", "entrypoint": entrypoint])?.isDesktopHosted == true)
        }
        #expect(SessionRegistryEntry(json: ["pid": 1, "sessionId": "s", "entrypoint": "cli"])?.isDesktopHosted == false)
        #expect(SessionRegistryEntry(json: ["pid": 1, "sessionId": "s"])?.isDesktopHosted == false)
        // Only Claude Code's own form: never a path.
        for bad in ["local_../../../.ssh/id_rsa", "LOCAL_0123abcd", "local_0123ABCD-0000", "local_0123", "0123abcd-89ef",
                    "local_" + String(repeating: "a", count: 73)] {
            #expect(SessionRegistryEntry(json: ["pid": 1, "sessionId": "s", "hostSessionId": bad])?.hostSessionId == nil, "\(bad)")
        }
    }

    /// Read from `<pid>.json` only: a `*.key` beside it is never opened.
    @Test func onlyTheEntryFileIsRead() throws {
        let configDir = home.configDir
        let sessions = configDir.appendingPathComponent("sessions")
        try FileManager.default.createDirectory(at: sessions, withIntermediateDirectories: true)
        let entry: [String: Any] = ["pid": 4242, "sessionId": CloudFixture.sessionA, "entrypoint": "claude-desktop",
                                    "hostSessionId": Self.hostId]
        try JSONSerialization.data(withJSONObject: entry).write(to: sessions.appendingPathComponent("4242.json"))
        var decoy = entry
        decoy["pid"] = 4343
        decoy["hostSessionId"] = "local_99999999-0000"
        try JSONSerialization.data(withJSONObject: decoy).write(to: sessions.appendingPathComponent("4343.key"))
        let read = SessionRegistryScanner.readEntries(configDir: configDir.path)
        #expect(read.map(\.hostSessionId) == [Self.hostId])
    }

    /// The session follows its current process's entry: resumed outside
    /// Claude Desktop, it has no Desktop id any more.
    @Test func theSessionKeepsItsProcesssDesktopId() async throws {
        let store = SessionStore.forTests(reviewFile: home.reviewFile)
        let configDir = home.configDir.path
        let hosted = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", entrypoint: "claude-desktop",
                                          status: "idle", statusUpdatedAt: Date(), hostSessionId: Self.hostId)
        await store.process(.registrySnapshot(configDir: configDir, entries: [hosted]))
        #expect(await store.session(for: "s1")?.hostSessionId == Self.hostId)
        let resumed = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", entrypoint: "cli",
                                           status: "busy", statusUpdatedAt: Date().addingTimeInterval(1))
        await store.process(.registrySnapshot(configDir: configDir, entries: [resumed]))
        #expect(await store.session(for: "s1")?.hostSessionId == nil)
    }

    /// Regression (review): the session takes its current process's
    /// entrypoint too, as it takes its Desktop id: resumed by Claude Desktop
    /// in a new process, it is Desktop-hosted (never its old host's folder's
    /// account for certain); resumed in a terminal, it isn't.
    @Test func theSessionTakesItsNewProcesssEntrypoint() async throws {
        let store = SessionStore.forTests(reviewFile: home.reviewFile)
        let configDir = home.configDir.path
        let cli = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", entrypoint: "cli",
                                       status: "idle", statusUpdatedAt: Date())
        await store.process(.registrySnapshot(configDir: configDir, entries: [cli]))
        #expect(await store.session(for: "s1")?.entrypoint == "cli")
        let desktop = SessionRegistryEntry(pid: Int(getppid()), sessionId: "s1", cwd: "/tmp/proj", entrypoint: "claude-desktop",
                                           status: "busy", statusUpdatedAt: Date().addingTimeInterval(1), hostSessionId: Self.hostId)
        await store.process(.registrySnapshot(configDir: configDir, entries: [desktop]))
        let hosted = try #require(await store.session(for: "s1"))
        #expect(hosted.entrypoint == "claude-desktop" && hosted.hostSessionId == Self.hostId)
        #expect(DesktopHostedSessions.isDesktopHosted(entrypoint: hosted.entrypoint))
        let back = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", entrypoint: "cli",
                                        status: "idle", statusUpdatedAt: Date().addingTimeInterval(2))
        await store.process(.registrySnapshot(configDir: configDir, entries: [back]))
        let resumed = try #require(await store.session(for: "s1"))
        #expect(resumed.entrypoint == "cli" && resumed.hostSessionId == nil)
        // An entry that names no entrypoint keeps the one known.
        let unnamed = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", entrypoint: nil,
                                           status: "busy", statusUpdatedAt: Date().addingTimeInterval(3))
        await store.process(.registrySnapshot(configDir: configDir, entries: [unnamed]))
        #expect(await store.session(for: "s1")?.entrypoint == "cli")
    }

    // MARK: - Whose it is

    @Test func theSessionIsTheIdentityWhoseRecordExists() throws {
        func identity() -> String? {
            DesktopHostedSessions.identity(hostSessionId: Self.hostId, candidates: Self.candidates, root: root.path)
        }
        // No record yet: nobody's (not the only account, not the folder's).
        #expect(identity() == nil)
        let file = try record(account: Self.accountW, organization: Self.organizationW)
        #expect(identity() == CloudFixture.workIdentityId)
        // Found by a stat alone: an unreadable record is still found.
        try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: file.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path) }
        #expect(identity() == CloudFixture.workIdentityId)
        // Another session's record, another organization's, or an unknown account's: not this one.
        try record(account: Self.accountA, organization: Self.organizationA, id: "local_ffffffff-0000")
        try record(account: Self.accountA, organization: Self.organizationW)
        try record(account: "c0ffee00-0000-4000-8000-000000000000", organization: Self.organizationA)
        #expect(identity() == CloudFixture.workIdentityId)
        // Under two identities: can't tell.
        try record(account: Self.accountA, organization: Self.organizationA)
        #expect(identity() == nil)
    }

    @Test func anIdentityWithNoKnownOrganizationIsLookedUpInEachOfItsOwn() throws {
        try record(account: Self.accountA, organization: Self.organizationA)
        let candidates = [DesktopHostedSessions.Candidate(identityId: CloudFixture.identityId, accountUuid: Self.accountA.uppercased(),
                                                          organizationUuid: nil)]
        #expect(DesktopHostedSessions.identity(hostSessionId: Self.hostId, candidates: candidates, root: root.path)
                == CloudFixture.identityId)
        #expect(DesktopHostedSessions.identity(hostSessionId: nil, candidates: candidates, root: root.path) == nil)
    }

    /// Regression (review): a link is never followed. A record that is a
    /// link to a real file, or one reached through a linked organization
    /// folder, account folder or root, is no record, and a linked account
    /// folder isn't listed.
    @Test func linksAreNeverFollowed() throws {
        let manager = FileManager.default
        let work = [DesktopHostedSessions.Candidate(identityId: CloudFixture.workIdentityId, accountUuid: Self.accountW,
                                                    organizationUuid: Self.organizationW)]
        let noOrganization = [DesktopHostedSessions.Candidate(identityId: CloudFixture.workIdentityId,
                                                              accountUuid: Self.accountW, organizationUuid: nil)]
        func identity(_ root: URL, _ candidates: [DesktopHostedSessions.Candidate] = work) -> String? {
            DesktopHostedSessions.identity(hostSessionId: Self.hostId, candidates: candidates, root: root.path)
        }
        // A real record elsewhere, in the same layout.
        let elsewhere = home.root.appendingPathComponent("elsewhere/claude-code-sessions")
        let realAccount = elsewhere.appendingPathComponent(Self.accountW)
        let realOrganization = realAccount.appendingPathComponent(Self.organizationW)
        try manager.createDirectory(at: realOrganization, withIntermediateDirectories: true)
        let realRecord = realOrganization.appendingPathComponent("\(Self.hostId).json")
        try Data("{}".utf8).write(to: realRecord)
        #expect(identity(elsewhere) == CloudFixture.workIdentityId)
        #expect(identity(elsewhere, noOrganization) == CloudFixture.workIdentityId)

        func makeRoot(_ name: String) throws -> URL {
            let root = home.root.appendingPathComponent("\(name)/claude-code-sessions")
            try manager.createDirectory(at: root, withIntermediateDirectories: true)
            return root
        }
        // The record is a link to a real file.
        let linkedRecord = try makeRoot("record")
        let organization = linkedRecord.appendingPathComponent(Self.accountW).appendingPathComponent(Self.organizationW)
        try manager.createDirectory(at: organization, withIntermediateDirectories: true)
        try manager.createSymbolicLink(at: organization.appendingPathComponent("\(Self.hostId).json"), withDestinationURL: realRecord)
        #expect(identity(linkedRecord) == nil)
        #expect(identity(linkedRecord, noOrganization) == nil)
        // The organization's folder is a link.
        let linkedOrganization = try makeRoot("organization")
        let account = linkedOrganization.appendingPathComponent(Self.accountW)
        try manager.createDirectory(at: account, withIntermediateDirectories: true)
        try manager.createSymbolicLink(at: account.appendingPathComponent(Self.organizationW), withDestinationURL: realOrganization)
        #expect(identity(linkedOrganization) == nil)
        #expect(identity(linkedOrganization, noOrganization) == nil)
        // The account's folder is a link: not listed either.
        let linkedAccount = try makeRoot("account")
        try manager.createSymbolicLink(at: linkedAccount.appendingPathComponent(Self.accountW), withDestinationURL: realAccount)
        #expect(identity(linkedAccount) == nil)
        #expect(identity(linkedAccount, noOrganization) == nil)
        #expect(DesktopHostedSessions.listing(linkedAccount.appendingPathComponent(Self.accountW).path).isEmpty)
        // The root itself is a link.
        let parent = home.root.appendingPathComponent("root")
        try manager.createDirectory(at: parent, withIntermediateDirectories: true)
        let linkedRoot = parent.appendingPathComponent("claude-code-sessions")
        try manager.createSymbolicLink(at: linkedRoot, withDestinationURL: elsewhere)
        #expect(identity(linkedRoot) == nil)
        #expect(identity(linkedRoot, noOrganization) == nil)
        // A folder where the record should be: not a record either.
        let folderRecord = try makeRoot("folder")
        try manager.createDirectory(at: folderRecord.appendingPathComponent(Self.accountW).appendingPathComponent(Self.organizationW)
            .appendingPathComponent("\(Self.hostId).json"), withIntermediateDirectories: true)
        #expect(identity(folderRecord) == nil)
    }

    /// Only stats (and folder listings) under Desktop's folder, of the
    /// session's own record; a malformed id asks nothing.
    @Test func onlyTheRecordsPlaceIsLookedAt() {
        var asked: [String] = []
        let found = DesktopHostedSessions.identity(hostSessionId: Self.hostId, candidates: Self.candidates, root: root.path,
                                                   exists: { asked.append($0); return false }, list: { _ in [] })
        #expect(found == nil && asked.count == 2)
        #expect(asked.allSatisfy { $0.hasPrefix(root.path + "/") && $0.hasSuffix("/\(Self.hostId).json") })
        asked = []
        _ = DesktopHostedSessions.identity(hostSessionId: "local_../../x", candidates: Self.candidates, root: root.path,
                                           exists: { asked.append($0); return true }, list: { _ in ["x"] })
        #expect(asked.isEmpty)
        #expect(DesktopHostedSessions.root(home: "/Users/me")
                == "/Users/me/Library/Application Support/Claude/claude-code-sessions")
    }

    /// The hub's rule: a session Desktop hosts is its record's identity's,
    /// else unsure (shown under its folder's account, never recorded); any
    /// other session keeps its folder's attribution.
    @Test func theHubAttributesDesktopSessionsByTheirRecordOnly() {
        let folder = FolderAttribution.known("uuid:folder")
        #expect(DesktopHostedSessions.attribution(folder: folder, isDesktopHosted: true, desktopIdentity: "uuid:desktop")
                == .known("uuid:desktop"))
        #expect(DesktopHostedSessions.attribution(folder: folder, isDesktopHosted: true, desktopIdentity: nil)
                == .unsure(current: "uuid:folder"))
        #expect(DesktopHostedSessions.attribution(folder: folder, isDesktopHosted: false, desktopIdentity: nil) == folder)
        #expect(DesktopHostedSessions.isDesktopHosted(entrypoint: "remote_desktop"))
    }

    /// Regression (review): only real uncertainty is unsure. A session the
    /// hub simply hasn't placed yet waits, for a moment only: one Claude
    /// Desktop hosts whose registry entry hasn't been read (its state just
    /// made by a hook), or one in a folder not grouped yet.
    @Test func onlyRealUncertaintyIsUnsure() {
        typealias Hub = ClaudeControlHub
        let seen = Date()
        let soon = seen.addingTimeInterval(3), late = seen.addingTimeInterval(Hub.placementGrace + 1)
        var desktop = SessionState(sessionId: CloudFixture.sessionA, cwd: "/tmp/proj", createdAt: seen)
        desktop.entrypoint = "claude-desktop"
        let notFound = FolderAttribution.unsure(current: "uuid:folder")
        #expect(Hub.cloudPlacement(notFound, isDesktopHosted: true, state: desktop, now: soon) == .waiting)
        #expect(Hub.cloudPlacement(notFound, isDesktopHosted: true, state: desktop, now: late) == .unsure)
        // Its entry read: with an id whose record isn't found, or with none, it can't be told.
        var read = desktop
        read.registryStatus = "busy"
        #expect(Hub.cloudPlacement(notFound, isDesktopHosted: true, state: read, now: soon) == .unsure)
        read.hostSessionId = Self.hostId
        #expect(Hub.cloudPlacement(notFound, isDesktopHosted: true, state: read, now: soon) == .unsure)
        var withId = desktop
        withId.hostSessionId = Self.hostId
        #expect(Hub.cloudPlacement(notFound, isDesktopHosted: true, state: withId, now: soon) == .unsure)
        #expect(Hub.cloudPlacement(.known("uuid:desktop"), isDesktopHosted: true, state: withId, now: soon) == .certain)
        // A folder not grouped yet.
        let plain = SessionState(sessionId: CloudFixture.sessionB, cwd: "/tmp/proj", createdAt: seen)
        #expect(Hub.cloudPlacement(.known(nil), isDesktopHosted: false, state: plain, now: soon) == .waiting)
        #expect(Hub.cloudPlacement(.known(nil), isDesktopHosted: false, state: plain, now: late) == .unsure)
        // A mirrored ~/.claude around an account switch: really unsure, at once.
        #expect(Hub.cloudPlacement(.unsure(current: "uuid:a"), isDesktopHosted: false, state: plain, now: soon) == .unsure)
        #expect(Hub.cloudPlacement(.known("uuid:a"), isDesktopHosted: false, state: plain, now: late) == .certain)
    }

    /// Looked up again after a miss (Desktop may write its record a moment
    /// after the session starts), kept once found.
    @Test func lookupsAreRememberedAndRetried() throws {
        var stats = 0
        let attributor = DesktopSessionAttributor(root: root.path, exists: { stats += 1; return DesktopHostedSessions.isRecord($0) })
        let now = Date()
        #expect(attributor.identity(hostSessionId: Self.hostId, candidates: Self.candidates, now: now) == nil)
        let file = try record(account: Self.accountA, organization: Self.organizationA)
        #expect(attributor.identity(hostSessionId: Self.hostId, candidates: Self.candidates, now: now.addingTimeInterval(1)) == nil)
        let later = now.addingTimeInterval(DesktopSessionAttributor.retryAfter + 1)
        #expect(attributor.identity(hostSessionId: Self.hostId, candidates: Self.candidates, now: later) == CloudFixture.identityId)
        let asked = stats
        try FileManager.default.removeItem(at: file)
        #expect(attributor.identity(hostSessionId: Self.hostId, candidates: Self.candidates, now: later.addingTimeInterval(600))
                == CloudFixture.identityId)
        #expect(stats == asked)
        // Other known identities: asked again.
        #expect(attributor.identity(hostSessionId: Self.hostId, candidates: Array(Self.candidates.prefix(1)),
                                    now: later.addingTimeInterval(601)) == nil)
        #expect(attributor.identity(hostSessionId: nil, candidates: Self.candidates, now: later) == nil)
        attributor.retain([])
    }
}
