import Foundation
@testable import ClaudeControl

// Shared by the cloud suites: the contract's fixtures, a stand-in network,
// a stand-in engine, and transcript lines. Nothing here reaches the network,
// the real home or `claude`.

extension TestPaths {
    /// web/contract/fixtures, the JSON both the app and the website test against.
    nonisolated static let contractFixtures = packageRoot
        .deletingLastPathComponent()   // Packages
        .deletingLastPathComponent()   // repository
        .appendingPathComponent("web/contract/fixtures", isDirectory: true)

    nonisolated static func contractFixture(_ name: String) throws -> Data {
        try Data(contentsOf: contractFixtures.appendingPathComponent(name))
    }
}

/// Answers requests the way a test says, and remembers every one.
nonisolated final class FakeTransport: CloudTransport, @unchecked Sendable {
    struct Answer {
        var status: Int
        var body: Data
        var headers: [String: String] = [:]

        static func json(_ status: Int, _ object: Any, headers: [String: String] = [:]) -> Answer {
            Answer(status: status, body: (try? JSONSerialization.data(withJSONObject: object)) ?? Data(), headers: headers)
        }
    }

    private let lock = NSLock()
    /// Async, so a test can hold a request (see `CloudSyncRegressionTests.Gate`)
    /// by suspending, never by blocking a thread of the cooperative pool.
    private var handler: @Sendable (URLRequest) async throws -> Answer
    private(set) var requests: [URLRequest] = []

    init(_ handler: @escaping @Sendable (URLRequest) async throws -> Answer) {
        self.handler = handler
    }

    func answer(_ handler: @escaping @Sendable (URLRequest) async throws -> Answer) {
        lock.withLock { self.handler = handler }
    }

    var recorded: [URLRequest] { lock.withLock { requests } }

    func requests(to path: String) -> [URLRequest] {
        recorded.filter { $0.url?.path.hasSuffix(path) == true }
    }

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let handler = lock.withLock { () -> @Sendable (URLRequest) async throws -> Answer in
            requests.append(request)
            return self.handler
        }
        let answer = try await handler(request)
        let response = HTTPURLResponse(url: request.url!, statusCode: answer.status, httpVersion: "HTTP/1.1",
                                       headerFields: answer.headers.merging(["Content-Type": "application/json"]) { a, _ in a })!
        return (answer.body, response)
    }

    /// The JSON body a request carried.
    static func body(_ request: URLRequest) -> [String: Any]? {
        request.httpBody.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
    }
}

/// The engine as `CloudSync` sees it, without a registry.
@MainActor
final class FakeCloudEnvironment: CloudSyncEnvironment {
    var accountList: [CloudAccountInfo]
    var folders: [CloudBackfill.Folder] = []
    /// Folder → login, as the registry would read them (nil: not read yet).
    var logins: [String: String]? = [:]
    /// Identity id → 5-hour window used, in percent.
    var utilization: [String: Double] = [:]
    var folder: CloudSummaryFolder? = CloudSummaryFolder(configDir: "/tmp/agentnotch-none/.claude-work",
                                                         configDirEnv: "/tmp/agentnotch-none/.claude-work", configDirs: [])
    var stillRuns = true
    var launching = false
    private(set) var folderRequests: [String] = []

    init(accounts: [CloudAccountInfo]) {
        accountList = accounts
    }

    func accounts() -> [CloudAccountInfo] { accountList }
    func folderLogins() -> [String: String]? { logins }
    func backfillFolders() -> [CloudBackfill.Folder] { folders }
    func sessionUtilization(identityId: String) -> Double? { utilization[identityId] }

    func summaryFolder(forIdentity identityId: String) async -> CloudSummaryFolder? {
        folderRequests.append(identityId)
        return folder
    }

    func summaryFolderStillRuns(_ folder: CloudSummaryFolder, identityId: String) async -> Bool { stillRuns }
    var isLaunchingClaude: Bool { launching }
}

