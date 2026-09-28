import Foundation
import Testing
@testable import ClaudeControl

/// Regressions for the fix check's open items, over the same stand-in
/// website and engine as `CloudSyncTests`: a pass and each request stay
/// with the sign-in and website they started with, a split session's parts
/// are each priced by their own responses, a pass sends at most five
/// requests, the install secret is made only when a project key is needed,
/// and turning summaries off deletes the ones never sent.
@MainActor
struct CloudSyncBindingTests {
    typealias Harness = CloudSyncTests.Harness
    typealias Gate = CloudSyncRegressionTests.Gate
    typealias L = CloudTranscriptLines

    static let website = "https://agentnotch.example.com"
    static let sessionD = "44444444-5555-4666-8777-888888888888"

    static func sessions(_ harness: Harness, _ request: URLRequest) -> [[String: Any]] {
        harness.body(request)["sessions"] as? [[String: Any]] ?? []
    }

    static func eventually(_ condition: () -> Bool) async {
        await CloudSyncRegressionTests.eventually(condition)
    }

    static var emptyRequest: CloudSyncRequest {
        CloudSyncRequest(device: .init(id: UUID().uuidString, name: "Mac", appVersion: "1"), accounts: [], sessions: [], usage: [])
    }

    // MARK: - A request's 401 retry keeps to its sign-in and website

    /// The first send goes out with website A's token; before A answers 401
    /// a sign-in made through website B replaces it (a run pointed at B with
    /// AGENTNOTCH_WEB_URL shares the session file). The retry must not carry
    /// B's token to A.
    @Test(.timeLimit(.minutes(1)))
    func aRetryNeverCarriesAnotherWebsitesToken() async throws {
        let gate = Gate()
        let transport = FakeTransport { request in
            switch (request.url?.host, request.url?.path) {
            case ("agentnotch.example.com", "/api/app/v1/sync"):
                await gate.holdFirst()
                return .init(status: 401, body: try TestPaths.contractFixture("error.json"))
            case (_, "/auth/v1/token"):
                return CloudAuthTests.tokenAnswer(access: "b-refreshed", refresh: "b-refresh-2")
            default:
                return .init(status: 204, body: Data())
            }
        }
        let auth = CloudAuth(transport: transport, store: CloudSessionMemoryStore(CloudAuthTests.session(expiresIn: 3600)))
        let api = CloudAPI(website: URL(string: Self.website)!, transport: transport, auth: auth)
        let call = Task { try await api.sync(Self.emptyRequest) }
        await Self.eventually { transport.requests(to: "/api/app/v1/sync").count == 1 }

        await auth.signOut()
        var other = CloudAuthTests.session(expiresIn: 3600, access: "b-access", refresh: "b-refresh")
        other.websiteURL = "https://other.example.com"
        await auth.adopt(other)
        gate.open()

        await #expect(throws: CloudAuthError.otherWebsite) { _ = try await call.value }
        let sentToA = transport.recorded.filter { $0.url?.host == "agentnotch.example.com" }
            .map { $0.value(forHTTPHeaderField: "Authorization") }
        #expect(sentToA == ["Bearer access-1"])
        // B's session wasn't refreshed for A's request either.
        #expect(transport.requests(to: "/auth/v1/token").isEmpty)
        #expect(await auth.currentSession()?.accessToken == "b-access")
    }

