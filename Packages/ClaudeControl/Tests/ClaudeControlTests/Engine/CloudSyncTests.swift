import Foundation
import Testing
@testable import ClaudeControl

/// The sync service end to end, over a stand-in website and engine: what
/// may be sent, when, and what is never sent.
@MainActor
struct CloudSyncTests {
    // MARK: - Harness

    nonisolated final class Clock: @unchecked Sendable {
        private let lock = NSLock()
        private var value: Date
        init(_ value: Date) { self.value = value }
        var now: Date {
            get { lock.withLock { value } }
            set { lock.withLock { value = newValue } }
        }
        func advance(_ seconds: TimeInterval) { now = now.addingTimeInterval(seconds) }
    }

    nonisolated final class Switches: @unchecked Sendable {
        private let lock = NSLock()
        private var summaries = true
        private var listing: Set<String>?
        var summariesAllowed: Bool {
            get { lock.withLock { summaries } }
            set { lock.withLock { summaries = newValue } }
        }
        var desktop: Set<String>? {
            get { lock.withLock { listing } }
            set { lock.withLock { listing = newValue } }
        }
    }

    /// Stands in for `claude -p` summaries.
    nonisolated final class Runner: @unchecked Sendable {
        private let lock = NSLock()
        private var list: [SessionSummarizer.Request] = []
        private var answer: SessionSummarizer.Outcome = .summary(.init(text: "Fixed the token refresh.", model: "claude-haiku-4-5-20251001", costUsd: 0.001))
        private var hangs = false
        private var stopped = false
        var requests: [SessionSummarizer.Request] { lock.withLock { list } }
        /// Runs until its task is cancelled (a `claude` that takes its time).
        var waitsForCancel: Bool {
            get { lock.withLock { hangs } }
            set { lock.withLock { hangs = newValue } }
        }
        /// A waiting run saw its task cancelled.
        var wasCancelled: Bool { lock.withLock { stopped } }
        func answer(_ outcome: SessionSummarizer.Outcome) { lock.withLock { answer = outcome } }
        func run(_ request: SessionSummarizer.Request) -> SessionSummarizer.Outcome {
            lock.withLock {
                list.append(request)
                return answer
            }
        }
        func runAsync(_ request: SessionSummarizer.Request) async -> SessionSummarizer.Outcome {
            let outcome = run(request)
            guard waitsForCancel else { return outcome }
            while !Task.isCancelled { try? await Task.sleep(for: .milliseconds(20)) }
            lock.withLock { stopped = true }
            return .failed(SessionSummarizer.cancelledReason)
        }
    }

    final class Harness {
        let root: URL
        let defaults = TestDefaults()
        /// For the deinit, which can't reach `defaults`.
        nonisolated let suiteName: String
        let transport: FakeTransport
        let sessionStore: CloudSessionMemoryStore
        let environment = FakeCloudEnvironment(accounts: [CloudFixture.account])
        let clock = Clock(CloudFixture.base.addingTimeInterval(3600))
        let switches = Switches()
        let runner = Runner()
        let sealed: Bool
        private(set) var sync: CloudSync!

        init(signedIn: Bool = true, syncOn: Bool = true, summariesOn: Bool = false, sealed: Bool = false,
             website: String? = "https://agentnotch.example.com") {
            root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-sync"))
            suiteName = defaults.name
            self.sealed = sealed
            sessionStore = CloudSessionMemoryStore(signedIn ? CloudAuthTests.session(expiresIn: 7 * 24 * 3600) : nil)
            transport = FakeTransport(Self.website)
            let store = defaults.store
            store.cloudWebsiteURL = website
            store.cloudSyncEnabled = syncOn
            store.cloudSummariesEnabled = summariesOn
            sync = CloudSync(dependencies: { [self] in self.dependencies() })
        }

        deinit {
            UserDefaults(suiteName: suiteName)?.removePersistentDomain(forName: suiteName)
            try? FileManager.default.removeItem(at: root)
        }

