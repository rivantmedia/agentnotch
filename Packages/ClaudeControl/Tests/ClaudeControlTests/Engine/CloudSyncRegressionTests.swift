import Foundation
import Testing
@testable import ClaudeControl

/// Regressions for the cloud review's findings, over the same stand-in
/// website and engine as `CloudSyncTests`. Each test names its finding.
@MainActor
struct CloudSyncRegressionTests {
    typealias Harness = CloudSyncTests.Harness
    typealias L = CloudTranscriptLines

    static func sessions(_ harness: Harness, _ request: URLRequest) -> [[String: Any]] {
        harness.body(request)["sessions"] as? [[String: Any]] ?? []
    }

    static func allSessions(_ harness: Harness) -> [[String: Any]] {
        harness.syncRequests.flatMap { sessions(harness, $0) }
    }

    /// Waits until `condition` holds (generously: a busy CI runner is slow).
    static func eventually(_ condition: () -> Bool) async {
        let deadline = Date().addingTimeInterval(30)
        while !condition(), Date() < deadline {
            try? await Task.sleep(for: .milliseconds(20))
        }
    }

    // MARK: - Finding 0: backfill never guesses

    @Test func backfillReadsOnlyWhatBeganAfterTheFolderWasSignedInAsItsAccount() async throws {
        let harness = Harness()
        await harness.start()
        let own = harness.root.appendingPathComponent(".claude-own").path
        let slug = (own as NSString).appendingPathComponent("projects/-Users-me-work-billing")
        func write(_ id: String, at seconds: TimeInterval) throws {
            try L.write([
                L.user("prompt", session: id, at: CloudFixture.stamp(seconds), cwd: "/Users/me/work/billing"),
                L.assistant(id: "\(id)-m", request: "\(id)-r", session: id, input: 1, output: 1,
                            at: CloudFixture.stamp(seconds + 5), cwd: "/Users/me/work/billing"),
            ], to: (slug as NSString).appendingPathComponent("\(id).jsonl"))
        }
        // Someone was signed in here before: their session. Then ours.
        try write(CloudFixture.sessionA, at: 0)
        try write(CloudFixture.sessionB, at: 7200)
        harness.environment.folders = [CloudBackfill.Folder(configDir: own, identityId: CloudFixture.identityId,
                                                            accountKey: CloudFixture.accountKey, login: "login-now")]

        // The login there was never seen: nothing is backfilled.
        await harness.sync.syncNow()
        #expect(Self.allSessions(harness).isEmpty)
        #expect(harness.sync.stores?.ledger.count == 0)

        // First seen signed in as the account an hour in: only what began after.
        let relaunched = CloudSync(dependencies: { harness.dependencies() })
        harness.environment.logins = [own: "login-now"]
        relaunched.start(environment: harness.environment, usage: nil, runsLoop: false)
        await relaunched.restoreSession()
        await relaunched.tick()
        let sent = Self.allSessions(harness).compactMap { $0["sessionId"] as? String }
        #expect(sent == [CloudFixture.sessionB])
        #expect(relaunched.stores?.ledger.knows(CloudFixture.sessionA) == false)
    }

    // MARK: - Findings 4 and 25: summaries of new sessions only, within limits

    @Test func summariesSkipWhatEndedBeforeTheyWereTurnedOn() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.syncNow()
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(61)
        await harness.sync.tick()
        #expect(harness.sync.stores?.ledger.entry(CloudFixture.sessionA)?.endedAt != nil)

