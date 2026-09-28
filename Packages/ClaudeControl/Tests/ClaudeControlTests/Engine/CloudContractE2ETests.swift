import Foundation
import Testing
@testable import ClaudeControl

/// The app's half of the cross-side check (`Scripts/cloud-contract-e2e.sh`):
/// a sync request built and encoded by the app's own code, from transcripts,
/// the ledger and the usage outbox, and the website's own answer read back.
///
/// Without the script's variables both tests still run: the request is
/// checked against the contract's rules, and the contract's response fixture
/// is read. With them, the request is also written to
/// `AGENTNOTCH_CONTRACT_OUT` (the website's integration test posts it through
/// its real sync route into Postgres), and the answer that route gave is read
/// from `AGENTNOTCH_CONTRACT_RESPONSE`. Nothing here reaches the network.
@MainActor
struct CloudContractE2ETests {
    typealias Harness = CloudSyncTests.Harness
    typealias L = CloudTranscriptLines

    nonisolated static let requestFileVariable = "AGENTNOTCH_CONTRACT_OUT"
    nonisolated static let responseFileVariable = "AGENTNOTCH_CONTRACT_RESPONSE"

    /// A path the script passed, if any.
    static func file(_ variable: String) -> URL? {
        guard let path = ProcessInfo.processInfo.environment[variable]?.trimmingCharacters(in: .whitespacesAndNewlines),
              !path.isEmpty else { return nil }
        return URL(fileURLWithPath: path)
    }

    // MARK: - The scenario

    static let personal = CloudAccountInfo(identityId: CloudFixture.identityId, accountKey: CloudFixture.accountKey,
                                           accountUuid: CloudFixture.accountUuid, email: "me@example.com",
                                           organizationName: nil, plan: "Max 20x", label: "Personal ☕")
    static let work = CloudAccountInfo(identityId: CloudFixture.workIdentityId, accountKey: CloudFixture.workAccountKey,
                                       accountUuid: CloudFixture.workUuid, email: "me@company.com",
                                       organizationName: "Société Exemple", plan: "Team", label: nil)

    /// Run by the personal account, ended, summarised; one subagent.
    static let sessionA = CloudFixture.sessionA
    /// Run by the work account in VS Code, still running.
    static let sessionB = CloudFixture.sessionB
    /// Begun as the personal account, resumed as the work one: two rows.
    static let sessionC = CloudFixture.sessionC
    /// Found on disk in a folder only the personal account uses (backfilled).
    static let sessionD = "5e6f7a8b-9c0d-4e1f-8a2b-3c4d5e6f7a8b"

    static let cwdA = "/Users/me/code/agentnotch"
    static let cwdB = "/Users/me/work/billing-service"
    static let cwdC = "/Users/me/code/données-client"
    static let cwdD = "/Users/me/work/reports"