        func dependencies() -> CloudSync.Dependencies {
            CloudSync.Dependencies(
                directory: sealed ? nil : root.appendingPathComponent("support", isDirectory: true),
                persists: !sealed, sealed: sealed, settings: defaults.store,
                transport: transport, sessionStore: sessionStore,
                summaryRunner: { [runner] request in await runner.runAsync(request) },
                clock: { [clock] in clock.now }, home: root.path, appVersion: "9.9", deviceName: "Test Mac",
                websiteOverride: nil,
                summariesAllowed: { [switches] in switches.summariesAllowed },
                desktopAccounts: { [switches] in switches.desktop })
        }

        func start() async {
            sync.start(environment: environment, usage: nil, runsLoop: false)
            await sync.restoreSession()
        }

        /// The website: config, Supabase's token and logout, me, sync.
        nonisolated static let website: @Sendable (URLRequest) throws -> FakeTransport.Answer = { request in
            switch request.url?.path {
            case "/api/app/v1/config":
                return .init(status: 200, body: try TestPaths.contractFixture("config.json"))
            case "/auth/v1/token":
                return CloudAuthTests.tokenAnswer(access: "access-2", refresh: "refresh-2")
            case "/auth/v1/logout":
                return .init(status: 204, body: Data())
            case "/api/app/v1/me":
                return .init(status: 200, body: try TestPaths.contractFixture("me.json"))
            case "/api/app/v1/sync":
                return .init(status: 200, body: try TestPaths.contractFixture("sync-response.json"))
            default:
                return .json(404, ["error": ["code": "BAD_REQUEST", "message": "no such route"]])
            }
        }

        var projects: String { root.appendingPathComponent(".claude/projects/-Users-me-code-app").path }

        func transcript(_ id: String) -> String { (projects as NSString).appendingPathComponent("\(id).jsonl") }

        /// A session with a prompt, a tool call and its output, and two responses.
        func writeSession(_ id: String, extra: Int = 0) throws {
            var lines = [
                CloudTranscriptLines.user("MY SECRET PROMPT about the login bug", session: id, at: CloudFixture.stamp(0)),
                CloudTranscriptLines.assistant(id: "\(id)-m1", request: "r1", session: id, input: 100, output: 50,
                                          cacheCreation: 1000, cacheRead: 5000, at: CloudFixture.stamp(10),
                                          text: "Looking into it.", toolUse: true),
                CloudTranscriptLines.toolResult("TOOL OUTPUT WITH A PATH /Users/me/secret", session: id, at: CloudFixture.stamp(11)),
                CloudTranscriptLines.assistant(id: "\(id)-m2", request: "r2", session: id, model: "claude-haiku-4-5", input: 10,
                                          output: 5, at: CloudFixture.stamp(20), text: "Fixed it."),
            ]
            for index in 0..<extra {
                lines.append(CloudTranscriptLines.assistant(id: "\(id)-x\(index)", request: "rx\(index)", session: id, input: 1,
                                                       output: 1, at: CloudFixture.stamp(30 + Double(index))))
            }
            try CloudTranscriptLines.write(lines, to: transcript(id))
        }

        func observation(_ id: String, entrypoint: String = "cli", account: CloudAccountInfo = CloudFixture.account) -> LiveSessionObservation {
            LiveSessionObservation(sessionId: id, identityId: account.identityId, accountKey: account.accountKey,
                                   cwd: "/Users/me/code/app", transcriptPath: transcript(id),
                                   configDir: root.appendingPathComponent(".claude").path, entrypoint: entrypoint,
                                   startedAt: CloudFixture.base, lastActivityAt: CloudFixture.base.addingTimeInterval(20),
                                   model: "claude-opus-4-5", costUsd: 0.37, title: "Fix the login bug")
        }

        var usage: UsageObservation {
            UsageObservation(identityId: CloudFixture.identityId, source: .desktop,
                             observedAt: CloudFixture.base.addingTimeInterval(600),
                             windows: [.init(id: "session", utilization: 42, resetsAt: CloudFixture.base.addingTimeInterval(9000))])
        }

        var syncRequests: [URLRequest] { transport.requests(to: "/api/app/v1/sync") }

        func body(_ request: URLRequest) -> [String: Any] { FakeTransport.body(request) ?? [:] }
    }

    // MARK: - Consent