    /// The same, on one website: signed out and in again (as anyone) while
    /// the request was out. The old request's retry is not the new sign-in's.
    @Test(.timeLimit(.minutes(1)))
    func aRetryNeverCarriesALaterSignInsToken() async throws {
        let gate = Gate()
        let transport = FakeTransport { request in
            switch request.url?.path {
            case "/api/app/v1/sync":
                await gate.holdFirst()
                return .init(status: 401, body: try TestPaths.contractFixture("error.json"))
            case "/auth/v1/token":
                return CloudAuthTests.tokenAnswer(access: "new-refreshed", refresh: "new-refresh-2")
            default:
                return .init(status: 204, body: Data())
            }
        }
        let auth = CloudAuth(transport: transport, store: CloudSessionMemoryStore(CloudAuthTests.session(expiresIn: 3600)))
        let api = CloudAPI(website: URL(string: Self.website)!, transport: transport, auth: auth)
        let call = Task { try await api.sync(Self.emptyRequest) }
        await Self.eventually { transport.requests(to: "/api/app/v1/sync").count == 1 }

        await auth.signOut()
        await auth.adopt(CloudAuthTests.session(expiresIn: 3600, access: "new-access", refresh: "new-refresh"))
        gate.open()

        await #expect(throws: CloudAuthError.signedOut) { _ = try await call.value }
        #expect(transport.requests(to: "/api/app/v1/sync").map { $0.value(forHTTPHeaderField: "Authorization") }
                == ["Bearer access-1"])
        #expect(transport.requests(to: "/auth/v1/token").isEmpty)
        // A 401 for the sign-in in use is still retried once after a refresh.
        let grant = try await auth.accessGrant(for: Self.website)
        #expect(grant.token == "new-access")
        let fresh = try await auth.refreshAfterUnauthorized(failedToken: grant.token, website: grant.website,
                                                             signIn: grant.signIn)
        #expect(fresh == "new-refreshed")
    }

    // MARK: - A pass's result belongs to its sign-in

    /// Holds the pass's first request, signs out and in again meanwhile
    /// (the website user changes: `me.json`'s id, where the pass began with
    /// none), then lets the request answer with `answer`.
    func passAcrossANewSignIn(_ harness: Harness, answer: @escaping @Sendable (URLRequest) throws -> FakeTransport.Answer) async throws {
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        let gate = Gate()
        let first = FirstOnly()
        harness.transport.answer { request in
            if request.url?.path == "/api/app/v1/sync", first.take() {
                await gate.holdFirst()
                return try answer(request)
            }
            return try Harness.website(request)
        }
        let pass = Task { await harness.sync.syncNow() }
        await Self.eventually { harness.syncRequests.count == 1 }
        await harness.sync.signOut()
        let signedIn = await harness.sync.signIn { _ in URL(string: "agentnotch://auth-callback?code=again")! }
        #expect(signedIn && harness.sync.state.isSignedIn)
        gate.open()
        await pass.value
    }

    @Test(.timeLimit(.minutes(1)))
    func aPassAnsweredAfterANewSignInMarksNothingSent() async throws {
        let harness = Harness()
        await harness.start()
        try await passAcrossANewSignIn(harness) { _ in
            .init(status: 200, body: try TestPaths.contractFixture("sync-response.json"))
        }
        // The old sign-in's batch isn't remembered as the new one's.
        #expect(harness.sync.stores?.memory.sent(CloudSyncTests.keyA) == nil)
        #expect(harness.sync.stores?.memory.lastSyncAt == nil)

        // So once the user turns sync on for the new sign-in, it is sent to it.
        harness.sync.setSyncEnabled(true)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.syncNow()
        let last = try #require(harness.syncRequests.last)
        #expect(harness.syncRequests.count == 2)
        #expect(last.value(forHTTPHeaderField: "Authorization") == "Bearer access-2")
        #expect(Self.sessions(harness, last).first?["sessionId"] as? String == CloudFixture.sessionA)
    }

    /// A 401 to the old sign-in's request: no retry with the new token, and
    /// the new sign-in isn't signed out for it.
    @Test(.timeLimit(.minutes(1)))
    func aRefusalAfterANewSignInIsNotHeldAgainstIt() async throws {
        let harness = Harness()
        await harness.start()
        try await passAcrossANewSignIn(harness) { _ in
            .init(status: 401, body: try TestPaths.contractFixture("error.json"))
        }
        #expect(harness.syncRequests.count == 1)
        #expect(harness.sync.state.auth == .signedIn(email: "me@example.com"))
        #expect(harness.sync.state.lastError == nil)
        #expect(harness.sessionStore.load()?.accessToken == "access-2")
    }

    /// A server error to the old sign-in's request: no error shown and no
    /// backoff for the new sign-in.
    @Test(.timeLimit(.minutes(1)))
    func aFailureAfterANewSignInIsNotHeldAgainstIt() async throws {
        let harness = Harness()
        await harness.start()
        try await passAcrossANewSignIn(harness) { _ in
            .json(503, ["error": ["code": "INTERNAL", "message": "Down."]], headers: ["Retry-After": "600"])
        }
        #expect(harness.sync.state.lastError == nil && harness.sync.state.isSignedIn)
        harness.sync.setSyncEnabled(true)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 2)
        #expect(harness.sync.state.lastError == nil)
    }

    // MARK: - A split session: each part priced on its own

    @Test func aSessionSplitAcrossAccountsPricesEachPartOnItsOwn() async throws {
        let harness = Harness()
        let personal = CloudFixture.account, work = CloudFixture.workAccount
        harness.environment.accountList = [personal, work]
        await harness.start()
        let id = CloudFixture.sessionA
        try L.write([
            L.user("start", session: id, at: CloudFixture.stamp(0)),
            L.assistant(id: "m1", request: "r1", session: id, input: 100, output: 10, at: CloudFixture.stamp(10)),
        ], to: harness.transcript(id))
        // Run by one account so far: its cost goes.
        var mine0 = harness.observation(id, account: personal)
        mine0.lastActivityAt = CloudFixture.base.addingTimeInterval(10)
        harness.sync.observeLive([mine0], liveIDs: [id])
        await harness.sync.syncNow()
        let firstRequest = try #require(harness.syncRequests.first)
        let first = try #require(Self.sessions(harness, firstRequest).first)
        #expect(first["costUsd"] as? Double == 0.37)

        // Resumed as the work account: Claude Code restored the total so far
        // and adds to it, so its figure can't be divided. Each part is its
        // own responses at list prices instead.
        try L.write([
            L.user("go on", session: id, at: CloudFixture.stamp(1000)),
            L.assistant(id: "m2", request: "r2", session: id, input: 7, output: 3, at: CloudFixture.stamp(1010)),
            L.assistant(id: "m3", request: "r3", session: id, input: 3, output: 1, at: CloudFixture.stamp(1020)),
        ], to: harness.transcript(id), append: true)
        var resumed = harness.observation(id, account: work)
        resumed.processStartedAt = CloudFixture.base.addingTimeInterval(900)
        resumed.lastActivityAt = CloudFixture.base.addingTimeInterval(1010)
        resumed.costUsd = 0.52
        harness.sync.observeLive([resumed], liveIDs: [id])
        await harness.sync.syncNow()
        let lastRequest = try #require(harness.syncRequests.last)
        let rows = Dictionary(Self.sessions(harness, lastRequest).map {
            ($0["accountKey"] as? String ?? "", $0)
        }, uniquingKeysWith: { a, _ in a })
        let mine = try #require(rows[personal.accountKey]), theirs = try #require(rows[work.accountKey])
        // Opus 4.5: 100 in and 10 out; 10 in and 4 out.
        #expect(mine["costUsd"] as? Double == 0.00075 && theirs["costUsd"] as? Double == 0.00015)
        // Tokens stay split exactly.
        #expect(mine["tokens"] as? [String: Int] == ["input": 100, "output": 10, "cacheCreation": 0, "cacheRead": 0])
        #expect(theirs["tokens"] as? [String: Int] == ["input": 10, "output": 4, "cacheCreation": 0, "cacheRead": 0])
    }

    // MARK: - At most five requests a pass

    @Test func aPassSendsAtMostFiveRequestsAndTheRestSoonAfter() async throws {
        #expect(CloudSyncPass.maxRequestsPerPass == 5)
        let harness = Harness()
        await harness.start()
        // 3,000 readings: six requests of 500.
        for index in 0..<3_000 {
            harness.sync.record(UsageObservation(identityId: CloudFixture.identityId, source: .probe,
                                                 observedAt: CloudFixture.base.addingTimeInterval(Double(index)),
                                                 windows: [.init(id: "session", utilization: Double(index % 100) + 0.5,
                                                                 resetsAt: nil)]))
        }
        #expect(harness.sync.stores?.recorder.pendingCount == 3_000)
        await harness.sync.syncNow()
        #expect(harness.syncRequests.count == 5)
        #expect(harness.sync.state.pendingUsage == 500 && harness.sync.state.lastError == nil)
        // Not before the next pass, half a minute on.
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 5)
        harness.clock.advance(CloudSync.soonDelay + 1)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 6)
        #expect(harness.sync.state.pendingUsage == 0)
    }

    // MARK: - The install secret is made when a project key is needed

    @Test func theInstallSecretIsMadeOnlyWhenASyncNeedsAProjectKey() async throws {
        // Signed out, sync off: launched and ticking, no secret.
        let idle = Harness(signedIn: false, syncOn: false)
        await idle.start()
        await idle.sync.tick()
        let idleFile = idle.root.appendingPathComponent("support/\(CloudInstallSecret.fileName)")
        #expect(!FileManager.default.fileExists(atPath: idleFile.path))

        // Signed in with sync on, nothing to send yet: still none.
        let harness = Harness()
        let file = harness.root.appendingPathComponent("support/\(CloudInstallSecret.fileName)")
        await harness.start()
        harness.sync.record(harness.usage)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 1)
        #expect(!FileManager.default.fileExists(atPath: file.path))

        // The first session to send: made, private, and kept.
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.syncNow()
        let secret = try Data(contentsOf: file)
        #expect(secret.count == CloudInstallSecret.length)
        #expect(CloudFiles.permissions(of: file) == 0o600)
        let withSession = try #require(harness.syncRequests.last)
        let key = (Self.sessions(harness, withSession).first?["project"] as? [String: Any])?["key"] as? String
        let path = CloudKeys.projectPath(forCwd: "/Users/me/code/app", home: harness.root.path)
        #expect(key == CloudKeys.projectKey(accountKey: CloudFixture.accountKey, path: path, secret: secret))

        harness.sync.stop()
        let again = CloudSync(dependencies: { harness.dependencies() })
        again.start(environment: harness.environment, usage: nil, runsLoop: false)
        #expect(again.stores?.secret() == secret)
    }

    // MARK: - Turning summaries off deletes the ones never sent

    /// A session seen running, synced (so its totals are known), gone, and
    /// then summarised once it has been quiet for ten minutes. Not synced
    /// after that.
    func summarisedSession(_ harness: Harness, _ id: String) async throws {
        try harness.writeSession(id)
        harness.sync.observeLive([harness.observation(id)], liveIDs: [id])
        await harness.sync.syncNow()
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(61)
        await harness.sync.tick()
        harness.clock.advance(11 * 60)
        let key = CloudLedgerEntry.key(sessionId: id, accountKey: CloudFixture.accountKey)
        await harness.sync.summarizeNext()
        await Self.eventually { harness.sync.stores?.summaries.summary(for: key) != nil }
        #expect(harness.sync.stores?.summaries.summary(for: key) != nil)
    }

    @Test func turningSummariesOffDeletesOnlyTheOnesNeverSent() async throws {
        let harness = Harness(summariesOn: true)
        await harness.start()
        let keyA = CloudSyncTests.keyA
        let keyC = CloudLedgerEntry.key(sessionId: CloudFixture.sessionC, accountKey: CloudFixture.accountKey)
        let keyD = CloudLedgerEntry.key(sessionId: Self.sessionD, accountKey: CloudFixture.accountKey)

        // A's summary was sent; C's was made but never sent.
        try await summarisedSession(harness, CloudFixture.sessionA)
        await harness.sync.syncNow()
        let sentA = try #require(harness.syncRequests.last)
        #expect(Self.sessions(harness, sentA).first?["summary"] is [String: Any])
        try await summarisedSession(harness, CloudFixture.sessionC)
        #expect(harness.sync.state.summarizedSessions == 2)

        harness.sync.setSummariesEnabled(false)
        #expect(harness.sync.stores?.summaries.summary(for: keyA) != nil)
        #expect(harness.sync.stores?.summaries.summary(for: keyC) == nil)
        #expect(harness.sync.state.summarizedSessions == 1)

        // Signing out turns them off too.
        harness.sync.setSummariesEnabled(true)
        try await summarisedSession(harness, Self.sessionD)
        await harness.sync.signOut()
        #expect(harness.sync.stores?.summaries.summary(for: keyD) == nil)
        #expect(harness.sync.stores?.summaries.summary(for: keyA) != nil)
    }
}

/// True the first time only: tells a test's first request from the rest.
nonisolated final class FirstOnly: @unchecked Sendable {
    private let lock = NSLock()
    private var taken = false

    func take() -> Bool {
        lock.withLock {
            defer { taken = true }
            return !taken
        }
    }
}