    /// A morning on two accounts, as the sync service captures and sends it:
    /// live sessions the hub attributed, a session resumed under the other
    /// account, a folder's own history backfilled, usage readings from four
    /// sources, and a summary. The website is down for the first pass, so
    /// the next one carries everything in one request. Everything is dated
    /// a few hours before now: the website checks dates against its own
    /// clock (and keeps readings for 90 days).
    static func morning() async throws -> (harness: Harness, base: Date) {
        let base = Date(timeIntervalSince1970: (Date().timeIntervalSince1970 - 4 * 3600).rounded(.down))
        func at(_ seconds: TimeInterval) -> Date { base.addingTimeInterval(seconds) }
        func stamp(_ seconds: TimeInterval) -> String { CloudJSON.string(from: at(seconds)) }
        let a = sessionA, b = sessionB, c = sessionC, d = sessionD

        let harness = Harness(summariesOn: true)
        harness.environment.accountList = [personal, work]
        harness.runner.answer(.summary(.init(text: "Réparé le rafraîchissement du jeton ✓ — rotating refresh tokens are kept, with tests.",
                                             model: "claude-haiku-4-5-20251001", costUsd: 0.002)))
        harness.clock.now = at(-120)
        await harness.start()

        // The backfilled folder, seen signed in as the personal account
        // before its session began (with sync off, so nothing is sent yet).
        let own = harness.root.appendingPathComponent(".claude-own").path
        let ownProjects = (own as NSString).appendingPathComponent("projects/-Users-me-work-reports")
        try L.write([
            L.user("MY SECRET PROMPT about the report", session: d, at: stamp(60), cwd: cwdD, entrypoint: "sdk-ts"),
            L.aiTitle("Monthly usage report", session: d),
            L.assistant(id: "d1", request: "rd1", session: d, model: "claude-sonnet-4-5", input: 1200, output: 340,
                        cacheCreation: 5000, at: stamp(90), cwd: cwdD),
            L.assistant(id: "d2", request: "rd2", session: d, model: "claude-sonnet-4-5", input: 30, output: 900,
                        cacheRead: 5000, at: stamp(120), cwd: cwdD),
        ], to: (ownProjects as NSString).appendingPathComponent("\(d).jsonl"))
        harness.environment.folders = [CloudBackfill.Folder(configDir: own, identityId: personal.identityId,
                                                            accountKey: personal.accountKey, login: "login-personal")]
        harness.environment.logins = [own: "login-personal"]
        harness.clock.now = at(-60)
        harness.sync.setSyncEnabled(false)
        await harness.sync.tick()
        harness.sync.setSyncEnabled(true)

        // The live sessions' transcripts (and A's subagent).
        let projects = harness.root.appendingPathComponent(".claude/projects").path
        func transcript(_ slug: String, _ id: String) -> String {
            (projects as NSString).appendingPathComponent("\(slug)/\(id).jsonl")
        }
        let pathA = transcript("-Users-me-code-agentnotch", a)
        let pathB = transcript("-Users-me-work-billing-service", b)
        let pathC = transcript("-Users-me-code-donn-es-client", c)
        try L.write([
            L.user("MY SECRET PROMPT about the login bug", session: a, at: stamp(300), cwd: cwdA),
            L.assistant(id: "a1", request: "ra1", session: a, input: 1800, output: 420, cacheCreation: 24000, cacheRead: 120_000,
                        at: stamp(330), text: "Looking into it.", toolUse: true, cwd: cwdA),
            L.toolResult("TOOL OUTPUT WITH A PATH /Users/me/secret", session: a, at: stamp(331)),
            L.assistant(id: "a2", request: "ra2", session: a, model: "claude-haiku-4-5", input: 60, output: 35, cacheRead: 2000,
                        at: stamp(360), text: "Fixed it.", cwd: cwdA),
            L.assistant(id: "a3", request: "ra3", session: a, input: 12, output: 880, cacheCreation: 3000, cacheRead: 144_000,
                        at: stamp(600), cwd: cwdA),
        ], to: pathA)
        try L.write([
            L.assistant(id: "a-sub", request: "ra-sub", session: a, model: "claude-sonnet-4-5", input: 900, output: 150,
                        cacheCreation: 8000, at: stamp(400), sidechain: true, cwd: cwdA),
        ], to: ((pathA as NSString).deletingPathExtension as NSString).appendingPathComponent("subagents/agent-review.jsonl"))
        // B reads billions of cached tokens: more than a 32-bit integer holds.
        try L.write([
            L.user("MY SECRET PROMPT about invoices", session: b, at: stamp(900), cwd: cwdB, entrypoint: "claude-vscode"),
            L.assistant(id: "b1", request: "rb1", session: b, model: "claude-sonnet-4-5", input: 5120, output: 8840,
                        cacheCreation: 64000, cacheRead: 3_210_987_654, at: stamp(960), cwd: cwdB),
            L.assistant(id: "b2", request: "rb2", session: b, model: "claude-sonnet-4-5", input: 44, output: 1210,
                        cacheRead: 65000, at: stamp(1100), cwd: cwdB),
        ], to: pathB)
        try L.write([
            L.user("MY SECRET PROMPT about imports", session: c, at: stamp(1200), cwd: cwdC),
            L.assistant(id: "c1", request: "rc1", session: c, input: 400, output: 90, cacheCreation: 1500, cacheRead: 9000,
                        at: stamp(1230), cwd: cwdC),
            L.user("go on", session: c, at: stamp(1560), cwd: cwdC),
            L.assistant(id: "c2", request: "rc2", session: c, model: "claude-sonnet-4-5", input: 70, output: 30, cacheRead: 11000,
                        at: stamp(1590), cwd: cwdC),
            L.assistant(id: "c3", request: "rc3", session: c, model: "claude-sonnet-4-5", input: 5, output: 400, cacheRead: 12000,
                        at: stamp(1620), cwd: cwdC),
        ], to: pathC)

        let configDir = harness.root.appendingPathComponent(".claude").path
        func live(_ id: String, _ account: CloudAccountInfo, cwd: String, path: String, entrypoint: String, start: TimeInterval,
                  last: TimeInterval, cost: Double?, title: String?, process: TimeInterval? = nil) -> LiveSessionObservation {
            LiveSessionObservation(sessionId: id, identityId: account.identityId, accountKey: account.accountKey, cwd: cwd,
                                   transcriptPath: path, configDir: configDir, entrypoint: entrypoint, startedAt: at(start),
                                   lastActivityAt: at(last), model: "claude-opus-4-5", costUsd: cost, title: title,
                                   processStartedAt: process.map(at))
        }
        let liveA = live(a, personal, cwd: cwdA, path: pathA, entrypoint: "cli", start: 300, last: 600, cost: 2.4617,
                         title: "Fix the login bug 🐛")
        let liveB = live(b, work, cwd: cwdB, path: pathB, entrypoint: "claude-vscode", start: 900, last: 1100, cost: 1.25,
                         title: nil)
        let titleC = "Tidy the données-client imports"
        let liveC = live(c, personal, cwd: cwdC, path: pathC, entrypoint: "cli", start: 1200, last: 1230, cost: 0.4, title: titleC)
        let resumedC = live(c, work, cwd: cwdC, path: pathC, entrypoint: "claude-vscode", start: 1500, last: 1620, cost: 0.9,
                            title: titleC, process: 1500)
        func usage(_ account: CloudAccountInfo, _ source: CloudUsageSource, _ seconds: TimeInterval,
                   _ windows: [(String, Double, Date?)]) -> UsageObservation {
            UsageObservation(identityId: account.identityId, source: source, observedAt: at(seconds),
                             windows: windows.map { .init(id: $0.0, utilization: $0.1, resetsAt: $0.2) })
        }
        let personalReset = at(4 * 3600), workReset = at(5 * 3600)
        let personalWeek = at(4 * 24 * 3600), workWeek = at(6 * 24 * 3600)

        harness.clock.now = at(620)
        harness.sync.observeLive([liveA], liveIDs: [a])
        harness.sync.record(usage(personal, .desktop, 610, [("session", 42, personalReset), ("weekly_all", 61.5, personalWeek),
                                                            ("weekly_opus", 12, nil), ("extra_usage", 3.25, nil)]))
        harness.clock.now = at(1110)
        harness.sync.observeLive([liveA, liveB], liveIDs: [a, b])
        harness.clock.now = at(1240)
        harness.sync.observeLive([liveA, liveB, liveC], liveIDs: [a, b, c])
        harness.sync.record(usage(personal, .statusLine, 1235, [("session", 44.5, personalReset), ("weekly_all", 62, personalWeek)]))
        harness.clock.now = at(1630)
        harness.sync.observeLive([liveA, liveB, resumedC], liveIDs: [a, b, c])
        harness.sync.record(usage(work, .probe, 1625, [("session", 8, workReset), ("weekly_all", 0, workWeek),
                                                       ("weekly_sonnet", 17.25, workWeek)]))
        harness.sync.record(usage(work, .claudeJson, 1640, [("session", 9, workReset)]))

        // The website is down for the first pass: nothing is marked sent
        // (the pass still backfills and reads the transcripts).
        harness.clock.now = at(1700)
        harness.transport.answer { request in
            guard request.url?.path == "/api/app/v1/sync" else { return try Harness.website(request) }
            return .json(503, ["error": ["code": "INTERNAL", "message": "The website is restarting."]], headers: ["Retry-After": "120"])
        }
        await harness.sync.syncNow()
        #expect(harness.sync.state.lastError == "The website is restarting.")
        harness.transport.answer(Harness.website)

        // A ends (gone for a minute), and is summarised once it has been quiet.
        harness.clock.now = at(1800)
        harness.sync.observeLive([liveB, resumedC], liveIDs: [b, c])
        harness.clock.now = at(1870)
        harness.sync.observeLive([liveB, resumedC], liveIDs: [b, c])
        harness.clock.now = at(2460)
        await harness.sync.summarizeNext()
        #expect(harness.runner.requests.map(\.sessionId) == [a])

        // "Sync now": the rest of the morning in one request.
        harness.clock.now = at(2480)
        await harness.sync.syncNow()
        return (harness, base)
    }