    @Test func nothingIsCapturedOrSentWhileSyncIsOff() async throws {
        let harness = Harness(syncOn: false)
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.sync.record(harness.usage)
        await harness.sync.tick()
        await harness.sync.syncNow()
        #expect(harness.transport.recorded.isEmpty)
        #expect(harness.sync.stores?.ledger.count == 0)
        #expect(harness.sync.stores?.recorder.pendingCount == 0)
        #expect(harness.sync.state.isSignedIn && !harness.sync.state.syncEnabled)
    }

    @Test func nothingIsSentWhileSignedOut() async throws {
        let harness = Harness(signedIn: false)
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.sync.record(harness.usage)
        await harness.sync.tick()
        await harness.sync.syncNow()
        #expect(harness.transport.recorded.isEmpty)
        #expect(harness.sync.state.auth == .signedOut)
        #expect(!harness.sync.canUpload)
        // Nor captured (review finding 17): the switch was agreed to for a sign-in.
        #expect(harness.sync.stores?.ledger.count == 0)
        #expect(harness.sync.stores?.recorder.pendingCount == 0)
    }

    @Test func noWebsiteNoSync() async throws {
        let harness = Harness(website: nil)
        await harness.start()
        await harness.sync.tick()
        #expect(harness.transport.recorded.isEmpty)
        // A session made through another website isn't used, and isn't
        // deleted either (a dev run pointed elsewhere shares the folder).
        #expect(harness.sync.state.auth == .signedOut)
        #expect(harness.sessionStore.load() != nil)
    }

    // MARK: - What is sent

    @Test func aSyncSendsNamesAndNumbersOnly() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.sync.record(harness.usage)
        await harness.sync.tick()

        let request = try #require(harness.syncRequests.first)
        #expect(harness.syncRequests.count == 1)
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer access-1")
        let text = String(decoding: request.httpBody ?? Data(), as: UTF8.self)
        let secret = try #require(harness.sync.stores?.secret())
        let secretHex = secret.map { String(format: "%02x", $0) }.joined()
        for leaked in ["MY SECRET PROMPT", "TOOL OUTPUT", "/Users/me", harness.root.path, "Looking into it", "Fixed it.",
                       CloudFixture.accountUuid, ".jsonl", secretHex] {
            #expect(!text.contains(leaked), "\(leaked) was sent")
        }
        // The install secret is kept beside the rest, private (findings 18 and 2).
        let secretFile = harness.root.appendingPathComponent("support/\(CloudInstallSecret.fileName)")
        #expect(try Data(contentsOf: secretFile) == secret)
        #expect(CloudFiles.permissions(of: secretFile) == 0o600)

