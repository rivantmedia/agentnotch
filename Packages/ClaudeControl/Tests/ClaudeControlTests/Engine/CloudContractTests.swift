import Foundation
import Testing
@testable import ClaudeControl

/// The app's side of `web/contract`: keys, the request it encodes, the
/// answers it decodes, dates and limits.
struct CloudContractTests {
    // MARK: - Keys

    @Test func keysMatchTheFixture() throws {
        let fixture = try #require(try JSONSerialization.jsonObject(with: TestPaths.contractFixture("keys.json")) as? [String: Any])
        let accounts = try #require(fixture["accounts"] as? [[String: Any]])
        #expect(accounts.count == 2)
        for account in accounts {
            let uuid = try #require(account["accountUuid"] as? String)
            let organization = account["organizationUuid"] as? String
            #expect(CloudKeys.accountKey(accountUuid: uuid, organizationUuid: organization) == account["key"] as? String)
            // Case never matters: the key is of the lowercased value.
            #expect(CloudKeys.accountKey(accountUuid: uuid.uppercased(), organizationUuid: organization?.uppercased())
                    == account["key"] as? String)
        }
        // Project keys: an HMAC with the install's secret, never a plain hash of the path.
        let secret = CloudFixture.installSecret
        #expect(secret.count == CloudInstallSecret.length)
        let projects = try #require(fixture["projects"] as? [[String: Any]])
        #expect(projects.count == 2)
        for project in projects {
            let accountKey = try #require(project["accountKey"] as? String)
            let path = try #require(project["path"] as? String)
            #expect(CloudKeys.projectKey(accountKey: accountKey, path: path, secret: secret) == project["key"] as? String)
            #expect(CloudKeys.projectKey(accountKey: accountKey, path: path, secret: CloudInstallSecret.random())
                    != project["key"] as? String)
            #expect(CloudKeys.sha256Hex(accountKey + ":" + path) != project["key"] as? String)
        }
    }

    static func identity(_ id: String, organization: String? = nil, scope: String? = nil,
                         folders: [ClaudeAccount] = []) -> ClaudeIdentityAccount {
        ClaudeIdentityAccount(id: id, ringID: "claude-acct-x", organizationUuid: organization,
                              accountUuid: AccountIdentityGrouping.accountUuid(ofKey: id), runDirs: folders, storeDirs: [],
                              colorIndex: 0, isHidden: false, organizationScope: scope)
    }

    /// Regression (review finding 6): the key comes from the account's own
    /// UUID and organization, never from how this Mac groups identities.
    @Test func accountKeysDependOnlyOnTheAccountsOwnOrganization() {
        let org = CloudFixture.workOrganization
        let folder = ClaudeAccount(configDir: "/Users/me/.claude-work", organizationUuid: org.uppercased(),
                                   accountUuid: CloudFixture.workUuid)
        // Alone on this Mac: not split, but its organization is known.
        let alone = Self.identity(CloudFixture.workIdentityId, folders: [folder])
        // Beside a folder of the same login in another organization: split.
        let split = Self.identity(CloudFixture.workIdentityId + "/" + org, scope: org, folders: [folder])
        #expect(CloudKeys.accountKey(identity: alone) == CloudFixture.workAccountKey)
        #expect(CloudKeys.accountKey(identity: split) == CloudFixture.workAccountKey)
        // Organization unknown: the account UUID alone (keys.json's first account).
        let noOrganization = Self.identity(CloudFixture.identityId,
                                           folders: [ClaudeAccount(configDir: "/Users/me/.claude", accountUuid: CloudFixture.accountUuid)])
        #expect(CloudKeys.accountKey(identity: noOrganization) == CloudFixture.accountKey)
        // A mirrored copy's (stale) organization is never used.
        let mirrored = ClaudeAccount(configDir: "/Users/me/.claude", organizationUuid: "stale-org",
                                     accountUuid: CloudFixture.accountUuid)
        let withMirror = Self.identity(CloudFixture.identityId, organization: "stale-org", folders: [mirrored])
        #expect(CloudKeys.accountKey(identity: withMirror, correctedFolders: [mirrored.id]) == CloudFixture.accountKey)
        // No account UUID: nothing the same on every Mac.
        #expect(CloudKeys.accountKey(identity: Self.identity("email:me@example.com")) == nil)
        #expect(CloudKeys.accountKey(identity: Self.identity("dir:/Users/me/.claude-work")) == nil)

        let accounts = LiveCloudEnvironment.accounts(identities: [alone], labels: [:])
        #expect(accounts.map(\.accountKey) == [CloudFixture.workAccountKey])
        #expect(accounts.first?.accountUuid == CloudFixture.workUuid)
    }

    /// Regression (review findings 18 and 2): the install secret is made
    /// once, kept private, and read back unchanged.
    @Test func theInstallSecretIsMadeOnceAndKeptPrivately() throws {
        let root = URL(fileURLWithPath: TestPaths.temporaryRoot("cloud-secret"))
        defer { try? FileManager.default.removeItem(at: root) }
        let first = try #require(CloudInstallSecret.load(directory: root))
        #expect(first.count == 32)
        let file = root.appendingPathComponent(CloudInstallSecret.fileName)
        #expect(CloudFiles.permissions(of: file) == 0o600)
        #expect(CloudInstallSecret.load(directory: root) == first)
        #expect(try Data(contentsOf: file) == first)
        // Only the file itself: no temporary file left beside it.
        #expect(try FileManager.default.contentsOfDirectory(atPath: root.path) == [CloudInstallSecret.fileName])
        // Created only if absent: a second maker loses and reads the first's.
        #expect(try CloudFiles.createExclusively(CloudInstallSecret.random(), at: file) == false)
        #expect(CloudInstallSecret.load(directory: root) == first)
        // A damaged one is replaced.
        try Data([1, 2, 3]).write(to: file)
        let replaced = try #require(CloudInstallSecret.load(directory: root))
        #expect(replaced.count == 32 && replaced != first)
        #expect(CloudFiles.permissions(of: file) == 0o600)
    }

    @Test func projectPathsExpandTildeAndResolveLinks() throws {
        let root = TestPaths.temporaryRoot("cloud-path")
        defer { try? FileManager.default.removeItem(atPath: root) }
        let real = (root as NSString).appendingPathComponent("real/app")
        try FileManager.default.createDirectory(atPath: real, withIntermediateDirectories: true)
        let link = (root as NSString).appendingPathComponent("link")
        try FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: (root as NSString).appendingPathComponent("real"))
        let resolved = CloudKeys.projectPath(forCwd: link + "/app/", home: root)
        #expect(resolved == TranscriptLocator.realPath(real))
        #expect(CloudKeys.projectPath(forCwd: "~/real/app", home: root) == resolved)
        #expect(CloudKeys.projectName(forCwd: "/Users/me/code/agentnotch/") == "agentnotch")
        #expect(CloudKeys.projectName(forCwd: "/Users/me/work/billing-service") == "billing-service")
    }

    // MARK: - The request

    /// The fixture's request, built from Swift values.
    static func fixtureRequest() -> CloudSyncRequest {
        func date(_ text: String) -> Date { CloudJSON.date(from: text)! }
        let keyA = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0"
        let keyB = "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874"
        let secret = CloudFixture.installSecret
        return CloudSyncRequest(
            device: .init(id: "0E6F0B4C-2F7A-4E53-9D1B-6A2C7F9E1D35", name: "Studio MacBook Pro", appVersion: "1.18.0"),
            accounts: [
                .init(key: keyA, email: "me@example.com", organizationName: nil, plan: "Max 20x", label: "Personal"),
                .init(key: keyB, email: "me@company.com", organizationName: "Company", plan: "Team", label: nil),
            ],
            sessions: [
                .init(accountKey: keyA, sessionId: "a1b2c3d4-e5f6-4789-8abc-def012345678",
                      project: .init(key: CloudKeys.projectKey(accountKey: keyA, path: "/Users/me/code/agentnotch", secret: secret),
                                     name: "agentnotch"),
                      title: "Keep sessions working while background agents run", source: .vscode,
                      models: ["claude-opus-4-5-20251101", "claude-haiku-4-5-20251001"],
                      startedAt: date("2026-09-25T08:02:11.482Z"), lastActivityAt: date("2026-09-25T09:47:03Z"),
                      endedAt: date("2026-09-25T09:48:00Z"), messageCount: 212,
                      tokens: .init(input: 18234, output: 96512, cacheCreation: 402118, cacheRead: 12873120),
                      costUsd: 14.82,
                      summary: .init(text: "Changed the notch so a Claude Code session stays 'working' while workflows it started are still running, with tests.",
                                     model: "claude-haiku-4-5-20251001", generatedAt: date("2026-09-25T10:05:00Z"))),
                .init(accountKey: keyB, sessionId: "0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b",
                      project: .init(key: CloudKeys.projectKey(accountKey: keyB, path: "/Users/me/work/billing-service", secret: secret),
                                     name: "billing-service"),
                      title: nil, source: .cli, models: ["claude-sonnet-4-5-20250929"],
                      startedAt: date("2026-09-25T11:00:00Z"), lastActivityAt: date("2026-09-25T11:20:42Z"),
                      endedAt: nil, messageCount: 37,
                      tokens: .init(input: 5120, output: 8840, cacheCreation: 64000, cacheRead: 910000),
                      costUsd: nil),
            ],
            usage: [
                .init(accountKey: keyA, source: .desktop, observedAt: date("2026-09-25T09:50:00Z"), windows: [
                    .init(id: "session", utilization: 42, resetsAt: date("2026-09-25T13:00:00Z")),
                    .init(id: "weekly_all", utilization: 61.5, resetsAt: date("2026-09-29T08:00:00Z")),
                    .init(id: "weekly_opus", utilization: 12, resetsAt: nil),
                ]),
                .init(accountKey: keyB, source: .probe, observedAt: date("2026-09-25T11:15:00Z"), windows: [
                    .init(id: "session", utilization: 8, resetsAt: date("2026-09-25T15:00:00Z")),
                ]),
            ]
        )
    }

    @Test func encodedRequestHasTheFixturesShape() throws {
        let fixture = try JSONSerialization.jsonObject(with: TestPaths.contractFixture("sync-request.json"))
        let encoded = try CloudJSON.makeEncoder().encode(Self.fixtureRequest().clamped(now: CloudJSON.date(from: "2026-09-25T12:00:00Z")!))
        let ours = try JSONSerialization.jsonObject(with: encoded)
        var differences: [String] = []
        Self.compare(ours, fixture, path: "$", into: &differences)
        #expect(differences.isEmpty, "\(differences)")
    }

    @Test func fixtureRequestDecodesAndReencodesToItself() throws {
        let data = try TestPaths.contractFixture("sync-request.json")
        let decoded = try CloudJSON.makeDecoder().decode(CloudSyncRequest.self, from: data)
        #expect(decoded == Self.fixtureRequest())
        #expect(decoded.clamped(now: CloudJSON.date(from: "2026-09-25T12:00:00Z")!) == decoded)
    }

    /// The contract: every date between 2023-01-01 and a day after now. A
    /// session or reading dated outside is left out (the website would
    /// refuse the whole request); a summary dated outside goes alone.
    @Test func datesOutsideTheContractsRangeAreLeftOut() {
        let now = CloudJSON.date(from: "2026-09-25T12:00:00Z")!
        var request = Self.fixtureRequest()
        #expect(request.clamped(now: now) == request)
        var ancient = request.sessions[1]
        ancient.sessionId = UUID().uuidString
        ancient.startedAt = CloudJSON.date(from: "2019-05-01T00:00:00Z")!
        var future = request.sessions[1]
        future.sessionId = UUID().uuidString
        future.lastActivityAt = now.addingTimeInterval(2 * 24 * 3600)
        var endsLater = request.sessions[1]
        endsLater.sessionId = UUID().uuidString
        endsLater.endedAt = now.addingTimeInterval(3 * 24 * 3600)
        request.sessions += [ancient, future, endsLater]
        request.sessions[0].summary?.generatedAt = CloudJSON.date(from: "2022-12-31T23:59:59Z")!
        var oldReading = request.usage[1]
        oldReading.observedAt = CloudJSON.date(from: "2020-01-01T00:00:00Z")!
        request.usage.append(oldReading)
        // A weekly window resets days ahead: that date stays.
        request.usage[0].windows[1].resetsAt = now.addingTimeInterval(6 * 24 * 3600)

        let clamped = request.clamped(now: now)
        #expect(clamped.sessions.map(\.sessionId) == [request.sessions[0].sessionId, request.sessions[1].sessionId])
        #expect(clamped.sessions[0].summary == nil)
        #expect(clamped.usage.count == 2)
        #expect(clamped.usage[0].windows[1].resetsAt == now.addingTimeInterval(6 * 24 * 3600))
        #expect(CloudContract.accepts(CloudContract.earliestDate, now: now))
        #expect(!CloudContract.accepts(CloudContract.earliestDate.addingTimeInterval(-1), now: now))
        #expect(CloudContract.accepts(now.addingTimeInterval(24 * 3600), now: now))
        #expect(!CloudContract.accepts(now.addingTimeInterval(24 * 3600 + 1), now: now))
    }

    /// Regression (fix check): the website takes a window's reset time from
    /// 2023-01-01 to 32 days after its clock and refuses the whole request
    /// over one outside. Such a time is sent as null; the reading, and the
    /// window's utilization, still go.
    @Test func resetTimesTheWebsiteWouldRefuseAreSentAsNull() throws {
        let now = CloudJSON.date(from: "2026-09-25T12:00:00Z")!
        let day: TimeInterval = 24 * 3600
        let times: [Date?] = [
            Date(timeIntervalSince1970: 0),                          // a zero
            CloudContract.earliestDate.addingTimeInterval(-1),
            CloudContract.earliestDate,
            now.addingTimeInterval(7 * day),                         // next week's reset
            now.addingTimeInterval(32 * day),
            now.addingTimeInterval(32 * day + 1),
            now.addingTimeInterval(400 * day),
            nil,
        ]
        var request = Self.fixtureRequest()
        let ids = ["session", "weekly_all", "extra_usage", "weekly_opus", "weekly_sonnet", "weekly_haiku", "weekly_x", "weekly_y"]
        request.usage[0].windows = zip(ids, times).map { .init(id: $0, utilization: 12.5, resetsAt: $1) }
        let clamped = request.clamped(now: now)
        let windows = try #require(clamped.usage.first?.windows)
        #expect(windows.map(\.id) == ids)
        #expect(windows.allSatisfy { $0.utilization == 12.5 })
        #expect(windows.map(\.resetsAt) == [nil, nil, CloudContract.earliestDate, now.addingTimeInterval(7 * day),
                                            now.addingTimeInterval(32 * day), nil, nil, nil])
        #expect(clamped.clamped(now: now) == clamped)
        // Written as an explicit null, as the contract wants.
        let json = try #require(try JSONSerialization.jsonObject(with: CloudJSON.makeEncoder().encode(clamped)) as? [String: Any])
        let first = try #require(((json["usage"] as? [[String: Any]])?.first?["windows"] as? [[String: Any]])?.first)
        #expect(first["resetsAt"] is NSNull)
    }

    /// Same keys at every level (an explicit null counts as present), same
    /// values; dates compared as instants (the app writes milliseconds).
    static func compare(_ lhs: Any, _ rhs: Any, path: String, into differences: inout [String]) {
        switch (lhs, rhs) {
        case let (left as [String: Any], right as [String: Any]):
            if Set(left.keys) != Set(right.keys) {
                differences.append("\(path): keys \(left.keys.sorted()) vs \(right.keys.sorted())")
            }
            for key in Set(left.keys).intersection(right.keys) {
                compare(left[key]!, right[key]!, path: "\(path).\(key)", into: &differences)
            }
        case let (left as [Any], right as [Any]):
            guard left.count == right.count else {
                differences.append("\(path): \(left.count) items vs \(right.count)")
                return
            }
            for (index, pair) in zip(left, right).enumerated() {
                compare(pair.0, pair.1, path: "\(path)[\(index)]", into: &differences)
            }
        case let (left as String, right as String):
            if left == right { return }
            if let a = CloudJSON.date(from: left), let b = CloudJSON.date(from: right), abs(a.timeIntervalSince(b)) < 0.001 { return }
            differences.append("\(path): \(left) vs \(right)")
        case let (left as NSNumber, right as NSNumber):
            if left.doubleValue != right.doubleValue { differences.append("\(path): \(left) vs \(right)") }
        case (is NSNull, is NSNull):
            return
        default:
            differences.append("\(path): \(type(of: lhs)) vs \(type(of: rhs))")
        }
    }

    // MARK: - Answers

    @Test func everyResponseFixtureDecodes() throws {
        let decoder = CloudJSON.makeDecoder()
        let config = try decoder.decode(CloudConfig.self, from: TestPaths.contractFixture("config.json"))
        #expect(config.redirectUrl == CloudContract.redirectURL)
        #expect(config.supabaseUrl == "https://abcdefghijklmnop.supabase.co")
        #expect(config.supabasePublishableKey.hasPrefix("sb_publishable_"))
        #expect(CloudWebsite.validatedLink(config.dashboardUrl) != nil)

        let me = try decoder.decode(CloudMe.self, from: TestPaths.contractFixture("me.json"))
        #expect(me.user.email == "me@example.com" && me.user.name == "Me Example")

        let sync = try decoder.decode(CloudSyncResponse.self, from: TestPaths.contractFixture("sync-response.json"))
        #expect(sync.accepted == .init(sessions: 2, usage: 2))
        #expect(sync.serverTime == CloudJSON.date(from: "2026-09-25T11:21:00Z"))

        let error = try decoder.decode(CloudErrorBody.self, from: TestPaths.contractFixture("error.json"))
        #expect(CloudErrorCode(rawValue: error.error.code) == .unauthorized)
        let mapped = CloudAPIError.from(status: 401, data: try TestPaths.contractFixture("error.json"), retryAfterHeader: nil)
        #expect(mapped == .server(status: 401, code: "UNAUTHORIZED", message: "Sign in again.", retryAfter: nil))
        #expect(mapped.isUnauthorized)
        #expect(CloudAPIError.from(status: 429, data: Data(), retryAfterHeader: "12").retryAfter == 12)
    }

    @Test func datesAreUTCWithZAndReadEitherWay() {
        let date = CloudJSON.date(from: "2026-09-25T08:02:11.482Z")!
        #expect(CloudJSON.string(from: date) == "2026-09-25T08:02:11.482Z")
        #expect(CloudJSON.date(from: "2026-09-25T09:47:03Z") != nil)
        #expect(CloudJSON.string(from: CloudJSON.date(from: "2026-09-25T09:47:03Z")!) == "2026-09-25T09:47:03.000Z")
        #expect(CloudJSON.date(from: "yesterday") == nil)
    }

    // MARK: - Limits

    @Test func clampingLeavesOutWhatTheWebsiteWouldRefuse() {
        var request = Self.fixtureRequest()
        let key = request.accounts[0].key
        request.accounts.append(.init(key: "not-a-key", email: nil, organizationName: nil, plan: nil, label: nil))
        var bad = request.sessions[1]
        bad.sessionId = "not-a-uuid"
        request.sessions.append(bad)
        var stranger = request.sessions[1]
        stranger.sessionId = UUID().uuidString
        stranger.accountKey = String(repeating: "a", count: 64)
        request.sessions.append(stranger)
        request.sessions[0].title = String(repeating: "t", count: 500)
        request.sessions[0].models = ["", "m", "m"] + (0..<20).map { "model-\($0)" }
        request.sessions[0].costUsd = .nan
        request.sessions[0].tokens.input = -5
        request.usage[0].windows.append(.init(id: "bogus", utilization: 3, resetsAt: nil))
        request.usage[0].windows.append(.init(id: "weekly_" + String(repeating: "x", count: 80), utilization: 3, resetsAt: nil))
        request.usage[0].windows.append(.init(id: "weekly_neg", utilization: -2, resetsAt: nil))
        request.usage.append(.init(accountKey: String(repeating: "b", count: 64), source: .probe, observedAt: CloudFixture.base,
                                   windows: [.init(id: "session", utilization: 1, resetsAt: nil)]))

        let clamped = request.clamped(now: CloudJSON.date(from: "2026-09-25T12:00:00Z")!)
        #expect(clamped.accounts.map(\.key) == [key, request.accounts[1].key])
        #expect(clamped.sessions.count == 2)
        #expect(clamped.sessions[0].title?.count == 200)
        #expect(clamped.sessions[0].models.count == 10 && clamped.sessions[0].models.first == "m")
        #expect(clamped.sessions[0].costUsd == nil)
        #expect(clamped.sessions[0].tokens.input == 0)
        #expect(clamped.usage.count == 2)
        #expect(clamped.usage[0].windows.map(\.id) == ["session", "weekly_all", "weekly_opus", "weekly_neg"])
        #expect(clamped.usage[0].windows.last?.utilization == 0)
    }

    @Test func windowIdsFollowTheWebsitesRule() {
        #expect(CloudKeys.isWindowID("session") && CloudKeys.isWindowID("weekly_all") && CloudKeys.isWindowID("extra_usage"))
        #expect(CloudKeys.isWindowID("weekly_sonnet_4_5") && CloudKeys.isWindowID("weekly_opus-4.1"))
        #expect(!CloudKeys.isWindowID("weekly_") && !CloudKeys.isWindowID("weekly__x") && !CloudKeys.isWindowID("monthly"))
        let long = "weekly_" + String(repeating: "a", count: 90)
        #expect(CloudKeys.contractWindowID(long)?.count == 7 + 57)
        #expect(CloudKeys.contractWindowID("nonsense") == nil)
        // The website takes lowercase only.
        #expect(CloudKeys.contractWindowID("weekly_Opus") == "weekly_opus")
        #expect(!CloudKeys.isWindowID("weekly_Opus"))
    }

    @Test func sessionSourcesFromEntrypoints() {
        #expect(CloudSessionSource.from(entrypoint: "cli") == .cli)
        #expect(CloudSessionSource.from(entrypoint: "claude-vscode") == .vscode)
        #expect(CloudSessionSource.from(entrypoint: "claude-desktop") == .desktop)
        #expect(CloudSessionSource.from(entrypoint: "claude-desktop-3p") == .desktop)
        #expect(CloudSessionSource.from(entrypoint: "sdk-cli") == .sdk)
        #expect(CloudSessionSource.from(entrypoint: "sdk-ts") == .sdk)
        #expect(CloudSessionSource.from(entrypoint: nil) == .other)
        #expect(CloudSessionSource.from(entrypoint: "something-new") == .other)
    }

    // MARK: - The website's address

    @Test func onlyHttpsOrThisMacAreAccepted() {
        #expect(CloudWebsite.validated("https://agentnotch.example.com/")?.absoluteString == "https://agentnotch.example.com")
        #expect(CloudWebsite.validated("agentnotch.example.com")?.absoluteString == "https://agentnotch.example.com")
        #expect(CloudWebsite.validated("HTTPS://Example.com/app/")?.absoluteString == "https://example.com/app")
        #expect(CloudWebsite.validated("http://localhost:3000")?.absoluteString == "http://localhost:3000")
        #expect(CloudWebsite.validated("http://127.0.0.1:3000")?.absoluteString == "http://127.0.0.1:3000")
        #expect(CloudWebsite.validated("http://example.com") == nil)
        #expect(CloudWebsite.validated("ftp://example.com") == nil)
        #expect(CloudWebsite.validated("https://example.com/?next=x") == nil)
        #expect(CloudWebsite.validated("https://user:pw@example.com") == nil)
        #expect(CloudWebsite.validated("   ") == nil)
        #expect(CloudWebsite.validated(nil) == nil)
        #expect(CloudWebsite.endpoint(URL(string: "https://example.com/app")!, CloudContract.Path.sync).absoluteString
                == "https://example.com/app/api/app/v1/sync")
    }
}