    // MARK: - The request

    /// The request the app sends keeps every rule of the contract the app
    /// can check, carries what the transcripts and readings say, and leaks
    /// nothing. With `AGENTNOTCH_CONTRACT_OUT` set, its exact bytes are
    /// written there for the website's side.
    @Test func theAppsSyncRequestKeepsTheContract() async throws {
        let (harness, base) = try await Self.morning()
        func at(_ seconds: TimeInterval) -> String { CloudJSON.string(from: base.addingTimeInterval(seconds)) }
        #expect(harness.syncRequests.count == 2)
        let request = try #require(harness.syncRequests.last)
        let body = try #require(request.httpBody)
        #expect(harness.sync.state.lastError == nil)
        #expect(harness.sync.state.pendingUsage == 0 && harness.sync.state.pendingSessions == 0)

        let json = try JSONSerialization.jsonObject(with: body)
        let problems = CloudContractRules.violations(of: json, now: Date())
        #expect(problems.isEmpty, "\(problems)")
        // The app reads back what it wrote, and it is already within the limits.
        let decoded = try CloudJSON.makeDecoder().decode(CloudSyncRequest.self, from: body)
        #expect(decoded.clamped(now: Date()) == decoded)

        let text = String(decoding: body, as: UTF8.self)
        let secretHex = try #require(harness.sync.stores?.secret()).map { String(format: "%02x", $0) }.joined()
        for leaked in ["MY SECRET PROMPT", "TOOL OUTPUT", "Looking into it", "Fixed it.", "go on", "/Users/me", harness.root.path,
                       ".jsonl", CloudFixture.accountUuid, CloudFixture.workUuid, CloudFixture.workOrganization, secretHex] {
            #expect(!text.contains(leaked), "\(leaked) was sent")
        }

        let object = try #require(json as? [String: Any])
        let accounts = try #require(object["accounts"] as? [[String: Any]])
        #expect(accounts.map { $0["key"] as? String } == [Self.personal.accountKey, Self.work.accountKey].sorted())
        let personal = try #require(accounts.first { $0["key"] as? String == Self.personal.accountKey })
        #expect(personal["label"] as? String == "Personal ☕" && personal["organizationName"] is NSNull)
        let work = try #require(accounts.first { $0["key"] as? String == Self.work.accountKey })
        #expect(work["organizationName"] as? String == "Société Exemple" && work["label"] is NSNull)

        let sessions = try #require(object["sessions"] as? [[String: Any]])
        let rows = Dictionary(sessions.map { ("\($0["sessionId"] as? String ?? "")|\($0["accountKey"] as? String ?? "")", $0) },
                              uniquingKeysWith: { first, _ in first })
        #expect(rows.count == 5 && sessions.count == 5)
        func row(_ id: String, _ account: CloudAccountInfo) throws -> [String: Any] {
            try #require(rows["\(id)|\(account.accountKey)"], "no row for \(id) on \(account.label ?? account.email ?? "")")
        }
        func tokens(_ row: [String: Any]) -> [String: Int]? { row["tokens"] as? [String: Int] }
        func project(_ row: [String: Any]) -> String? { (row["project"] as? [String: Any])?["name"] as? String }

        let a = try row(Self.sessionA, Self.personal)
        #expect(tokens(a) == ["input": 2772, "output": 1485, "cacheCreation": 35000, "cacheRead": 266_000])
        #expect(a["messageCount"] as? Int == 4 && a["source"] as? String == "cli" && project(a) == "agentnotch")
        #expect((a["models"] as? [String])?.first == "claude-opus-4-5" && (a["models"] as? [String])?.count == 3)
        #expect(a["title"] as? String == "Fix the login bug 🐛" && a["costUsd"] as? Double == 2.4617)
        #expect(a["startedAt"] as? String == at(300) && a["lastActivityAt"] as? String == at(600))
        #expect(a["endedAt"] as? String == at(1800))
        let summary = try #require(a["summary"] as? [String: Any])
        #expect((summary["text"] as? String)?.hasPrefix("Réparé le rafraîchissement du jeton ✓") == true)
        #expect(summary["model"] as? String == "claude-haiku-4-5-20251001" && summary["generatedAt"] as? String == at(2460))

        let b = try row(Self.sessionB, Self.work)
        #expect(tokens(b) == ["input": 5164, "output": 10050, "cacheCreation": 64000, "cacheRead": 3_211_052_654])
        #expect(b["messageCount"] as? Int == 2 && b["source"] as? String == "vscode" && project(b) == "billing-service")
        #expect(b["title"] is NSNull && b["endedAt"] is NSNull && b["summary"] == nil)
        // Its billions of cache reads at Sonnet 4.5's list prices come to more than the $1.25
        // reported: the larger goes.
        #expect(b["costUsd"] as? Double == 963.722038)

        // Resumed under the other account: each part only its own responses, and not Claude Code's
        // cost (it can't be split) but those responses at list prices.
        let mine = try row(Self.sessionC, Self.personal), theirs = try row(Self.sessionC, Self.work)
        #expect(tokens(mine) == ["input": 400, "output": 90, "cacheCreation": 1500, "cacheRead": 9000])
        #expect(tokens(theirs) == ["input": 75, "output": 430, "cacheCreation": 0, "cacheRead": 23000])
        #expect(mine["messageCount"] as? Int == 1 && theirs["messageCount"] as? Int == 2)
        #expect(mine["endedAt"] as? String == at(1500) && theirs["endedAt"] is NSNull)
        #expect(mine["costUsd"] as? Double == 0.018125 && theirs["costUsd"] as? Double == 0.013575)
        #expect(project(mine) == "données-client" && project(theirs) == "données-client")
        #expect((mine["project"] as? [String: Any])?["key"] as? String != (theirs["project"] as? [String: Any])?["key"] as? String)

        let d = try row(Self.sessionD, Self.personal)
        #expect(tokens(d) == ["input": 1230, "output": 1240, "cacheCreation": 5000, "cacheRead": 5000])
        #expect(d["source"] as? String == "sdk" && d["title"] as? String == "Monthly usage report" && project(d) == "reports")
        #expect(d["startedAt"] as? String == at(60) && d["endedAt"] as? String == at(120))
        // Found only on disk: no status line reported a cost, so its responses at list prices.
        #expect(d["costUsd"] as? Double == 0.04254)

        let usage = try #require(object["usage"] as? [[String: Any]])
        #expect(usage.compactMap { $0["source"] as? String }.sorted() == ["claudeJson", "desktop", "probe", "statusLine"])
        #expect(usage.reduce(0) { $0 + (($1["windows"] as? [Any])?.count ?? 0) } == 10)

        if let out = Self.file(Self.requestFileVariable) {
            try body.write(to: out, options: .atomic)
        }
    }

