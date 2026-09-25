import Foundation
@testable import ClaudeControl

/// Stores and helpers shared by the session-core suites.
extension SessionStore {
    /// A private store for tests: its own review file, a publish after every
    /// event, no side effects on the app's socket server or registry scanner,
    /// and Stops confirmed at once (no registry to wait for) unless
    /// `completionTiming` says otherwise.
    static func forTests(
        reviewFile: URL,
        parser: ConversationParser = ConversationParser(),
        effects: SessionStoreEffects = .none,
        completionTiming: TurnCompletion.Timing = .immediate,
        backgroundWaitTiming: BackgroundWork.WaitTiming = .standard
    ) -> SessionStore {
        SessionStore(
            reviewStore: ReviewStateStore(fileURL: reviewFile, writeDelay: 0, createsFolder: false),
            parser: parser,
            publishInterval: 0,
            effects: effects,
            completionTiming: completionTiming,
            backgroundWaitTiming: backgroundWaitTiming
        )
    }

    static func forTests(
        reviewStore: ReviewStateStore,
        parser: ConversationParser = ConversationParser(),
        effects: SessionStoreEffects = .none,
        completionTiming: TurnCompletion.Timing = .immediate,
        backgroundWaitTiming: BackgroundWork.WaitTiming = .standard
    ) -> SessionStore {
        SessionStore(
            reviewStore: reviewStore,
            parser: parser,
            publishInterval: 0,
            effects: effects,
            completionTiming: completionTiming,
            backgroundWaitTiming: backgroundWaitTiming
        )
    }
}

extension TurnCompletion.Timing {
    /// A Stop with no registry following it completes at once.
    nonisolated static let immediate = TurnCompletion.Timing(fallbackDelay: 0, registryTimeout: 90, clockTolerance: 1)
}

/// Records what a store asked of the rest of the app.
nonisolated final class EffectsRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled: [String] = []
    private var cancelledSessions: [String] = []
    private var rescans: [String] = []

    var effects: SessionStoreEffects {
        SessionStoreEffects(
            cancelPermissions: { ids in self.record { $0.cancelled += ids } },
            cancelSessionPermissions: { id in self.record { $0.cancelledSessions.append(id) } },
            rescanRegistry: { dir in self.record { $0.rescans.append(dir) } }
        )
    }

    var cancelledPermissions: [String] { read { $0.cancelled } }
    var rescannedDirs: [String] { read { $0.rescans } }

    private func record(_ body: (EffectsRecorder) -> Void) {
        lock.lock(); body(self); lock.unlock()
    }

    private func read<T>(_ body: (EffectsRecorder) -> T) -> T {
        lock.lock(); defer { lock.unlock() }
        return body(self)
    }
}

/// A temporary folder with a fake account (`<root>/.claude`), removed when
/// the test's value goes away.
nonisolated final class TemporaryAccount: @unchecked Sendable {
    let root: URL
    let configDir: URL
    let project: URL

    init(prefix: String = "agentnotch-a1") throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("\(prefix)-\(UUID().uuidString)")
        configDir = root.appendingPathComponent(".claude")
        project = configDir.appendingPathComponent("projects/-tmp-proj")
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)
    }

    deinit {
        // A store's review write still in flight (completions are written
        // at once, on the store's queue) can land inside the folder while
        // it is being removed, which then fails with ENOTEMPTY. Such a write
        // takes milliseconds: try again briefly.
        let fileManager = FileManager.default
        for attempt in 0..<20 {
            try? fileManager.removeItem(at: root)
            guard fileManager.fileExists(atPath: root.path) else { return }
            usleep(useconds_t(10_000 * (attempt + 1)))
        }
    }

    /// `<project>/<sessionId>.jsonl`, created empty.
    func transcript(_ sessionId: String) -> String {
        let path = project.appendingPathComponent("\(sessionId).jsonl").path
        if !FileManager.default.fileExists(atPath: path) {
            FileManager.default.createFile(atPath: path, contents: Data())
        }
        return path
    }

    var reviewFile: URL { root.appendingPathComponent("review-state.json") }
}

/// Transcript lines as Claude Code writes them.
enum TranscriptLines {
    static func iso(_ date: Date) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.string(from: date)
    }

    static func user(_ text: String, at date: Date = Date(), extra: [String: Any] = [:]) -> [String: Any] {
        var line: [String: Any] = [
            "type": "user", "uuid": UUID().uuidString, "timestamp": iso(date),
            "message": ["role": "user", "content": text],
        ]
        line.merge(extra) { _, new in new }
        return line
    }

    static func assistantText(_ text: String, at date: Date = Date(), id: String = UUID().uuidString) -> [String: Any] {
        [
            "type": "assistant", "uuid": UUID().uuidString, "timestamp": iso(date),
            "message": ["id": id, "role": "assistant", "content": [["type": "text", "text": text]]],
        ]
    }

    static func toolUse(id: String, name: String, input: [String: Any] = [:], at date: Date = Date()) -> [String: Any] {
        [
            "type": "assistant", "uuid": UUID().uuidString, "timestamp": iso(date),
            "message": ["id": "msg-\(id)", "role": "assistant", "content": [["type": "tool_use", "id": id, "name": name, "input": input]]],
        ]
    }

    static func toolResult(id: String, text: String = "ok", at date: Date = Date(), toolUseResult: [String: Any]? = nil) -> [String: Any] {
        var line: [String: Any] = [
            "type": "user", "uuid": UUID().uuidString, "timestamp": iso(date),
            "message": ["role": "user", "content": [["type": "tool_result", "tool_use_id": id, "content": text]]],
        ]
        if let toolUseResult { line["toolUseResult"] = toolUseResult }
        return line
    }

    static func append(_ lines: [[String: Any]], to path: String) throws {
        var data = Data()
        for line in lines {
            data += try JSONSerialization.data(withJSONObject: line) + Data("\n".utf8)
        }
        let handle = try FileHandle(forWritingTo: URL(fileURLWithPath: path))
        try handle.seekToEnd()
        handle.write(data)
        try handle.close()
    }
}
