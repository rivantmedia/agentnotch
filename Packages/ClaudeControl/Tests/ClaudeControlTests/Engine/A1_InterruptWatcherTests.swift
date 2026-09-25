import Foundation
import Testing
@testable import ClaudeControl

/// The transcript interrupt watcher: which lines are interrupts (bytes only,
/// no decoding), that it runs only while a main turn does, and that it
/// reports an interrupt appended to the transcript.
@Suite(.serialized)
@MainActor
final class A1_InterruptWatcherTests {
    private let account: TemporaryAccount

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-a1-interrupt")
    }

    private static func line(_ json: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: json)
    }

    @Test func interruptLinesAreRecognizedAsBytes() throws {
        let userInterrupt = try Self.line([
            "type": "user",
            "message": ["role": "user", "content": [["type": "text", "text": "[Request interrupted by user for tool use]"]]],
        ])
        let rejectedTool = try Self.line([
            "type": "user",
            "message": ["role": "user", "content": [["type": "tool_result", "tool_use_id": "t1", "is_error": true,
                                                       "content": "The user doesn't want to proceed with this tool use."]]],
        ])
        let interruptedBash = try Self.line([
            "type": "user",
            "message": ["role": "user", "content": [["type": "tool_result", "tool_use_id": "t2", "content": "partial output"]]],
            "toolUseResult": ["stdout": "partial output", "interrupted": true],
        ])
        #expect(JSONLInterruptWatcher.isInterruptLine(userInterrupt))
        #expect(JSONLInterruptWatcher.isInterruptLine(rejectedTool))
        #expect(JSONLInterruptWatcher.isInterruptLine(interruptedBash))

        // Claude quoting the phrase, a failed (not interrupted) tool, and a
        // plain result are not interrupts.
        let quoted = try Self.line([
            "type": "assistant",
            "message": ["role": "assistant", "content": [["type": "text", "text": "It printed [Request interrupted by user] earlier."]]],
        ])
        let failed = try Self.line([
            "type": "user",
            "message": ["role": "user", "content": [["type": "tool_result", "tool_use_id": "t3", "is_error": true, "content": "exit code 1"]]],
        ])
        let plain = try Self.line([
            "type": "user",
            "message": ["role": "user", "content": [["type": "tool_result", "tool_use_id": "t4", "content": "ok"]]],
        ])
        #expect(!JSONLInterruptWatcher.isInterruptLine(quoted))
        #expect(!JSONLInterruptWatcher.isInterruptLine(failed))
        #expect(!JSONLInterruptWatcher.isInterruptLine(plain))
    }

    @Test func watchesOnlyWhileTheMainTurnRuns() {
        let manager = InterruptWatcherManager.shared
        let transcript = account.transcript("iw-1")
        defer { manager.stopWatching(sessionId: "iw-1") }

        func event(_ name: String, status: String, agentId: String? = nil) -> HookEvent {
            HookEvent(sessionId: "iw-1", event: name, status: status, transcriptPath: transcript, agentId: agentId,
                      tool: name.hasSuffix("ToolUse") ? "Bash" : nil, toolUseId: name.hasSuffix("ToolUse") ? "t1" : nil)
        }

        // A background agent's event doesn't start it.
        manager.apply(event("PreToolUse", status: "running_tool", agentId: "bg-1"))
        #expect(!manager.isWatching(sessionId: "iw-1"))

        manager.apply(event("UserPromptSubmit", status: "processing"))
        #expect(manager.isWatching(sessionId: "iw-1"))
        manager.apply(event("PreToolUse", status: "running_tool"))
        #expect(manager.isWatching(sessionId: "iw-1"))

        // The turn's end stops it (no file handle and source per idle session).
        manager.apply(event("Stop", status: "waiting_for_input"))
        #expect(!manager.isWatching(sessionId: "iw-1"))

        manager.apply(event("UserPromptSubmit", status: "processing"))
        manager.apply(event("StopFailure", status: "waiting_for_input"))
        #expect(!manager.isWatching(sessionId: "iw-1"))

        manager.apply(event("UserPromptSubmit", status: "processing"))
        manager.apply(event("SessionEnd", status: "ended"))
        #expect(!manager.isWatching(sessionId: "iw-1"))
    }

    private nonisolated final class Recorder: JSONLInterruptWatcherDelegate, @unchecked Sendable {
        private let lock = NSLock()
        private var sessions: [String] = []
        nonisolated func didDetectInterrupt(sessionId: String, at: Date) {
            lock.lock(); sessions.append(sessionId); lock.unlock()
        }
        var detected: [String] {
            lock.lock(); defer { lock.unlock() }
            return sessions
        }
    }

    @Test func reportsAnInterruptAppendedToTheTranscript() async throws {
        let transcript = account.transcript("iw-2")
        try TranscriptLines.append([TranscriptLines.user("start the build")], to: transcript)
        let recorder = Recorder()
        let watcher = JSONLInterruptWatcher(sessionId: "iw-2", filePath: transcript)
        watcher.delegate = recorder
        watcher.start()
        defer { watcher.stop() }
        try await Task.sleep(nanoseconds: 200_000_000)

        // What was there before the watcher started isn't news; an ordinary
        // line isn't an interrupt.
        try TranscriptLines.append([TranscriptLines.assistantText("Building…")], to: transcript)
        try await Task.sleep(nanoseconds: 200_000_000)
        #expect(recorder.detected.isEmpty)

        try TranscriptLines.append([TranscriptLines.user("[Request interrupted by user]")], to: transcript)
        let deadline = Date().addingTimeInterval(5)
        while recorder.detected.isEmpty, Date() < deadline {
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        #expect(recorder.detected == ["iw-2"])
    }
}