    /// The rules are real: the contract's own fixture keeps them, and each
    /// thing the website's schemas refuse breaks one.
    @Test func theContractRulesCatchWhatTheWebsiteRefuses() throws {
        let now = try #require(CloudJSON.date(from: "2026-09-25T12:00:00Z"))
        let fixture = String(decoding: try TestPaths.contractFixture("sync-request.json"), as: UTF8.self)
        func problems(_ text: String) throws -> [String] {
            CloudContractRules.violations(of: try JSONSerialization.jsonObject(with: Data(text.utf8)), now: now)
        }
        let fixtureProblems = try problems(fixture)
        #expect(fixtureProblems.isEmpty, "\(fixtureProblems)")

        let personal = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0"
        let breakages: [(String, String, String)] = [
            ("another schema version", "\"schemaVersion\": 1", "\"schemaVersion\": 2"),
            ("a field the contract doesn't have", "\"schemaVersion\": 1,", "\"schemaVersion\": 1, \"extra\": true,"),
            ("a device id that isn't a UUID", "0E6F0B4C-2F7A-4E53-9D1B-6A2C7F9E1D35", "studio"),
            ("an uppercase account key", "\"key\": \"\(personal)\"", "\"key\": \"\(personal.uppercased())\""),
            ("a session of an account not listed", "\"key\": \"d8485d82", "\"key\": \"e8485d82"),
            ("a null left out", "\"endedAt\": null,", ""),
            ("an unknown source", "\"source\": \"cli\"", "\"source\": \"terminal\""),
            ("a date with an offset", "\"startedAt\": \"2026-09-25T11:00:00Z\"", "\"startedAt\": \"2026-09-25T12:00:00+01:00\""),
            ("a date before 2023", "\"startedAt\": \"2026-09-25T11:00:00Z\"", "\"startedAt\": \"2022-12-31T23:59:59Z\""),
            ("a date two days ahead", "\"endedAt\": \"2026-09-25T09:48:00Z\"", "\"endedAt\": \"2026-09-27T12:00:01Z\""),
            ("a reset time 33 days ahead", "\"resetsAt\": \"2026-09-29T08:00:00Z\"", "\"resetsAt\": \"2026-10-28T12:00:01Z\""),
            ("a fractional count", "\"messageCount\": 37", "\"messageCount\": 37.5"),
            ("a negative token count", "\"cacheRead\": 910000", "\"cacheRead\": -5"),
            ("a token count as text", "\"cacheRead\": 910000", "\"cacheRead\": \"910000\""),
            ("a negative cost", "\"costUsd\": 14.82", "\"costUsd\": -1"),
            ("a window id the website doesn't know", "\"id\": \"weekly_opus\"", "\"id\": \"monthly\""),
            ("a title that is too long", "\"title\": null", "\"title\": \"\(String(repeating: "x", count: 201))\""),
            ("a summary without its model", "\"model\": \"claude-haiku-4-5-20251001\",", ""),
            ("a list where a number goes", "\"costUsd\": 14.82", "\"costUsd\": [14.82]"),
            ("a boolean where a number goes", "\"messageCount\": 212", "\"messageCount\": true"),
        ]
        for (what, original, replacement) in breakages {
            #expect(fixture.contains(original), "\(what): the fixture no longer has \(original)")
            var broken = fixture
            if let range = broken.range(of: original) { broken.replaceSubrange(range, with: replacement) }
            let found = (try? problems(broken)) ?? ["not JSON"]
            #expect(!found.isEmpty && found != ["not JSON"], "\(what) wasn't caught")
        }
    }