        // Turned on after it ended: history is never summarised.
        harness.clock.advance(60)
        harness.sync.setSummariesEnabled(true)
        harness.clock.advance(24 * 3600)
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.isEmpty)

        // A session that ends from now on is.
        try harness.writeSession(CloudFixture.sessionC)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionC)], liveIDs: [CloudFixture.sessionC])
        await harness.sync.syncNow()
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(61)
        await harness.sync.tick()
        harness.clock.advance(11 * 60)
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.map(\.sessionId) == [CloudFixture.sessionC])
    }

    @Test func anAccountNearItsFiveHourLimitGetsNoSummaries() async throws {
        let harness = Harness(summariesOn: true)
        await harness.start()
        try await CloudSyncTests().endedSession(harness)
        harness.clock.advance(11 * 60)
        harness.environment.utilization = [CloudFixture.identityId: 80]
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.isEmpty)
        harness.environment.utilization = [CloudFixture.identityId: 79.5]
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.count == 1)
    }

    // MARK: - Findings 5 and 8: a session is read again until its end is sent

    @Test func aSessionSentWhileRunningIsSentAgainWhenItEnds() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.syncNow()
        #expect(Self.allSessions(harness).first?["endedAt"] is NSNull)

        // It went away just before the lid closed; the next look is 8 hours on.
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(8 * 3600)
        await harness.sync.tick()
        await harness.sync.tick()
        let last = try #require(Self.allSessions(harness).last)
        #expect(harness.syncRequests.count == 2)
        #expect(last["endedAt"] as? String == CloudJSON.string(from: harness.clock.now.addingTimeInterval(-8 * 3600)))
        #expect(last["messageCount"] as? Int == 2)

        // Sent as ended, transcript unchanged: not read or sent again.
        await harness.sync.syncNow()
        #expect(harness.syncRequests.count == 2)
        // It grew afterwards (a late write): sent again.
        try harness.writeSession(CloudFixture.sessionA, extra: 1)
        await harness.sync.syncNow()
        #expect(harness.syncRequests.count == 3)
        #expect(Self.allSessions(harness).last?["messageCount"] as? Int == 3)
    }

    @Test func aSessionNeverSentIsReadWhateverItsAge() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        // Captured, then gone, and no pass ran before it was long over.
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(61)
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(3 * 24 * 3600)
        await harness.sync.syncNow()
        let session = try #require(Self.allSessions(harness).first)
        #expect(session["sessionId"] as? String == CloudFixture.sessionA)
        #expect(session["messageCount"] as? Int == 2)
        #expect(session["endedAt"] is String)
    }

    // MARK: - Finding 6: the key is the account's own

    @Test func liveSessionsAreKeyedAsTheirAccountIs() async throws {
        let harness = Harness()
        harness.environment.accountList = [CloudFixture.workAccount]
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        // The hub's key for the identity without its organization: re-keyed
        // from the account (its own UUID and organization).
        var observation = harness.observation(CloudFixture.sessionA, account: CloudFixture.workAccount)
        observation.accountKey = CloudKeys.accountKey(accountUuid: CloudFixture.workUuid, organizationUuid: nil)
        harness.sync.observeLive([observation], liveIDs: [CloudFixture.sessionA])
        await harness.sync.syncNow()
        let session = try #require(Self.allSessions(harness).first)
        #expect(session["accountKey"] as? String == CloudFixture.workAccountKey)
        let accounts = harness.body(try #require(harness.syncRequests.first))["accounts"] as? [[String: Any]]
        #expect(accounts?.map { $0["key"] as? String } == [CloudFixture.workAccountKey])
    }

    // MARK: - Finding 7: a session resumed under another account

    @Test func aResumedSessionIsSentOncePerAccountAndSummarisedAsIts() async throws {
        let harness = Harness()
        let personal = CloudFixture.account, work = CloudFixture.workAccount
        harness.environment.accountList = [personal, work]
        await harness.start()
        let id = CloudFixture.sessionA
        try L.write([
            L.user("START WITH PERSONAL", session: id, at: CloudFixture.stamp(0)),
            L.assistant(id: "m1", request: "r1", session: id, input: 100, output: 10, at: CloudFixture.stamp(10), text: "Personal reply."),
            L.user("SECOND ACCOUNT PROMPT", session: id, at: CloudFixture.stamp(1000)),
            L.assistant(id: "m2", request: "r2", session: id, model: "claude-sonnet-4-5", input: 7, output: 3,
                        at: CloudFixture.stamp(1010), text: "Work reply."),
            L.assistant(id: "m3", request: "r3", session: id, model: "claude-sonnet-4-5", input: 3, output: 1,
                        at: CloudFixture.stamp(1020), text: "Done."),
        ], to: harness.transcript(id))
        harness.sync.observeLive([harness.observation(id, account: personal)], liveIDs: [id])
        var resumed = harness.observation(id, account: work)
        resumed.processStartedAt = CloudFixture.base.addingTimeInterval(900)
        resumed.lastActivityAt = CloudFixture.base.addingTimeInterval(1010)
        harness.sync.observeLive([resumed], liveIDs: [id])
        await harness.sync.syncNow()

        let rows = Dictionary(Self.allSessions(harness).map { ($0["accountKey"] as? String ?? "", $0) }, uniquingKeysWith: { a, _ in a })
        #expect(rows.count == 2)
        let mine = try #require(rows[personal.accountKey]), theirs = try #require(rows[work.accountKey])
        #expect(mine["sessionId"] as? String == id && theirs["sessionId"] as? String == id)
        #expect(mine["messageCount"] as? Int == 1 && (mine["tokens"] as? [String: Int])?["input"] == 100)
        #expect(theirs["messageCount"] as? Int == 2 && (theirs["tokens"] as? [String: Int])?["input"] == 10)
        #expect(mine["endedAt"] as? String == CloudJSON.string(from: CloudFixture.base.addingTimeInterval(900)))
        #expect(theirs["endedAt"] is NSNull)
        #expect(theirs["startedAt"] as? String == CloudJSON.string(from: CloudFixture.base.addingTimeInterval(1000)))
        #expect(theirs["models"] as? [String] == ["claude-sonnet-4-5"])

        // The work part's summary runs as the work account, from its own part.
        harness.sync.setSummariesEnabled(true)
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(61)
        await harness.sync.tick()
        harness.clock.advance(11 * 60)
        await harness.sync.summarizeNext()
        let request = try #require(harness.runner.requests.first)
        #expect(harness.runner.requests.count == 1)
        #expect(request.identityId == work.identityId)
        #expect(harness.environment.folderRequests == [work.identityId])
        #expect(request.input.contains("SECOND ACCOUNT PROMPT") && request.input.contains("Work reply."))
        #expect(!request.input.contains("START WITH PERSONAL") && !request.input.contains("Personal reply."))
    }

    // MARK: - Finding 17: consent is per sign-in

    @Test func signingOutTurnsSyncOffAndStopsCapture() async throws {
        let harness = Harness(summariesOn: true)
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        await harness.sync.signOut()
        #expect(!harness.sync.state.syncEnabled && !harness.sync.state.summariesEnabled)
        #expect(!harness.defaults.store.cloudSyncEnabled && !harness.defaults.store.cloudSummariesEnabled)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.sync.record(harness.usage)
        #expect(harness.sync.stores?.ledger.count == 0 && harness.sync.stores?.recorder.pendingCount == 0)

        // Signed in again (as anyone): sync stays off until turned on.
        let ok = await harness.sync.signIn { _ in URL(string: "agentnotch://auth-callback?code=c")! }
        #expect(ok && harness.sync.state.isSignedIn && !harness.sync.state.syncEnabled)
        await harness.sync.tick()
        #expect(harness.syncRequests.isEmpty)
        harness.sync.setSyncEnabled(true)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 1)
    }

    // MARK: - Finding 20: a sign-in is bound to its website

    /// The website is the build's now, fixed for the run; what a sign-in
    /// is still bound to is the sign-in state it started in.
    @Test func aSignInFinishedAfterSigningOutIsThrownAway() async throws {
        let harness = Harness(signedIn: false)
        await harness.start()
        let ok = await harness.sync.signIn { [harness] _ in
            // While the browser is open, the app signs out (or quits).
            await harness.sync.signOut()
            return URL(string: "agentnotch://auth-callback?code=the-code")!
        }
        #expect(!ok)
        #expect(harness.sync.state.auth == .signedOut && harness.sync.state.websiteURL == "https://agentnotch.example.com")
        #expect(harness.sessionStore.load() == nil)
        // The session it made was ended on Supabase with its own token, and
        // the website was never sent it.
        let logout = try #require(harness.transport.requests(to: "/auth/v1/logout").last)
        #expect(logout.value(forHTTPHeaderField: "Authorization") == "Bearer access-2")
        #expect(harness.transport.requests(to: "/api/app/v1/me").isEmpty)
        #expect(!harness.sync.canUpload)
    }

    @Test func aWebsiteIsNeverSentAnotherWebsitesToken() async throws {
        let transport = FakeTransport { _ in FakeTransport.Answer(status: 200, body: try TestPaths.contractFixture("me.json")) }
        let auth = CloudAuth(transport: transport, store: CloudSessionMemoryStore(CloudAuthTests.session(expiresIn: 3600)))
        let other = CloudAPI(website: URL(string: "https://other.example.com")!, transport: transport, auth: auth)
        await #expect(throws: CloudAuthError.otherWebsite) { _ = try await other.me() }
        #expect(transport.recorded.isEmpty)
        let own = CloudAPI(website: URL(string: "https://agentnotch.example.com")!, transport: transport, auth: auth)
        #expect(try await own.me().user.email == "me@example.com")
    }

    // MARK: - Finding 21: switching off stops what is in flight

    /// Holds the first sync request until the test lets it go. It suspends
    /// the request's task rather than blocking a thread: a blocked thread of
    /// the cooperative pool starves every other test on a small CI runner.
    nonisolated final class Gate: @unchecked Sendable {
        private let lock = NSLock()
        private var held = false
        private var isOpen = false
        private var waiter: CheckedContinuation<Void, Never>?

        func holdFirst() async {
            let first = lock.withLock { () -> Bool in
                defer { held = true }
                return !held && !isOpen
            }
            guard first else { return }
            await withCheckedContinuation { continuation in
                let openAlready = lock.withLock { () -> Bool in
                    if isOpen { return true }
                    waiter = continuation
                    return false
                }
                if openAlready { continuation.resume() }
            }
        }

        func open() {
            let waiting = lock.withLock { () -> CheckedContinuation<Void, Never>? in
                isOpen = true
                defer { waiter = nil }
                return waiter
            }
            waiting?.resume()
        }
    }

    @Test(.timeLimit(.minutes(1)))
    func turningSyncOffStopsAPassInFlight() async throws {
        let harness = Harness()
        await harness.start()
        // 600 readings: two requests.
        for index in 0..<600 {
            harness.sync.record(UsageObservation(identityId: CloudFixture.identityId, source: .probe,
                                                 observedAt: CloudFixture.base.addingTimeInterval(Double(index)),
                                                 windows: [.init(id: "session", utilization: Double(index % 100) + 0.5,
                                                                 resetsAt: nil)]))
        }
        #expect(harness.sync.stores?.recorder.pendingCount == 600)
        let gate = Gate()
        harness.transport.answer { request in
            if request.url?.path == "/api/app/v1/sync" { await gate.holdFirst() }
            return try Harness.website(request)
        }
        let pass = Task { await harness.sync.syncNow() }
        await Self.eventually { harness.syncRequests.count == 1 }
        harness.sync.setSyncEnabled(false)
        gate.open()
        await pass.value
        #expect(harness.syncRequests.count == 1)
    }

    @Test func summariesTurnedOffDuringAPassAreLeftOut() {
        let session = CloudSyncRequest.Session(
            accountKey: CloudFixture.accountKey, sessionId: CloudFixture.sessionA, project: .init(key: CloudFixture.accountKey, name: "p"),
            title: nil, source: .cli, models: [], startedAt: CloudFixture.base, lastActivityAt: CloudFixture.base, endedAt: nil,
            messageCount: 1, tokens: .zero, costUsd: nil,
            summary: .init(text: "Did it.", model: "m", generatedAt: CloudFixture.base))
        let record = CloudSyncPass.record(for: session)
        let batch = CloudSyncPass.Batch(request: CloudSyncRequest(device: .init(id: UUID().uuidString, name: "Mac", appVersion: "1"),
                                                                  accounts: [], sessions: [session], usage: []),
                                        records: [CloudSyncTests.keyA: record], readings: [])
        #expect(record.summary != nil)
        let stripped = batch.withoutSummaries()
        #expect(stripped.request.sessions.allSatisfy { $0.summary == nil })
        #expect(stripped.records[CloudSyncTests.keyA]?.summary == nil)
        #expect(stripped.records[CloudSyncTests.keyA]?.base == record.base)
    }

    @Test(.timeLimit(.minutes(1)))
    func turningSummariesOffStopsARunningSummary() async throws {
        let harness = Harness(summariesOn: true)
        await harness.start()
        try await CloudSyncTests().endedSession(harness)
        harness.clock.advance(11 * 60)
        harness.runner.waitsForCancel = true
        await harness.sync.tick()
        let task = try #require(harness.sync.summaryTask)
        await Self.eventually { harness.runner.requests.count == 1 }
        harness.sync.setSummariesEnabled(false)
        await task.value
        #expect(harness.runner.wasCancelled)
        #expect(harness.sync.stores?.summaries.summary(for: CloudSyncTests.keyA) == nil)
        #expect(harness.sync.stores?.summaries.attempt(for: CloudSyncTests.keyA) == nil)
    }

    // MARK: - Finding 22: a paused capture never dates an end

    @Test func turningSyncBackOnNeverDatesAnEndByIt() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.syncNow()
        harness.sync.setSyncEnabled(false)
        // It ends while sync is off; two days later sync is back on.
        harness.clock.advance(2 * 24 * 3600)
        harness.sync.setSyncEnabled(true)
        await harness.sync.tick()
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(61)
        await harness.sync.tick()
        harness.clock.advance(31)
        await harness.sync.tick()
        let last = try #require(Self.allSessions(harness).last)
        // Its transcript's last line, not the day sync came back.
        #expect(last["endedAt"] as? String == CloudFixture.stamp(20))
        #expect(harness.sync.stores?.ledger.entry(CloudFixture.sessionA)?.endedAt == CloudFixture.base.addingTimeInterval(20))
    }

    // MARK: - Finding 23: nothing but "Sync now" comes before a backoff ends

    @Test func aSessionEndingDuringABackoffDoesNotBringTheRetryForward() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.transport.answer { request in
            if request.url?.path == "/api/app/v1/sync" {
                return .json(503, ["error": ["code": "INTERNAL", "message": "Down."]], headers: ["Retry-After": "600"])
            }
            return try Harness.website(request)
        }
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 1)
        // The session ends a minute later: a sync would be due in 30 s.
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(61)
        await harness.sync.tick()
        #expect(harness.sync.stores?.ledger.entry(CloudFixture.sessionA)?.endedAt != nil)
        for _ in 0..<10 {
            harness.clock.advance(50)
            await harness.sync.tick()
        }
        #expect(harness.syncRequests.count == 1)
        harness.transport.answer(Harness.website)
        harness.clock.advance(50)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 2)
    }

    // MARK: - Finding 24: originals first

    @Test func backfillReadsTheOriginalBeforeACopyOfIt() async throws {
        let harness = Harness()
        await harness.start()
        let own = harness.root.appendingPathComponent(".claude-own").path
        let slug = (own as NSString).appendingPathComponent("projects/-Users-me-code-app")
        let original = CloudFixture.sessionA, copy = CloudFixture.sessionB
        #expect(copy < original)
        try L.write([
            L.user("start", session: original, at: CloudFixture.stamp(4000)),
            L.assistant(id: "m1", request: "r1", session: original, input: 10, output: 1, at: CloudFixture.stamp(4010)),
            L.assistant(id: "m2", request: "r2", session: original, input: 20, output: 2, at: CloudFixture.stamp(4020)),
        ], to: (slug as NSString).appendingPathComponent("\(original).jsonl"))
        // An older Claude Code resumed it into a new file, copying its last
        // response as the new session's.
        try L.write([
            L.assistant(id: "m2", request: "r2", session: copy, input: 20, output: 2, at: CloudFixture.stamp(4020)),
            L.user("go on", session: copy, at: CloudFixture.stamp(5000)),
            L.assistant(id: "m3", request: "r3", session: copy, input: 5, output: 5, at: CloudFixture.stamp(5010)),
        ], to: (slug as NSString).appendingPathComponent("\(copy).jsonl"))
        harness.environment.folders = [CloudBackfill.Folder(configDir: own, identityId: CloudFixture.identityId,
                                                            accountKey: CloudFixture.accountKey, login: "login-1")]
        harness.environment.logins = [own: "login-1"]
        harness.sync.setSyncEnabled(false)
        await harness.sync.tick()
        harness.sync.setSyncEnabled(true)
        harness.clock.advance(2 * 3600)
        await harness.sync.syncNow()
        let rows = Dictionary(Self.allSessions(harness).map { ($0["sessionId"] as? String ?? "", $0) }, uniquingKeysWith: { a, _ in a })
        #expect(rows[original]?["messageCount"] as? Int == 2)
        #expect(rows[copy]?["messageCount"] as? Int == 1)
        #expect((rows[copy]?["tokens"] as? [String: Int])?["input"] == 5)
    }
}