        let body = harness.body(request)
        #expect(body["schemaVersion"] as? Int == 1)
        let device = try #require(body["device"] as? [String: Any])
        #expect(CloudKeys.isUUID(device["id"] as? String ?? "") && device["name"] as? String == "Test Mac")
        let accounts = try #require(body["accounts"] as? [[String: Any]])
        #expect(accounts.map { $0["key"] as? String } == [CloudFixture.accountKey])
        #expect(accounts.first?["email"] as? String == "me@example.com" && accounts.first?["label"] as? String == "Personal")
        let session = try #require((body["sessions"] as? [[String: Any]])?.first)
        #expect(session["sessionId"] as? String == CloudFixture.sessionA)
        #expect((session["project"] as? [String: Any])?["name"] as? String == "app")
        let path = CloudKeys.projectPath(forCwd: "/Users/me/code/app", home: harness.root.path)
        #expect((session["project"] as? [String: Any])?["key"] as? String
                == CloudKeys.projectKey(accountKey: CloudFixture.accountKey, path: path, secret: secret))
        #expect((session["project"] as? [String: Any])?["key"] as? String != CloudKeys.sha256Hex(CloudFixture.accountKey + ":" + path))
        #expect(session["title"] as? String == "Fix the login bug")
        #expect(session["source"] as? String == "cli")
        #expect(session["models"] as? [String] == ["claude-haiku-4-5", "claude-opus-4-5"])
        #expect(session["messageCount"] as? Int == 2)
        #expect(session["tokens"] as? [String: Int] == ["input": 110, "output": 55, "cacheCreation": 1000, "cacheRead": 5000])
        #expect(session["costUsd"] as? Double == 0.37)
        #expect(session["endedAt"] is NSNull)
        #expect(session["summary"] == nil)
        #expect(session["startedAt"] as? String == CloudJSON.string(from: CloudFixture.base))
        let usage = try #require((body["usage"] as? [[String: Any]])?.first)
        #expect(usage["source"] as? String == "desktop" && usage["accountKey"] as? String == CloudFixture.accountKey)

        #expect(harness.sync.state.lastSyncAt != nil && harness.sync.state.lastError == nil)
        #expect(harness.sync.state.pendingUsage == 0 && harness.sync.state.pendingSessions == 0)
    }

    @Test func anUnchangedSessionIsNotSentAgain() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.syncNow()
        #expect(harness.syncRequests.count == 1)
        await harness.sync.syncNow()
        #expect(harness.syncRequests.count == 1)

        // It grew: sent again, with its new totals.
        try harness.writeSession(CloudFixture.sessionA, extra: 3)
        await harness.sync.syncNow()
        #expect(harness.syncRequests.count == 2)
        let session = try #require((harness.body(harness.syncRequests[1])["sessions"] as? [[String: Any]])?.first)
        #expect(session["messageCount"] as? Int == 5)

        // Remembered across a relaunch.
        harness.sync.stop()
        let again = CloudSync(dependencies: { harness.dependencies() })
        again.start(environment: harness.environment, usage: nil, runsLoop: false)
        await again.restoreSession()
        await again.syncNow()
        #expect(harness.syncRequests.count == 2)
    }

    @Test func onlyAllowedAccountsAreEverMentioned() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        let stranger = CloudFixture.workAccount
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA, account: stranger)], liveIDs: [CloudFixture.sessionA])
        harness.sync.record(UsageObservation(identityId: stranger.identityId, source: .probe, observedAt: CloudFixture.base,
                                             windows: [.init(id: "session", utilization: 1, resetsAt: nil)]))
        #expect(harness.sync.stores?.ledger.count == 0)
        #expect(harness.sync.stores?.recorder.pendingCount == 0)
        await harness.sync.syncNow()
        #expect(harness.syncRequests.isEmpty)
    }

    @Test func desktopSessionsNeedDesktopsOneAccountToBeTheirs() async throws {
        let harness = Harness()
        await harness.start()
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA, entrypoint: "claude-desktop")],
                                 liveIDs: [CloudFixture.sessionA])
        #expect(harness.sync.stores?.ledger.count == 0)
        // Two accounts in Claude Desktop: can't tell.
        harness.switches.desktop = [CloudFixture.accountUuid, "9d2c7b1a-0000-4e5f-8a9b-1c2d3e4f5a6b"]
        harness.clock.advance(120)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA, entrypoint: "claude-desktop")],
                                 liveIDs: [CloudFixture.sessionA])
        #expect(harness.sync.stores?.ledger.count == 0)
        harness.switches.desktop = [CloudFixture.accountUuid]
        harness.clock.advance(120)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA, entrypoint: "claude-desktop")],
                                 liveIDs: [CloudFixture.sessionA])
        #expect(harness.sync.stores?.ledger.entry(CloudFixture.sessionA)?.source == .desktop)
    }

    @Test func aFoldersOwnHistoryIsBackfilled() async throws {
        let harness = Harness()
        await harness.start()
        let own = harness.root.appendingPathComponent(".claude-own").path
        let slug = (own as NSString).appendingPathComponent("projects/-Users-me-work-billing")
        typealias L = CloudTranscriptLines
        try L.write([
            L.user("MY SECRET PROMPT", session: CloudFixture.sessionB, at: CloudFixture.stamp(0), cwd: "/Users/me/work/billing"),
            L.assistant(id: "b1", request: "rb1", session: CloudFixture.sessionB, input: 3, output: 4, at: CloudFixture.stamp(60),
                        cwd: "/Users/me/work/billing"),
        ], to: (slug as NSString).appendingPathComponent("\(CloudFixture.sessionB).jsonl"))
        // Hosted by Claude Desktop, whose account can't be told here: left out.
        try L.write([
            L.user("x", session: CloudFixture.sessionC, at: CloudFixture.stamp(0), cwd: "/Users/me/work/billing",
                   entrypoint: "claude-desktop"),
            L.assistant(id: "c1", request: "rc1", session: CloudFixture.sessionC, input: 1, output: 1, at: CloudFixture.stamp(5)),
        ], to: (slug as NSString).appendingPathComponent("\(CloudFixture.sessionC).jsonl"))
        harness.environment.folders = [CloudBackfill.Folder(configDir: own, identityId: CloudFixture.identityId,
                                                            accountKey: CloudFixture.accountKey, login: "login-1")]
        // The folder was seen signed in as this account from before these sessions.
        harness.environment.logins = [own: "login-1"]
        let now = harness.clock.now
        harness.clock.now = CloudFixture.base.addingTimeInterval(-60)
        harness.sync.setSyncEnabled(false)
        await harness.sync.tick()
        harness.sync.setSyncEnabled(true)
        harness.clock.now = now
        await harness.sync.syncNow()

        let request = try #require(harness.syncRequests.first)
        let sessions = try #require(harness.body(request)["sessions"] as? [[String: Any]])
        #expect(sessions.map { $0["sessionId"] as? String } == [CloudFixture.sessionB])
        let session = try #require(sessions.first)
        #expect((session["project"] as? [String: Any])?["name"] as? String == "billing")
        #expect((session["project"] as? [String: Any])?["key"] as? String
                == CloudKeys.projectKey(accountKey: CloudFixture.accountKey,
                                        path: CloudKeys.projectPath(forCwd: "/Users/me/work/billing", home: harness.root.path),
                                        secret: harness.sync.stores!.secret()))
        #expect(session["endedAt"] as? String == CloudJSON.string(from: CloudFixture.base.addingTimeInterval(60)))
        #expect(session["source"] as? String == "cli" && session["title"] is NSNull)
        let text = String(decoding: request.httpBody ?? Data(), as: UTF8.self)
        #expect(!text.contains("MY SECRET PROMPT") && !text.contains("/Users/me"))
        #expect(harness.sync.stores?.ledger.entry(CloudFixture.sessionB)?.origin == .backfill)
        #expect(harness.sync.stores?.ledger.entry(CloudFixture.sessionC) == nil)
    }

    @Test func requestsKeepToTheContractsLimits() {
        let device = CloudSyncRequest.Device(id: UUID().uuidString, name: "Mac", appVersion: "1")
        var accounts: [String: CloudAccountInfo] = [:]
        for index in 0..<60 {
            let key = CloudKeys.sha256Hex("account-\(index)")
            accounts[key] = CloudAccountInfo(identityId: "uuid:\(index)", accountKey: key, accountUuid: "\(index)",
                                             email: nil, organizationName: nil, plan: nil, label: nil)
        }
        let keys = accounts.keys.sorted()
        let sessions = (0..<450).map { index -> (CloudSyncRequest.Session, CloudSyncMemory.Record) in
            let key = keys[index % keys.count]
            return (CloudSyncRequest.Session(accountKey: key, sessionId: UUID().uuidString,
                                             project: .init(key: CloudKeys.sha256Hex("p"), name: "p"), title: nil, source: .cli,
                                             models: [], startedAt: CloudFixture.base, lastActivityAt: CloudFixture.base,
                                             endedAt: nil, messageCount: 1, tokens: .zero, costUsd: nil),
                    CloudSyncMemory.Record(base: "\(index)", summary: nil))
        }
        let readings = (0..<1_200).map { index in
            RecordedUsageReading(accountKey: keys[(index * 7) % keys.count], identityId: "x", source: .probe,
                                 observedAt: CloudFixture.base.addingTimeInterval(Double(index)),
                                 windows: [.init(id: "session", utilization: 1, resetsAt: nil)])
        }
        let batches = CloudBatcher.batches(sessions: sessions, readings: readings, accounts: accounts, device: device)
        #expect(batches.reduce(0) { $0 + $1.request.sessions.count } == 450)
        #expect(batches.reduce(0) { $0 + $1.request.usage.count } == 1_200)
        #expect(batches.reduce(0) { $0 + $1.readings.count } == 1_200)
        for batch in batches {
            let request = batch.request
            #expect(request.sessions.count <= 200 && request.usage.count <= 500 && request.accounts.count <= 50)
            let listed = Set(request.accounts.map(\.key))
            #expect(request.sessions.allSatisfy { listed.contains($0.accountKey) })
            #expect(request.usage.allSatisfy { listed.contains($0.accountKey) })
            #expect(request.clamped(now: CloudFixture.base) == request)
            #expect(Set(batch.records.keys)
                    == Set(request.sessions.map { CloudLedgerEntry.key(sessionId: $0.sessionId, accountKey: $0.accountKey) }))
        }
    }

    // MARK: - Failures

    @Test func failuresBackOffAndHonourRetryAfter() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.transport.answer { request in
            if request.url?.path == "/api/app/v1/sync" {
                return .json(500, ["error": ["code": "INTERNAL", "message": "Something went wrong. Try again later."]])
            }
            return try Harness.website(request)
        }
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 1)
        #expect(harness.sync.state.lastError == "Something went wrong. Try again later.")
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 1)
        harness.clock.advance(CloudSync.initialBackoff + 1)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 2)
        // The second failure waits twice as long.
        harness.clock.advance(CloudSync.initialBackoff + 1)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 2)

        harness.transport.answer { request in
            if request.url?.path == "/api/app/v1/sync" {
                return .json(429, ["error": ["code": "RATE_LIMITED", "message": "Too many syncs. Try again in 600 s."]],
                             headers: ["Retry-After": "600"])
            }
            return try Harness.website(request)
        }
        harness.clock.advance(2 * CloudSync.initialBackoff)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 3)
        harness.clock.advance(300)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 3)

        harness.transport.answer(Harness.website)
        harness.clock.advance(301)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 4)
        #expect(harness.sync.state.lastError == nil)
        #expect(CloudSync.backoff(afterFailures: 1) == 30 && CloudSync.backoff(afterFailures: 3) == 120)
        #expect(CloudSync.backoff(afterFailures: 40) == CloudSync.maxBackoff)
    }

    /// Regression (review finding 9): the website refusing a token the
    /// sign-in service just refreshed (its key set couldn't be fetched, say)
    /// backs off; the sign-in is kept.
    @Test func aRefusalAfterARefreshBacksOffAndKeepsTheSignIn() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.transport.answer { request in
            if request.url?.path == "/api/app/v1/sync" {
                return .init(status: 401, body: try TestPaths.contractFixture("error.json"))
            }
            return try Harness.website(request)
        }
        await harness.sync.syncNow()
        // Once, a refresh, once more: then a backoff, still signed in.
        #expect(harness.syncRequests.count == 2)
        #expect(harness.transport.requests(to: "/auth/v1/token").count == 1)
        #expect(harness.sync.state.isSignedIn && harness.sync.state.syncEnabled)
        #expect(harness.sync.state.lastError == "The website didn't accept this Mac's sign-in. Trying again later.")
        #expect(harness.sessionStore.load()?.refreshToken == "refresh-2")
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 2)
        // The website is fine again: the same sign-in works.
        harness.transport.answer(Harness.website)
        harness.clock.advance(CloudSync.initialBackoff + 1)
        await harness.sync.tick()
        #expect(harness.syncRequests.count == 3)
        #expect(harness.sync.state.lastError == nil && harness.sync.state.isSignedIn)
    }

    /// Only Supabase refusing the refresh token itself ends the sign-in;
    /// the sync switch goes off with it (review findings 9 and 17).
    @Test func aRefusedRefreshTokenSignsOut() async throws {
        let harness = Harness()
        await harness.start()
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        // Supabase down: a 503 from its token endpoint is tried again later.
        harness.transport.answer { request in
            switch request.url?.path {
            case "/api/app/v1/sync": return .init(status: 401, body: try TestPaths.contractFixture("error.json"))
            case "/auth/v1/token": return .json(503, ["msg": "Service Unavailable"])
            default: return try Harness.website(request)
            }
        }
        await harness.sync.syncNow()
        #expect(harness.sync.state.isSignedIn)
        #expect(harness.sessionStore.load()?.refreshToken == "refresh-1")

        harness.transport.answer { request in
            switch request.url?.path {
            case "/api/app/v1/sync": return .init(status: 401, body: try TestPaths.contractFixture("error.json"))
            case "/auth/v1/token":
                return .json(400, ["code": 400, "error_code": "refresh_token_already_used",
                                   "msg": "Invalid Refresh Token: Already Used"])
            default: return try Harness.website(request)
            }
        }
        harness.clock.advance(CloudSync.maxBackoff)
        await harness.sync.syncNow()
        #expect(harness.sync.state.auth == .signedOut)
        #expect(harness.sync.state.lastError == "Signed out of the website. Sign in again.")
        #expect(harness.sessionStore.load() == nil)
        #expect(!harness.sync.state.syncEnabled && !harness.defaults.store.cloudSyncEnabled)
        let sent = harness.syncRequests.count
        await harness.sync.tick()
        #expect(harness.syncRequests.count == sent)
    }

    // MARK: - Signing in and out

    @Test func signingInGoesThroughTheWebsitesSupabase() async throws {
        let harness = Harness(signedIn: false)
        await harness.start()
        let opened = OpenedURLs()
        let ok = await harness.sync.signIn { url in
            opened.add(url)
            return URL(string: "agentnotch://auth-callback?code=the-code")!
        }
        #expect(ok)
        #expect(harness.sync.state.auth == .signedIn(email: "me@example.com"))
        // A new sign-in starts with sync off, for the user to turn on (finding 17).
        #expect(!harness.sync.state.syncEnabled && !harness.sync.state.summariesEnabled)
        #expect(!harness.sync.canUpload)
        #expect(harness.sync.state.dashboardURL?.absoluteString == "https://agentnotch.example.com/dashboard")
        #expect(harness.sync.state.poolsURL?.absoluteString == "https://agentnotch.example.com/dashboard/pools")
        #expect(opened.urls.first?.host == "abcdefghijklmnop.supabase.co")
        #expect(harness.transport.recorded.map { $0.url!.path } == ["/api/app/v1/config", "/auth/v1/token", "/api/app/v1/me"])
        #expect(harness.sessionStore.load()?.websiteURL == "https://agentnotch.example.com")

        await harness.sync.signOut()
        #expect(harness.sync.state.auth == .signedOut)
        #expect(harness.transport.requests(to: "/auth/v1/logout").count == 1)
        #expect(harness.sessionStore.load() == nil)
    }

    @Test func aFailedOrClosedSignInSaysSo() async {
        let harness = Harness(signedIn: false)
        await harness.start()
        #expect(await harness.sync.signIn { _ in throw CancellationError() } == false)
        #expect(harness.sync.state.auth == .signedOut && harness.sync.state.lastError == nil)
        #expect(await harness.sync.signIn { _ in URL(string: "agentnotch://auth-callback?error=access_denied&error_description=Not+allowed")! } == false)
        #expect(harness.sync.state.auth == .error("Not allowed"))
    }

    @Test func anotherWebsiteSignsOutOfTheOldOne() async {
        let harness = Harness()
        await harness.start()
        #expect(harness.sync.state.isSignedIn)
        #expect(await harness.sync.setWebsite("http://evil.example.com") == false)
        #expect(harness.sync.state.isSignedIn)
        #expect(await harness.sync.setWebsite("https://other.example.com/"))
        #expect(harness.sync.state.websiteURL == "https://other.example.com")
        #expect(harness.sync.state.auth == .signedOut)
        #expect(harness.transport.requests(to: "/auth/v1/logout").count == 1)
        // Sync was agreed to for the old website (finding 17).
        #expect(!harness.sync.state.syncEnabled && !harness.defaults.store.cloudSyncEnabled)
        #expect(await harness.sync.setWebsite(nil))
        #expect(harness.sync.state.websiteURL == nil)
    }

    // MARK: - Summaries

    /// A session seen running, synced once (so its totals are known), then
    /// gone for good: ended.
    func endedSession(_ harness: Harness) async throws {
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        await harness.sync.syncNow()
        harness.sync.observeLive([], liveIDs: [])
        harness.clock.advance(61)
        await harness.sync.tick()
        #expect(harness.sync.stores?.ledger.entry(CloudFixture.sessionA)?.endedAt != nil)
    }

    /// The ledger key of `CloudFixture.sessionA` under the fixture account.
    static let keyA = CloudLedgerEntry.key(sessionId: CloudFixture.sessionA, accountKey: CloudFixture.accountKey)

    @Test func summariesNeverRunWhileOff() async throws {
        let harness = Harness(summariesOn: false)
        await harness.start()
        try await endedSession(harness)
        harness.clock.advance(11 * 60)
        await harness.sync.tick()
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.isEmpty)
        #expect(!harness.sync.canSummarize)

        // On, but this run may not launch Claude Code (sealed, a dev run…).
        harness.sync.setSummariesEnabled(true)
        harness.switches.summariesAllowed = false
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.isEmpty)
        // On, but sync off.
        harness.switches.summariesAllowed = true
        harness.sync.setSyncEnabled(false)
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.isEmpty)
        // Something else is launching Claude Code right now.
        harness.sync.setSyncEnabled(true)
        harness.environment.launching = true
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.isEmpty)
    }

    @Test func aSummaryIsWrittenOnceInTheRightFolderAndSent() async throws {
        let harness = Harness(summariesOn: true)
        await harness.start()
        try await endedSession(harness)
        // Not ten minutes yet.
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.isEmpty)

        harness.clock.advance(10 * 60)
        await harness.sync.summarizeNext()
        let request = try #require(harness.runner.requests.first)
        #expect(request.sessionId == CloudFixture.sessionA && request.identityId == CloudFixture.identityId)
        #expect(request.configDirEnv == harness.environment.folder?.configDirEnv)
        #expect(harness.environment.folderRequests == [CloudFixture.identityId])
        #expect(request.input.contains("User: MY SECRET PROMPT about the login bug"))
        #expect(request.input.contains("Claude: Fixed it."))
        #expect(!request.input.contains("TOOL OUTPUT"))
        #expect(request.workingDirectory.lastPathComponent == "session-summary")

        // Once only.
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.count == 1)

        // Sent with the session (the only change since it was last sent).
        await harness.sync.syncNow()
        let last = try #require(harness.syncRequests.last)
        let session = try #require((harness.body(last)["sessions"] as? [[String: Any]])?.first)
        let summary = try #require(session["summary"] as? [String: Any])
        #expect(summary["text"] as? String == "Fixed the token refresh.")
        #expect(summary["model"] as? String == "claude-haiku-4-5-20251001")
        #expect(harness.sync.state.summarizedSessions == 1)

        // Off again: not sent any more, and nothing resent for it.
        let sent = harness.syncRequests.count
        harness.sync.setSummariesEnabled(false)
        await harness.sync.syncNow()
        #expect(harness.syncRequests.count == sent)
    }

    @Test func aSummaryFromAFolderThatChangedHandsIsDropped() async throws {
        let harness = Harness(summariesOn: true)
        await harness.start()
        try await endedSession(harness)
        harness.clock.advance(11 * 60)
        harness.environment.stillRuns = false
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.count == 1)
        #expect(harness.sync.stores?.summaries.summary(for: Self.keyA) == nil)
        #expect(harness.sync.stores?.summaries.attempt(for: Self.keyA) != nil)

        // No folder signed in as the account: not run at all.
        let other = Harness(summariesOn: true)
        await other.start()
        try await endedSession(other)
        other.clock.advance(11 * 60)
        other.environment.folder = nil
        await other.sync.summarizeNext()
        #expect(other.runner.requests.isEmpty)
    }

    // MARK: - Sealed

    @Test func aSealedRunShowsAFixtureAndDoesNothing() async throws {
        let harness = Harness(sealed: true)
        await harness.start()
        #expect(harness.sync.state == ClaudeCloudState.sealedFixture(now: harness.clock.now))
        #expect(harness.sync.state.isSignedIn && harness.sync.state.email == "me@example.com")
        try harness.writeSession(CloudFixture.sessionA)
        harness.sync.observeLive([harness.observation(CloudFixture.sessionA)], liveIDs: [CloudFixture.sessionA])
        harness.sync.setSyncEnabled(false)
        #expect(await harness.sync.setWebsite("https://other.example.com") == false)
        #expect(await harness.sync.signIn { $0 } == false)
        await harness.sync.syncNow()
        await harness.sync.tick()
        await harness.sync.signOut()
        #expect(harness.transport.recorded.isEmpty)
        #expect(harness.runner.requests.isEmpty)
        #expect(harness.sync.stores == nil)
        #expect(!FileManager.default.fileExists(atPath: harness.root.appendingPathComponent("support").path))
        #expect(harness.defaults.store.cloudSyncEnabled)
    }
}