    // MARK: - The answer

    /// The website's answer to the request, read by the app's real client
    /// (`CloudAPI.sync`, the one the service calls) and taken by the sync
    /// service as success. With `AGENTNOTCH_CONTRACT_RESPONSE` set, the
    /// answer is the one the website's real route gave to the request in
    /// `AGENTNOTCH_CONTRACT_OUT`; otherwise the contract's fixture.
    @Test func theWebsitesSyncResponseIsRead() async throws {
        let responseFile = Self.file(Self.responseFileVariable)
        let answer = try responseFile.map { try Data(contentsOf: $0) } ?? TestPaths.contractFixture("sync-response.json")
        let requestData: Data
        if responseFile != nil {
            let requestFile = try #require(Self.file(Self.requestFileVariable), "the website's answer needs the request it answered")
            requestData = try Data(contentsOf: requestFile)
        } else {
            requestData = try TestPaths.contractFixture("sync-request.json")
        }
        let request = try CloudJSON.makeDecoder().decode(CloudSyncRequest.self, from: requestData)

        let transport = FakeTransport { request in
            guard request.url?.path == "/api/app/v1/sync" else { return .json(404, [String: String]()) }
            return .init(status: 200, body: answer)
        }
        let auth = CloudAuth(transport: transport, store: CloudSessionMemoryStore(CloudAuthTests.session(expiresIn: 3600)),
                             clock: { Date() })
        let api = CloudAPI(website: try #require(URL(string: "https://agentnotch.example.com")), transport: transport,
                           auth: auth, appVersion: "9.9")
        let response = try await api.sync(request)
        #expect(transport.requests(to: "/api/app/v1/sync").count == 1)
        if responseFile != nil {
            // Everything was new to the website, so everything was stored.
            #expect(response.accepted == .init(sessions: request.sessions.count, usage: request.usage.count))
            #expect(abs(response.serverTime.timeIntervalSinceNow) < 24 * 3600)
        } else {
            #expect(response.accepted == .init(sessions: 2, usage: 2))
            #expect(response.serverTime == CloudJSON.date(from: "2026-09-25T11:21:00Z"))
        }

        // The service takes the same answer as success: what it sent is marked sent.
        let harness = Harness()
        harness.transport.answer { request in
            guard request.url?.path == "/api/app/v1/sync" else { return try Harness.website(request) }
            return .init(status: 200, body: answer)
        }
        await harness.start()
        harness.sync.record(harness.usage)
        await harness.sync.syncNow()
        #expect(harness.syncRequests.count == 1)
        #expect(harness.sync.state.lastError == nil && harness.sync.state.lastSyncAt != nil)
        #expect(harness.sync.state.pendingUsage == 0)
    }
}