/// JSON lines as Claude Code writes them in a transcript.
nonisolated enum CloudTranscriptLines {
    static func line(_ object: [String: Any]) -> String {
        String(decoding: try! JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]), as: UTF8.self)
    }

    static func user(_ text: String, session: String, at stamp: String, cwd: String = "/Users/me/code/app",
                     entrypoint: String = "cli") -> String {
        line(["type": "user", "sessionId": session, "timestamp": stamp, "cwd": cwd, "entrypoint": entrypoint,
              "uuid": UUID().uuidString, "message": ["role": "user", "content": text]])
    }

    static func assistant(id: String, request: String, session: String, model: String = "claude-opus-4-5",
                          input: Int, output: Int, cacheCreation: Int = 0, cacheRead: Int = 0, at stamp: String,
                          text: String? = "Done.", toolUse: Bool = false, sidechain: Bool = false,
                          cwd: String = "/Users/me/code/app") -> String {
        var content: [[String: Any]] = []
        if let text { content.append(["type": "text", "text": text]) }
        if toolUse { content.append(["type": "tool_use", "id": "toolu_\(id)", "name": "Bash", "input": ["command": "cat secrets.txt"]]) }
        return line(["type": "assistant", "sessionId": session, "timestamp": stamp, "requestId": request, "cwd": cwd,
                     "isSidechain": sidechain, "uuid": UUID().uuidString,
                     "message": ["id": id, "role": "assistant", "model": model, "content": content,
                                 "usage": ["input_tokens": input, "output_tokens": output,
                                           "cache_creation_input_tokens": cacheCreation,
                                           "cache_read_input_tokens": cacheRead]]])
    }

    static func toolResult(_ output: String, session: String, at stamp: String) -> String {
        line(["type": "user", "sessionId": session, "timestamp": stamp, "uuid": UUID().uuidString,
              "toolUseResult": ["stdout": output],
              "message": ["role": "user", "content": [["type": "tool_result", "tool_use_id": "toolu_x", "content": output]]]])
    }

    static func aiTitle(_ title: String, session: String) -> String {
        line(["type": "ai-title", "sessionId": session, "aiTitle": title])
    }

    /// `line` as `/branch` copies it into a fork: the fork's session id,
    /// and where it came from.
    static func forked(_ line: String, into fork: String, from original: String) -> String {
        var object = (try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any]) ?? [:]
        object["sessionId"] = fork
        object["forkedFrom"] = ["sessionId": original, "messageUuid": object["uuid"] ?? UUID().uuidString]
        return Self.line(object)
    }

    static func write(_ lines: [String], to path: String, append: Bool = false) throws {
        let text = lines.map { $0 + "\n" }.joined()
        try FileManager.default.createDirectory(atPath: (path as NSString).deletingLastPathComponent,
                                                withIntermediateDirectories: true)
        if append, let handle = FileHandle(forWritingAtPath: path) {
            try handle.seekToEnd()
            try handle.write(contentsOf: Data(text.utf8))
            try handle.close()
        } else {
            try Data(text.utf8).write(to: URL(fileURLWithPath: path))
        }
    }
}

/// Session ids, account keys and times used across the cloud suites.
nonisolated enum CloudFixture {
    static let accountUuid = "3f1f0a3e-8a7b-4c1d-9e2f-5a6b7c8d9e0f"
    static let identityId = "uuid:\(accountUuid)"
    static let accountKey = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0"
    /// keys.json's second account: one in an organization.
    static let workUuid = "9d2c7b1a-0000-4e5f-8a9b-1c2d3e4f5a6b"
    static let workOrganization = "7a6b5c4d-3e2f-4a1b-9c8d-0e1f2a3b4c5d"
    static let workIdentityId = "uuid:\(workUuid)"
    static let workAccountKey = "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874"
    static let sessionA = "a1b2c3d4-e5f6-4789-8abc-def012345678"
    static let sessionB = "0f9e8d7c-6b5a-4493-8271-605f4e3d2c1b"
    static let sessionC = "11111111-2222-4333-8444-555555555555"

    static var account: CloudAccountInfo {
        CloudAccountInfo(identityId: identityId, accountKey: accountKey, accountUuid: accountUuid,
                         email: "me@example.com", organizationName: nil, plan: "Max 20x", label: "Personal")
    }

    static var workAccount: CloudAccountInfo {
        CloudAccountInfo(identityId: workIdentityId, accountKey: workAccountKey, accountUuid: workUuid,
                         email: "me@company.com", organizationName: "Company", plan: "Team", label: nil)
    }

    /// keys.json's `installSecretHex`: the install secret its project keys were made with.
    static var installSecret: Data {
        let object = (try? JSONSerialization.jsonObject(with: TestPaths.contractFixture("keys.json"))) as? [String: Any]
        let hex = object?["installSecretHex"] as? String ?? ""
        var bytes: [UInt8] = []
        var index = hex.startIndex
        while index < hex.endIndex, let next = hex.index(index, offsetBy: 2, limitedBy: hex.endIndex) {
            bytes.append(UInt8(hex[index..<next], radix: 16) ?? 0)
            index = next
        }
        return Data(bytes)
    }

    /// ISO 8601 for a moment `seconds` after 2026-09-25T08:00:00Z.
    static func stamp(_ seconds: TimeInterval) -> String {
        CloudJSON.string(from: base.addingTimeInterval(seconds))
    }

    static let base = CloudJSON.date(from: "2026-09-25T08:00:00Z")!
}
