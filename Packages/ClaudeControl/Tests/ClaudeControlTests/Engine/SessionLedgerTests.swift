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
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(300))], liveIDs: [CloudFixture.sessionA],
                       accounts: [:], now: start.addingTimeInterval(300))
        #expect(ledger.entry(CloudFixture.sessionA)?.endedAt == nil)
        // Still running but not attributable this time: its account's part
        // stops at its last activity (M2), and goes on once it is again.
        let key = CloudLedgerEntry.key(sessionId: CloudFixture.sessionA, accountKey: CloudFixture.accountKey)
        #expect(ledger.observe(live: [], liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(320))
                == [key])
        #expect(ledger.entry(key: key)?.endedAt == start.addingTimeInterval(300))
        #expect(ledger.isUncounted(CloudFixture.sessionA))
        #expect(ledger.settle(liveIDs: [CloudFixture.sessionA], now: start.addingTimeInterval(900)).isEmpty)
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(950))], liveIDs: [CloudFixture.sessionA],
                       accounts: [:], now: start.addingTimeInterval(950))
        #expect(ledger.entry(CloudFixture.sessionA)?.endedAt == nil)
        #expect(!ledger.isUncounted(CloudFixture.sessionA))
    }

    // MARK: - Unsure (M2)

    /// Regression (M2): a session the hub can't attribute for certain any
    /// more (a mirrored `~/.claude` switching accounts) stops counting for
    /// its account from its last activity seen while it was certain; its
    /// responses from then on are no one's until it is certain again, and
    /// then its new process's account takes over from that process's start.
    @Test func aSessionTheHubCantAttributeCountsForNoOne() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        let personal = CloudFixture.account, work = CloudFixture.workAccount
        let id = CloudFixture.sessionA
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(600), account: personal)],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(600))
        // Unsure: its new responses are nobody's; its part ends where it was last certain.
        let personalKey = CloudLedgerEntry.key(sessionId: id, accountKey: personal.accountKey)
        #expect(ledger.observe(live: [], liveIDs: [id], unsure: [id], accounts: [:], now: start.addingTimeInterval(700))
                == [personalKey])
        let nobodyFrom = start.addingTimeInterval(600 + SessionLedger.uncountedAfter)
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: personal.accountKey),
                                          SessionOwner(from: nobodyFrom, accountKey: "")])
        #expect(ledger.entry(key: personalKey)?.endedAt == start.addingTimeInterval(600))
        #expect(ledger.entry(id) == nil && ledger.isUncounted(id))
        // Still unsure: nothing more changes.
        #expect(ledger.observe(live: [], liveIDs: [id], unsure: [id], accounts: [:], now: start.addingTimeInterval(800)).isEmpty)
        #expect(ledger.owners(of: id).count == 2)
        // Certain again, in a new process as the other account: from its start.
        let resumedAt = start.addingTimeInterval(1000)
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(1100), account: work,
                                               startedAt: resumedAt, processStartedAt: resumedAt)],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(1100))
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: personal.accountKey),
                                          SessionOwner(from: nobodyFrom, accountKey: ""),
                                          SessionOwner(from: resumedAt, accountKey: work.accountKey)])
        #expect(ledger.entry(id)?.accountKey == work.accountKey && ledger.entry(id)?.startedAt == resumedAt)
        #expect(!ledger.isUncounted(id))

        // An unsure session is never counted for any account, even when
        // the hub reports it among the live ones by mistake.
        let both = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        both.observe(live: [Self.observation(account: personal)], liveIDs: [id], unsure: [id], accounts: [:], now: start)
        #expect(both.count == 0 && both.isUncounted(id))
    }

    /// Regression (M2): the same process losing and regaining certainty
    /// (a moment when the registry couldn't say) loses nothing: its
    /// responses in between were its own all along.
    @Test func theSameProcessCertainAgainKeepsItsResponses() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        let id = CloudFixture.sessionA
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(100), processStartedAt: start)],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(100))
        ledger.observe(live: [], liveIDs: [id], unsure: [id], accounts: [:], now: start.addingTimeInterval(200))
        #expect(ledger.owners(of: id).count == 2)
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(300), processStartedAt: start)],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(300))
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: CloudFixture.accountKey)])
        #expect(ledger.entry(id)?.endedAt == nil && ledger.entry(id)?.lastActivityAt == start.addingTimeInterval(300))
        #expect(ledger.count == 1)
    }

    /// Regression (M2): a session first seen unsure is no one's from its
    /// start: when a certain process of it turns up, only that process's
    /// responses are its account's; the backfill never adds it either.
    @Test func aSessionFirstSeenUnsureCountsOnlyFromItsCertainProcess() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-ledger-unsure"))
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent(SessionLedger.fileName)
        let start = CloudFixture.base
        let id = CloudFixture.sessionA
        let ledger = SessionLedger(fileURL: file, persists: true, home: "/Users/me")
        ledger.observe(live: [], liveIDs: [id], unsure: [id], accounts: [:], now: start.addingTimeInterval(100))
        #expect(ledger.count == 0 && ledger.knows(id) && ledger.isUncounted(id))
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: "")])
        // Remembered across a relaunch.
        ledger.saveNow()
        let reloaded = SessionLedger(fileURL: file, persists: true, home: "/Users/me")
        #expect(reloaded.isUncounted(id))
        // The backfill leaves it alone.
        #expect(reloaded.record(backfill: [Self.backfilled(id)], accounts: [:]) == 0)
        // A certain process of it: counted from that process's start.
        let resumedAt = start.addingTimeInterval(500)
        reloaded.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(600), startedAt: resumedAt,
                                                 processStartedAt: resumedAt)],
                         liveIDs: [id], accounts: [:], now: start.addingTimeInterval(600))
        #expect(reloaded.owners(of: id) == [SessionOwner(from: nil, accountKey: ""),
                                            SessionOwner(from: resumedAt, accountKey: CloudFixture.accountKey)])
        #expect(reloaded.entry(id)?.startedAt == resumedAt && !reloaded.isUncounted(id))
        // A session nobody reported unsure, merely not attributed, isn't remembered.
        reloaded.observe(live: [], liveIDs: [CloudFixture.sessionB], accounts: [:], now: start.addingTimeInterval(700))
        #expect(!reloaded.knows(CloudFixture.sessionB))
    }

    /// Regression (double counting): a session first seen certain whose
    /// transcript is in a shared history (Claude Parallel Profiles) may
    /// have been run by another account before (a conversation from before
    /// sync, continued after an account switch): only its process's
    /// responses are its account's, as for a session first seen unsure. One
    /// in its folder's own history counts whole, and a session the ledger
    /// knows goes on as it was.
    @Test func aSessionFirstSeenInASharedHistoryCountsFromItsProcess() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        let processStart = start.addingTimeInterval(500)
        var shared = Self.observation(lastActivity: start.addingTimeInterval(600), startedAt: start.addingTimeInterval(550),
                                      processStartedAt: processStart)
        shared.inSharedHistory = true
        ledger.observe(live: [shared], liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(600))
        #expect(ledger.owners(of: CloudFixture.sessionA) == [SessionOwner(from: nil, accountKey: ""),
                                                             SessionOwner(from: processStart, accountKey: CloudFixture.accountKey)])
        #expect(ledger.entry(CloudFixture.sessionA)?.startedAt == processStart)
        #expect(!ledger.isUncounted(CloudFixture.sessionA) && ledger.count == 1)
        // Seen again (marked or not): nothing more changes.
        ledger.observe(live: [shared], liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(700))
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(800))], liveIDs: [CloudFixture.sessionA],
                       accounts: [:], now: start.addingTimeInterval(800))
        #expect(ledger.owners(of: CloudFixture.sessionA).count == 2 && ledger.count == 1)
        #expect(ledger.entry(CloudFixture.sessionA)?.startedAt == processStart)

        // Its process's start unknown: from when the app first saw it.
        var noProcess = Self.observation(CloudFixture.sessionB, lastActivity: start.addingTimeInterval(650),
                                         startedAt: start.addingTimeInterval(640))
        noProcess.inSharedHistory = true
        ledger.observe(live: [noProcess], liveIDs: [CloudFixture.sessionB], accounts: [:], now: start.addingTimeInterval(650))
        #expect(ledger.owners(of: CloudFixture.sessionB).last == SessionOwner(from: start.addingTimeInterval(640),
                                                                              accountKey: CloudFixture.accountKey))

        // A folder's own history: the whole session is its account's.
        ledger.observe(live: [Self.observation(CloudFixture.sessionC, processStartedAt: processStart)],
                       liveIDs: [CloudFixture.sessionC], accounts: [:], now: start.addingTimeInterval(700))
        #expect(ledger.owners(of: CloudFixture.sessionC) == [SessionOwner(from: nil, accountKey: CloudFixture.accountKey)])
    }

    /// Regression (double counting): when a shared history's session stops
    /// running, what is written after it is no one's until the app sees a
    /// process of it again (another account may have continued it while the
    /// app wasn't capturing). The same process back after a moment's absence
    /// loses nothing; a session in its folder's own history is left as it was.
    @Test func aSharedSessionCountsOnlyWhatTheAppSawRunning() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        let id = CloudFixture.sessionA, account = CloudFixture.accountKey
        let key = CloudLedgerEntry.key(sessionId: id, accountKey: account)
        var running = Self.observation(lastActivity: start.addingTimeInterval(100), processStartedAt: start)
        running.inSharedHistory = true
        ledger.observe(live: [running], liveIDs: [id], accounts: [:], now: start.addingTimeInterval(100))
        // Gone: ended a minute later, at when it went.
        ledger.settle(liveIDs: [], now: start.addingTimeInterval(110))
        #expect(ledger.settle(liveIDs: [], now: start.addingTimeInterval(171)) == [key])
        let gone = start.addingTimeInterval(110 + SessionLedger.uncountedAfter)
        #expect(ledger.entry(key: key)?.endedAt == start.addingTimeInterval(110))
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: ""), SessionOwner(from: start, accountKey: account),
                                          SessionOwner(from: gone, accountKey: "")])
        // Whatever an unseen process wrote meanwhile is no one's.
        #expect(SessionOwners.owner(at: start.addingTimeInterval(500), in: ledger.owners(of: id)) == "")
        // A new process of it: counted from its start, in the same entry.
        let resumedAt = start.addingTimeInterval(900)
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(950), startedAt: resumedAt,
                                               processStartedAt: resumedAt)],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(950))
        #expect(ledger.owners(of: id).suffix(2) == [SessionOwner(from: gone, accountKey: ""),
                                                    SessionOwner(from: resumedAt, accountKey: account)])
        #expect(ledger.entry(key: key)?.endedAt == nil && ledger.count == 1)

        // Missing for a minute, then the same process again: nothing is lost.
        let flicker = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        flicker.observe(live: [running], liveIDs: [id], accounts: [:], now: start.addingTimeInterval(100))
        flicker.settle(liveIDs: [], now: start.addingTimeInterval(110))
        flicker.settle(liveIDs: [], now: start.addingTimeInterval(171))
        flicker.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(200), processStartedAt: start)],
                        liveIDs: [id], accounts: [:], now: start.addingTimeInterval(200))
        #expect(flicker.owners(of: id) == [SessionOwner(from: nil, accountKey: ""), SessionOwner(from: start, accountKey: account)])
        #expect(flicker.entry(key: key)?.endedAt == nil)

        // Its folder's own history: its end changes no owner.
        let own = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        var ownRunning = running
        ownRunning.inSharedHistory = false
        own.observe(live: [ownRunning], liveIDs: [id], accounts: [:], now: start.addingTimeInterval(100))
        own.settle(liveIDs: [], now: start.addingTimeInterval(110))
        #expect(own.settle(liveIDs: [], now: start.addingTimeInterval(171)) == [key])
        #expect(own.owners(of: id) == [SessionOwner(from: nil, accountKey: account)])
    }

    /// A shared session resumed again and again by one account (each end a
    /// stretch of nobody, each resume its account back) can still change
    /// hands: only hand-overs between accounts count toward the bound.
    @Test func resumingASharedSessionOftenNeverBlocksAnotherAccount() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let id = CloudFixture.sessionA
        let personal = CloudFixture.account, work = CloudFixture.workAccount
        var now = CloudFixture.base
        for round in 0..<(SessionLedger.maxOwners + 4) {
            now = now.addingTimeInterval(100)
            var running = Self.observation(lastActivity: now, account: personal, startedAt: now, processStartedAt: now)
            if round == 0 { running.inSharedHistory = true }
            ledger.observe(live: [running], liveIDs: [id], accounts: [:], now: now)
            ledger.settle(liveIDs: [], now: now.addingTimeInterval(1))
            now = now.addingTimeInterval(62)
            ledger.settle(liveIDs: [], now: now)
        }
        #expect(ledger.owners(of: id).filter(\.accountKey.isEmpty).count == SessionLedger.maxOwners)
        now = now.addingTimeInterval(100)
        ledger.observe(live: [Self.observation(lastActivity: now, account: work, startedAt: now, processStartedAt: now)],
                       liveIDs: [id], accounts: [:], now: now)
        #expect(ledger.owners(of: id).last == SessionOwner(from: now, accountKey: work.accountKey))
        #expect(ledger.entry(sessionId: id, accountKey: work.accountKey) != nil)
    }

    /// Whether a session's history is shared is told once and kept with its
    /// entries: a part another account takes over inherits it, it survives a
    /// relaunch, and a session from before the app looked learns it later.
    @Test func whetherAHistoryIsSharedIsKeptWithTheSession() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-ledger-shared"))
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent(SessionLedger.fileName)
        let start = CloudFixture.base
        let id = CloudFixture.sessionA
        let personal = CloudFixture.account, work = CloudFixture.workAccount
        let ledger = SessionLedger(fileURL: file, persists: true, home: "/Users/me")
        var shared = Self.observation(lastActivity: start.addingTimeInterval(100), account: personal, processStartedAt: start)
        shared.inSharedHistory = true
        ledger.observe(live: [shared], liveIDs: [id], accounts: [:], now: start.addingTimeInterval(100))
        #expect(ledger.sharedHistory(of: id) == true)
        // Taken over by another account (not looked up again): the same history.
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(300), account: work,
                                               processStartedAt: start.addingTimeInterval(250))],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(300))
        #expect(ledger.entry(sessionId: id, accountKey: work.accountKey)?.sharedHistory == true)
        ledger.saveNow()
        let reloaded = SessionLedger(fileURL: file, persists: true, home: "/Users/me")
        #expect(reloaded.sharedHistory(of: id) == true)
        #expect(reloaded.owners(of: id).map(\.accountKey) == ["", personal.accountKey, work.accountKey])

        // Not looked up (an entry from before): unknown until an observation says.
        reloaded.observe(live: [Self.observation(CloudFixture.sessionB)], liveIDs: [CloudFixture.sessionB], accounts: [:],
                         now: start.addingTimeInterval(400))
        #expect(reloaded.sharedHistory(of: CloudFixture.sessionB) == nil)
        var told = Self.observation(CloudFixture.sessionB)
        told.inSharedHistory = false
        reloaded.observe(live: [told], liveIDs: [CloudFixture.sessionB], accounts: [:], now: start.addingTimeInterval(410))
        #expect(reloaded.sharedHistory(of: CloudFixture.sessionB) == false)
        #expect(reloaded.owners(of: CloudFixture.sessionB) == [SessionOwner(from: nil, accountKey: personal.accountKey)])
    }

    /// A shared history's session the hub placed late, lost for a while and
    /// then handed to another account: each stretch where it should be.
    @Test func aSharedSessionPlacedLateThenUnsureThenHandedOver() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        let id = CloudFixture.sessionA
        let personal = CloudFixture.account, work = CloudFixture.workAccount
        // Not placed yet: nothing is remembered; placed: counted from its process's start.
        ledger.observe(live: [], liveIDs: [id], waiting: [id], accounts: [:], now: start.addingTimeInterval(5))
        #expect(!ledger.knows(id))
        var placed = Self.observation(lastActivity: start.addingTimeInterval(100), account: personal, processStartedAt: start)
        placed.inSharedHistory = true
        ledger.observe(live: [placed], liveIDs: [id], accounts: [:], now: start.addingTimeInterval(100))
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: ""), SessionOwner(from: start, accountKey: personal.accountKey)])
        // Unsure for a while, then certain again in the same process: nothing lost.
        ledger.observe(live: [], liveIDs: [id], unsure: [id], accounts: [:], now: start.addingTimeInterval(200))
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(300), account: personal, processStartedAt: start)],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(300))
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: ""), SessionOwner(from: start, accountKey: personal.accountKey)])
        // Another account's new process takes over from its start.
        let resumedAt = start.addingTimeInterval(450)
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(500), account: work,
                                               startedAt: resumedAt, processStartedAt: resumedAt)],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(500))
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: ""), SessionOwner(from: start, accountKey: personal.accountKey),
                                          SessionOwner(from: resumedAt, accountKey: work.accountKey)])
    }

    /// Regression (double counting): a session that changes hands while its
    /// process runs (an account's key changed, a window switched) leaves the
    /// old part's last response with it. The old part may never be sent
    /// again, so the new part counting it too would count it twice.
    @Test func aHandOverLeavesTheOldPartsLastResponseWithIt() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        let personal = CloudFixture.account, work = CloudFixture.workAccount
        let lastResponse = start.addingTimeInterval(600)
        ledger.observe(live: [Self.observation(lastActivity: lastResponse, account: personal, processStartedAt: start)],
                       liveIDs: [CloudFixture.sessionA], accounts: [:], now: lastResponse)
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(700), account: work,
                                               processStartedAt: start)],
                       liveIDs: [CloudFixture.sessionA], accounts: [:], now: start.addingTimeInterval(700))
        let boundary = lastResponse.addingTimeInterval(SessionLedger.uncountedAfter)
        let owners = ledger.owners(of: CloudFixture.sessionA)
        #expect(owners == [SessionOwner(from: nil, accountKey: personal.accountKey),
                           SessionOwner(from: boundary, accountKey: work.accountKey)])
        #expect(SessionOwners.owner(at: lastResponse, in: owners) == personal.accountKey)
        #expect(ledger.entry(sessionId: CloudFixture.sessionA, accountKey: personal.accountKey)?.endedAt == boundary)
        #expect(ledger.entry(sessionId: CloudFixture.sessionA, accountKey: work.accountKey)?.startedAt == boundary)
    }

    /// Regression (review): a session the hub hasn't placed yet (its state
    /// just made again by a hook, a folder not grouped yet) waits: a known
    /// one isn't paused (no stretch of nobody, its part not ended), a new
    /// one isn't remembered as no one's, and once placed nothing was split.
    @Test func aSessionNotPlacedYetWaits() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let start = CloudFixture.base
        let id = CloudFixture.sessionA
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(100), processStartedAt: start)],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(100))
        #expect(ledger.observe(live: [], liveIDs: [id], waiting: [id], accounts: [:], now: start.addingTimeInterval(110)).isEmpty)
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: CloudFixture.accountKey)])
        #expect(!ledger.isUncounted(id) && ledger.entry(id)?.endedAt == nil)
        // Waiting longer than a missing session's grace: still running.
        #expect(ledger.observe(live: [], liveIDs: [], waiting: [id], accounts: [:], now: start.addingTimeInterval(300)).isEmpty)
        #expect(ledger.settle(liveIDs: [id], now: start.addingTimeInterval(400)).isEmpty)
        #expect(ledger.entry(id)?.endedAt == nil)
        // Placed: a new process of the same account. One owner, one part.
        ledger.observe(live: [Self.observation(lastActivity: start.addingTimeInterval(500),
                                               processStartedAt: start.addingTimeInterval(450))],
                       liveIDs: [id], accounts: [:], now: start.addingTimeInterval(500))
        #expect(ledger.owners(of: id) == [SessionOwner(from: nil, accountKey: CloudFixture.accountKey)])
        #expect(ledger.count == 1 && ledger.entry(id)?.lastActivityAt == start.addingTimeInterval(500))
        // A new session not placed yet isn't remembered at all.
        ledger.observe(live: [], liveIDs: [CloudFixture.sessionB], waiting: [CloudFixture.sessionB], accounts: [:],
                       now: start.addingTimeInterval(600))
        #expect(!ledger.knows(CloudFixture.sessionB))
        // Unsure wins over waiting.
        ledger.observe(live: [], liveIDs: [id], unsure: [id], waiting: [id], accounts: [:], now: start.addingTimeInterval(700))
        #expect(ledger.isUncounted(id))
    }

    /// Regression (review): a session that keeps losing its account (a new
    /// process each time) is counted again whenever the hub is certain: the
    /// cap on hand-overs never blocks one away from nobody. Stretches of
    /// nobody are bounded on their own; past that, the session stays with
    /// the account it ran as, as a capped session always did.
    @Test func aSessionIsCountedAgainHoweverOftenItWasUnsure() throws {
        let ledger = SessionLedger(fileURL: nil, persists: false, home: "/Users/me")
        let id = CloudFixture.sessionA
        var now = CloudFixture.base
        func certain() {
            now = now.addingTimeInterval(100)
            ledger.observe(live: [Self.observation(lastActivity: now, startedAt: now, processStartedAt: now)],
                           liveIDs: [id], accounts: [:], now: now)
        }
        func unsure() {
            now = now.addingTimeInterval(100)
            ledger.observe(live: [], liveIDs: [id], unsure: [id], accounts: [:], now: now)
        }
        func nobodyStretches() -> Int { ledger.owners(of: id).filter { $0.accountKey.isEmpty }.count }
        certain()
        for cycle in 1...(SessionLedger.maxOwners / 2) {
            unsure()
            #expect(ledger.isUncounted(id), "cycle \(cycle)")
            certain()
            #expect(!ledger.isUncounted(id), "cycle \(cycle)")
            #expect(ledger.entry(id)?.endedAt == nil && ledger.entry(id)?.lastActivityAt == now, "cycle \(cycle)")
        }
        #expect(ledger.owners(of: id).count == 1 + SessionLedger.maxOwners)
        // Bounded: at most `maxOwners` stretches of nobody, and never stuck in one.
        for _ in 0..<(SessionLedger.maxOwners / 2 + 8) {
            unsure()
            certain()
        }
        #expect(nobodyStretches() == SessionLedger.maxOwners)
        #expect(ledger.owners(of: id).count == 1 + 2 * SessionLedger.maxOwners)
        #expect(!ledger.isUncounted(id) && ledger.entry(id)?.endedAt == nil)
        unsure()
        #expect(!ledger.isUncounted(id) && nobodyStretches() == SessionLedger.maxOwners)
    }

    /// A ledger written before `unattributed` existed still loads.
    @Test func aLedgerFromBeforeUnsureSessionsLoads() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-ledger-old"))
        defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent(SessionLedger.fileName)
        let ledger = SessionLedger(fileURL: file, persists: true, home: "/Users/me")
        ledger.observe(live: [Self.observation()], liveIDs: [CloudFixture.sessionA], accounts: [:], now: CloudFixture.base)
        ledger.saveNow()
        var object = try #require(try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        object.removeValue(forKey: "unattributed")
        try JSONSerialization.data(withJSONObject: object).write(to: file)
        #expect(SessionLedger(fileURL: file, persists: true, home: "/Users/me").entry(CloudFixture.sessionA) != nil)
    }

    @Test func ownersWithoutEmptyOrRepeatedStretches() {
        let t1 = CloudFixture.base, t2 = CloudFixture.base.addingTimeInterval(60)
        let a = "a", b = "b"
        #expect(SessionOwners.normalized([.init(from: nil, accountKey: a), .init(from: t1, accountKey: ""),
                                          .init(from: t1, accountKey: a)]) == [.init(from: nil, accountKey: a)])
        #expect(SessionOwners.normalized([.init(from: nil, accountKey: a), .init(from: t1, accountKey: ""),
                                          .init(from: t2, accountKey: b)])
                == [.init(from: nil, accountKey: a), .init(from: t1, accountKey: ""), .init(from: t2, accountKey: b)])
        #expect(SessionOwners.normalized([.init(from: nil, accountKey: a), .init(from: t1, accountKey: a)])
                == [.init(from: nil, accountKey: a)])
    }

    static func backfilled(_ id: String) -> CloudLedgerEntry {
        CloudLedgerEntry(sessionId: id, identityId: CloudFixture.identityId, accountKey: CloudFixture.accountKey,
                         projectName: "app", projectPath: "/Users/me/code/app", transcriptPath: nil, configDir: nil,
                         source: .cli, startedAt: CloudFixture.base, lastActivityAt: CloudFixture.base, endedAt: nil,
                         model: nil, costUsd: nil, title: nil, origin: .backfill)
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

    /// A running session's history is shared when it is reached through a
    /// link, or another known folder reaches it; then its lines from before
    /// its process may be another account's (see the ledger's notes).
    @Test func aSharedHistoryIsOneReachedThroughALinkOrByAnotherFolder() throws {
        let root = TestPaths.temporaryRoot("cloud-shared-history")
        defer { try? FileManager.default.removeItem(atPath: root) }
        let fm = FileManager.default
        func folder(_ name: String) -> String { (root as NSString).appendingPathComponent(name) }
        let slug = "-Users-me-code-app"
        // Claude Parallel Profiles: a window folder's projects/ links to the shared store.
        try fm.createDirectory(atPath: folder(".claude-shared/projects/\(slug)"), withIntermediateDirectories: true)
        try fm.createDirectory(atPath: folder(".claude-windows/a1b2c3d4e5f6"), withIntermediateDirectories: true)
        try fm.createSymbolicLink(atPath: folder(".claude-windows/a1b2c3d4e5f6/projects"),
                                  withDestinationPath: folder(".claude-shared/projects"))
        // A folder with a history of its own, and one whose project folder alone is a link.
        try fm.createDirectory(atPath: folder(".claude-own/projects/\(slug)"), withIntermediateDirectories: true)
        try fm.createDirectory(atPath: folder(".claude-per-repo/projects"), withIntermediateDirectories: true)
        try fm.createSymbolicLink(atPath: folder(".claude-per-repo/projects/\(slug)"),
                                  withDestinationPath: folder(".claude-shared/projects/\(slug)"))
        let id = CloudFixture.sessionA
        func transcript(_ configDir: String) -> String { folder("\(configDir)/projects/\(slug)/\(id).jsonl") }
        let known = [".claude-windows/a1b2c3d4e5f6", ".claude-own", ".claude-per-repo"]
            .map { CloudBackfill.Folder(configDir: folder($0), identityId: nil, accountKey: nil) }

        // Through the link, with or without a transcript yet.
        #expect(CloudBackfill.isShared(transcriptPath: transcript(".claude-windows/a1b2c3d4e5f6"),
                                       configDir: folder(".claude-windows/a1b2c3d4e5f6"), folders: known))
        #expect(CloudBackfill.isShared(transcriptPath: nil, configDir: folder(".claude-windows/a1b2c3d4e5f6"), folders: known))
        #expect(CloudBackfill.isShared(transcriptPath: transcript(".claude-per-repo"), configDir: folder(".claude-per-repo"),
                                       folders: known))
        // Its own: the folder itself among the known ones doesn't count.
        #expect(!CloudBackfill.isShared(transcriptPath: transcript(".claude-own"), configDir: folder(".claude-own"), folders: known))
        #expect(!CloudBackfill.isShared(transcriptPath: nil, configDir: folder(".claude-own") + "/", folders: []))
        // Nor do other spellings of the folder make it shared: a config
        // folder reached through a link (kept with dotfiles), another letter
        // case, or /private before /var.
        try fm.createDirectory(atPath: folder("dotfiles/claude/projects/\(slug)"), withIntermediateDirectories: true)
        try fm.createSymbolicLink(atPath: folder(".claude-dotfiles"), withDestinationPath: folder("dotfiles/claude"))
        #expect(!CloudBackfill.isShared(transcriptPath: transcript(".claude-dotfiles"), configDir: folder(".claude-dotfiles"),
                                        folders: known))
        let alias = CloudBackfill.Folder(configDir: folder("dotfiles/claude"), identityId: nil, accountKey: nil)
        #expect(!CloudBackfill.isShared(transcriptPath: transcript(".claude-dotfiles"), configDir: folder(".claude-dotfiles"),
                                        folders: known + [alias]))
        let ownUpper = folder(".CLAUDE-OWN")
        if fm.fileExists(atPath: ownUpper) {
            #expect(!CloudBackfill.isShared(transcriptPath: nil, configDir: ownUpper, folders: known))
        }
        if root.hasPrefix("/var/") {
            #expect(!CloudBackfill.isShared(transcriptPath: "/private" + transcript(".claude-own"),
                                            configDir: "/private" + folder(".claude-own"), folders: known))
        }

        // Until another known folder links to it.
        try fm.createDirectory(atPath: folder(".claude-adopted"), withIntermediateDirectories: true)
        try fm.createSymbolicLink(atPath: folder(".claude-adopted/projects"), withDestinationPath: folder(".claude-own/projects"))
        let adopted = known + [CloudBackfill.Folder(configDir: folder(".claude-adopted"), identityId: nil, accountKey: nil)]
        #expect(CloudBackfill.isShared(transcriptPath: transcript(".claude-own"), configDir: folder(".claude-own"), folders: adopted))
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