/// The contract's rules (`web/contract/README.md`) over a sync request as
/// JSON, checked on the encoded bytes rather than on the Swift values: every
/// field there and no other, explicit nulls, types, limits, key and id
/// formats, dates as ISO 8601 UTC with a `Z` from 2023-01-01 to a day after
/// `now` (a window's reset time to 32 days after), and every session and
/// reading naming a listed account. Pure.
nonisolated enum CloudContractRules {
    static let datePattern = #"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?Z$"#
    static let keyPattern = "^[0-9a-f]{64}$"
    static let uuidPattern = "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
    static let windowPattern = "^(session|weekly_all|extra_usage|weekly_[a-z0-9._-]{1,60})$"
    static let sessionSources = ["cli", "vscode", "desktop", "sdk", "other"]
    static let usageSources = ["probe", "statusLine", "claudeJson", "desktop"]
    static let earliest = Date(timeIntervalSince1970: 1_672_531_200)

    static func violations(of json: Any, now: Date) -> [String] {
        var check = Checker(now: now)
        check.request(json)
        return check.problems
    }

    private struct Checker {
        let now: Date
        var problems: [String] = []

        mutating func fail(_ path: String, _ message: String) { problems.append("\(path): \(message)") }

        mutating func request(_ json: Any) {
            guard let root = object(json, "$", ["schemaVersion", "device", "accounts", "sessions", "usage"]) else { return }
            if let version = number(root["schemaVersion"], "$.schemaVersion", integer: true, max: 1), version != 1 {
                fail("$.schemaVersion", "must be 1")
            }
            if let device = object(root["device"], "$.device", ["id", "name", "appVersion"]) {
                string(device["id"], "$.device.id", max: 36, pattern: CloudContractRules.uuidPattern)
                string(device["name"], "$.device.name", max: 120)
                string(device["appVersion"], "$.device.appVersion", max: 40)
            }
            var accountKeys: Set<String> = []
            for (index, value) in array(root["accounts"], "$.accounts", max: 50).enumerated() {
                let path = "$.accounts[\(index)]"
                guard let account = object(value, path, ["key", "email", "organizationName", "plan", "label"]) else { continue }
                if let key = string(account["key"], "\(path).key", max: 64, pattern: CloudContractRules.keyPattern) {
                    accountKeys.insert(key)
                }
                string(account["email"], "\(path).email", max: 320, nullable: true)
                string(account["organizationName"], "\(path).organizationName", max: 200, nullable: true)
                string(account["plan"], "\(path).plan", max: 60, nullable: true)
                string(account["label"], "\(path).label", max: 80, nullable: true)
            }
            for (index, value) in array(root["sessions"], "$.sessions", max: 200).enumerated() {
                session(value, "$.sessions[\(index)]", accounts: accountKeys)
            }
            for (index, value) in array(root["usage"], "$.usage", max: 500).enumerated() {
                reading(value, "$.usage[\(index)]", accounts: accountKeys)
            }
        }

        mutating func session(_ value: Any, _ path: String, accounts: Set<String>) {
            let fields: Set<String> = ["accountKey", "sessionId", "project", "title", "source", "models", "startedAt",
                                       "lastActivityAt", "endedAt", "messageCount", "tokens", "costUsd"]
            guard let session = object(value, path, fields, optional: ["summary"]) else { return }
            accountKey(session["accountKey"], "\(path).accountKey", accounts: accounts)
            string(session["sessionId"], "\(path).sessionId", max: 36, pattern: CloudContractRules.uuidPattern)
            if let project = object(session["project"], "\(path).project", ["key", "name"]) {
                string(project["key"], "\(path).project.key", max: 64, pattern: CloudContractRules.keyPattern)
                string(project["name"], "\(path).project.name", max: 120)
            }
            string(session["title"], "\(path).title", max: 200, nullable: true)
            if let source = string(session["source"], "\(path).source", max: 20),
               !CloudContractRules.sessionSources.contains(source) {
                fail("\(path).source", "\(source) isn't a session source")
            }
            for (index, model) in array(session["models"], "\(path).models", max: 10).enumerated() {
                string(model, "\(path).models[\(index)]", max: 200)
            }
            date(session["startedAt"], "\(path).startedAt")
            date(session["lastActivityAt"], "\(path).lastActivityAt")
            date(session["endedAt"], "\(path).endedAt", nullable: true)
            number(session["messageCount"], "\(path).messageCount", integer: true, max: 2_147_483_647)
            if let tokens = object(session["tokens"], "\(path).tokens", ["input", "output", "cacheCreation", "cacheRead"]) {
                for field in ["input", "output", "cacheCreation", "cacheRead"] {
                    number(tokens[field], "\(path).tokens.\(field)", integer: true, max: 9_007_199_254_740_991)
                }
            }
            number(session["costUsd"], "\(path).costUsd", integer: false, max: 100_000_000, nullable: true, exclusive: true)
            if session.keys.contains("summary"),
               let summary = object(session["summary"], "\(path).summary", ["text", "model", "generatedAt"]) {
                string(summary["text"], "\(path).summary.text", max: 2000)
                string(summary["model"], "\(path).summary.model", max: 80)
                date(summary["generatedAt"], "\(path).summary.generatedAt")
            }
        }

        mutating func reading(_ value: Any, _ path: String, accounts: Set<String>) {
            guard let reading = object(value, path, ["accountKey", "source", "observedAt", "windows"]) else { return }
            accountKey(reading["accountKey"], "\(path).accountKey", accounts: accounts)
            if let source = string(reading["source"], "\(path).source", max: 20), !CloudContractRules.usageSources.contains(source) {
                fail("\(path).source", "\(source) isn't a usage source")
            }
            date(reading["observedAt"], "\(path).observedAt")
            for (index, value) in array(reading["windows"], "\(path).windows", max: 20).enumerated() {
                let windowPath = "\(path).windows[\(index)]"
                guard let window = object(value, windowPath, ["id", "utilization", "resetsAt"]) else { continue }
                string(window["id"], "\(windowPath).id", max: 67, pattern: CloudContractRules.windowPattern)
                number(window["utilization"], "\(windowPath).utilization", integer: false, max: .greatestFiniteMagnitude)
                date(window["resetsAt"], "\(windowPath).resetsAt", nullable: true, ahead: 32 * 24 * 3600)
            }
        }

        mutating func accountKey(_ value: Any?, _ path: String, accounts: Set<String>) {
            if let key = string(value, path, max: 64, pattern: CloudContractRules.keyPattern), !accounts.contains(key) {
                fail(path, "must appear in accounts[]")
            }
        }

        /// Every required field present (an explicit null counts), and no other.
        mutating func object(_ value: Any?, _ path: String, _ required: Set<String>,
                             optional: Set<String> = []) -> [String: Any]? {
            guard let object = value as? [String: Any] else {
                fail(path, "must be an object")
                return nil
            }
            let keys = Set(object.keys)
            let missing = required.subtracting(keys)
            if !missing.isEmpty { fail(path, "missing \(missing.sorted())") }
            let extra = keys.subtracting(required).subtracting(optional)
            if !extra.isEmpty { fail(path, "not in the contract: \(extra.sorted())") }
            return object
        }

        mutating func array(_ value: Any?, _ path: String, max: Int) -> [Any] {
            guard let items = value as? [Any] else {
                fail(path, "must be a list")
                return []
            }
            if items.count > max { fail(path, "more than \(max) items") }
            return items
        }

        /// Lengths in UTF-16 units, as the website's schemas count them.
        @discardableResult
        mutating func string(_ value: Any?, _ path: String, max: Int, nullable: Bool = false, pattern: String? = nil) -> String? {
            if value is NSNull {
                if !nullable { fail(path, "must not be null") }
                return nil
            }
            guard let text = value as? String else {
                fail(path, "must be a string")
                return nil
            }
            if text.utf16.count > max { fail(path, "longer than \(max)") }
            if let pattern, text.range(of: pattern, options: .regularExpression) == nil {
                fail(path, "\(text) doesn't match \(pattern)")
            }
            return text
        }

        /// A JSON number (never a boolean), finite and ≥ 0.
        @discardableResult
        mutating func number(_ value: Any?, _ path: String, integer: Bool, max: Double, nullable: Bool = false,
                             exclusive: Bool = false) -> Double? {
            if value is NSNull {
                if !nullable { fail(path, "must not be null") }
                return nil
            }
            guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else {
                fail(path, "must be a number")
                return nil
            }
            let double = number.doubleValue
            if !double.isFinite || double < 0 { fail(path, "must be finite and not negative") }
            if integer, double.rounded() != double { fail(path, "must be a whole number") }
            if exclusive ? double >= max : double > max { fail(path, "too large") }
            return double
        }

        mutating func date(_ value: Any?, _ path: String, nullable: Bool = false, ahead: TimeInterval = 24 * 3600) {
            guard let text = string(value, path, max: 40, nullable: nullable, pattern: CloudContractRules.datePattern) else { return }
            let fractional = ISO8601DateFormatter()
            fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            guard let date = fractional.date(from: text) ?? ISO8601DateFormatter().date(from: text) else {
                fail(path, "\(text) isn't a date")
                return
            }
            if date < CloudContractRules.earliest { fail(path, "before 2023-01-01") }
            if date > now.addingTimeInterval(ahead) { fail(path, "too far after the clock") }
        }
    }
}
